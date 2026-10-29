"""Actual public Core consumer: large corrected source, selective UTF-8 recall.

The baseline expects a whole-reference tool success followed by a context-budget
failure. The windows profile requires search coordinates, actual source body and
an independently checked native answer. All Session prerequisites use Core APIs.
"""
import argparse
import asyncio
import hashlib
import json
import os
from pathlib import Path
import re
import sys
import time

TESTS = Path(__file__).resolve().parents[2] / 'ara-agent-lite-jsonl/tests'
sys.path.insert(0, str(TESTS))
from controlled_provider import ControlledProvider, call, text

KEY = 'calibration_code='
VALUE = 'cobalt-37'
TOKEN = 'reference-window-fixture-no-provider-grant'


def require(value, message):
    if not value:
        raise AssertionError(message)


def sha(data):
    return hashlib.sha256(data).hexdigest()


def save(path, value):
    with Path(path).open('x', encoding='utf-8') as stream:
        json.dump(value, stream, ensure_ascii=False, indent=2)
        stream.write('\n')
        stream.flush()
        os.fsync(stream.fileno())


def actual_tool_value(request, name):
    names = {item['id']: item['function']['name']
             for message in request['messages'] for item in message.get('tool_calls', [])}
    for message in reversed(request['messages']):
        if message.get('role') == 'tool' and names.get(message['tool_call_id']) == name:
            return json.loads(message['content'])
    raise AssertionError('actual native tool result missing: ' + name)


def large_source():
    head = '原文🙂\n' + 'x' * 24576 + '\n' + KEY + VALUE + '\n末尾🙂\n'
    return head + 'z' * (49152 - len(head.encode('utf-8')))


class DirectoryProvider(ControlledProvider):
    def __init__(self, replies):
        super().__init__(replies)
        base = self.server.RequestHandlerClass
        outer = self

        class Handler(base):
            def do_GET(self):
                require(self.path == '/v1/models', 'unexpected catalog request')
                outer.catalog_requests += 1
                payload = json.dumps({'data': [{'id': outer.model, 'protocols': ['openai-completions'],
                    'context_window': 32768, 'max_tokens': 8192,
                    'omp': {'supportsTools': True, 'input': ['text'], 'reasoning': False}}]}).encode()
                self.send_response(200)
                self.send_header('Content-Type', 'application/json')
                self.send_header('Content-Length', str(len(payload)))
                self.end_headers()
                self.wfile.write(payload)

        self.server.RequestHandlerClass = Handler


