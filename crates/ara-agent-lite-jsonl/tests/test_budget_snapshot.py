"""Actual native budget snapshots, fixed Host files and typed SDK callbacks.

Run only against the explicitly pinned current image. No actual model or product.
"""
from __future__ import annotations

import asyncio
from contextlib import contextmanager
import copy
import hashlib
import json
import os
from pathlib import Path
import subprocess
import sys
import time
import unittest
from unittest.mock import patch

HERE = Path(__file__).resolve().parent
sys.path.insert(0, str(HERE.parent / 'clients/python'))
from ara_agent_lite import ReconciliationRequired, encoded, read_budget_review_reference, run_task
from controlled_provider import ControlledProvider, call, text

BINARY = Path(os.environ.get('ARA_JSONL_TEST_BINARY', 'D:/Temp/ara-build/ara-github-target-20261005/debug/ara-agent-lite-jsonl.exe'))
EXPECTED_SHA = os.environ.get('ARA_JSONL_BUDGET_BINARY_SHA256', '')
EVIDENCE = Path(os.environ.get('ARA_JSONL_BUDGET_EVIDENCE', 'D:/WorkSpace/tmp/ara-core-capability-20261022/closed-budget-snapshot-v1-20261025/native'))
TOKEN = 'controlled-budget-no-real-provider-grant'


def sha(path):
    return hashlib.sha256(Path(path).read_bytes()).hexdigest()


def save(path, value):
    with Path(path).open('x', encoding='utf-8') as stream:
        json.dump(value, stream, ensure_ascii=False, indent=2)
        stream.write('\n')
        stream.flush()
        os.fsync(stream.fileno())


class FileHost:
    def __init__(self, root, config):
        self.root, self.config = root, config
        self.writes, self.budget_reviews, self.budget_prepares = [], [], []
        self.ordinary_reviews, self.ordinary_prepares = [], []
        self.unknown = self.known_error = self.tamper_snapshot = self.tamper_continuation = False

    async def tool_call(self, request, cancelled):
        assert not cancelled.is_set() and request['tool_name'] == 'write_answer'
        assert set(request['arguments']) == {'content'}
        number = len(self.writes) + 1
        save(self.root / f'write-{number}-started.json', request)
        target = self.root / 'answer.txt'
        with target.open('w', encoding='utf-8') as stream:
            stream.write(request['arguments']['content'])
            stream.flush()
            os.fsync(stream.fileno())
        receipt = self.root / f'write-{number}-settled.json'
        save(receipt, {'source_request': request, 'actual_answer': target.read_text(encoding='utf-8'), 'sha256': sha(target)})
        self.writes.append(request)
        return {'value': {'written': True, 'known_error': self.known_error}, 'is_error': self.known_error,
                'settlement': 'Unknown' if self.unknown else 'Settled', 'operation_receipt': str(receipt)}

    def receipt(self, subject, number, prefix):
        actual = (self.root / 'answer.txt').read_text(encoding='utf-8')
        path = self.root / f'{prefix}-{number}.json'
        save(path, {'subject': subject, 'actual_answer': actual, 'actual_sha256': sha(self.root / 'answer.txt')})
        return {'basis': subject['basis'], 'committed': True, 'settlement': 'Settled',
                'verdict': 'Verified' if actual == 'beta' else {'NeedsChanges': {'feedback': 'Actual file is not beta. Write beta.'}},
                'evidence': [str(path) + '#sha256=' + sha(path)]}

    async def verify(self, candidate, cancelled):
        self.ordinary_reviews.append(candidate)
        return self.receipt(candidate, len(self.ordinary_reviews), 'ordinary-review')

    async def prepare(self, continuation, cancelled):
        self.ordinary_prepares.append(continuation)
        return {'run_id_next': f"run-{continuation['next_run_number']}", 'objective': 'Write beta. ' + continuation['receipt']['verdict']['NeedsChanges']['feedback']}

    async def verify_budget_snapshot(self, snapshot, cancelled):
        assert not cancelled.is_set()
        assert set(snapshot) == {'basis', 'diagnostic_output', 'goal', 'termination'}
        assert snapshot['termination'] == {'version': 1, 'cause': 'call_budget_exhausted', 'native_round_closed': True, 'model_final_stop': False}
        self.budget_reviews.append(copy.deepcopy(snapshot))
        result = self.receipt(snapshot, len(self.budget_reviews), 'budget-review')
        if self.tamper_snapshot:
            changed = {**snapshot, 'diagnostic_output': 'tampered in verifier'}
            path = self.root / 'harness' / f"run-{snapshot['basis']['run_number']}-budget-snapshot.json"
            path.write_text(json.dumps(changed), encoding='utf-8')
        return result

    async def prepare_budget_stop_continuation(self, continuation, cancelled):
        assert not cancelled.is_set()
        assert continuation['deadline_unix_ms'] == self.config['task']['deadline_unix_ms']
        assert continuation['remaining_calls'] > 0
        assert set(continuation['previous']) == {'basis', 'diagnostic_output', 'goal', 'termination'}
        self.budget_prepares.append(copy.deepcopy(continuation))
        if self.tamper_continuation:
            path = self.root / 'harness' / f"jsonl-budget-continuation-{continuation['next_run_number']}.json"
            path.write_text(json.dumps({**continuation, 'remaining_calls': 999}), encoding='utf-8')
        return {'run_id_next': f"run-{continuation['next_run_number']}", 'objective': 'Write beta. ' + continuation['receipt']['verdict']['NeedsChanges']['feedback']}


