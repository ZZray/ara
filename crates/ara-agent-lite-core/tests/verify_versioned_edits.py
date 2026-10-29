"""Actual business_host versioned-edit acceptance with a controlled provider.

The provider scripts tool selection through the native Agent loop. File effects,
tool receipts and process settlement are real; this is not model intelligence,
a filesystem CAS, a concurrency guarantee or an OS sandbox. No external network
or user credentials are needed. Each case uses one fresh Session and <=8 calls.
"""
import argparse
import hashlib
import http.server
import json
import os
from pathlib import Path
import subprocess
import threading
import time


SOURCE = '\ufeffalpha beta\r\n中文\r\n'.encode('utf-8')
CANDIDATE = '\ufeffbeta gamma\r\n中文\r\n'.encode('utf-8')
EXTERNAL = SOURCE + b'# controlled external change\r\n'
TOKEN = 'versioned-edits-fixture-key'
MODEL = 'versioned-edits-fixture-model'
CASES = ('snapshot-preview-apply-readback', 'stale-hash', 'late-block-failure', 'plan-forged-edit')


def require(ok, message):
    if not ok:
        raise AssertionError(message)


def digest(data):
    return hashlib.sha256(data).hexdigest()


def write_json(path, value):
    path.write_bytes((json.dumps(value, ensure_ascii=False, indent=2) + '\n').encode('utf-8'))


def tool_text(message):
    value = message.get('content')
    if isinstance(value, str):
        return value
    require(isinstance(value, list), 'tool message content has unsupported type')
    return ''.join(block.get('text', '') for block in value if isinstance(block, dict))


