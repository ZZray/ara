"""Actual sidecar/Core compaction and Session reopen; deterministic loopback only."""
from __future__ import annotations

import asyncio
from contextlib import AbstractContextManager
import hashlib
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
import json
import os
from pathlib import Path
import re
import sys
import threading
import time
import traceback
import unittest
from unittest.mock import patch


HERE = Path(__file__).resolve().parent
sys.path.insert(0, str(HERE.parent / 'clients/python'))
from ara_agent_lite import run_task

BINARY = Path(os.environ.get('ARA_JSONL_TEST_BINARY',
    'D:/Temp/ara-build/ara-github-target-20261005/debug/ara-agent-lite-jsonl.exe'))
EXPECTED_BINARY_SHA256 = os.environ.get('ARA_JSONL_EXPECTED_BINARY_SHA256',
    'b68ecea895dc4627baa946a2fece2603f66b354163787933d01f6a7a63b142b6')
EVIDENCE = Path(os.environ.get('ARA_JSONL_COMPACTION_EVIDENCE',
    'D:/WorkSpace/tmp/ara-core-capability-20261022/lite-context-continuity-v1-20261025'))
CORRECTION = 'The final artifact must use beta instead of alpha.'
TOKEN = 'owned-loopback-compaction-grant-only'


def sha(path):
    return hashlib.sha256(Path(path).read_bytes()).hexdigest()


def save(path, value):
    with Path(path).open('x', encoding='utf-8') as stream:
        json.dump(value, stream, ensure_ascii=False, indent=2)
        stream.write('\n')
        stream.flush()
        os.fsync(stream.fileno())


def content(message):
    value = message.get('content', '')
    if isinstance(value, str):
        return value
    return '\n'.join(item.get('text', '') for item in value if isinstance(item, dict))


def last_tool(body):
    message = next(row for row in reversed(body['messages']) if row['role'] == 'tool')
    return json.loads(content(message))


def correction_value(text):
    found = re.search(r'The final artifact must use ([a-z]+) instead of alpha\.', text)
    assert found, 'the actual original-reference tool result must contain the correction'
    return found.group(1)


def correction_line(text):
    found = re.search(r'The final artifact must use [a-z]+ instead of alpha\.', text)
    assert found, 'recall artifact must quote the actual read correction'
    return found.group(0)


def working_context(body):
    for row in body['messages']:
        text = content(row)
        if text.startswith('Host-derived working context;'):
            return json.loads(text.split('\n', 1)[1])
    raise AssertionError('actual Core source directory absent')


def correction_id(body):
    return next(row['id'] for row in working_context(body)['source_directory']['references']
                if row['excerpt'].startswith('RUN2_ORIGINAL_CORRECTION'))


def reference_args(body):
    args = {'id': correction_id(body)}
    schema = next(tool['function']['parameters'] for tool in body['tools']
                  if tool['function']['name'] == 'reference_read')
    if 'limit' in schema['properties']:
        args['limit'] = 16384
    return args


def original_reference(body):
    reference = last_tool(body)
    assert reference['text'] == objective(2), 'must recover the entire original correction, not only the answer'
    assert hashlib.sha256(reference['text'].encode()).hexdigest() == reference['hash']
    if 'truncated' in reference:
        assert reference['truncated'] is False
        assert reference['offset'] == 0 and reference['next_offset'] == reference['bytes']
    return reference


CALL_IDS = iter(range(1, 1_000_000))


def call(name, arguments, ordinal):
    # Native call IDs must be unique within a Session (requirement 24.7); the
    # recall provider reopens the same Session, so request ordinals alone repeat.
    return {'delta': {'tool_calls': [{'index': 0, 'id': f'actual-call-{ordinal}-{next(CALL_IDS)}',
        'type': 'function', 'function': {'name': name, 'arguments': json.dumps(arguments)}}]},
        'finish': 'tool_calls'}


