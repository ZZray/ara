"""Real ACR ModelRelay ledger across two native Harness Runs; loopback only."""
from __future__ import annotations

import asyncio
import hashlib
import importlib.util
import json
import os
from pathlib import Path
import subprocess
import sys
import time
import unittest
from unittest.mock import patch


HERE = Path(__file__).resolve().parent
CLIENT = HERE.parent / 'clients/python'
PRODUCT = Path('E:/repos/ai-codereview')
EVIDENCE = Path(os.environ.get('ARA_JSONL_RELAY_EVIDENCE',
    'D:/WorkSpace/tmp/ara-core-capability-20261022/product-host-adapters-v1-20261025/relay-budget-v1'))
BINARY = Path(os.environ.get('ARA_JSONL_TEST_BINARY',
    'D:/Temp/ara-build/ara-github-target-20261005/debug/ara-agent-lite-jsonl.exe'))
NATIVE_TOOLS = ('task_update', 'history_read', 'history_search', 'reference_read',
                'reference_search', 'memory_propose', 'experience_propose', 'experience_search')


def save(path: Path, value) -> None:
    with path.open('x', encoding='utf-8') as stream:
        json.dump(value, stream, ensure_ascii=False, indent=2)
        stream.write('\n')
        stream.flush()
        os.fsync(stream.fileno())


def sha(path: Path) -> str:
    return hashlib.sha256(path.read_bytes()).hexdigest()


def product_helpers():
    """Load only a module object; never export its unittest classes here."""
    sys.path.insert(0, str(PRODUCT))
    spec = importlib.util.spec_from_file_location('_acr_relay_budget_loopback_helpers',
        PRODUCT / 'tests/test_agent_model_relay.py')
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


class LedgerHost:
    def __init__(self, root, relay, config):
        self.root, self.relay, self.config = root, relay, config
        self.started = relay._started
        self.writes = 0
        self.reviews = []
        self.prepares = []

    async def tool_call(self, request, cancelled):
        assert not cancelled.is_set()
        assert request['tool_name'] == 'write_answer'
        assert set(request['arguments']) == {'content'}
        self.writes += 1
        start = self.root / f'effect-{self.writes}-started.json'
        save(start, {'correlation': request, 'status': 'started'})
        answer = self.root / 'answer.txt'
        answer.write_text(request['arguments']['content'], encoding='utf-8')
        receipt = self.root / f'effect-{self.writes}-completed.json'
        save(receipt, {'status': 'completed', 'answer_sha256': sha(answer)})
        return {'value': {'written': True}, 'is_error': False, 'settlement': 'Settled',
                'operation_receipt': str(receipt)}

    async def verify(self, candidate, cancelled):
        assert not cancelled.is_set()
        answer = self.root / 'answer.txt'
        correct = answer.read_text(encoding='utf-8') == 'correct'
        row = {'basis': candidate['basis'], 'correct': correct, 'answer_sha256': sha(answer),
               'actual_attempts': self.relay._attempts, 'relay_started': self.relay._started}
        self.reviews.append(row)
        receipt = self.root / f'verification-{len(self.reviews)}.json'
        save(receipt, row)
        return {'basis': candidate['basis'], 'committed': True, 'settlement': 'Settled',
                'verdict': 'Verified' if correct else {'NeedsChanges': {
                    'feedback': 'Actual answer file is partial; write exactly correct and submit.'}},
                'evidence': [str(receipt) + '#sha256=' + sha(receipt)]}

    async def prepare(self, continuation, cancelled):
        assert not cancelled.is_set()
        assert self.relay._attempts == 3
        assert self.relay._started == self.started
        assert continuation['remaining_calls'] == 2
        assert continuation['deadline_unix_ms'] == self.config['task']['deadline_unix_ms']
        feedback = continuation['receipt']['verdict']['NeedsChanges']['feedback']
        assert feedback
        self.prepares.append({'next_run_number': continuation['next_run_number'],
            'remaining_core_calls': continuation['remaining_calls'],
            'deadline_unix_ms': continuation['deadline_unix_ms'], 'actual_attempts': self.relay._attempts})
        return {'run_id_next': 'run-2', 'objective': 'Apply the actual Host feedback: ' + feedback}