class Provider:
    def __init__(self, folder, name, target):
        self.folder, self.name, self.target = folder, name, target
        self.requests, self.catalogs, self.errors, self.observations, self.actions = [], [], [], [], []
        self.snapshot = None
        self.finished = False
        owner = self

        class Handler(http.server.BaseHTTPRequestHandler):
            def log_message(self, *args):
                pass

            def authorize(self):
                require(self.headers.get('Authorization') == 'Bearer ' + TOKEN, 'wrong explicit fixture credential')
                require(not any(key.lower().startswith('x-handwave') for key in self.headers),
                        'independent Host leaked HandWave binding headers')

            def do_GET(self):
                try:
                    self.authorize()
                    require(self.path == '/v1/models', 'wrong catalog path')
                    owner.catalogs.append(self.path)
                    body = json.dumps({'data': [{'id': MODEL, 'protocols': ['openai-completions'],
                        'context_window': 128000, 'max_tokens': 4096,
                        'omp': {'supportsTools': True, 'input': ['text'], 'reasoning': False}}]}).encode()
                    self.send_response(200)
                    self.send_header('Content-Type', 'application/json')
                    self.send_header('Content-Length', str(len(body)))
                    self.end_headers()
                    self.wfile.write(body)
                except Exception as error:
                    owner.errors.append(type(error).__name__ + ': ' + str(error))
                    self.send_error(500)

            def do_POST(self):
                try:
                    self.authorize()
                    require(self.path == '/v1/chat/completions', 'wrong provider path')
                    request = json.loads(self.rfile.read(int(self.headers['Content-Length'])))
                    require(request.get('model') == MODEL, 'wrong selected model')
                    owner.requests.append(request)
                    write_json(folder / ('call-%02d.request.json' % len(owner.requests)), request)
                    delta, finish = owner.next_action(request)
                    self.send_response(200)
                    self.send_header('Content-Type', 'text/event-stream')
                    self.end_headers()
                    for value, reason in [({'role': 'assistant'}, None), (delta, None), ({}, finish)]:
                        packet = {'id': 'versioned-edits-fixture', 'object': 'chat.completion.chunk',
                            'model': MODEL, 'choices': [{'index': 0, 'delta': value, 'finish_reason': reason}]}
                        self.wfile.write(('data: ' + json.dumps(packet) + '\n\n').encode())
                        self.wfile.flush()
                    self.wfile.write(b'data: [DONE]\n\n')
                except (BrokenPipeError, ConnectionResetError, ConnectionAbortedError) as error:
                    owner.errors.append(type(error).__name__ + ': connection closed before scripted response settled')
                except Exception as error:
                    owner.errors.append(type(error).__name__ + ': ' + str(error))
                    self.send_error(500)

        self.server = http.server.ThreadingHTTPServer(('127.0.0.1', 0), Handler)
        self.server.daemon_threads = False
        self.thread = threading.Thread(target=self.server.serve_forever, daemon=True)
        self.thread.start()
        self.endpoint = 'http://127.0.0.1:%d/v1' % self.server.server_port

    def observe(self, step, expected, actor='native-tool'):
        actual = self.target.read_bytes()
        self.observations.append({'before_provider_response': step, 'actor': actor,
            'sha256': digest(actual), 'bytes': len(actual), 'expected_sha256': digest(expected),
            'exact_expected_bytes': actual == expected})
        require(actual == expected, '%s step %d: actual file bytes differ' % (self.name, step))

    def call(self, name, args):
        call_id = '%s-%d' % (self.name, len(self.actions))
        self.actions.append({'id': call_id, 'name': name, 'arguments': args})
        return {'tool_calls': [{'index': 0, 'id': call_id, 'type': 'function',
            'function': {'name': name, 'arguments': json.dumps(args, ensure_ascii=False)}}]}, 'tool_calls'

    def previous(self, tools):
        require(len(tools) == len(self.actions), 'missing, duplicated or unexpected native tool response')
        for message, action in zip(tools, self.actions):
            require(message.get('tool_call_id') == action['id'], 'native tool response ID mismatch')
        return tool_text(tools[-1]) if tools else None

    def next_action(self, request):
        step = len(self.requests)
        require(step <= 8, 'case exceeded model-call allowance')
        tools = [message for message in request['messages'] if message.get('role') == 'tool']
        previous = self.previous(tools)
        advertised = {item['function']['name']: item['function'] for item in request.get('tools', [])}
        if step == 1:
            self.observe(step, SOURCE)
            require('snapshot' in advertised['read_file']['parameters']['properties'], 'new snapshot schema missing')
            if self.name == 'plan-forged-edit':
                require('edit_file' not in advertised, 'Plan advertises edit_file')
                return self.call('edit_file', {'path': 'source.txt', 'old_text': 'alpha',
                    'new_text': 'MUST_NOT_WRITE', 'dry_run': False})
            require({'expected_sha256', 'additional_edits', 'dry_run'} <=
                    set(advertised['edit_file']['parameters']['properties']), 'enhanced edit schema missing')
            return self.call('read_file', {'path': 'source.txt', 'snapshot': True})
        if self.name == 'plan-forged-edit':
            require(step == 2 and previous, 'Plan denial did not return a tool response')
            self.observe(step, SOURCE)
            require('MUST_NOT_WRITE' not in self.target.read_text(encoding='utf-8'), 'forged Plan edit executed')
            self.finished = True
            return {'content': 'VERSIONED_EDIT_DENIAL_OBSERVED'}, 'stop'
        if step == 2:
            self.observe(step, SOURCE)
            self.snapshot = json.loads(previous)
            require(self.snapshot['sha256'] == digest(SOURCE), 'snapshot hash differs from original bytes')
            require(self.snapshot['content'].encode('utf-8') == SOURCE, 'snapshot normalized BOM or CRLF')
            require(self.snapshot['bytes'] == len(SOURCE) and self.snapshot['truncated'] is False,
                    'snapshot is incomplete or wrong size')
            args = {'path': 'source.txt', 'old_text': 'alpha', 'new_text': 'beta',
                    'expected_sha256': self.snapshot['sha256'],
                    'additional_edits': [{'old_text': 'beta', 'new_text': 'gamma'}]}
            if self.name == 'snapshot-preview-apply-readback':
                args['dry_run'] = True
            elif self.name == 'stale-hash':
                # An explicit fixture actor changes the file AFTER the native snapshot.
                self.target.write_bytes(EXTERNAL)
                self.observe(step, EXTERNAL, actor='controlled-external-fixture-write')
            elif self.name == 'late-block-failure':
                args['additional_edits'][0]['old_text'] = 'missing-late-block'
            return self.call('edit_file', args)
        if self.name == 'snapshot-preview-apply-readback':
            if step == 3:
                self.observe(step, SOURCE)
                preview = json.loads(previous)
                require(preview['status'] == 'preview' and preview['committed'] is False,
                        'dry run claimed a committed edit')
                require(preview['before_sha256'] == digest(SOURCE) and
                        preview['candidate_sha256'] == digest(CANDIDATE) and
                        preview['observed_after_sha256'] is None and preview['edit_count'] == 2,
                        'dry-run hashes or original-based edit count wrong')
                require(len(previous.encode()) < 16 * 1024, 'small edit receipt unexpectedly large')
                args = dict(self.actions[-1]['arguments'])
                args['dry_run'] = False
                return self.call('edit_file', args)
            if step == 4:
                self.observe(step, CANDIDATE)
                applied = json.loads(previous)
                require(applied['status'] == 'applied' and applied['committed'] is True,
                        'apply did not acknowledge actual commit')
                require(applied['before_sha256'] == digest(SOURCE) and
                        applied['candidate_sha256'] == digest(CANDIDATE) and
                        applied['observed_after_sha256'] == digest(CANDIDATE) and applied['edit_count'] == 2,
                        'apply hashes or original-based edit count wrong')
                require('not_filesystem_CAS' in applied['concurrency'], 'edit receipt omits concurrency boundary')
                return self.call('read_file', {'path': 'source.txt', 'snapshot': True})
            require(step == 5, 'unexpected successful-edit sequence')
            expected = CANDIDATE
        else:
            expected = EXTERNAL if self.name == 'stale-hash' else SOURCE
            self.observe(step, expected)
            if step == 3:
                marker = 'SHA256 changed' if self.name == 'stale-hash' else 'old_text was not found'
                require(marker in previous, 'expected preflight rejection missing: ' + repr(previous))
                require('effects_unknown' not in previous, 'preflight-only failure claims unknown write effects')
                return self.call('read_file', {'path': 'source.txt', 'snapshot': True})
            require(step == 4, 'unexpected rejected-edit sequence')
        self.observe(step, expected)
        readback = json.loads(previous)
        require(readback['sha256'] == digest(expected) and readback['content'].encode('utf-8') == expected,
                'native readback differs from independently read file')
        self.finished = True
        return {'content': 'VERSIONED_EDIT_CASE_VERIFIED'}, 'stop'

    def close(self):
        self.server.shutdown()
        self.server.server_close()
        self.thread.join(timeout=2)