class CompactionProvider(AbstractContextManager):
    """Scripts tool choices, but learns source IDs/correction/history from actual HTTP bodies."""
    def __init__(self, root, fault=None, recall=False, summary_hold_seconds=10):
        self.root, self.fault, self.recall = root, fault, recall
        self.summary_hold_seconds = summary_hold_seconds
        self.requests, self.errors, self.summaries = [], [], []
        self.agent_calls = 0
        self.reference = None
        self.history_match = None
        self.summary_inflight = threading.Event()
        self.release_summary = threading.Event()
        self.started = time.monotonic()
        outer = self

        class Handler(BaseHTTPRequestHandler):
            def log_message(self, *_):
                pass

            def do_POST(self):
                try:
                    assert self.path == '/v1/chat/completions'
                    length = int(self.headers['Content-Length'])
                    assert 0 < length <= 1024 * 1024
                    body = json.loads(self.rfile.read(length))
                    summary = content(body['messages'][0]).startswith('Summarize the supplied closed conversation')
                    row = {'ordinal': len(outer.requests) + 1,
                           'phase': 'summary' if summary else 'agent',
                           'elapsed_seconds': time.monotonic() - outer.started, 'body': body}
                    outer.requests.append(row)
                    save(outer.root / f'http-{row["ordinal"]}-request.json', row)
                    response = outer.reply(body, summary)
                    save(outer.root / f'http-{row["ordinal"]}-response.json', response)
                    self.send_response(200)
                    self.send_header('Content-Type', 'text/event-stream')
                    self.end_headers()
                    self.packet({'role': 'assistant'}, None)
                    self.wfile.flush()
                    if summary and outer.fault in ('cancel', 'deadline'):
                        outer.summary_inflight.set()
                        if not outer.release_summary.wait(outer.summary_hold_seconds):
                            raise AssertionError('summary fault hold expired before explicit test release')
                    self.packet(response['delta'], None)
                    self.packet({}, response['finish'])
                    self.wfile.write(b'data: [DONE]\n\n')
                    self.wfile.flush()
                except (BrokenPipeError, ConnectionResetError, ConnectionAbortedError):
                    pass  # Expected when the real Core cancels an in-flight summary.
                except Exception:
                    outer.errors.append(traceback.format_exc())
                    save(outer.root / f'provider-error-{len(outer.errors)}.json', outer.errors[-1])
                    self.send_error(500)

            def packet(self, delta, finish):
                value = {'id': 'owned-compaction-fixture', 'object': 'chat.completion.chunk',
                         'choices': [{'index': 0, 'delta': delta, 'finish_reason': finish}]}
                self.wfile.write(('data: ' + json.dumps(value) + '\n\n').encode())

        self.server = ThreadingHTTPServer(('127.0.0.1', 0), Handler)
        self.server.daemon_threads = True
        self.endpoint = f'http://127.0.0.1:{self.server.server_port}/v1'
        self.thread = threading.Thread(target=self.server.serve_forever, daemon=True)

    def reply(self, body, summary):
        if summary:
            assert not body.get('tools'), 'real summary request must not advertise tools'
            supplied = json.loads(content(body['messages'][-1]))
            assert set(supplied) == {'source_directory', 'previous_summary', 'closed_old_messages'}
            closed = json.dumps(supplied['closed_old_messages'], ensure_ascii=False)
            # Generate from actual supplied data, including any actual prior summary.
            text = 'Closed transcript SHA256=' + hashlib.sha256(closed.encode()).hexdigest()
            text += '; active originals=' + ','.join(row['id'] for row in supplied['source_directory']['references'])
            if CORRECTION in closed:
                text += '; original correction: ' + CORRECTION
            if supplied['previous_summary']:
                text += '; previous=' + supplied['previous_summary']['hash']
            source = supplied['source_directory']['references'][0]
            candidates = [{'kind': 'fact', 'text': 'Historical input, unadopted suggestion: ' + source['excerpt'],
                           'source_ids': [source['id']]}]
            text += '; candidate observations remain unadopted'
            if self.fault == 'fake_source':
                candidates.append({'kind': 'fact', 'text': 'Invalid proposed source must be rejected.',
                                   'source_ids': ['ref-never-existed']})
            output = {'summary': text, 'candidates': candidates}
            self.summaries.append({'input': supplied, 'output': output})
            return {'delta': {'content': json.dumps(output)},
                    'finish': 'length' if self.fault == 'length' else 'stop'}

        self.agent_calls += 1
        assert working_context(body)['accepted_memories'] == [], 'summarizer proposals are never adopted'
        n = self.agent_calls
        if self.recall:
            if n == 1:
                return call('reference_read', reference_args(body), n)
            if n == 2:
                self.reference = original_reference(body)
                assert correction_value(self.reference['text']) == 'beta'
                return call('history_search', {'query': CORRECTION, 'start': 0, 'scan_limit': 256}, n)
            if n == 3:
                matches = last_tool(body)['matches']
                assert matches, 'reopened Session must expose its original correction receipt'
                self.history_match = matches[0]
                return call('history_read', {'id': matches[0]['id'], 'hash': matches[0]['hash'],
                                            'offset': 0, 'limit': 16384}, n)
            if n == 4:
                history = last_tool(body)
                assert CORRECTION in history['text'], 'actual native history read must return the correction'
                value = {'answer': correction_value(self.reference['text']),
                         'reference_id': self.reference['id'], 'reference_hash': self.reference['hash'],
                         'original_text': correction_line(self.reference['text']), 'history_id': history['id'],
                         'history_hash': history['hash'], 'history_text': correction_line(history['text'])}
                return call('write_answer', {'content': json.dumps(value)}, n)
            assert n == 5
            return {'delta': {'content': 'Reopened Session sources read and recall file submitted.'}, 'finish': 'stop'}

        if n in (1, 3):
            assert len(self.summaries) == (0 if n == 1 else 1)
            return call('get_data', {'bytes': 28 * 1024 if n == 1 else 12 * 1024}, n)
        if n in (2, 4):
            data = last_tool(body)
            assert len(data['payload']) == (28 * 1024 if n == 2 else 12 * 1024)
            return {'delta': {'content': 'Data receipt inspected; submit this iteration for Host review.'}, 'finish': 'stop'}
        if n == 5:
            assert len(self.summaries) == 2, 'Run3 must follow two actual summary HTTP requests'
            return call('reference_read', reference_args(body), n)
        if n == 6:
            self.reference = original_reference(body)
            return call('write_answer', {'content': correction_value(self.reference['text'])}, n)
        assert n == 7
        assert last_tool(body)['written'] is True
        return {'delta': {'content': 'Original correction retrieved and actual file submitted.'}, 'finish': 'stop'}

    def __enter__(self):
        self.thread.start()
        return self

    def __exit__(self, *_):
        self.release_summary.set()
        self.server.shutdown()
        self.server.server_close()
        self.thread.join(timeout=3)
        assert not self.thread.is_alive()
        save(self.root / 'provider-ledger.json', {'requests': self.requests, 'summaries': self.summaries,
                                                 'errors': self.errors})