class BudgetSnapshotTest(unittest.IsolatedAsyncioTestCase):
    def setUp(self):
        self.assertRegex(EXPECTED_SHA, r'^[0-9a-fA-F]{64}$', 'pin ARA_JSONL_BUDGET_BINARY_SHA256')
        self.assertEqual(sha(BINARY), EXPECTED_SHA.lower())
        EVIDENCE.mkdir(parents=True, exist_ok=True)
        self.root = EVIDENCE / (self._testMethodName + '-' + str(time.time_ns()))
        self.root.mkdir()
        self.processes = []
        save(self.root / 'identity.json', {'binary': str(BINARY), 'binary_sha256': sha(BINARY), 'test_sha256': sha(__file__), 'actual_model': False})
        print('BUDGET_SNAPSHOT_EVIDENCE=' + str(self.root), flush=True)

    @contextmanager
    def provider(self, replies):
        with ControlledProvider(replies) as provider:
            try:
                yield provider
            finally:
                save(self.root / 'provider-requests.json', provider.requests)
                save(self.root / 'provider-counts.json', {'chat_http': len(provider.requests), 'catalog_http': provider.catalog_requests})

    def config(self, endpoint, *, run_calls=1, task_calls=1, runs=1, policy='review_closed_tool_round'):
        result = {'kind': 'start_task', 'version': 1,
            'task': {'task_id': self._testMethodName, 'criteria_revision': 'actual-beta-file-v1', 'authorization_revision': 'owned-answer-only-v1',
                'kind': 'Coding', 'evidence_dir': str(self.root / 'harness'), 'max_runs': runs, 'max_calls': task_calls,
                'deadline_unix_ms': int(time.time() * 1000) + 30000},
            'run': {'run_id': 'run-1', 'objective': 'Write beta to the actual Host file.', 'cwd': str(self.root),
                'session_path': str(self.root / 'session.json'), 'environment_state': str(self.root / 'environment'),
                'device_fp': 'budget-fixture', 'session_key': 'budget/original-session', 'mode': 'execute', 'endpoint': endpoint,
                'model': 'jsonl-fixture', 'max_calls': run_calls, 'max_seconds': 25, 'max_output': 512,
                'model_resolution': {'kind': 'catalog'}, 'tools': [{'name': 'write_answer', 'description': 'Write one owned answer file.',
                    'parameters': {'type': 'object', 'properties': {'content': {'type': 'string'}}, 'required': ['content'], 'additionalProperties': False}, 'allow_in_plan': False}]},
            'cancel_grace_ms': 1000, 'emit_events': False}
        if policy is not None:
            result['budget_stop_review'] = policy
        return result

    async def invoke(self, config, host, *, expected_callback_fault=False):
        save(self.root / 'start.json', config)
        original = asyncio.create_subprocess_exec
        async def process(*args, **kwargs):
            proc = await original(*args, **kwargs)
            self.processes.append(proc)
            return proc
        try:
            with patch('asyncio.create_subprocess_exec', process):
                try:
                    outcome = await asyncio.wait_for(run_task(BINARY, config, host, token=TOKEN,
                        operation_directory=self.root / 'host-operations'), timeout=40)
                except ReconciliationRequired:
                    if not expected_callback_fault:
                        raise
                    path = self.root / 'harness' / 'outcome.json'
                    outcome = json.loads(path.read_text(encoding='utf-8'))
                    save(self.root / 'sdk-callback-fault.json', {'kind': 'ReconciliationRequired', 'no_replay': True})
            save(self.root / 'consumer-outcome.json', outcome)
            return outcome
        finally:
            save(self.root / 'processes.json', [{'pid': p.pid, 'exit_code': p.returncode} for p in self.processes])
            save(self.root / 'host-ledger.json', {'writes': len(host.writes), 'budget_reviews': host.budget_reviews,
                'budget_prepares': host.budget_prepares, 'ordinary_reviews': host.ordinary_reviews, 'ordinary_prepares': host.ordinary_prepares})

    def events(self):
        return [json.loads(line)['event'] for path in sorted((self.root / 'harness').glob('run-*-events.jsonl')) for line in path.read_bytes().splitlines()]

    def assert_budget_terminal(self, calls):
        events = self.events()
        self.assertEqual(len([e for e in events if e['type'] == 'model_call']), calls)
        ends = [e for e in events if e['type'] == 'agent_end']
        self.assertTrue(ends)
        for end in ends:
            self.assertEqual((end['status'], end['code']), ('failed', -1))
            self.assertEqual(end['termination'], {'version': 1, 'cause': 'call_budget_exhausted', 'native_round_closed': True, 'model_final_stop': False})

    def assert_no_review_artifacts(self, outcome, host):
        self.assertEqual(outcome['fence_presence'], 'Present')
        self.assertTrue(Path(outcome['reconciliation_fence']).is_file())
        self.assertFalse(outcome['task_accepted'])
        self.assertIsNone(outcome['last_candidate'])
        self.assertIsNone(outcome['last_receipt'])
        self.assertEqual((host.budget_reviews, host.budget_prepares, host.ordinary_reviews, host.ordinary_prepares), ([], [], [], []))
        for pattern in ('run-*-budget-snapshot.json', 'run-*-budget-review-index.json', 'run-*-budget-review.json',
                        'run-*-candidate.json', 'run-*-review.json'):
            self.assertFalse(list((self.root / 'harness').glob(pattern)), pattern)
        self.assertFalse((self.root / 'host-operations/budget-review.json').exists())

    async def test_default_reject_keeps_original_failed_task(self):
        with self.provider([call('write', 'write_answer', {'content': 'beta'})]) as provider:
            config = self.config(provider.endpoint, policy=None)
            host = FileHost(self.root, config)
            outcome = await self.invoke(config, host)
            self.assertEqual((outcome['stop'], outcome['fence_presence']), ('ReconciliationRequired', 'Present'))
            self.assertFalse(host.budget_reviews)
            self.assertEqual(len(provider.requests), 1)
            self.assertFalse((self.root / 'host-operations/budget-review.json').exists())

    async def test_one_call_closed_write_is_verified_without_extra_model(self):
        with self.provider([call('write', 'write_answer', {'content': 'beta'})]) as provider:
            config = self.config(provider.endpoint)
            host = FileHost(self.root, config)
            outcome = await self.invoke(config, host)
            self.assertEqual((outcome['stop'], outcome['fence_presence']), ('AwaitingHostAcceptance', 'Released'))
            self.assertFalse(outcome['task_accepted'])
            self.assertIsNone(outcome['last_candidate'])
            self.assertEqual((len(host.budget_reviews), len(host.ordinary_reviews), len(provider.requests)), (1, 0, 1))
            self.assert_budget_terminal(1)
            ledger = json.loads((self.root / 'host-operations/budget-review.json').read_text())
            record = read_budget_review_reference(ledger['budget_review_ref'], self.root / 'harness', config['task'], outcome)
            self.assertEqual(record['receipt'], outcome['last_receipt'])
            stored = json.loads((self.root / 'harness/released.json').read_text())
            self.assertEqual(outcome, stored, 'SDK must not inject derived outcome fields')

    async def test_bad_file_stops_budget_after_committed_rejection(self):
        with self.provider([call('write', 'write_answer', {'content': 'alpha'})]) as provider:
            config = self.config(provider.endpoint)
            host = FileHost(self.root, config)
            outcome = await self.invoke(config, host)
            self.assertEqual((outcome['stop'], outcome['fence_presence']), ('BudgetExhausted', 'Released'))
            self.assertFalse(outcome['task_accepted'])
            self.assertFalse(host.budget_prepares)
            self.assertEqual(len(provider.requests), 1)

    async def test_run_cap_with_remaining_task_budget_uses_typed_continuation(self):
        with self.provider([call('bad', 'write_answer', {'content': 'alpha'}), call('fixed', 'write_answer', {'content': 'beta'})]) as provider:
            config = self.config(provider.endpoint, task_calls=2, runs=2)
            host = FileHost(self.root, config)
            outcome = await self.invoke(config, host)
            self.assertEqual((outcome['stop'], outcome['runs'], outcome['observed_calls']), ('AwaitingHostAcceptance', 2, 2))
            self.assertEqual((len(host.budget_reviews), len(host.budget_prepares), len(host.ordinary_reviews), len(host.ordinary_prepares)), (2, 1, 0, 0))
            self.assertEqual(host.budget_prepares[0]['remaining_calls'], 1)
            self.assertEqual(outcome['last_receipt']['basis']['run_number'], 2)
            self.assertIsNone(outcome['last_candidate'])
            self.assertEqual(len(provider.requests), 2)
            self.assert_budget_terminal(2)

    async def test_prior_ordinary_candidate_is_not_reused_for_snapshot(self):
        with self.provider([call('bad', 'write_answer', {'content': 'alpha'}), text('ordinary submission'), call('fixed', 'write_answer', {'content': 'beta'})]) as provider:
            config = self.config(provider.endpoint, run_calls=2, task_calls=3, runs=2)
            host = FileHost(self.root, config)
            outcome = await self.invoke(config, host)
            self.assertEqual(outcome['stop'], 'AwaitingHostAcceptance')
            self.assertEqual((len(host.ordinary_reviews), len(host.ordinary_prepares), len(host.budget_reviews)), (1, 1, 1))
            self.assertIsNone(outcome['last_candidate'])
            self.assertEqual(outcome['last_receipt']['basis']['run_number'], 2)
            self.assertTrue((self.root / 'harness/run-1-candidate.json').exists())
            self.assertFalse((self.root / 'harness/run-2-candidate.json').exists())

    async def test_unknown_effect_never_reaches_snapshot_verifier(self):
        with self.provider([call('unknown', 'write_answer', {'content': 'beta'})]) as provider:
            config = self.config(provider.endpoint)
            host = FileHost(self.root, config)
            host.unknown = True
            started = time.monotonic()
            outcome = await self.invoke(config, host)
            self.assertLess(time.monotonic() - started, config['run']['max_seconds'], 'Run deadline would confound the unknown-effect cancellation cause')
            self.assertLess(time.time() * 1000, config['task']['deadline_unix_ms'], 'Task deadline would confound the unknown-effect cancellation cause')
            observation = json.loads((self.root / 'harness/run-1-observation.json').read_text())
            self.assertTrue(observation['core_joined'])
            self.assertFalse(observation['event_failure'])
            # Bridge.fail wakes a separately spawned fault_task. There is no
            # acknowledgement ordering it before the joined Core/drain result.
            # Assert each source-defined branch, not a generic pair of statuses.
            if outcome['stop'] == 'Cancelled':
                self.assertEqual(outcome['detail'], 'canceled/deadline; original Core future joined; reconcile effects')
                branch = 'Task cancellation observed before native-unknown scan'
            elif outcome['stop'] == 'ReconciliationRequired':
                self.assertFalse(observation['stopped'])
                self.assertEqual(outcome['detail'], 'native tool receipt reports unknown/partial effects; no retry')
                branch = 'native-unknown scan before spawned Task cancellation observed'
            else:
                self.fail('Unknown effect did not reach either documented fail-closed branch: ' + repr(outcome))
            self.assert_no_review_artifacts(outcome, host)
            self.assertEqual((outcome['runs'], outcome['observed_calls'], len(provider.requests), len(host.writes)), (1, 1, 1, 1))
            self.assertEqual((self.root / 'answer.txt').read_text(encoding='utf-8'), 'beta')
            reply = json.loads((self.root / 'harness/jsonl-rpc-1-reply.json').read_text())
            self.assertEqual(reply['result']['settlement'], 'Unknown')
            original_receipt = Path(reply['result']['operation_receipt'])
            self.assertTrue(original_receipt.is_file())
            returned = json.loads((self.root / 'host-operations' / (hashlib.sha256(b'rpc-1').hexdigest() + '-host-returned.json')).read_text())
            self.assertEqual(returned['settlement'], 'Unknown')
            self.assertEqual(returned['operation_receipt'], str(original_receipt))
            session = json.loads((self.root / 'session.json').read_text())
            native = [m for m in session['messages'] if m.get('toolCallId') == 'unknown']
            self.assertEqual(len(native), 1)
            self.assertEqual(native[0]['details'], {'executed': 'unknown', 'source': 'jsonl_external_callback'})
            terminal = [event for event in self.events() if event['type'] == 'agent_end']
            self.assertEqual(len(terminal), 1)
            # A native closed proof can legitimately exist: it never asserts
            # that the external Host operation was settled.
            if 'termination' in terminal[0]:
                self.assertEqual(terminal[0]['termination'], {'version': 1, 'cause': 'call_budget_exhausted', 'native_round_closed': True, 'model_final_stop': False})
            save(self.root / 'unknown-rejection-proof.json', {'observed_source_branch': branch,
                'raw_host_settlement': 'Unknown', 'native_details': native[0]['details'],
                'native_termination': terminal[0].get('termination'), 'pending_run': session.get('pending_run'),
                'original_operation_receipt': str(original_receipt), 'artifact_exists_but_not_accepted': True,
                'task_fence': outcome['fence_presence'], 'extra_http': 0})

    async def test_opt_in_plain_length_is_not_a_budget_snapshot(self):
        reply = text('Partial answer')
        reply['finish'] = 'length'
        with self.provider([reply]) as provider:
            config = self.config(provider.endpoint)
            host = FileHost(self.root, config)
            outcome = await self.invoke(config, host)
            self.assertEqual(outcome['stop'], 'ReconciliationRequired')
            self.assert_no_review_artifacts(outcome, host)
            self.assertEqual((len(provider.requests), len(host.writes)), (1, 0))
            results = [event for event in self.events() if event['type'] == 'model_result']
            self.assertEqual([event['stop_reason'] for event in results], ['length'])
            self.assertFalse(any('termination' in event for event in self.events() if event['type'] == 'agent_end'))

    async def test_opt_in_length_tool_call_is_failed_without_dispatch_or_snapshot(self):
        reply = call('truncated-never-execute', 'write_answer', {'content': 'beta'})
        reply['finish'] = 'length'
        with self.provider([reply]) as provider:
            config = self.config(provider.endpoint)
            host = FileHost(self.root, config)
            outcome = await self.invoke(config, host)
            self.assertEqual(outcome['stop'], 'ReconciliationRequired')
            self.assert_no_review_artifacts(outcome, host)
            self.assertEqual((len(provider.requests), len(host.writes)), (1, 0))
            self.assertFalse((self.root / 'answer.txt').exists())
            session = json.loads((self.root / 'session.json').read_text())
            rejected = [m for m in session['messages'] if m.get('toolCallId') == 'truncated-never-execute']
            self.assertEqual(len(rejected), 1)
            self.assertTrue(rejected[0]['isError'])
            self.assertFalse(any('termination' in event for event in self.events() if event['type'] == 'agent_end'))

    async def test_opt_in_provider_http_error_is_not_a_budget_snapshot(self):
        with self.provider([]) as provider:
            def fail_post(handler):
                body = json.loads(handler.rfile.read(int(handler.headers['Content-Length'])))
                with provider.lock:
                    provider.requests.append(body)
                raw = b'{"error":{"message":"controlled provider unavailable","type":"server_error"}}'
                handler.send_response(503)
                handler.send_header('Content-Type', 'application/json')
                handler.send_header('Content-Length', str(len(raw)))
                handler.end_headers()
                handler.wfile.write(raw)
            # A fresh provider instance owns this handler class. Only this one
            # actual HTTP response is changed, not the shared provider helper.
            provider.server.RequestHandlerClass.do_POST = fail_post
            config = self.config(provider.endpoint)
            host = FileHost(self.root, config)
            outcome = await self.invoke(config, host)
            self.assertEqual(outcome['stop'], 'ReconciliationRequired')
            self.assert_no_review_artifacts(outcome, host)
            self.assertEqual((len(provider.requests), len(host.writes)), (1, 0))
            results = [event for event in self.events() if event['type'] == 'model_result']
            self.assertEqual([event['stop_reason'] for event in results], ['error'])
            self.assertFalse(any('termination' in event for event in self.events() if event['type'] == 'agent_end'))

    async def test_opt_in_context_failure_precedes_any_model_reservation(self):
        with self.provider([]) as provider:
            config = self.config(provider.endpoint)
            config['run']['model_resolution'] = {'kind': 'host_declared_loopback',
                'capabilities': {'context_window': 8192, 'max_tokens': 4096, 'input': ['text']}}
            config['run']['objective'] = 'Current Run must remain exact. ' + 'x' * 6000
            host = FileHost(self.root, config)
            outcome = await self.invoke(config, host)
            self.assertEqual(outcome['stop'], 'ReconciliationRequired')
            self.assert_no_review_artifacts(outcome, host)
            self.assertEqual((len(provider.requests), provider.catalog_requests, len(host.writes), outcome['observed_calls']), (0, 0, 0, 0))
            terminal = [event for event in self.events() if event['type'] == 'agent_end']
            self.assertEqual(len(terminal), 1)
            self.assertTrue(terminal[0]['detail'].startswith('context maintenance failed:'), terminal)
            self.assertNotIn('termination', terminal[0])

    async def test_known_settled_tool_error_can_be_independently_verified(self):
        with self.provider([call('known', 'write_answer', {'content': 'beta'})]) as provider:
            config = self.config(provider.endpoint)
            host = FileHost(self.root, config)
            host.known_error = True
            outcome = await self.invoke(config, host)
            self.assertEqual(outcome['stop'], 'AwaitingHostAcceptance')
            self.assertEqual(len(host.budget_reviews), 1)
            self.assertEqual(len(provider.requests), 1)

    async def test_snapshot_mutation_during_verify_retains_fence(self):
        with self.provider([call('write', 'write_answer', {'content': 'beta'})]) as provider:
            config = self.config(provider.endpoint)
            host = FileHost(self.root, config)
            host.tamper_snapshot = True
            outcome = await self.invoke(config, host, expected_callback_fault=True)
            self.assertEqual((outcome['stop'], outcome['fence_presence']), ('ReconciliationRequired', 'Present'))
            self.assertFalse((self.root / 'harness/run-1-budget-review-index.json').exists())
            self.assertEqual(len(provider.requests), 1)

    async def test_continuation_mutation_cannot_start_another_run(self):
        with self.provider([call('write', 'write_answer', {'content': 'alpha'})]) as provider:
            config = self.config(provider.endpoint, task_calls=2, runs=2)
            host = FileHost(self.root, config)
            host.tamper_continuation = True
            outcome = await self.invoke(config, host, expected_callback_fault=True)
            self.assertEqual((outcome['stop'], outcome['fence_presence']), ('ReconciliationRequired', 'Present'))
            self.assertEqual(outcome['runs'], 1)
            self.assertEqual(len(provider.requests), 1)

    async def test_ordinary_reply_tag_cannot_satisfy_budget_pending_kind(self):
        import ara_agent_lite
        with self.provider([call('write', 'write_answer', {'content': 'beta'})]) as provider:
            config = self.config(provider.endpoint)
            host = FileHost(self.root, config)
            original_encoded = ara_agent_lite.encoded
            def wrong_reply(value):
                if value.get('kind') == 'verify_budget_reply':
                    value = {**value, 'kind': 'verify_reply'}
                return original_encoded(value)
            with patch('ara_agent_lite.encoded', wrong_reply):
                outcome = await self.invoke(config, host, expected_callback_fault=True)
            self.assertEqual((outcome['stop'], outcome['fence_presence']), ('ReconciliationRequired', 'Present'))
            self.assertEqual(len(host.budget_reviews), 1)
            self.assertFalse((self.root / 'harness/run-1-budget-review-index.json').exists())

    async def test_sdk_missing_budget_callbacks_rejects_before_spawn(self):
        with self.provider([]) as provider:
            config = self.config(provider.endpoint)
            host = FileHost(self.root, config)
            host.verify_budget_snapshot = None
            with self.assertRaises(ValueError):
                await run_task(BINARY, config, host, token=TOKEN, operation_directory=self.root / 'host-operations')
            self.assertFalse((self.root / 'host-operations').exists())
            self.assertEqual((len(provider.requests), provider.catalog_requests), (0, 0))

    async def test_native_invalid_flags_and_plan_reject_before_provider(self):
        with self.provider([]) as provider:
            cases = [('null', None, 'execute'), ('object', {}, 'execute'), ('unknown', 'allow_any_error', 'execute'), ('plan', 'review_closed_tool_round', 'plan')]
            for name, flag, mode in cases:
                with self.subTest(name=name):
                    config = self.config(provider.endpoint)
                    config['budget_stop_review'] = flag
                    config['run']['mode'] = mode
                    config['run']['session_path'] = str(self.root / f'{name}-session.json')
                    save(self.root / f'{name}-start.json', config)
                    kwargs = {'creationflags': subprocess.CREATE_NO_WINDOW} if os.name == 'nt' else {}
                    proc = await asyncio.create_subprocess_exec(str(BINARY), stdin=asyncio.subprocess.PIPE, stdout=asyncio.subprocess.PIPE,
                        stderr=asyncio.subprocess.PIPE, env={**os.environ, 'ARA_AGENT_MODEL_TOKEN': TOKEN}, **kwargs)
                    stdout, stderr = await asyncio.wait_for(proc.communicate(encoded(config)), timeout=8)
                    save(self.root / f'{name}-process.json', {'pid': proc.pid, 'exit_code': proc.returncode})
                    (self.root / f'{name}-stdout.bin').write_bytes(stdout)
                    (self.root / f'{name}-stderr.bin').write_bytes(stderr)
                    self.assertEqual(proc.returncode, 2)
                    self.assertFalse(Path(config['run']['session_path']).exists())
            self.assertEqual((len(provider.requests), provider.catalog_requests), (0, 0))

    async def node_case(self, replies, *, task_calls=1, runs=1, mode='normal'):
        with self.provider(replies) as provider:
            config = self.config(provider.endpoint, task_calls=task_calls, runs=runs)
            save(self.root / 'node-start.json', config)
            kwargs = {'creationflags': subprocess.CREATE_NO_WINDOW} if os.name == 'nt' else {}
            proc = await asyncio.create_subprocess_exec('node', str(HERE.parent / 'clients/node/budget-probe.mjs'), str(BINARY),
                str(self.root / 'node-start.json'), str(self.root), mode, stdout=asyncio.subprocess.PIPE, stderr=asyncio.subprocess.PIPE,
                env={**os.environ, 'ARA_AGENT_MODEL_TOKEN': TOKEN}, **kwargs)
            stdout, stderr = await asyncio.wait_for(proc.communicate(), timeout=40)
            save(self.root / 'node-process.json', {'pid': proc.pid, 'exit_code': proc.returncode})
            (self.root / 'node-stdout.bin').write_bytes(stdout)
            (self.root / 'node-stderr.bin').write_bytes(stderr)
            self.assertEqual(proc.returncode, 0, stderr.decode('utf-8', 'replace'))
            result = json.loads((self.root / 'node-result.json').read_text())
            return result, len(provider.requests)

    async def test_node_one_call_snapshot_uses_dedicated_callback_and_ledger(self):
        result, calls = await self.node_case([call('node-write', 'write_answer', {'content': 'beta'})])
        self.assertEqual((result['delivery'], result['outcome']['stop'], calls), ('validated', 'AwaitingHostAcceptance', 1))
        self.assertIsNone(result['outcome']['last_candidate'])
        self.assertTrue((self.root / 'node-operations/budget-review.json').exists())

    async def test_node_typed_continuation_preserves_original_budget(self):
        result, calls = await self.node_case([call('node-bad', 'write_answer', {'content': 'alpha'}), call('node-fixed', 'write_answer', {'content': 'beta'})], task_calls=2, runs=2)
        self.assertEqual((result['delivery'], result['outcome']['stop'], calls), ('validated', 'AwaitingHostAcceptance', 2))
        self.assertEqual(result['outcome']['last_receipt']['basis']['run_number'], 2)

    async def test_node_snapshot_tamper_is_not_validated(self):
        result, calls = await self.node_case([call('node-write', 'write_answer', {'content': 'beta'})], mode='tamper')
        self.assertEqual(calls, 1)
        self.assertTrue(result['reconciliation_required'])
        self.assertFalse((self.root / 'harness/run-1-budget-review-index.json').exists())


if __name__ == '__main__':
    unittest.main(verbosity=2)
