"""Ordinary Rust process plus real loopback provider and Python Host effects."""
import asyncio
import copy
import hashlib
import json
import os
from pathlib import Path
import subprocess
import sys
import time
import unittest

HERE = Path(__file__).resolve().parent
sys.path.insert(0, str(HERE.parent / 'clients/python'))
from ara_agent_lite import ReconciliationRequired, encoded, read_reference, run_task, validate_candidate_identity
from controlled_provider import ControlledProvider, call, text

BINARY = Path(os.environ.get('ARA_JSONL_TEST_BINARY', 'D:/Temp/ara-build/ara-github-target-20261005/debug/ara-agent-lite-jsonl.exe'))
EVIDENCE = Path(os.environ['ARA_JSONL_TEST_EVIDENCE'])


def save(path, value):
    with Path(path).open('x', encoding='utf-8') as stream:
        json.dump(value, stream, ensure_ascii=False, indent=2)
        stream.write('\n')
        stream.flush()
        os.fsync(stream.fileno())


def sha(path):
    return hashlib.sha256(Path(path).read_bytes()).hexdigest()


class FileHost:
    def __init__(self, root):
        self.root = root
        self.writes = 0
        self.reviews = 0
        self.prepares = 0
        self.started = asyncio.Event()
        self.joined = False
        self.slow = False
        self.unknown = False

    async def tool_call(self, request, cancelled):
        path = self.root / ('host-' + request['rpc_id'] + '.json')
        save(path, {'request': request, 'state': 'started'})
        self.writes += 1
        (self.root / 'answer.txt').write_text(request['arguments']['content'], encoding='utf-8')
        self.started.set()
        if self.slow:
            await asyncio.wait_for(cancelled.wait(), timeout=8)
            await asyncio.sleep(.15)
        self.joined = True
        complete = path.with_suffix('.completed.json')
        save(complete, {'state': 'completed', 'answer_sha256': sha(self.root / 'answer.txt')})
        return {'value': {'written': True}, 'is_error': False,
                'settlement': 'Unknown' if self.unknown or self.slow else 'Settled', 'operation_receipt': str(complete)}

    async def verify(self, candidate, cancelled):
        self.reviews += 1
        correct = (self.root / 'answer.txt').read_text(encoding='utf-8') == 'correct'
        evidence = self.root / f'review-{self.reviews}.json'
        save(evidence, {'basis': candidate['basis'], 'actual_answer_sha256': sha(self.root / 'answer.txt'), 'correct': correct})
        return {'basis': candidate['basis'], 'committed': True, 'settlement': 'Settled',
                'verdict': 'Verified' if correct else {'NeedsChanges': {'feedback': 'Actual answer file differs from correct.'}},
                'evidence': [str(evidence) + '#sha256=' + sha(evidence)]}

    async def prepare(self, continuation, cancelled):
        self.prepares += 1
        feedback = continuation['receipt']['verdict']['NeedsChanges']['feedback']
        assert feedback
        return {'run_id_next': 'run-' + str(continuation['next_run_number']),
                'objective': 'Correct the actual file. Independent Host feedback: ' + feedback}


