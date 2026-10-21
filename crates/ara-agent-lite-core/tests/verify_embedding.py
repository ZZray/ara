"""Bounded independent consumer/worker acceptance; controlled provider, no Hub.

The actual Rust business_host example links the Core and selects every tool
through the native loop. Deterministic responses test host behavior, not model
intelligence. Effects and credentials are isolated test fixtures, not a sandbox.
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


def require(ok, message):
    if not ok:
        raise AssertionError(message)


class Provider:
    def __init__(self):
        self.requests, self.catalogs, self.errors = [], [], []
        owner = self

        class Handler(http.server.BaseHTTPRequestHandler):
            def log_message(self, *args):
                pass

            def authorize(self):
                require(self.headers.get('Authorization') == 'Bearer embedding-fixture-key', 'wrong explicit credential')
                require(not any(key.lower().startswith('x-handwave') for key in self.headers), 'business host leaked HandWave binding headers')

            def do_GET(self):
                try:
                    self.authorize()
                    require(self.path == '/v1/models', 'wrong catalog path')
                    owner.catalogs.append(self.path)
                    payload = json.dumps({'data': [{'id': 'embedding-model', 'protocols': ['openai-completions'],
                        'context_window': 128000, 'max_tokens': 4096,
                        'omp': {'supportsTools': True, 'input': ['text'], 'reasoning': False}}]}).encode()
                    self.send_response(200)
                    self.send_header('Content-Type', 'application/json')
                    self.send_header('Content-Length', str(len(payload)))
                    self.end_headers()
                    self.wfile.write(payload)
                except Exception as error:
                    owner.errors.append(str(error))
                    self.send_error(500)

            def do_POST(self):
                try:
                    self.authorize()
                    require(self.path == '/v1/chat/completions', 'wrong provider path')
                    body = json.loads(self.rfile.read(int(self.headers['Content-Length'])))
                    require(body['model'] == 'embedding-model', 'wrong selected model')
                    owner.requests.append(body)
                    current = next((m['content'] for m in reversed(body['messages'])
                        if m['role'] == 'user' and isinstance(m.get('content'), str)
                        and ('Current request:' in m['content'] or m['content'].startswith('CASE_'))), '')
                    tools = [m for m in body['messages'] if m['role'] == 'tool']
                    delta, finish = {'content': 'EMBEDDING_DONE'}, 'stop'
                    if 'CASE_HOLD' in current:
                        time.sleep(1)
                    elif 'CASE_LOOKUP' in current and not tools:
                        delta, finish = {'tool_calls': [{'index': 0, 'id': 'embedding-lookup', 'type': 'function',
                            'function': {'name': 'business_lookup', 'arguments': '{}'}}]}, 'tool_calls'
                    elif 'CASE_LOCAL_EFFECT' in current and not tools:
                        delta, finish = {'tool_calls': [{'index': 0, 'id': 'embedding-write', 'type': 'function',
                            'function': {'name': 'write_file', 'arguments': json.dumps({'path': 'local-effect.txt', 'content': 'ACTUAL_CORE_EFFECT'})}}]}, 'tool_calls'
                    elif 'CASE_FORBIDDEN' in current and not tools:
                        delta, finish = {'tool_calls': [{'index': 0, 'id': 'embedding-forbidden', 'type': 'function',
                            'function': {'name': 'write_file', 'arguments': json.dumps({'path': 'forbidden.txt', 'content': 'MUST_NOT_EXIST'})}}]}, 'tool_calls'
                    self.send_response(200)
                    self.send_header('Content-Type', 'text/event-stream')
                    self.end_headers()
                    chunks = [({'role': 'assistant'}, None), (delta, None), ({}, finish)]
                    for value, reason in chunks:
                        packet = {'id': 'embedding', 'object': 'chat.completion.chunk',
                            'choices': [{'index': 0, 'delta': value, 'finish_reason': reason}]}
                        self.wfile.write(('data: ' + json.dumps(packet) + '\n\n').encode())
                        self.wfile.flush()
                    self.wfile.write(b'data: [DONE]\n\n')
                except (BrokenPipeError, ConnectionResetError, ConnectionAbortedError):
                    pass
                except Exception as error:
                    owner.errors.append(str(error))
                    self.send_error(500)

        self.server = http.server.ThreadingHTTPServer(('127.0.0.1', 0), Handler)
        self.server.daemon_threads = True
        threading.Thread(target=self.server.serve_forever, daemon=True).start()
        self.endpoint = f'http://127.0.0.1:{self.server.server_port}/v1'


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--binary', type=Path, required=True)
    parser.add_argument('--output', type=Path, required=True)
    args = parser.parse_args()
    root = args.output.resolve()
    root.mkdir(parents=True, exist_ok=False)
    provider = Provider()
    result = {'status': 'FAIL', 'cases': [], 'scope': 'actual independent business_host + controlled provider + real workers; not real-model intelligence'}
    started = time.monotonic()
    environment = os.environ.copy()
    environment['ARA_LITE_FIXTURE_KEY'] = 'embedding-fixture-key'
    environment['NO_PROXY'] = '127.0.0.1,localhost'

    def run(name, **overrides):
        work = root / name
        work.mkdir()
        config = {'id': name, 'objective': 'CASE_NORMAL', 'cwd': str(work),
            'session_path': str(work / 'session.json'), 'environment_state': str(work / 'private-state'),
            'endpoint': provider.endpoint, 'model': 'embedding-model', 'token_env': 'ARA_LITE_FIXTURE_KEY',
            'extension_name': 'business_lookup', **overrides}
        request = root / (name + '.json')
        request.write_bytes(json.dumps(config).encode() + b'\n')
        before = len(provider.requests)
        process = subprocess.run([str(args.binary.resolve()), str(request)], env=environment,
            cwd=work, capture_output=True, timeout=55)
        (root / (name + '.stdout')).write_bytes(process.stdout)
        (root / (name + '.stderr')).write_bytes(process.stderr)
        packets = [json.loads(line) for line in process.stdout.decode('utf-8').splitlines() if line.startswith('{')]
        return process, packets, work, provider.requests[before:], request

    def passed(name):
        result['cases'].append({'name': name, 'status': 'PASS'})

    try:
        process, packets, work, requests, first_path = run('business-plan', objective='CASE_LOOKUP')
        require(process.returncode == 0, 'independent Plan custom tool failed')
        require(any(p.get('kind') == 'event' and p['event'].get('type') == 'tool_end'
            and 'BUSINESS_LOOKUP_OK' in p['event'].get('output', '') for p in packets), 'actual custom tool receipt missing')
        require(not {'exec', 'write_file', 'job_start', 'knowledge_write', 'web_search'} &
            {t['function']['name'] for t in requests[0]['tools']}, 'default host exposed local tools')
        require(not (work / '.handwave').exists() and not (work / 'docs').exists(), 'default host initialized project libraries')
        require(packets[-1]['kind'] == 'result' and packets[-1]['code'] == 0, 'custom loop did not settle')
        passed('independent-default-plan-custom-tool-and-no-local-profile')

        before = len(provider.requests)
        duplicate = subprocess.run([str(args.binary.resolve()), str(first_path)], env=environment, capture_output=True, timeout=10)
        require(duplicate.returncode != 0 and len(provider.requests) == before, 'duplicate Run replayed model/tool effects')
        passed('duplicate-run-rejected-before-model')

        first = json.loads(first_path.read_text())
        first.update(id='new-run', objective='CASE_NORMAL')
        resumed_path = root / 'resumed.json'
        resumed_path.write_bytes(json.dumps(first).encode())
        reopened = subprocess.run([str(args.binary.resolve()), str(resumed_path)], env=environment, capture_output=True, timeout=20)
        require(reopened.returncode == 0 and len(provider.requests) == before + 1, 'fresh process did not resume same Session')
        require(any(m.get('role') == 'tool' and 'BUSINESS_LOOKUP_OK' in str(m.get('content'))
            for m in provider.requests[-1]['messages']), 'prior native tool history was not resumed')
        passed('fresh-process-session-resume-preserves-native-tool-history')

        process, _, _, requests, _ = run('collision', extension_name='exec')
        require(process.returncode != 0 and not requests, 'reserved extension name reached model')
        passed('reserved-extension-name-rejected')

        process, _, work, requests, _ = run('custom-denied', objective='CASE_LOOKUP', extension_denied_in_plan=True)
        require(all(t['function']['name'] != 'business_lookup' for t in requests[0]['tools']), 'Plan advertised denied extension')
        require(b'BUSINESS_LOOKUP_OK' not in process.stdout, 'Plan executed denied business extension')
        passed('custom-plan-denial-advertisement-and-execution')

        process, _, work, requests, _ = run('plan-forbidden', objective='CASE_FORBIDDEN', enable_local_tools=True)
        require(not (work / 'forbidden.txt').exists(), 'Plan wrote a file')
        require(all(t['function']['name'] != 'write_file' for t in requests[0]['tools']), 'Plan exposed write_file')
        passed('plan-rejects-model-invented-local-write')

        process, packets, work, _, _ = run('local-opt-in', objective='CASE_LOCAL_EFFECT', mode='execute', enable_local_tools=True)
        require(process.returncode == 0 and (work / 'local-effect.txt').read_text() == 'ACTUAL_CORE_EFFECT', 'opt-in native file effect missing')
        passed('explicit-local-tools-real-file-effect')

        process, _, work, _, _ = run('cancel', objective='CASE_HOLD', cancel_after_ms=150)
        require(process.returncode != 0 and not (work / 'local-effect.txt').exists(), 'canceled Run reported success or wrote file')
        passed('cancellation-settles-without-fabricated-success')

        process, _, _, _, _ = run('closed-events', close_events=True)
        require(process.returncode != 0, 'closed event consumer reported successful Run')
        passed('closed-event-consumer-settles-without-hang')

        for terminal, name in [(False, 'worker'), (True, 'pty-worker')]:
            process, packets, work, _, _ = run(name, action='pty_smoke' if terminal else 'job_smoke', enable_worker=True)
            require(process.returncode == 0, name + ' failed; inspect retained stderr/stdout')
            require((work / 'business-startup.txt').read_bytes() == b'startup\n', 'worker re-entered ordinary business startup')
            require(packets[-1]['receipt']['state'] == 'succeeded', 'worker did not settle succeeded')
            if terminal:
                require('READ:hello' in packets[-1]['log']['text'], 'PTY input/output did not reach actual child')
            else:
                require((work / 'job-effect.txt').read_text(encoding='utf-8').splitlines() == ['once'],
                    'duplicate operation repeated actual effect')
                require(len(list(work.rglob('request.json'))) == 1, 'duplicate operation created another spawn intent')
            passed('independent-' + name + '-real-effect-and-idempotency')

        process, _, work, _, _ = run('worker-disabled', action='job_smoke', enable_worker=False)
        require(process.returncode != 0 and not list(work.rglob('request.json')) and not (work / 'job-effect.txt').exists(),
            'disabled worker created spawn intent or side effect')
        passed('missing-worker-rejected-before-spawn-intent')
        require(not provider.errors, 'provider assertion errors: ' + repr(provider.errors))
        result['status'] = 'PASS'
    except Exception as error:
        result['error'] = type(error).__name__ + ': ' + str(error)
    finally:
        provider.server.shutdown()
        result.update(elapsed_seconds=round(time.monotonic() - started, 3), requests=len(provider.requests),
            catalogs=len(provider.catalogs), binary_sha256=hashlib.sha256(args.binary.read_bytes()).hexdigest())
        (root / 'provider-requests.json').write_bytes(json.dumps(provider.requests, ensure_ascii=False, indent=2).encode() + b'\n')
        (root / 'result.json').write_bytes(json.dumps(result, ensure_ascii=False, indent=2).encode() + b'\n')
        print(json.dumps(result, ensure_ascii=False, indent=2))
    require(result['status'] == 'PASS', 'Embedding acceptance failed; inspect retained evidence')


if __name__ == '__main__':
    main()