class RelayBudgetTest(unittest.IsolatedAsyncioTestCase):
    async def test_actual_attempt_ledger_survives_feedback_run_without_reset(self):
        self.assertTrue(BINARY.is_file(), 'use the delivered native JSONL binary; no skip acceptance')
        EVIDENCE.mkdir(parents=True, exist_ok=True)
        root = EVIDENCE / f'attempt-{time.time_ns()}-{os.getpid()}'
        root.mkdir()
        self.root = root
        isolation = {'APP_DB_PATH': str(root / 'unused-test.db'), 'APP_DATABASE_URL': '',
            'APP_REPO_ROOT': str(root / 'repos'), 'APP_CRASH_ROOT': str(root / 'crashes'),
            'APP_AUTH_ENABLED': 'false', 'PYTHONDONTWRITEBYTECODE': '1'}
        started = time.monotonic()
        save(root / 'isolation.json', {'environment': isolation, 'binary': str(BINARY.resolve()),
            'binary_sha256': sha(BINARY), 'test_source_sha256': sha(Path(__file__)),
            'relay_source_sha256': sha(PRODUCT / 'app/services/agent_model_relay.py'),
            'helper_source_sha256': sha(PRODUCT / 'tests/test_agent_model_relay.py'),
            'product_head': subprocess.check_output(['git', '-C', str(PRODUCT), 'rev-parse', 'HEAD'], text=True).strip(),
            'real_models': False, 'database_connections_requested': False})
        with patch.dict(os.environ, isolation):
            helpers = product_helpers()
            from app.config import config as product_config
            self.assertEqual(product_config.db_file, root / 'unused-test.db',
                'run in a fresh Python process so product configuration is isolated before import')
            self.assertEqual(product_config.database_url, '')
            self.assertEqual(product_config.repo_root_dir, root / 'repos')
            self.assertEqual(product_config.crash_root_dir, root / 'crashes')
            sys.path.insert(0, str(CLIENT))
            from ara_agent_lite import run_task

            def tool(identifier, content):
                return {'id': identifier, 'type': 'function', 'function': {'name': 'write_answer',
                    'arguments': json.dumps({'content': content})}}

            replies = [(503, {'error': {'code': 'server_is_overloaded'}}),
                (200, helpers.completion(content=None, calls=[tool('partial-native', 'partial')], reason='tool_calls')),
                (200, helpers.completion(content='First submission.', reason='stop')),
                (200, helpers.completion(content=None, calls=[tool('correct-native', 'correct')], reason='tool_calls')),
                (200, helpers.completion(content='Corrected submission.', reason='stop'))]
            with patch('app.services.agent_model_relay.http_client',
                    lambda timeout: helpers.httpx.Client(trust_env=False, timeout=timeout)):
                with helpers.upstream(replies) as (url, requests):
                    cfg = {'base_url': url, 'api_key': 'loopback-fixture-no-real-provider-key',
                        'model': 'ledger-primary', 'timeout': 5, 'model_fallback_enabled': True,
                        'fallback_models': ['ledger-fallback']}
                    with helpers.ModelRelay(cfg, {*NATIVE_TOOLS, 'write_answer'},
                            max_calls=5, max_seconds=45, max_output=512) as relay:
                        config = {'kind': 'start_task', 'version': 1,
                            'task': {'task_id': 'one-relay-two-runs', 'criteria_revision': 'actual-file-v1',
                                'authorization_revision': 'owned-answer-only-v1', 'kind': 'Coding',
                                'evidence_dir': str(root / 'harness'), 'max_runs': 2, 'max_calls': 4,
                                'deadline_unix_ms': int(time.time() * 1000) + 40000},
                            'run': {'run_id': 'run-1', 'objective': 'Use write_answer, submit the actual file and finish.',
                                'cwd': str(root), 'session_path': str(root / 'session.json'),
                                'environment_state': str(root / 'environment'), 'device_fp': 'relay-ledger-fixture',
                                'session_key': 'relay-ledger/same-session', 'mode': 'execute', 'endpoint': relay.url,
                                'model': cfg['model'], 'max_calls': 4, 'max_seconds': 35, 'max_output': 512,
                                'model_resolution': {'kind': 'host_declared_loopback', 'capabilities': {
                                    'context_window': 1000000, 'max_tokens': 4096, 'input': ['text']}},
                                'tools': [{'name': 'write_answer', 'description': 'Write the one Host-owned answer.',
                                    'parameters': {'type': 'object', 'properties': {'content': {'type': 'string'}},
                                        'required': ['content'], 'additionalProperties': False}, 'allow_in_plan': False}]},
                            'cancel_grace_ms': 1000, 'emit_events': False}
                        save(root / 'start.json', config)
                        host = LedgerHost(root, relay, config)
                        outcome = await asyncio.wait_for(run_task(BINARY, config, host, token=relay.token,
                            operation_directory=root / 'host-operations'), timeout=40)
                        save(root / 'consumer-outcome.json', outcome)
                        actual = [row for row in relay.receipts if type(row.get('call')) is int]
                        skipped = [row for row in relay.receipts if row.get('status') == 'skipped']
                        save(root / 'relay-receipts.json', relay.receipts)
                        save(root / 'upstream-requests.json', [
                            {'path': row['path'], 'body': row['body'], 'authorization': 'fixture-key-redacted'}
                            for row in requests])
                        save(root / 'host-reviews.json', host.reviews)
                        save(root / 'host-prepares.json', host.prepares)
                        self.assertEqual(outcome['stop'], 'AwaitingHostAcceptance', outcome)
                        self.assertEqual((outcome['runs'], outcome['observed_calls']), (2, 4))
                        self.assertFalse(outcome['task_accepted'])
                        self.assertEqual((host.writes, len(host.reviews), len(host.prepares)), (2, 2, 1))
                        self.assertEqual([row['actual_attempts'] for row in host.reviews], [3, 5])
                        self.assertEqual([row['call'] for row in actual], [1, 2, 3, 4, 5])
                        self.assertEqual([row['status'] for row in actual], ['failed'] + ['success'] * 4)
                        self.assertTrue(skipped, 'the overloaded primary must remain cooling within the same relay')
                        self.assertEqual(relay._attempts, 5)
                        self.assertEqual(relay._started, host.started)
                        self.assertEqual([row['body']['model'] for row in requests],
                            ['ledger-primary'] + ['ledger-fallback'] * 4)
                        self.assertEqual(host.reviews[0]['basis']['session_path'], host.reviews[1]['basis']['session_path'])
                        self.assertEqual((root / 'answer.txt').read_text(encoding='utf-8'), 'correct')
                        self.assertIn('Actual answer file is partial', json.dumps(requests[3]['body']))

                        with helpers.httpx.Client(trust_env=False, timeout=5) as client:
                            rejected = client.post(relay.url + '/chat/completions',
                                headers={'Authorization': 'Bearer ' + relay.token}, json={
                                    'model': cfg['model'], 'messages': [{'role': 'user', 'content': 'budget probe'}],
                                    'max_tokens': 512})
                        self.assertEqual(rejected.status_code, 502)
                        self.assertEqual(rejected.json()['error']['code'], 'relay_model_call_budget')
                        self.assertEqual(len(requests), 5, 'no sixth upstream attempt after the same ledger exhausted')
                        self.assertEqual(relay._attempts, 5)
                        save(root / 'exhausted-ledger-probe.json', {'status': rejected.status_code,
                            'category': rejected.json()['error']['code'], 'upstream_requests_after_probe': len(requests)})

                        logical = []
                        for run_number in (1, 2):
                            rows = [json.loads(line) for line in
                                (root / f'harness/run-{run_number}-events.jsonl').read_bytes().splitlines()]
                            calls = [row['event'] for row in rows if row['event']['type'] == 'model_call']
                            self.assertEqual([row['call'] for row in calls], [1, 2])
                            self.assertEqual([row['phase'] for row in calls], ['agent', 'agent'])
                            logical.extend({'run_number': run_number, **row} for row in calls)
                        save(root / 'result.json', {'status': 'PASS', 'core_runs': 2, 'core_logical_calls': 4,
                            'relay_actual_attempts': 5, 'actual_ordinals': [row['call'] for row in actual],
                            'failed_attempts_charged': 1, 'cooldown_records': len(skipped),
                            'task_deadline_unix_ms': config['task']['deadline_unix_ms'],
                            'relay_max_calls': relay.max_calls, 'relay_started_unchanged': relay._started == host.started,
                            'logical_events': logical, 'host_checks_actual_attempts': [3, 5],
                            'task_accepted': False, 'elapsed_seconds': time.monotonic() - started,
                            'limits': ['deterministic loopback provider only', 'no real model or production service',
                                'no database access', 'summary phase not exercised', 'no product default/pin change']})
        self.assertFalse((root / 'unused-test.db').exists(), 'this relay fixture must not open a database')
        print('RELAY_BUDGET_EVIDENCE=' + str(root), flush=True)


if __name__ == '__main__':
    unittest.main(verbosity=2)
