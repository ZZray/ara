"""Generation-policy JSONL wiring through actual sidecars and controlled HTTP."""
from __future__ import annotations

import asyncio
from contextlib import contextmanager
import hashlib
import json
import os
from pathlib import Path
import sys
import time
import unittest
from unittest.mock import patch


HERE = Path(__file__).resolve().parent
sys.path.insert(0, str(HERE.parent / 'clients/python'))
from ara_agent_lite import encoded, run_task
from controlled_provider import ControlledProvider, call, text

BINARY = Path(os.environ.get('ARA_JSONL_TEST_BINARY',
    'D:/Temp/ara-build/ara-github-target-20261005/debug/ara-agent-lite-jsonl.exe'))
EXPECTED_SHA = os.environ.get('ARA_JSONL_GENERATION_BINARY_SHA256', '')
EVIDENCE = Path(os.environ.get('ARA_JSONL_GENERATION_EVIDENCE',
    'D:/WorkSpace/tmp/ara-core-capability-20261022/jsonl-generation-policy-v1-20261025'))
TOKEN = 'generation-policy-controlled-grant-only'
FEEDBACK = 'The actual answer file contains alpha. Write exactly beta and submit the corrected actual file.'


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
        self.writes, self.reviews, self.prepares = [], [], []

    async def tool_call(self, request, cancelled):
        assert not cancelled.is_set()
        assert request['tool_name'] == 'write_answer' and set(request['arguments']) == {'content'}
        number = len(self.writes) + 1
        save(self.root / f'write-{number}-started.json', request)
        path = self.root / 'answer.txt'
        with path.open('w', encoding='utf-8') as stream:
            stream.write(request['arguments']['content'])
            stream.flush()
            os.fsync(stream.fileno())
        # Preserve both actual file versions, including the rejected alpha.
        (self.root / f'answer-after-write-{number}.txt').write_bytes(path.read_bytes())
        receipt = self.root / f'write-{number}-settled.json'
        row = {'request': request, 'answer': path.read_text(encoding='utf-8'), 'answer_sha256': sha(path)}
        save(receipt, row)
        self.writes.append(row)
        return {'value': {'written': True, 'answer_sha256': sha(path)}, 'is_error': False,
                'settlement': 'Settled', 'operation_receipt': str(receipt)}

    async def verify(self, candidate, cancelled):
        assert not cancelled.is_set()
        actual = (self.root / 'answer.txt').read_text(encoding='utf-8')
        row = {'basis': candidate['basis'], 'actual_answer': actual,
               'answer_sha256': sha(self.root / 'answer.txt'), 'correct': actual == 'beta'}
        self.reviews.append(row)
        receipt = self.root / f'review-{len(self.reviews)}.json'
        save(receipt, row)
        return {'basis': candidate['basis'], 'committed': True, 'settlement': 'Settled',
                'verdict': 'Verified' if row['correct'] else {'NeedsChanges': {'feedback': FEEDBACK}},
                'evidence': [str(receipt) + '#sha256=' + sha(receipt)]}

    async def prepare(self, continuation, cancelled):
        assert not cancelled.is_set()
        assert continuation['deadline_unix_ms'] == self.config['task']['deadline_unix_ms']
        assert continuation['remaining_calls'] == 2 and continuation['next_run_number'] == 2
        assert continuation['receipt']['verdict']['NeedsChanges']['feedback'] == FEEDBACK
        result = {'run_id_next': 'run-2', 'objective': 'Apply the committed Host correction. ' + FEEDBACK}
        row = {'continuation': continuation, 'prepared': result}
        self.prepares.append(row)
        save(self.root / 'prepare-2.json', row)
        return result