def objective(number):
    if number == 1:
        return 'RUN1_INITIAL_ALPHA: Initial answer is alpha. Get the bounded data and submit this iteration.'
    if number == 2:
        return ('RUN2_ORIGINAL_CORRECTION: Read this source exactly when needed.\n'
                + 'prior observations are source data. ' * 135 + '\n' + CORRECTION + '\n'
                + 'bounded analysis context. ' * 170 + '\nGet bounded data and submit this iteration.')
    return ('RUN3_RETRIEVE_AND_WRITE: Read the exact prior correction with reference_read, then write_answer.\n'
            + 'retained current work notes. ' * 510)


class FileHost:
    def __init__(self, root, config, recall=False):
        self.root, self.config, self.recall = root, config, recall
        self.operations, self.reviews, self.prepares = [], [], []
        self.failures = []

    async def tool_call(self, request, cancelled):
        assert not cancelled.is_set()
        number = len(self.operations) + 1
        start = self.root / f'host-effect-{number}-started.json'
        save(start, request)
        name, args = request['tool_name'], request['arguments']
        if name == 'get_data':
            assert args['bytes'] in (28 * 1024, 12 * 1024)
            result = {'payload': 'd' * args['bytes']}
        else:
            assert name == 'write_answer'
            path = self.root / ('recall-artifact.json' if self.recall else 'answer.txt')
            with path.open('x', encoding='utf-8') as stream:
                stream.write(args['content'])
                stream.flush()
                os.fsync(stream.fileno())
            result = {'written': True, 'path': str(path), 'sha256': sha(path)}
        receipt = self.root / f'host-effect-{number}-settled.json'
        save(receipt, {'request': request, 'result_bytes': len(json.dumps(result)),
                       'result_sha256': hashlib.sha256(json.dumps(result).encode()).hexdigest(), 'settled': True})
        self.operations.append({'tool': name, 'receipt': str(receipt), 'args': args})
        return {'value': result, 'is_error': False, 'settlement': 'Settled', 'operation_receipt': str(receipt)}

    async def verify(self, candidate, cancelled):
        assert not cancelled.is_set()
        number = len(self.reviews) + 1
        path = self.root / ('recall-artifact.json' if self.recall else 'answer.txt')
        if self.recall:
            actual = json.loads(path.read_bytes())
            correct = (actual['answer'] == 'beta' and CORRECTION in actual['original_text']
                       and CORRECTION in actual['history_text'])
        else:
            correct = number == 3 and path.is_file() and path.read_text(encoding='utf-8') == 'beta'
        row = {'basis': candidate['basis'], 'correct': correct,
               'artifact_sha256': sha(path) if path.exists() else None}
        self.reviews.append(row)
        receipt = self.root / f'host-review-{number}.json'
        save(receipt, row)
        return {'basis': candidate['basis'], 'committed': True, 'settlement': 'Settled',
                'verdict': 'Verified' if correct else {'NeedsChanges': {'feedback':
                    'Continue the next iteration; exact original correction remains the source of truth.'}},
                'evidence': [str(receipt) + '#sha256=' + sha(receipt)]}

    async def prepare(self, continuation, cancelled):
        assert not cancelled.is_set()
        assert continuation['deadline_unix_ms'] == self.config['task']['deadline_unix_ms']
        expected = self.config['task']['max_calls'] - (2 if len(self.prepares) == 0 else 5)
        assert continuation['remaining_calls'] == expected
        number = continuation['next_run_number']
        assert number in (2, 3)
        self.prepares.append(continuation)
        save(self.root / f'prepare-{number}.json', continuation)
        save(self.root / f'context-before-run-{number}.json',
             json.loads(Path(self.config['run']['session_path']).with_suffix('.context.json').read_bytes()))
        return {'run_id_next': f'run-{number}', 'objective': objective(number)}


