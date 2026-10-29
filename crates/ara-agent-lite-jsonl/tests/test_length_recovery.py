"""Actual shared Core length recovery, within the original native Run budget."""
import asyncio
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
from ara_agent_lite import encoded, run_task
from controlled_provider import ControlledProvider, call, text

BINARY = Path(os.environ.get('ARA_JSONL_TEST_BINARY',
    'D:/Temp/ara-build/ara-github-target-20261005/debug/ara-agent-lite-jsonl.exe'))
EVIDENCE = Path(os.environ['ARA_JSONL_LENGTH_EVIDENCE'])
TOKEN = 'length-fixture-no-provider-grant'
PRIVATE = 'CONTROLLED_THINKING_SENTINEL_DO_NOT_REPLAY'


def save(path, value):
    with Path(path).open('x', encoding='utf-8') as stream:
        json.dump(value, stream, ensure_ascii=False, indent=2)
        stream.write('\n')
        stream.flush()
        os.fsync(stream.fileno())


def length(*, thinking=False):
    return {'delta': {'reasoning_content' if thinking else 'content': PRIVATE if thinking else 'Partial answer.'},
            'finish': 'length'}


class FileHost:
    def __init__(self, root):
        self.root = root
        self.writes = self.rejections = self.reviews = 0
        self.unknown = False

    async def tool_call(self, request, cancelled):
        self.assert_not_cancelled(cancelled)
        receipt = self.root / ('operation-' + request['rpc_id'] + '.json')
        if request['arguments']['content'] == 'known-reject':
            self.rejections += 1
            save(receipt, {'status': 'known_not_started', 'subprocess_started': False, 'request': request})
            return {'value': {'category': 'checks_require_actual_patch', 'subprocess_started': False},
                    'is_error': True, 'settlement': 'Settled', 'operation_receipt': str(receipt)}
        self.writes += 1
        with (self.root / 'answer.txt').open('w', encoding='utf-8') as stream:
            stream.write(request['arguments']['content'])
            stream.flush()
            os.fsync(stream.fileno())
        save(receipt, {'status': 'completed', 'request': request,
             'answer_sha256': hashlib.sha256((self.root / 'answer.txt').read_bytes()).hexdigest()})
        return {'value': {'written': True}, 'is_error': False,
                'settlement': 'Unknown' if self.unknown else 'Settled', 'operation_receipt': str(receipt)}

    @staticmethod
    def assert_not_cancelled(cancelled):
        if cancelled.is_set():
            raise RuntimeError('not invoked after cancellation')

    async def verify(self, candidate, cancelled):
        self.assert_not_cancelled(cancelled)
        self.reviews += 1
        assert (self.root / 'answer.txt').read_text() == 'correct'
        receipt = self.root / 'actual-review.json'
        save(receipt, {'basis': candidate['basis'], 'actual_answer': 'correct'})
        return {'basis': candidate['basis'], 'committed': True, 'settlement': 'Settled',
                'verdict': 'Verified', 'evidence': [str(receipt) + '#sha256=' + hashlib.sha256(receipt.read_bytes()).hexdigest()]}

    async def prepare(self, *_):
        raise AssertionError('length recovery must not use a new Harness Run')