class GenerationPolicyTest(unittest.IsolatedAsyncioTestCase):
    def setUp(self):
        self.assertRegex(EXPECTED_SHA.lower(), r'^[0-9a-f]{64}$', 'pin the expected built image with ARA_JSONL_GENERATION_BINARY_SHA256')
        self.assertEqual(sha(BINARY), EXPECTED_SHA.lower())
        EVIDENCE.mkdir(parents=True, exist_ok=True)
        self.root = EVIDENCE / (self._testMethodName + '-' + str(time.time_ns()))
        self.root.mkdir()
        self.processes = []
        save(self.root / 'identity.json', {'binary': str(BINARY), 'binary_sha256': sha(BINARY),
             'test_file_sha256': sha(__file__), 'provider': 'controlled loopback; no actual-model claim'})
        print('GENERATION_POLICY_EVIDENCE=' + str(self.root), flush=True)

    @contextmanager
    def provider(self, replies):
        with ControlledProvider(replies) as provider:
            try:
                yield provider
            finally:
                save(self.root / 'provider-requests.json', provider.requests)
                save(self.root / 'provider-counts.json', {'chat_http_requests': len(provider.requests),
                                                        'catalog_http_requests': provider.catalog_requests})

    def config(self, endpoint, *, calls=2, runs=1):
        return {'kind': 'start_task', 'version': 1,
            'task': {'task_id': self._testMethodName, 'criteria_revision': 'actual-beta-file-v1',
                'authorization_revision': 'owned-answer-only-v1', 'kind': 'Coding',
                'evidence_dir': str(self.root / 'harness'), 'max_runs': runs, 'max_calls': calls,
                'deadline_unix_ms': int(time.time() * 1000) + 30000},
            'run': {'run_id': 'run-1', 'objective': 'Write exactly beta to the actual owned answer file and submit.',
                'cwd': str(self.root), 'session_path': str(self.root / 'session.json'),
                'environment_state': str(self.root / 'environment'), 'device_fp': 'generation-policy-fixture',
                'session_key': 'generation-policy/original-session', 'mode': 'execute',
                'endpoint': endpoint, 'model': 'jsonl-fixture', 'max_calls': calls, 'max_seconds': 25,
                'max_output': 512, 'model_resolution': {'kind': 'catalog'},
                'tools': [{'name': 'write_answer', 'description': 'Write the one Host-owned answer file.',
                    'parameters': {'type': 'object', 'properties': {'content': {'type': 'string'}},
                        'required': ['content'], 'additionalProperties': False}, 'allow_in_plan': False}]},
            'cancel_grace_ms': 1000, 'emit_events': False}

    async def invoke(self, config, host):
        save(self.root / 'start.json', config)
        original = asyncio.create_subprocess_exec

        async def real_process(*args, **kwargs):
            proc = await original(*args, **kwargs)
            self.processes.append(proc)
            return proc

        try:
            with patch('asyncio.create_subprocess_exec', real_process):
                outcome = await asyncio.wait_for(run_task(BINARY, config, host, token=TOKEN,
                    operation_directory=self.root / 'host-operations'), timeout=40)
            save(self.root / 'consumer-outcome.json', outcome)
            return outcome
        finally:
            save(self.root / 'processes.json', [{'pid': p.pid, 'exit_code': p.returncode} for p in self.processes])
            save(self.root / 'host-ledger.json', {'writes': host.writes, 'reviews': host.reviews, 'prepares': host.prepares})

    def events(self):
        return [json.loads(line) for path in sorted((self.root / 'harness').glob('run-*-events.jsonl'))
                for line in path.read_bytes().splitlines()]

    def check_success(self, outcome, host, config, provider, *, runs, calls, disabled):
        self.assertEqual((outcome['stop'], outcome['runs'], outcome['observed_calls']),
                         ('AwaitingHostAcceptance', runs, calls), outcome)
        self.assertEqual(outcome['fence_presence'], 'Released')
        self.assertFalse(outcome['task_accepted'])
        self.assertEqual((self.root / 'answer.txt').read_text(), 'beta')
        self.assertEqual(len(provider.requests), calls)
        self.assertGreater(provider.catalog_requests, 0)
        self.assertEqual(len(self.processes), 1)
        self.assertEqual(self.processes[0].returncode, 0)
        for body in provider.requests:
            self.assertEqual(body['max_tokens'], 512)
            if disabled:
                self.assertEqual(body.get('thinking'), {'type': 'disabled'})
            else:
                self.assertNotIn('thinking', body)
        events = self.events()
        policies = [row['event'] for row in events if row['event']['type'] == 'generation_policy']
        self.assertEqual(len(policies), runs if disabled else 0)
        for event in policies:
            self.assertEqual((event['policy'], event['protocol']), ('thinking_type_disabled', 'openai-completions'))
            self.assertEqual((event['requested_output_tokens'], event['effective_output_tokens']), (512, 512))
            self.assertFalse(event['provider_effect_verified'])
        logical = [row for row in events if row['event']['type'] == 'model_call']
        self.assertEqual(len(logical), calls)
        self.assertTrue(all(row['event']['phase'] == 'agent' for row in logical))
        for number in range(1, runs + 1):
            admission = json.loads((self.root / f'harness/run-{number}-admission.json').read_bytes())
            self.assertEqual(admission['deadline_unix_ms'], config['task']['deadline_unix_ms'])
            self.assertEqual(admission['remaining_calls'], calls if number == 1 else 2)
            self.assertLessEqual(admission['max_seconds'], config['run']['max_seconds'])
        session = json.loads((self.root / 'session.json').read_bytes())
        self.assertIsNone(session['pending_run'])
        self.assertEqual(session['completed_runs'], [f'run-{number}' for number in range(1, runs + 1)])
        native = [row for row in session['messages'] if row.get('toolName') == 'write_answer']
        self.assertEqual(len(native), len(host.writes))
        for receipt, operation in zip(native, host.writes):
            self.assertFalse(receipt['isError'])
            value = json.loads(''.join(row.get('text', '') for row in receipt['content']))
            self.assertEqual(value, {'written': True, 'answer_sha256': operation['answer_sha256']})
            self.assertEqual(receipt['toolCallId'], operation['request']['native_call_id'])
        context = json.loads((self.root / 'session.context.json').read_bytes())
        refs = [row for row in context['references'] if row['kind'] == 'objective']
        expected_objectives = [config['run']['objective']] + [row['prepared']['objective'] for row in host.prepares]
        self.assertEqual([row['text'] for row in refs], expected_objectives)
        self.assertTrue(all(row['status'] == 'active' and row['hash'] == hashlib.sha256(row['text'].encode()).hexdigest()
                            for row in context['references']))
        for review in host.reviews:
            self.assertTrue(os.path.samefile(review['basis']['session_path'], self.root / 'session.json'))
            self.assertEqual(review['basis']['task_id'], config['task']['task_id'])
        save(self.root / 'native-write-receipts.json', native)
        save(self.root / 'context-final.json', context)
        save(self.root / 'result.json', {'status': 'PASS', 'runs': runs, 'logical_calls': calls,
             'chat_http_requests': len(provider.requests), 'catalog_http_requests': provider.catalog_requests,
             'thinking_disabled': disabled, 'task_accepted': False, 'session_sha256': sha(self.root / 'session.json'),
             'answer_sha256': sha(self.root / 'answer.txt'), 'limits': ['controlled HTTP wiring only', 'no actual-model or product acceptance']})

    async def test_omitted_policy_preserves_default_wire(self):
        with self.provider([call('write-default', 'write_answer', {'content': 'beta'}), text('Actual file submitted.')]) as provider:
            config = self.config(provider.endpoint)
            host = FileHost(self.root, config)
            outcome = await self.invoke(config, host)
            self.check_success(outcome, host, config, provider, runs=1, calls=2, disabled=False)

    async def test_explicit_provider_default_preserves_default_wire(self):
        with self.provider([call('write-explicit-default', 'write_answer', {'content': 'beta'}), text('Actual file submitted.')]) as provider:
            config = self.config(provider.endpoint)
            config['run']['generation_policy'] = 'provider_default'
            host = FileHost(self.root, config)
            outcome = await self.invoke(config, host)
            self.check_success(outcome, host, config, provider, runs=1, calls=2, disabled=False)

    async def test_disabled_policy_survives_real_file_feedback_and_original_task_budget(self):
        replies = [call('write-alpha', 'write_answer', {'content': 'alpha'}), text('First actual file submitted.'),
                   call('write-beta', 'write_answer', {'content': 'beta'}), text('Corrected actual file submitted.')]
        with self.provider(replies) as provider:
            config = self.config(provider.endpoint, calls=4, runs=2)
            config['run']['generation_policy'] = 'thinking_type_disabled'
            host = FileHost(self.root, config)
            outcome = await self.invoke(config, host)
            self.check_success(outcome, host, config, provider, runs=2, calls=4, disabled=True)
            self.assertEqual([row['actual_answer'] for row in host.reviews], ['alpha', 'beta'])
            self.assertEqual([row['correct'] for row in host.reviews], [False, True])
            self.assertEqual(len(host.prepares), 1)
            self.assertIn(FEEDBACK, json.dumps(provider.requests[2]))
            for number, expected in ((1, 'alpha'), (2, 'beta')):
                self.assertEqual((self.root / f'answer-after-write-{number}.txt').read_text(), expected)
            calls = [row for row in self.events() if row['event']['type'] == 'model_call']
            self.assertEqual([(row['run_id'], row['event']['call']) for row in calls],
                             [('run-1', 1), ('run-1', 2), ('run-2', 1), ('run-2', 2)])
            self.assertEqual([(row['basis']['run_calls'], row['basis']['total_calls']) for row in host.reviews],
                             [(2, 2), (2, 4)])

    async def test_disabled_policy_survives_same_run_length_replan(self):
        truncated = {'delta': {'content': 'Partial closed text; actual task remains unfinished.'}, 'finish': 'length'}
        with self.provider([truncated, call('write-after-length', 'write_answer', {'content': 'beta'}),
                            text('Actual file submitted after bounded recovery.')]) as provider:
            config = self.config(provider.endpoint, calls=3)
            config['run'].update(generation_policy='thinking_type_disabled', length_recovery='replan_once')
            host = FileHost(self.root, config)
            outcome = await self.invoke(config, host)
            self.check_success(outcome, host, config, provider, runs=1, calls=3, disabled=True)
            self.assertEqual((len(host.writes), len(host.reviews), len(host.prepares)), (1, 1, 0))
            events = [row['event'] for row in self.events()]
            self.assertEqual(sum(row['type'] == 'length_recovery' for row in events), 1)
            self.assertEqual([row['call'] for row in events if row['type'] == 'model_call'], [1, 2, 3])
            session = json.loads((self.root / 'session.json').read_bytes())
            lengths = [row for row in session['messages'] if row.get('role') == 'assistant' and row.get('stopReason') == 'length']
            self.assertEqual(len(lengths), 1)
            self.assertIn('Partial closed text', json.dumps(lengths[0]))

    async def rejected_policy(self, value):
        with self.provider([text('Must not receive HTTP.')]) as provider:
            config = self.config(provider.endpoint)
            config['run']['generation_policy'] = value
            host = FileHost(self.root, config)
            save(self.root / 'rejected-start.json', config)
            env = {key: value for key, value in os.environ.items()
                   if key.upper() in ('PATH', 'SYSTEMROOT', 'WINDIR', 'TEMP', 'TMP', 'USERPROFILE', 'HOME', 'LANG')}
            env['ARA_AGENT_MODEL_TOKEN'] = TOKEN
            options = {'creationflags': 0x08000000} if os.name == 'nt' else {}
            process = await asyncio.create_subprocess_exec(str(BINARY), stdin=asyncio.subprocess.PIPE,
                stdout=asyncio.subprocess.PIPE, stderr=asyncio.subprocess.PIPE, env=env, **options)
            communication = asyncio.create_task(process.communicate(encoded(config)))
            timed_out = False
            try:
                stdout, stderr = await asyncio.wait_for(asyncio.shield(communication), timeout=10)
            except TimeoutError:
                timed_out = True
                if process.returncode is None:
                    process.kill()
                stdout, stderr = await asyncio.wait_for(communication, timeout=5)
            finally:
                if process.returncode is None:
                    process.kill()
                    await process.wait()
            (self.root / 'stdout.bin').write_bytes(stdout)
            (self.root / 'stderr.bin').write_bytes(stderr)
            save(self.root / 'rejected-process.json', {'pid': process.pid, 'exit_code': process.returncode,
                                                      'timed_out': timed_out})
            self.assertFalse(timed_out, 'native initial-frame rejection timed out')
            self.assertEqual(process.returncode, 2)
            self.assertEqual(stdout, b'')
            self.assertEqual(stderr.strip(), b'start_config_rejected')
            self.assertEqual((len(provider.requests), provider.catalog_requests), (0, 0))
            self.assertEqual((len(host.writes), len(host.reviews), len(host.prepares)), (0, 0, 0))
            for name in ('session.json', 'session.context.json', 'session.journal', 'harness', 'environment', 'host-operations'):
                self.assertFalse((self.root / name).exists(), 'native state was created before start rejection: ' + name)
            self.assertEqual(list(self.root.glob('.ara-lite-harness-*')), [])
            save(self.root / 'result.json', {'status': 'PASS', 'native_rejection': 'start_config_rejected',
                'chat_http_requests': 0, 'catalog_http_requests': 0, 'native_session_created': False,
                'host_callbacks': 0, 'limits': ['test driver created evidence files; no blanket zero-filesystem-effect claim']})

    async def test_unknown_policy_rejected_by_native_first_frame(self):
        await self.rejected_policy('invented_policy')

    async def test_null_policy_rejected_by_native_first_frame(self):
        await self.rejected_policy(None)

    async def test_object_policy_rejected_by_native_first_frame(self):
        await self.rejected_policy({'type': 'disabled'})


if __name__ == '__main__':
    unittest.main(verbosity=2)