class NativeProcessTest(unittest.IsolatedAsyncioTestCase):
    def setUp(self):
        self.root = EVIDENCE / self._testMethodName
        self.root.mkdir()
        self.host = FileHost(self.root)

    def config(self, endpoint, **overrides):
        result = {'kind': 'start_task', 'version': 1,
            'task': {'task_id': 'ipc-' + self._testMethodName, 'criteria_revision': 'file-check-v1',
                     'authorization_revision': 'owned-file-only-v1', 'kind': 'Coding',
                     'evidence_dir': str(self.root / 'harness'), 'max_runs': 2, 'max_calls': 4,
                     'deadline_unix_ms': int(time.time() * 1000) + 30000},
            'run': {'run_id': 'run-1', 'objective': 'Use write_answer, submit the actual file and finish.',
                    'cwd': str(self.root), 'session_path': str(self.root / 'session.json'),
                    'environment_state': str(self.root / 'environment'), 'device_fp': 'native-ipc-test',
                    'session_key': 'native/ipc', 'mode': 'execute', 'endpoint': endpoint, 'model': 'jsonl-fixture',
                    'max_calls': 4, 'max_seconds': 25, 'max_output': 512,
                    'model_resolution': {'kind': 'catalog'},
                    'tools': [{'name': 'write_answer', 'description': 'Write the one Host-owned answer file.',
                       'parameters': {'type': 'object', 'properties': {'content': {'type': 'string'}},
                                      'required': ['content'], 'additionalProperties': False}, 'allow_in_plan': False}]},
            'cancel_grace_ms': 1000, 'emit_events': False}
        result.update(overrides)
        save(self.root / 'start.json', result)
        return result

    async def invoke(self, config, **options):
        save(self.root / 'sent-start.json', config)
        result = await run_task(BINARY, config, self.host, token='controlled-no-provider-grant',
                                operation_directory=self.root / 'host-operations', **options)
        save(self.root / 'consumer-outcome.json', result)
        return result

    async def test_python_real_file_feedback_and_same_task_budget(self):
        with ControlledProvider([call('first', 'write_answer', {'content': 'partial'}), text('First submission.'),
                                 call('fixed', 'write_answer', {'content': 'correct'}), text('Corrected submission.')]) as provider:
            outcome = await self.invoke(self.config(provider.endpoint))
            self.assertEqual(outcome['stop'], 'AwaitingHostAcceptance')
            self.assertEqual((outcome['runs'], outcome['observed_calls']), (2, 4))
            self.assertFalse(outcome['task_accepted'])
            self.assertEqual((self.host.writes, self.host.reviews, self.host.prepares), (2, 2, 1))
            self.assertEqual(len(provider.requests), 4)
            save(self.root / 'provider-requests.json', provider.requests)
            self.assertIn('Actual answer file differs', json.dumps(provider.requests[2]))
            rows = [json.loads(row) for row in (self.root / 'harness/run-1-events.jsonl').read_bytes().splitlines()]
            capability = next(row['event'] for row in rows if row['event']['type'] == 'model_capabilities')
            self.assertEqual(capability['source'], 'hub_model_directory')
            self.assertGreater(provider.catalog_requests, 0)

    async def test_node_real_public_host_feedback_uses_original_session_and_budget(self):
        with ControlledProvider([call('first', 'write_answer', {'content': 'partial'}), text('First submission.'),
                                 call('fixed', 'write_answer', {'content': 'correct'}), text('Corrected submission.')]) as provider:
            config = self.config(provider.endpoint)
            probe = HERE.parent / 'clients/node/native-probe.mjs'
            env = {**os.environ, 'ARA_AGENT_MODEL_TOKEN': 'controlled-no-provider-grant'}
            kwargs = {'creationflags': subprocess.CREATE_NO_WINDOW} if os.name == 'nt' else {}
            proc = await asyncio.create_subprocess_exec('node', str(probe), str(BINARY), str(self.root / 'start.json'),
                str(self.root), stdout=asyncio.subprocess.PIPE, stderr=asyncio.subprocess.PIPE, env=env, **kwargs)
            stdout, stderr = await asyncio.wait_for(proc.communicate(), timeout=25)
            (self.root / 'node-stdout.bin').write_bytes(stdout)
            (self.root / 'node-stderr.bin').write_bytes(stderr)
            self.assertEqual(proc.returncode, 0, stderr.decode('utf-8', 'replace'))
            outcome = json.loads((self.root / 'node-outcome.json').read_bytes())
            self.assertEqual(outcome['stop'], 'AwaitingHostAcceptance')
            self.assertEqual((outcome['runs'], outcome['observed_calls']), (2, 4))
            self.assertFalse(outcome['task_accepted'])
            self.assertEqual(len(provider.requests), 4)
            save(self.root / 'provider-requests.json', provider.requests)

    async def test_host_declared_caps_are_not_labeled_as_a_catalog_lookup(self):
        with ControlledProvider([call('one', 'write_answer', {'content': 'correct'}), text('Submitted.')]) as provider:
            config = self.config(provider.endpoint)
            config['run']['model_resolution'] = {'kind': 'host_declared_loopback',
                'capabilities': {'context_window': 1000000, 'max_tokens': 4096, 'input': ['text']}}
            outcome = await self.invoke(config)
            self.assertEqual(outcome['stop'], 'AwaitingHostAcceptance')
            rows = [json.loads(row) for row in (self.root / 'harness/run-1-events.jsonl').read_bytes().splitlines()]
            capability = next(row['event'] for row in rows if row['event']['type'] == 'model_capabilities')
            self.assertEqual(capability['source'], 'host_model_configuration')
            self.assertEqual(provider.catalog_requests, 0)

    async def test_catalog_capped_lowers_the_declared_window_below_the_catalog_value(self):
        with ControlledProvider([call('one', 'write_answer', {'content': 'correct'}), text('Submitted.')]) as provider:
            config = self.config(provider.endpoint)
            config['run']['model_resolution'] = {'kind': 'catalog_capped', 'context_window_cap': 98304}
            outcome = await self.invoke(config)
            self.assertEqual(outcome['stop'], 'AwaitingHostAcceptance')
            rows = [json.loads(row) for row in (self.root / 'harness/run-1-events.jsonl').read_bytes().splitlines()]
            capability = next(row['event'] for row in rows if row['event']['type'] == 'model_capabilities')
            self.assertEqual(capability['source'], 'hub_model_directory')
            # The loopback catalog declares 1000000 tokens; max_output 512 is the effective output limit.
            self.assertEqual(capability['declared']['context_window'], 98304)
            self.assertEqual(capability['text_budget'], {'projection_bytes': 46848, 'request_bytes': 93696})
            self.assertGreater(provider.catalog_requests, 0)

    async def test_catalog_capped_out_of_range_is_refused_before_any_request(self):
        with ControlledProvider([]) as provider:
            config = self.config(provider.endpoint)
            for cap in (1023, 10_000_001):
                config['run']['model_resolution'] = {'kind': 'catalog_capped', 'context_window_cap': cap}
                save(self.root / f'sent-start-{cap}.json', config)
                options = {'creationflags': subprocess.CREATE_NO_WINDOW} if os.name == 'nt' else {}
                proc = await asyncio.create_subprocess_exec(str(BINARY), stdin=asyncio.subprocess.PIPE,
                    stdout=asyncio.subprocess.PIPE, stderr=asyncio.subprocess.PIPE,
                    env={**os.environ, 'ARA_AGENT_MODEL_TOKEN': 'controlled-no-provider-grant'}, **options)
                stdout, stderr = await asyncio.wait_for(proc.communicate(encoded(config)), timeout=10)
                self.assertEqual((proc.returncode, stdout), (2, b''), cap)
                self.assertEqual(stderr.decode('utf-8').strip(), 'start_config_rejected', cap)
            self.assertEqual((provider.catalog_requests, len(provider.requests)), (0, 0))
            self.assertFalse((self.root / 'harness').exists())

    async def test_original_budget_never_restarts_relay_or_second_run(self):
        with ControlledProvider([call('first', 'write_answer', {'content': 'partial'}), text('Submit.')]) as provider:
            config = self.config(provider.endpoint)
            config['task']['max_calls'] = 2
            outcome = await self.invoke(config)
            self.assertEqual(outcome['stop'], 'BudgetExhausted')
            self.assertEqual((outcome['runs'], outcome['observed_calls']), (1, 2))
            self.assertEqual(self.host.prepares, 0)
            self.assertEqual(len(provider.requests), 2)

    async def test_unknown_tool_effect_is_never_verified_or_replayed(self):
        self.host.unknown = True
        with ControlledProvider([call('unknown', 'write_answer', {'content': 'correct'}), text('Not proof.')]) as provider:
            outcome = await self.invoke(self.config(provider.endpoint))
            self.assertNotEqual(outcome['stop'], 'AwaitingHostAcceptance')
            self.assertEqual(outcome['fence_presence'], 'Present')
            self.assertEqual((self.host.writes, self.host.reviews, self.host.prepares), (1, 0, 0))
            self.assertTrue(Path(outcome['reconciliation_fence']).is_file())

    async def test_secret_collision_is_rejected_without_rewriting_the_authorized_operation(self):
        with ControlledProvider([call('collision', 'write_answer', {'content': 'controlled-no-provider-grant'})]) as provider:
            outcome = await self.invoke(self.config(provider.endpoint))
            self.assertNotEqual(outcome['stop'], 'AwaitingHostAcceptance')
            self.assertEqual(outcome['fence_presence'], 'Present')
            self.assertEqual((self.host.writes, self.host.reviews, self.host.prepares), (0, 0, 0))
            self.assertFalse((self.root / 'answer.txt').exists())
            self.assertEqual(len(provider.requests), 1)

    async def test_cancel_keeps_original_callback_until_it_settles(self):
        self.host.slow = True
        cancel = asyncio.Event()
        with ControlledProvider([call('slow', 'write_answer', {'content': 'correct'}), text('Not admitted.')]) as provider:
            task = asyncio.create_task(self.invoke(self.config(provider.endpoint), cancellation=cancel))
            await asyncio.wait_for(self.host.started.wait(), timeout=8)
            cancel.set()
            await asyncio.sleep(.04)
            self.assertFalse(task.done())
            self.assertFalse(self.host.joined)
            outcome = await asyncio.wait_for(task, timeout=8)
            self.assertTrue(self.host.joined)
            self.assertNotEqual(outcome['stop'], 'AwaitingHostAcceptance')
            self.assertEqual(outcome['fence_presence'], 'Present')
            self.assertEqual((self.host.writes, self.host.reviews, self.host.prepares), (1, 0, 0))
            self.assertEqual(len(provider.requests), 1)

    async def raw_start(self, config):
        env = os.environ.copy()
        env['ARA_AGENT_MODEL_TOKEN'] = 'controlled-no-provider-grant'
        kwargs = {'creationflags': subprocess.CREATE_NO_WINDOW} if os.name == 'nt' else {}
        proc = await asyncio.create_subprocess_exec(str(BINARY), stdin=asyncio.subprocess.PIPE,
            stdout=asyncio.subprocess.PIPE, stderr=asyncio.subprocess.PIPE, env=env, limit=262145, **kwargs)
        proc.stdin.write(encoded(config))
        await proc.stdin.drain()
        return proc

    async def raw_read(self, proc, kind):
        while True:
            raw = await asyncio.wait_for(proc.stdout.readline(), timeout=12)
            self.assertTrue(raw, 'peer exited without expected protocol result')
            message = json.loads(raw)
            if message['kind'] == kind:
                return message

    async def raw_finish(self, proc):
        frame = await self.raw_read(proc, 'task_result')
        outcome = read_reference(frame['outcome_ref'], self.root / 'harness')
        await asyncio.wait_for(proc.wait(), timeout=5)
        stderr = await proc.stderr.read()
        (self.root / 'runtime-stderr.bin').write_bytes(stderr)
        save(self.root / 'raw-task-result.json', frame)
        save(self.root / 'consumer-outcome.json', outcome)
        return outcome

    async def bad_correlation(self, field, replacement):
        with ControlledProvider([call('original', 'write_answer', {'content': 'correct'})]) as provider:
            proc = await self.raw_start(self.config(provider.endpoint))
            try:
                request = await self.raw_read(proc, 'tool_call')
                # Observe an actual Host effect before the malformed reply.
                (self.root / 'answer.txt').write_text('written-once', encoding='utf-8')
                save(self.root / 'original-operation.json', {'request': request, 'effect': 'written-once'})
                reply = {key: value for key, value in request.items() if key != 'arguments'}
                reply['kind'] = 'tool_reply'
                reply['result'] = {'value': 'done', 'is_error': False, 'settlement': 'Settled',
                                   'operation_receipt': str(self.root / 'original-operation.json')}
                reply[field] = replacement
                proc.stdin.write(encoded(reply))
                await proc.stdin.drain()
                outcome = await self.raw_finish(proc)
                self.assertEqual(outcome['fence_presence'], 'Present')
                self.assertNotEqual(outcome['stop'], 'AwaitingHostAcceptance')
                self.assertEqual((self.root / 'answer.txt').read_text(), 'written-once')
                self.assertEqual(len(provider.requests), 1)
            finally:
                if proc.returncode is None:
                    proc.kill()
                    await proc.wait()

    async def test_wrong_task_reply_fences_original_operation(self):
        await self.bad_correlation('task_id', 'different-task')

    async def test_wrong_tool_identity_reply_fences_original_operation(self):
        await self.bad_correlation('native_call_id', 'different-native-call')

    async def test_wrong_reply_kind_never_becomes_a_host_review(self):
        await self.bad_correlation('kind', 'verify_reply')

    async def test_stdin_loss_preserves_original_effect_and_session_fence(self):
        with ControlledProvider([call('original', 'write_answer', {'content': 'correct'})]) as provider:
            proc = await self.raw_start(self.config(provider.endpoint))
            try:
                request = await self.raw_read(proc, 'tool_call')
                (self.root / 'answer.txt').write_text('original-effect', encoding='utf-8')
                save(self.root / 'original-operation.json', request)
                proc.stdin.close()
                outcome = await self.raw_finish(proc)
                self.assertEqual(outcome['fence_presence'], 'Present')
                self.assertEqual((self.root / 'answer.txt').read_text(), 'original-effect')
                self.assertEqual(len(provider.requests), 1)
            finally:
                if proc.returncode is None:
                    proc.kill()
                    await proc.wait()

    async def test_declared_caps_reject_public_non_catalog_route_before_effects(self):
        with ControlledProvider([text('Must not run.')]) as provider:
            config = self.config(provider.endpoint)
            config['run']['endpoint'] = 'https://example.invalid/v1'
            config['run']['model_resolution'] = {'kind': 'host_declared_loopback',
                'capabilities': {'context_window': None, 'max_tokens': 8192, 'input': ['text']}}
            with self.assertRaises(ReconciliationRequired):
                await self.invoke(config)
            self.assertEqual(provider.requests, [])
            self.assertFalse((self.root / 'answer.txt').exists())

    async def test_slow_stdout_does_not_block_original_core_drain_or_task_exit(self):
        flood = text('unused')
        flood['deltas'] = [{'content': 'x' * 4096} for _ in range(96)]
        with ControlledProvider([flood]) as provider:
            config = self.config(provider.endpoint, emit_events=True)
            config['task']['deadline_unix_ms'] = int(time.time() * 1000) + 9000
            config['run']['max_seconds'] = 8
            env = {**os.environ, 'ARA_AGENT_MODEL_TOKEN': 'controlled-no-provider-grant'}
            with (self.root / 'stderr.bin').open('wb') as errors:
                proc = subprocess.Popen([str(BINARY)], stdin=subprocess.PIPE, stdout=subprocess.PIPE,
                    stderr=errors, env=env, creationflags=subprocess.CREATE_NO_WINDOW if os.name == 'nt' else 0)
                try:
                    proc.stdin.write(encoded(config))
                    proc.stdin.flush()
                    # Deliberately never drain stdout until the original process exits.
                    code = await asyncio.to_thread(proc.wait, timeout=18)
                    raw = proc.stdout.read()
                    (self.root / 'undrained-stdout.bin').write_bytes(raw)
                    events = self.root / 'harness/run-1-events.jsonl'
                    self.assertTrue(events.is_file())
                    self.assertGreater(len(events.read_bytes().splitlines()), 8)
                    self.assertTrue(list(self.root.glob('.ara-lite-harness-*.fence.json')))
                    save(self.root / 'native-exit.json', {'code': code, 'stdout_drained_before_exit': False,
                         'durable_event_count': len(events.read_bytes().splitlines())})
                finally:
                    if proc.poll() is None:
                        proc.kill()
                        proc.wait(timeout=5)
                    proc.stdin.close()
                    proc.stdout.close()