class LengthRecoveryTest(unittest.IsolatedAsyncioTestCase):
    def setUp(self):
        self.root = EVIDENCE / self._testMethodName
        self.root.mkdir()
        self.host = FileHost(self.root)

    def config(self, endpoint, *, policy='replan_once', calls=6):
        run = {'run_id': 'one-original-run', 'objective': 'Write the actual answer file with exactly correct and submit.',
            'cwd': str(self.root), 'session_path': str(self.root / 'session.json'),
            'environment_state': str(self.root / 'environment'), 'device_fp': 'length-fixture',
            'session_key': 'length/original', 'mode': 'execute', 'endpoint': endpoint, 'model': 'jsonl-fixture',
            'max_calls': calls, 'max_seconds': 20, 'max_output': 512,
            'model_resolution': {'kind': 'catalog'},
            'tools': [{'name': 'write_answer', 'description': 'Write the one authorized answer file.',
                'parameters': {'type': 'object', 'properties': {'content': {'type': 'string'}},
                    'required': ['content'], 'additionalProperties': False}, 'allow_in_plan': False}]}
        if policy is not None:
            run['length_recovery'] = policy
        return {'kind': 'start_task', 'version': 1,
            'task': {'task_id': self._testMethodName, 'criteria_revision': 'actual-file-v1',
                'authorization_revision': 'one-file-v1', 'kind': 'Coding', 'evidence_dir': str(self.root / 'harness'),
                'max_runs': 1, 'max_calls': calls, 'deadline_unix_ms': int(time.time() * 1000) + 25000},
            'run': run, 'cancel_grace_ms': 1000, 'emit_events': False}

    async def invoke(self, config, **options):
        save(self.root / 'start.json', config)
        result = await asyncio.wait_for(run_task(BINARY, config, self.host, token=TOKEN,
            operation_directory=self.root / 'host-operations', **options), timeout=30)
        save(self.root / 'outcome.json', result)
        return result

    def events(self):
        return [json.loads(row)['event'] for row in (self.root / 'harness/run-1-events.jsonl').read_bytes().splitlines()]

    def retained_length(self, expected=1):
        session = json.loads((self.root / 'session.json').read_bytes())
        assistants = [row for row in session['messages'] if row.get('role') == 'assistant']
        # Native AssistantMessage is untagged at this level; do not infer success
        # from final text. Retain its original explicit provider stopReason.
        lengths = [row for row in assistants if row.get('stopReason') == 'length']
        self.assertEqual(len(lengths), expected)
        self.assertIsNone(session['pending_run'])
        self.assertEqual(session['completed_runs'], ['one-original-run'])
        return lengths

    def successful(self, outcome, calls, recoveries=1):
        self.assertEqual((outcome['stop'], outcome['runs'], outcome['observed_calls']),
                         ('AwaitingHostAcceptance', 1, calls))
        self.assertFalse(outcome['task_accepted'])
        self.assertEqual(outcome['fence_presence'], 'Released')
        self.assertEqual((self.host.writes, self.host.reviews), (1, 1))
        events = self.events()
        self.assertEqual(sum(row['type'] == 'agent_start' for row in events), 1)
        self.assertEqual(sum(row['type'] == 'agent_end' for row in events), 1)
        self.assertEqual([row['call'] for row in events if row['type'] == 'model_call'], list(range(1, calls + 1)))
        self.assertEqual(sum(row['type'] == 'length_recovery' for row in events), recoveries)
        self.retained_length()

    async def test_default_omitted_preserves_the_original_length_failure(self):
        with ControlledProvider([length(), text('Must not be requested.')]) as provider:
            outcome = await self.invoke(self.config(provider.endpoint, policy=None))
            self.assertEqual((outcome['stop'], outcome['runs'], outcome['observed_calls']), ('ReconciliationRequired', 1, 1))
            self.assertEqual(outcome['fence_presence'], 'Present')
            self.assertEqual(len(provider.requests), 1)
            self.assertEqual((self.host.writes, self.host.reviews), (0, 0))
            self.assertFalse(any(row['type'] == 'length_recovery' for row in self.events()))
            self.retained_length()

    async def test_text_only_length_replans_in_the_same_run(self):
        with ControlledProvider([length(), call('only-write', 'write_answer', {'content': 'correct'}), text('Verified file submitted.')]) as provider:
            outcome = await self.invoke(self.config(provider.endpoint))
            self.successful(outcome, 3)
            save(self.root / 'requests.json', provider.requests)
            self.assertEqual({row['max_tokens'] for row in provider.requests}, {512})
            self.assertEqual(provider.requests[0]['tools'], provider.requests[1]['tools'])

    async def test_thinking_only_length_does_not_replay_private_reasoning(self):
        with ControlledProvider([length(thinking=True), call('only-write', 'write_answer', {'content': 'correct'}), text('Verified file submitted.')]) as provider:
            outcome = await self.invoke(self.config(provider.endpoint))
            self.successful(outcome, 3)
            self.assertNotIn(PRIVATE, json.dumps(provider.requests[1]))
            lengths = self.retained_length()
            self.assertIn(PRIVATE, json.dumps(lengths))
            save(self.root / 'requests.json', provider.requests)

    async def test_second_length_still_fails_without_unbounded_replanning(self):
        with ControlledProvider([length(thinking=True), length(), text('Must not be requested.')]) as provider:
            outcome = await self.invoke(self.config(provider.endpoint))
            self.assertEqual((outcome['stop'], outcome['observed_calls']), ('ReconciliationRequired', 2))
            self.assertEqual(outcome['fence_presence'], 'Present')
            self.assertEqual(len(provider.requests), 2)
            self.assertEqual(sum(row['type'] == 'length_recovery' for row in self.events()), 1)
            self.assertEqual((self.host.writes, self.host.reviews), (0, 0))
            self.retained_length(expected=2)

    async def test_original_call_budget_prevents_an_extra_http_request(self):
        with ControlledProvider([length(thinking=True), text('Must not be requested.')]) as provider:
            outcome = await self.invoke(self.config(provider.endpoint, calls=1))
            self.assertEqual(outcome['observed_calls'], 1)
            self.assertNotEqual(outcome['stop'], 'AwaitingHostAcceptance')
            self.assertEqual(len(provider.requests), 1)
            self.assertEqual((self.host.writes, self.host.reviews), (0, 0))
            self.assertFalse(any(row['type'] == 'length_recovery' for row in self.events()))

    async def test_settled_effect_before_length_is_not_replayed(self):
        with ControlledProvider([call('only-write', 'write_answer', {'content': 'correct'}), length(), text('Verified file submitted.')]) as provider:
            outcome = await self.invoke(self.config(provider.endpoint))
            self.successful(outcome, 3)
            self.assertTrue(any(row.get('role') == 'tool' for row in provider.requests[2]['messages']))
            save(self.root / 'requests.json', provider.requests)

    async def test_known_no_effect_tool_error_allows_scoped_replanning(self):
        with ControlledProvider([call('known-reject', 'write_answer', {'content': 'known-reject'}), length(),
                call('only-write', 'write_answer', {'content': 'correct'}), text('Verified file submitted.')]) as provider:
            outcome = await self.invoke(self.config(provider.endpoint))
            self.successful(outcome, 4)
            self.assertEqual(self.host.rejections, 1)
            save(self.root / 'requests.json', provider.requests)

    async def test_truncated_tool_call_uses_original_native_fail_all_guard(self):
        incomplete = call('never-execute', 'write_answer', {'content': 'must-not-be-written'})
        incomplete['finish'] = 'length'
        with ControlledProvider([incomplete, call('only-write', 'write_answer', {'content': 'correct'}), text('Verified file submitted.')]) as provider:
            outcome = await self.invoke(self.config(provider.endpoint))
            self.successful(outcome, 3, recoveries=0)
            session = json.loads((self.root / 'session.json').read_bytes())
            rejected = [row for row in session['messages'] if row.get('toolCallId') == 'never-execute']
            self.assertEqual(len(rejected), 1)
            self.assertTrue(rejected[0]['isError'])
            save(self.root / 'requests.json', provider.requests)

    async def test_unknown_prior_effect_still_stops_without_followup(self):
        self.host.unknown = True
        with ControlledProvider([call('unknown-write', 'write_answer', {'content': 'correct'}), length()]) as provider:
            outcome = await self.invoke(self.config(provider.endpoint))
            self.assertEqual(outcome['fence_presence'], 'Present')
            self.assertNotEqual(outcome['stop'], 'AwaitingHostAcceptance')
            self.assertEqual((self.host.writes, self.host.reviews), (1, 0))
            self.assertFalse(any(row['type'] == 'length_recovery' for row in self.events()))
            self.assertEqual(len(provider.requests), 1)

    async def test_context_reference_commit_failure_stops_before_another_request(self):
        def refuse_commit(_):
            original = self.root / 'session.context.json'
            original.rename(self.root / 'session.context-before-fault.json')
            original.mkdir()  # Real rename target fault; preserve the original bytes.
            return length(thinking=True)

        with ControlledProvider([refuse_commit, text('Must not be requested.')]) as provider:
            outcome = await self.invoke(self.config(provider.endpoint))
            self.assertEqual(outcome['fence_presence'], 'Present')
            self.assertNotEqual(outcome['stop'], 'AwaitingHostAcceptance')
            self.assertEqual(len(provider.requests), 1)
            self.assertEqual((self.host.writes, self.host.reviews), (0, 0))
            self.assertFalse(any(row['type'] == 'length_recovery' for row in self.events()))
            before = json.loads((self.root / 'session.context-before-fault.json').read_bytes())
            self.assertFalse(any(row['kind'] == 'length_recovery' for row in before['references']))
            self.retained_length()

    async def test_cancelled_model_turn_never_replans_or_invents_a_human_turn(self):
        cancellation = asyncio.Event()
        loop = asyncio.get_running_loop()

        def cancel_original(_):
            loop.call_soon_threadsafe(cancellation.set)
            return {**length(thinking=True), 'delay_ms': 600}

        with ControlledProvider([cancel_original, text('Must not be requested.')]) as provider:
            outcome = await self.invoke(self.config(provider.endpoint), cancellation=cancellation)
            self.assertNotEqual(outcome['stop'], 'AwaitingHostAcceptance')
            self.assertEqual(outcome['fence_presence'], 'Present')
            self.assertEqual(len(provider.requests), 1)
            self.assertFalse(any(row['type'] == 'length_recovery' for row in self.events()))
            self.assertEqual((self.host.writes, self.host.reviews), (0, 0))

    async def test_original_deadline_cancels_the_turn_without_length_followup(self):
        with ControlledProvider([{**length(thinking=True), 'delay_ms': 6000}, text('Must not be requested.')]) as provider:
            config = self.config(provider.endpoint)
            # Windows first-use persistence can consume a one-second window
            # before HTTP starts. Keep startup outside the discriminating
            # interval, then hold the actual upstream beyond the Run deadline.
            config['run']['max_seconds'] = 4
            config['task']['deadline_unix_ms'] = int(time.time() * 1000) + 12000
            outcome = await self.invoke(config)
            self.assertNotEqual(outcome['stop'], 'AwaitingHostAcceptance')
            self.assertEqual(outcome['fence_presence'], 'Present')
            self.assertEqual(len(provider.requests), 1)
            self.assertFalse(any(row['type'] == 'length_recovery' for row in self.events()))
            self.assertEqual((self.host.writes, self.host.reviews), (0, 0))

    async def test_explicit_plan_and_unknown_policy_are_rejected_before_provider_io(self):
        with ControlledProvider([]) as provider:
            for name, mode, policy in [('plan', 'plan', 'replan_once'), ('unknown', 'execute', 'unbounded')]:
                config = self.config(provider.endpoint, policy=policy)
                config['run']['mode'] = mode
                save(self.root / (name + '-start.json'), config)
                options = {'creationflags': subprocess.CREATE_NO_WINDOW} if os.name == 'nt' else {}
                process = await asyncio.create_subprocess_exec(str(BINARY), stdin=asyncio.subprocess.PIPE,
                    stdout=asyncio.subprocess.PIPE, stderr=asyncio.subprocess.PIPE,
                    env={**os.environ, 'ARA_AGENT_MODEL_TOKEN': TOKEN}, **options)
                stdout, stderr = await asyncio.wait_for(process.communicate(encoded(config)), timeout=8)
                (self.root / (name + '-stdout.bin')).write_bytes(stdout)
                (self.root / (name + '-stderr.bin')).write_bytes(stderr)
                self.assertEqual(process.returncode, 2)
                self.assertFalse(stdout)
            self.assertEqual(provider.catalog_requests, 0)
            self.assertEqual(provider.requests, [])
            self.assertFalse((self.root / 'session.json').exists())
            self.assertFalse((self.root / 'harness').exists())


if __name__ == '__main__':
    unittest.main(verbosity=2)