class CompactionProcessTest(unittest.IsolatedAsyncioTestCase):
    def setUp(self):
        self.assertEqual(sha(BINARY), EXPECTED_BINARY_SHA256, 'verify exact delivered sidecar before execution')
        EVIDENCE.mkdir(parents=True, exist_ok=True)
        self.root = EVIDENCE / (self._testMethodName + '-' + str(time.time_ns()))
        self.root.mkdir()
        save(self.root / 'identity.json', {'binary': str(BINARY), 'binary_sha256': sha(BINARY),
             'test_file': str(Path(__file__).resolve()), 'test_sha256': sha(__file__), 'python': sys.executable})
        self.processes = []
        print('COMPACTION_EVIDENCE=' + str(self.root), flush=True)

    def config(self, endpoint, max_calls=9, deadline_ms=40000, recall=False, session=None):
        root = self.root
        return {'kind': 'start_task', 'version': 1,
            'task': {'task_id': 'recall-task' if recall else 'three-feedback-runs',
                     'criteria_revision': 'actual-original-and-file-v1', 'authorization_revision': 'owned-file-v1',
                     'kind': 'Coding', 'evidence_dir': str(root / ('recall-harness' if recall else 'harness')),
                     'max_runs': 1 if recall else 3, 'max_calls': max_calls,
                     'deadline_unix_ms': int(time.time() * 1000) + deadline_ms},
            'run': {'run_id': 'recall-run' if recall else 'run-1',
                    'objective': ('RECALL_ORIGINAL: Reopen this Session; use reference_read and native history '
                                  'to retrieve the previous exact correction and write the recall artifact.') if recall else objective(1),
                    'cwd': str(root), 'session_path': str(session or root / 'session.json'),
                    'environment_state': str(root / 'environment'), 'device_fp': 'compaction-native-process',
                    'session_key': 'compaction/same-released-session', 'mode': 'execute', 'endpoint': endpoint,
                    'model': 'owned-compaction-model', 'max_calls': max_calls, 'max_seconds': 35, 'max_output': 8192,
                    'model_resolution': {'kind': 'host_declared_loopback', 'capabilities': {
                        'context_window': 82000, 'max_tokens': 8192, 'input': ['text']}},
                    'tools': [{'name': name, 'description': description,
                        'parameters': {'type': 'object', 'properties': {key: {'type': kind}},
                                       'required': [key], 'additionalProperties': False}, 'allow_in_plan': False}
                        for name, description, key, kind in (
                            ('get_data', 'Get a bounded owned data receipt.', 'bytes', 'integer'),
                            ('write_answer', 'Write the owned final answer artifact.', 'content', 'string'))]},
            'cancel_grace_ms': 1000, 'emit_events': False}

    async def invoke(self, config, host, prefix='', cancellation=None):
        save(self.root / (prefix + 'start.json'), config)
        original = asyncio.create_subprocess_exec

        async def actual_process(*args, **kwargs):
            proc = await original(*args, **kwargs)
            self.processes.append(proc)
            return proc

        try:
            with patch('asyncio.create_subprocess_exec', actual_process):
                outcome = await run_task(BINARY, config, host, token=TOKEN,
                    operation_directory=self.root / (prefix + 'host-operations'), cancellation=cancellation)
            save(self.root / (prefix + 'consumer-outcome.json'), outcome)
            return outcome
        finally:
            save(self.root / (prefix + 'processes.json'), [{'pid': p.pid, 'returncode': p.returncode}
                                                        for p in self.processes])
            save(self.root / (prefix + 'host-ledger.json'), {'operations': host.operations,
                   'reviews': host.reviews, 'prepares': host.prepares})

    def events(self, dirname='harness'):
        paths = sorted((self.root / dirname).glob('run-*-events.jsonl'))
        return [json.loads(line) for path in paths for line in path.read_bytes().splitlines()]

    def original_sources(self, summary_expected):
        store = json.loads((self.root / 'session.context.json').read_bytes())
        self.assertEqual(store['watermark'], 0)
        self.assertEqual(bool(store['summary']), summary_expected)
        self.assertEqual(len(store['memories']), 2 if summary_expected else 0)
        self.assertTrue(all(row['status'] == 'candidate' and row['adoption'] is None
                            for row in store['memories']))
        refs = store['references']
        self.assertEqual([r['status'] for r in refs], ['active'] * len(refs))
        for ref in refs:
            self.assertEqual(ref['hash'], hashlib.sha256(ref['text'].encode()).hexdigest())
        original = next(r for r in refs if r['text'].startswith('RUN2_ORIGINAL_CORRECTION'))
        self.assertIn(CORRECTION, original['text'])
        self.assertGreater(original['text'].find(CORRECTION), 512)
        return store

    def held(self, outcome):
        self.assertNotEqual(outcome['stop'], 'AwaitingHostAcceptance', outcome)
        self.assertFalse(outcome['task_accepted'])
        self.assertEqual(outcome['fence_presence'], 'Present', outcome)
        self.assertTrue(Path(outcome['reconciliation_fence']).is_file())
        self.assertFalse((self.root / 'harness/released.json').exists())

    async def test_two_summaries_three_runs_and_new_process_reopens_released_session(self):
        started = time.monotonic()
        with CompactionProvider(self.root) as provider:
            config = self.config(provider.endpoint)
            host = FileHost(self.root, config)
            outcome = await self.invoke(config, host)
            self.assertFalse(provider.errors, provider.errors)
            self.assertEqual(outcome['stop'], 'AwaitingHostAcceptance', outcome)
            self.assertEqual((outcome['runs'], outcome['observed_calls']), (3, 9))
            self.assertEqual(outcome['fence_presence'], 'Released')
            self.assertFalse(outcome['task_accepted'])
            self.assertEqual([r['phase'] for r in provider.requests],
                             ['agent', 'agent', 'summary', 'agent', 'agent', 'summary', 'agent', 'agent', 'agent'])
            self.assertEqual([r['correct'] for r in host.reviews], [False, False, True])
            self.assertEqual((self.root / 'answer.txt').read_text(), 'beta')
            store = self.original_sources(True)
            first = json.loads((self.root / 'context-before-run-3.json').read_bytes())['summary']
            second = store['summary']
            self.assertIsNone(first['previous_summary_hash'])
            self.assertEqual(second['previous_summary_hash'], first['hash'])
            self.assertGreater(second['prefix_end'], first['prefix_end'])
            self.assertEqual(provider.summaries[1]['input']['previous_summary']['hash'], first['hash'])
            self.assertIsNone(provider.summaries[0]['input']['previous_summary'])
            self.assertIn(CORRECTION, json.dumps(provider.summaries[1]['input']['closed_old_messages']))
            for n, remaining in ((1, 9), (2, 7), (3, 4)):
                admission = json.loads((self.root / f'harness/run-{n}-admission.json').read_bytes())
                self.assertEqual(admission['remaining_calls'], remaining)
                self.assertEqual(admission['deadline_unix_ms'], config['task']['deadline_unix_ms'])
            calls = [r['event'] for r in self.events() if r['event']['type'] == 'model_call']
            self.assertEqual([r['phase'] for r in calls], [r['phase'] for r in provider.requests])
            self.assertEqual(sum(r['event']['type'] == 'compact' for r in self.events()), 2)

        recall_root = self.root / 'reopened-provider'
        recall_root.mkdir()
        recall_host_root = self.root / 'recall-host'
        recall_host_root.mkdir()
        with CompactionProvider(recall_root, recall=True) as provider:
            config2 = self.config(provider.endpoint, max_calls=6, recall=True)
            # Same ordinary Session/environment/identity; fresh Task/process/evidence.
            host2 = FileHost(recall_host_root, config2, recall=True)
            outcome2 = await self.invoke(config2, host2, 'recall-')
            self.assertFalse(provider.errors, provider.errors)
            self.assertEqual(outcome2['stop'], 'AwaitingHostAcceptance', outcome2)
            self.assertEqual(outcome2['fence_presence'], 'Released')
            self.assertEqual(outcome2['observed_calls'], 6)
            self.assertEqual(len(provider.summaries), 1)
            self.assertEqual(provider.summaries[0]['input']['previous_summary']['hash'], second['hash'])
            self.assertEqual([r['phase'] for r in provider.requests],
                             ['agent', 'summary', 'agent', 'agent', 'agent', 'agent'])
            self.assertEqual([p.returncode for p in self.processes], [0, 0])
            self.assertNotEqual(self.processes[0].pid, self.processes[1].pid)
            recalled = json.loads((recall_host_root / 'recall-artifact.json').read_bytes())
            self.assertEqual(recalled['answer'], 'beta')
            self.assertEqual(recalled['reference_hash'], provider.reference['hash'])
            self.assertEqual(recalled['history_hash'], provider.history_match['hash'])
            self.assertFalse(outcome2['task_accepted'])
            reopened_store = json.loads((self.root / 'session.context.json').read_bytes())
            self.assertEqual(reopened_store['summary']['previous_summary_hash'], second['hash'])
            self.assertGreater(reopened_store['summary']['prefix_end'], second['prefix_end'])
            self.assertEqual(len(reopened_store['memories']), 3)
            self.assertTrue(all(row['status'] == 'candidate' and row['adoption'] is None
                                for row in reopened_store['memories']))
            save(self.root / 'context-after-reopen.json', reopened_store)
        save(self.root / 'result.json', {'status': 'PASS', 'summary_http_calls': 2, 'task_http_calls': 9,
             'runs': 3, 'reopen_http_calls': 6, 'reopen_summary_http_calls': 1,
             'sidecar_pids': [p.pid for p in self.processes],
             'elapsed_seconds': time.monotonic() - started, 'task_accepted': False,
             'limits': ['controlled provider; no model competence claim', 'Lite JSONL path only',
                        'no product or database or release acceptance']})

    async def test_original_eight_call_budget_blocks_ninth_http_after_actual_write(self):
        with CompactionProvider(self.root) as provider:
            config = self.config(provider.endpoint, max_calls=8)
            host = FileHost(self.root, config)
            outcome = await self.invoke(config, host)
            self.held(outcome)
            self.assertFalse(provider.errors, provider.errors)
            self.assertEqual((outcome['runs'], outcome['observed_calls']), (3, 8))
            self.assertEqual(len(provider.requests), 8, 'no ninth HTTP even though beta was already written')
            self.assertEqual(len(provider.summaries), 2)
            self.assertEqual((self.root / 'answer.txt').read_text(), 'beta')
            self.assertEqual(len(host.reviews), 2, 'partial final Run cannot become a candidate')
            self.original_sources(True)

    async def bad_summary(self, fault):
        deadline_ms = 15000 if fault == 'deadline' else 40000
        with CompactionProvider(self.root, fault=fault,
                                summary_hold_seconds=deadline_ms / 1000 + 5) as provider:
            config = self.config(provider.endpoint, deadline_ms=deadline_ms)
            host = FileHost(self.root, config)
            cancellation = asyncio.Event()
            summary_started_unix_ms = None

            async def observe_inflight():
                nonlocal summary_started_unix_ms
                present = await asyncio.to_thread(provider.summary_inflight.wait, 10)
                self.assertTrue(present, 'actual SSE summary must be in flight before cancellation')
                summary_started_unix_ms = int(time.time() * 1000)
                if fault == 'cancel':
                    cancellation.set()
                else:
                    self.assertLess(summary_started_unix_ms, config['task']['deadline_unix_ms'] - 1000,
                                    'summary must start with time left to exercise the deadline')
                save(self.root / 'inflight-summary.json', {'http_calls': len(provider.requests),
                     'summary_inflight': present, 'unix_ms': int(time.time() * 1000)})

            watcher = asyncio.create_task(observe_inflight()) if fault in ('cancel', 'deadline') else None
            try:
                # This watchdog bounds fixture cleanup, not the Task's work budget.
                # The actual original Task deadline remains in the start frame.
                outcome = await asyncio.wait_for(self.invoke(config, host, cancellation=cancellation),
                                                 timeout=deadline_ms / 1000 + 10)
                completed_unix_ms = int(time.time() * 1000)
                if watcher:
                    await watcher
            finally:
                provider.release_summary.set()
                if watcher and not watcher.done():
                    watcher.cancel()
                    await asyncio.gather(watcher, return_exceptions=True)
            self.held(outcome)
            self.assertFalse(provider.errors, provider.errors)
            self.assertEqual(len(provider.requests), 3, 'failed summary must prevent later agent HTTP')
            self.assertEqual(len(provider.summaries), 1)
            self.assertEqual(len(host.operations), 1, 'no subsequent tool or artifact write')
            self.assertEqual(len(host.reviews), 1)
            self.assertFalse((self.root / 'answer.txt').exists())
            self.original_sources(False)
            self.assertEqual(sum(r['event']['type'] == 'compact' for r in self.events()), 0)
            if fault in ('cancel', 'deadline'):
                self.assertTrue(provider.summary_inflight.is_set())
                if fault == 'cancel':
                    self.assertEqual(outcome['stop'], 'Cancelled')
                else:
                    first = json.loads((self.root / 'harness/run-1-admission.json').read_bytes())
                    second = json.loads((self.root / 'harness/run-2-admission.json').read_bytes())
                    self.assertEqual(first['deadline_unix_ms'], config['task']['deadline_unix_ms'])
                    self.assertEqual(second['deadline_unix_ms'], config['task']['deadline_unix_ms'])
                    # Floored seconds only drop when Run 1 takes at least the
                    # sub-second remainder; a fast sidecar can finish it sooner.
                    self.assertLessEqual(second['max_seconds'], first['max_seconds'])
                    self.assertFalse(cancellation.is_set(), 'deadline case sends no external cancel')
                    observation = json.loads((self.root / 'harness/run-2-observation.json').read_bytes())
                    self.assertTrue(observation['core_joined'], 'the same Core future must settle')
                    # Core starts its floored relative timer after admission I/O.
                    # Either it or the original absolute Task deadline may win.
                    if outcome['stop'] == 'Cancelled':
                        self.assertTrue(observation['stopped'])
                        self.assertEqual(outcome['detail'],
                            'canceled/deadline; original Core future joined; reconcile effects')
                    else:
                        self.assertEqual(outcome['stop'], 'ReconciliationRequired')
                        self.assertFalse(observation['stopped'])
                        self.assertEqual(outcome['detail'],
                            'Core/event completion cannot establish a settled ordinary candidate')
                    deadline_details = {'context maintenance failed: context maintenance deadline exceeded',
                                        'context maintenance failed: context maintenance canceled'}
                    terminals = [r for r in self.events() if r['run_id'] == 'run-2'
                                 and r['event']['type'] == 'agent_end']
                    self.assertEqual(len(terminals), 1)
                    terminal = terminals[0]
                    self.assertIn(terminal['event']['detail'], deadline_details)
                    # The floored Core deadline can be less than one second
                    # early. Join/checkpoint/fence I/O has no one-second SLA;
                    # the watchdog bounds the fixture instead of renewing work.
                    self.assertGreaterEqual(completed_unix_ms, config['task']['deadline_unix_ms'] - 1000)
                    save(self.root / 'inherited-deadline.json', {'original_task_deadline_unix_ms':
                         config['task']['deadline_unix_ms'], 'completed_unix_ms': completed_unix_ms,
                         'summary_started_unix_ms': summary_started_unix_ms,
                         'terminal_event': terminal, 'core_observation': observation,
                         'run1_max_seconds': first['max_seconds'], 'run2_max_seconds': second['max_seconds'],
                         'reason': 'Original Task deadline never renewed; terminal boundary and joined settlement recorded separately'})
            else:
                results = [r['event'] for r in self.events()
                           if r['event']['type'] == 'model_result' and r['event']['phase'] == 'summary']
                self.assertEqual(len(results), 1)
                self.assertEqual(results[0]['stop_reason'], 'length' if fault == 'length' else 'stop')

    async def test_summary_length_rejects_even_valid_json(self):
        await self.bad_summary('length')

    async def test_summary_invented_source_rejects_without_installing(self):
        await self.bad_summary('fake_source')

    async def test_cancel_during_actual_summary_http(self):
        await self.bad_summary('cancel')

    async def test_original_deadline_bounds_inflight_summary_across_runs(self):
        await self.bad_summary('deadline')


if __name__ == '__main__':
    unittest.main(verbosity=2)