class ClientArtifactTest(unittest.TestCase):
    def test_nested_candidate_identity_is_checked_before_host_callback(self):
        spec = {'task_id': 'task-one', 'criteria_revision': 'criteria-one', 'authorization_revision': 'grant-one'}
        frame = {'run_id': 'run-one'}
        basis = {**spec, 'run_id': frame['run_id']}
        validate_candidate_identity({'basis': basis}, frame, spec)
        for field in (*spec, 'run_id'):
            with self.subTest(field=field), self.assertRaises(ReconciliationRequired):
                validate_candidate_identity({'basis': {**basis, field: 'foreign'}}, frame, spec)

    def test_json_frame_limit_counts_encoded_unicode_bytes(self):
        with self.assertRaises(ReconciliationRequired):
            encoded({'text': '汉' * 100000})

    def test_reference_requires_original_hash_and_contained_path(self):
        root = EVIDENCE / self._testMethodName
        root.mkdir()
        path = root / 'outcome.json'
        save(path, {'task_id': 'one'})
        reference = {'path': str(path), 'sha256': sha(path)}
        self.assertEqual(read_reference(reference, root), {'task_id': 'one'})
        if os.name == 'nt':
            verbatim = {'path': '\\\\?\\' + str(path), 'sha256': sha(path)}
            self.assertEqual(read_reference(verbatim, root), {'task_id': 'one'})
            self.assertEqual(read_reference(reference, Path('\\\\?\\' + str(root))), {'task_id': 'one'})
        path.write_text('{}')
        with self.assertRaises(ReconciliationRequired):
            read_reference(reference, root)
        with self.assertRaises(ReconciliationRequired):
            read_reference({'path': str(path.parent.parent / 'outside.json'), 'sha256': '0' * 64}, root)


if __name__ == '__main__':
    unittest.main(verbosity=2)