def run_case(binary, root, name):
    folder = root / name
    folder.mkdir()
    work = folder / 'project'
    work.mkdir()
    target = work / 'source.txt'
    target.write_bytes(SOURCE)
    provider = Provider(folder, name, target)
    config = {'id': name, 'objective': 'Controlled native tool acceptance: ' + name,
        'cwd': str(work), 'session_path': str(folder / 'session.json'),
        'environment_state': str(folder / 'environment-state'), 'endpoint': provider.endpoint,
        'model': MODEL, 'token_env': 'ARA_LITE_VERSIONED_FIXTURE_KEY',
        'mode': 'plan' if name == 'plan-forged-edit' else 'execute', 'enable_local_tools': True}
    config_path = folder / 'request.json'
    write_json(config_path, config)
    environment = os.environ.copy()
    environment['ARA_LITE_VERSIONED_FIXTURE_KEY'] = TOKEN
    environment['NO_PROXY'] = '127.0.0.1,localhost'
    case = {'name': name, 'status': 'FAIL', 'before_sha256': digest(SOURCE)}
    started = time.monotonic()
    try:
        try:
            process = subprocess.run([str(binary), str(config_path)], cwd=work, env=environment,
                                     capture_output=True, timeout=55)
            code, stdout, stderr = process.returncode, process.stdout, process.stderr
            case['process_exit'] = code
        except subprocess.TimeoutExpired as error:
            code, stdout, stderr = None, error.stdout or b'', error.stderr or b''
            case.update(timeout=True, effects_unknown=True)
        (folder / 'host.stdout.jsonl').write_bytes(stdout)
        (folder / 'host.stderr.txt').write_bytes(stderr)
        require(code is not None, 'actual native process timed out; do not replay')
        packets = [json.loads(line) for line in stdout.decode('utf-8').splitlines() if line.strip()]
        events = [packet['event'] for packet in packets if packet.get('kind') == 'event']
        ends = [event for event in events if event.get('type') == 'tool_end']
        write_json(folder / 'tool-end.json', ends)
        require(not provider.errors, 'provider assertions failed: ' + repr(provider.errors))
        require(provider.finished, 'native loop did not reach all independent checks')
        require(code == 0 and packets[-1].get('kind') == 'result' and packets[-1].get('code') == 0,
                'native Run did not settle successfully')
        require(0 < len(provider.requests) <= 8, 'native model call count outside allowance')
        native_calls = [event for event in events if event.get('type') == 'model_call']
        require(len(native_calls) == len(provider.requests), 'native/provider model call count differs')
        require(len(ends) == len(provider.actions), 'native tool_end count differs from scripted actions')
        for receipt, action in zip(ends, provider.actions):
            require(receipt.get('step_id') == action['id'] and receipt.get('title') == action['name'],
                    'actual tool_end identity differs')
            expected_status = ('failed' if name == 'plan-forged-edit' or
                (name in ('stale-hash', 'late-block-failure') and action['name'] == 'edit_file') else 'completed')
            require(receipt.get('status') == expected_status, 'actual tool status differs from expected result')
        expected = CANDIDATE if name == 'snapshot-preview-apply-readback' else EXTERNAL if name == 'stale-hash' else SOURCE
        require(target.read_bytes() == expected, 'final independent file verification failed')
        case.update(status='PASS', native_model_calls=len(native_calls), native_tool_ends=len(ends),
                    final_sha256=digest(target.read_bytes()), verified_original_based_edits=name == CASES[0])
    except Exception as error:
        case['error'] = type(error).__name__ + ': ' + str(error)
    finally:
        provider.close()
        write_json(folder / 'provider-requests.json', provider.requests)
        write_json(folder / 'provider-errors.json', provider.errors)
        write_json(folder / 'scripted-actions.json', provider.actions)
        write_json(folder / 'disk-observations.json', provider.observations)
        case.update(elapsed_seconds=round(time.monotonic() - started, 3),
                    observed_final_sha256=digest(target.read_bytes()))
        write_json(folder / 'case.json', case)
    return case


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--binary', type=Path, required=True)
    parser.add_argument('--output', type=Path, required=True)
    args = parser.parse_args()
    binary, root = args.binary.resolve(), args.output.resolve()
    require(binary.is_file(), '--binary must exist; this harness does not build it')
    root.mkdir(parents=True, exist_ok=False)
    result = {'schema': 'ara-lite-versioned-edits-native-v1', 'status': 'FAIL',
        'scope': 'controlled provider; actual native tools/process/files; not real-model intelligence or CAS',
        'binary_sha256': digest(binary.read_bytes()), 'harness_sha256': digest(Path(__file__).read_bytes()),
        'budget_per_case': {'model_calls': 8, 'host_seconds': 45, 'outer_seconds': 55}, 'cases': []}
    started = time.monotonic()
    try:
        for name in CASES:
            result['cases'].append(run_case(binary, root, name))
        result['status'] = 'PASS' if all(case['status'] == 'PASS' for case in result['cases']) else 'FAIL'
    finally:
        result['elapsed_seconds'] = round(time.monotonic() - started, 3)
        write_json(root / 'result.json', result)
        print(json.dumps(result, ensure_ascii=False, indent=2))
    require(result['status'] == 'PASS', 'native versioned-edit acceptance failed; inspect retained evidence')


if __name__ == '__main__':
    main()