class Fixture:
    def __init__(self, args):
        self.binary = Path(args.binary).resolve(strict=True)
        self.root = Path(args.evidence)
        require(self.root.is_absolute() and not self.root.exists(), 'fresh absolute evidence directory required')
        self.root.mkdir(parents=True)
        (self.root / 'state').mkdir()
        self.session = self.root / 'state/session.json'
        self.context = self.root / 'state/session.context.json'
        self.profile = args.expect
        self.source = large_source()
        self.source_hash = sha(self.source.encode())
        self.records = []
        self.selection = {}
        save(self.root / 'identity.json', {'binary': str(self.binary), 'binary_sha256': sha(self.binary.read_bytes()),
            'test_sha256': sha(Path(__file__).read_bytes()), 'profile': self.profile,
            'source_bytes': len(self.source.encode()), 'source_sha256': self.source_hash,
            'context_window': 32768, 'output': 512, 'projection_bytes': 14080, 'request_bytes': 28160,
            'provider': 'controlled local catalog/SSE; no real model', 'product_or_database': 'NOT_RUN'})

    async def invoke(self, endpoint, phase, objective, calls):
        config = {'id': phase + '-run', 'objective': objective, 'cwd': str(self.root),
            'session_path': str(self.session), 'environment_state': str(self.root / 'state/environment'),
            'mode': 'execute', 'endpoint': endpoint, 'model': 'jsonl-fixture',
            'token_env': 'ARA_AGENT_MODEL_TOKEN', 'enable_local_tools': False,
            'max_calls': calls, 'max_seconds': 30, 'max_output': 512}
        path = self.root / (phase + '-request.json')
        save(path, config)
        started = time.time()
        options = {'creationflags': 0x08000000} if os.name == 'nt' else {}
        process = await asyncio.create_subprocess_exec(str(self.binary), str(path),
            stdout=asyncio.subprocess.PIPE, stderr=asyncio.subprocess.PIPE,
            env={**os.environ, 'ARA_AGENT_MODEL_TOKEN': TOKEN}, **options)
        stdout, stderr = await asyncio.wait_for(process.communicate(), timeout=45)
        (self.root / (phase + '-stdout.jsonl')).write_bytes(stdout)
        (self.root / (phase + '-stderr.bin')).write_bytes(stderr)
        frames = [json.loads(row) for row in stdout.splitlines()]
        result = next(row for row in frames if row['kind'] == 'result')
        events = [row['event'] for row in frames if row['kind'] == 'event']
        record = {'phase': phase, 'pid': process.pid, 'exit_code': process.returncode,
            'result': result, 'elapsed_seconds': time.time() - started,
            'reserved_calls': len([row for row in events if row['type'] == 'model_call'])}
        self.records.append(record)
        save(self.root / (phase + '-process.json'), record)
        return record, events

    def select_page(self, body):
        matches = actual_tool_value(body, 'reference_search')['matches']
        match = next(row for row in matches if row['id'] == self.corrected['id'])
        require(match['hash'] == self.source_hash, 'search source hash changed')
        args = {'id': match['id']}
        if 'match_offset' in match:
            require(match['match_offset'] > 20 * 1024, 'test did not select deep source body')
            require(KEY in match['excerpt'], 'search excerpt is not near the actual match')
            args.update(offset=match['match_offset'], limit=4096)
        self.selection.update(args=args, match=match)
        return call('source-page', 'reference_read', args)

    def extract_value(self, body):
        page = actual_tool_value(body, 'reference_read')
        require(page['id'] == self.corrected['id'] and page['hash'] == self.source_hash, 'read identity changed')
        require(len(page['text'].encode()) <= 4096 and page['truncated'], 'original source was not windowed')
        require(page['bytes'] == len(self.source.encode()), 'whole source size lost')
        require(page['hash_scope'] == 'full_source_text_utf8', 'slice masqueraded as whole-source hash')
        found = re.search(r'calibration_code=([a-z0-9-]+)', page['text'])
        require(found is not None, 'actual source page did not contain the requested field')
        self.selection['page'] = page
        return text(json.dumps({'calibration_code': found.group(1), 'source_id': page['id'], 'source_sha256': page['hash']}))

    async def execute(self):
        with DirectoryProvider([text('Source registered.'), call('source-find', 'reference_search', {'query': KEY}),
                self.select_page, self.extract_value]) as provider:
            seed, _ = await self.invoke(provider.endpoint, 'seed',
                'Register an original source; an authorized correction follows later.', 1)
            require(seed['exit_code'] == seed['result']['code'] == 0, 'seed did not complete')
            before = json.loads(self.session.read_bytes())
            old = json.loads(self.context.read_bytes())['references'][0]
            corrected, _ = await self.invoke(provider.endpoint, 'correct',
                '/reference correct ' + old['id'] + ' ' + self.source, 1)
            require(corrected['exit_code'] == corrected['result']['code'] == 0, 'source correction failed')
            require(corrected['reserved_calls'] == 0, 'management unexpectedly used a model')
            after = json.loads(self.session.read_bytes())
            require(before['messages'] == after['messages'], 'management source was fabricated as a raw turn')
            require(KEY not in json.dumps(after['messages']), 'deep source already available in native history')
            refs = json.loads(self.context.read_bytes())['references']
            self.corrected = next(row for row in refs if row['status'] == 'active')
            require(self.corrected['text'] == self.source and self.corrected['hash'] == self.source_hash,
                    'corrected original source changed')
            require(self.corrected['version'] == 2 and next(row for row in refs if row['id'] == old['id'])['status'] == 'superseded',
                    'public management provenance was not retained')
            save(self.root / 'management-source-proof.json', {'id': self.corrected['id'], 'hash': self.source_hash,
                'bytes': len(self.source.encode()), 'unchanged_raw_messages': len(after['messages']),
                'native_history_contains_corrected_document': False})
            read, events = await self.invoke(provider.endpoint, 'read',
                'Find calibration_code in the corrected authoritative source. Read the actual source and return its value as JSON with source_id and source_sha256.', 3)
            save(self.root / 'provider-requests.json', provider.requests)
            save(self.root / 'selection.json', self.selection)
            ends = [row for row in events if row['type'] == 'tool_end' and row.get('title') == 'reference_read']
            require(len(ends) == 1 and ends[0]['status'] == 'completed', 'reference tool did not return a real completed result')
            require(not any(row['type'] == 'model_call' and row.get('phase') == 'summary' for row in events),
                    'unexpected summary confounds the source-window experiment')
            if self.profile == 'baseline':
                require(read['exit_code'] != 0 and read['result']['code'] == -1, 'baseline unexpectedly completed')
                require(read['reserved_calls'] == 2 and len(provider.requests) == 3, 'baseline did not stop after the whole read')
                require(KEY + VALUE in ends[0]['output'], 'baseline failure was not preceded by the full source body')
                require('current run exceeds context byte budget' in read['result']['error'], 'baseline failed for another reason')
                require(not (self.root / 'answer.json').exists(), 'baseline fabricated an answer artifact')
            else:
                require(read['exit_code'] == read['result']['code'] == 0, 'windowed source task failed')
                require(read['reserved_calls'] == 3 and len(provider.requests) == 4, 'source-window budget changed')
                answer = json.loads(read['result']['text'])
                require(answer == {'calibration_code': VALUE, 'source_id': self.corrected['id'], 'source_sha256': self.source_hash},
                        'native result does not match the independent original-source oracle')
                # The Host stores the real native result verbatim, then verifies
                # it. This is not a claim that an Agent write tool was invoked.
                (self.root / 'answer.json').write_text(read['result']['text'], encoding='utf-8')
                require(json.loads((self.root / 'answer.json').read_bytes()) == answer, 'actual Host artifact readback failed')
            require(len({row['pid'] for row in self.records}) == 3, 'did not exercise new native processes')
            require(sha(self.corrected['text'].encode()) == self.source_hash, 'stored source was mutated')
            save(self.root / 'result.json', {'profile': self.profile,
                'observable_result': 'whole-reference-baseline-task-failed' if self.profile == 'baseline' else 'selected-original-window-native-answer-verified',
                'read_exit': read['exit_code'], 'read_code': read['result']['code'],
                'read_reserved_calls': read['reserved_calls'], 'actual_http_requests_all_runs': len(provider.requests),
                'source_bytes': len(self.source.encode()), 'source_sha256': self.source_hash,
                'native_pids': [row['pid'] for row in self.records],
                'artifact_owner': 'Host persisted actual native result; no Agent file-write tool',
                'actual_model': False, 'original_session_preserved': True})
        print(json.dumps({'profile': self.profile, 'evidence': str(self.root), 'status': 'PASS'}))


if __name__ == '__main__':
    parser = argparse.ArgumentParser()
    parser.add_argument('--binary', required=True)
    parser.add_argument('--evidence', required=True)
    parser.add_argument('--expect', choices=('baseline', 'windows'), required=True)
    asyncio.run(Fixture(parser.parse_args()).execute())
