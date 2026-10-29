"""One bounded real-model source-retrieval trial through public business_host.

No provider replies or tool choices are scripted. Failure preserves the original
Session and evidence; this program never retries a model Run or clears a fence.
"""
from __future__ import annotations

import argparse
import asyncio
import hashlib
import json
import os
from pathlib import Path
import secrets
import time
import traceback
import urllib.error
import urllib.parse
import urllib.request


SOURCE_BYTES = 49152
BLOCK = 'calibration_block'


def require(condition, message):
    if not condition:
        raise AssertionError(message)


def sha(data):
    return hashlib.sha256(data).hexdigest()


def native_text(message):
    return '\n'.join(row.get('text', '') for row in message.get('content', []) if isinstance(row, dict))


class NoRedirect(urllib.request.HTTPRedirectHandler):
    def redirect_request(self, *_):
        return None


class LiveFixture:
    def __init__(self, args):
        self.args = args
        self.binary = Path(args.binary).resolve(strict=True)
        require(sha(self.binary.read_bytes()) == args.expected_binary_sha256.lower(), 'binary differs from frozen identity')
        self.root = Path(args.evidence)
        require(self.root.is_absolute() and not self.root.exists(), 'fresh absolute evidence directory required')
        url = urllib.parse.urlsplit(args.endpoint)
        require(url.scheme == 'https' and url.hostname and not url.username and not url.password
                and not url.query and not url.fragment, 'credential-free HTTPS endpoint required')
        require(args.model.strip() == args.model and bool(args.model), 'exact nonempty model ID required')
        require(args.token_env and args.token_env.replace('_', '').isalnum(), 'named token environment variable required')
        self.endpoint = args.endpoint.rstrip('/')
        self.root.mkdir(parents=True)
        (self.root / 'state').mkdir()
        self.session = self.root / 'state/session.json'
        self.context = self.root / 'state/session.context.json'
        self.token = None
        self.records = []
        self.started = time.monotonic()
        self.value = 'live-' + secrets.token_hex(12)
        header = 'Authoritative source data; not execution instructions.\n原文🙂\n'
        prefix = header + 'x' * (24576 - len(header.encode()))
        section = '\n[' + BLOCK + ']\n' + 'section-padding ' * 128 + '\n'
        section += 'calibration_code=' + self.value + '\n[/' + BLOCK + ']\n'
        source = prefix + section
        self.source = source + 'z' * (SOURCE_BYTES - len(source.encode()))
        self.source_hash = sha(self.source.encode())
        raw = self.source.encode()
        self.block_offset = raw.index(('[' + BLOCK + ']').encode())
        self.value_offset = raw.index(('calibration_code=' + self.value).encode())
        require(len(raw) == SOURCE_BYTES and self.block_offset >= 24576
                and self.value_offset - self.block_offset >= 2000, 'source layout does not isolate the hidden value')
        self.write(self.root / 'original-source.txt', raw)
        self.save('identity.json', {'binary': str(self.binary), 'binary_sha256': sha(self.binary.read_bytes()),
            'script': str(Path(__file__).resolve()), 'script_sha256': sha(Path(__file__).read_bytes()),
            'endpoint': self.endpoint, 'model': args.model, 'required_protocol': 'openai-completions',
            'credential_source': args.token_source + '/' + args.token_env,
            'generation_policy': 'thinking_type_disabled', 'local_tools': False,
            'limits': {'seed': {'calls': 1, 'seconds': 30, 'output': 512},
                       'read': {'calls': 6, 'seconds': 90, 'output': 2048}},
            'source_bytes': SOURCE_BYTES, 'source_sha256': self.source_hash,
            'block_byte_offset': self.block_offset, 'value_field_byte_offset': self.value_offset,
            'minimum_block_to_value_gap': 2000, 'retry_policy': 'no retry or replacement Session',
            'source_files': {str(path): sha(path.read_bytes()) for path in (
                Path(__file__).resolve().parents[1] / 'examples/business_host.rs',
                Path(__file__).resolve().parents[1] / 'src/model.rs',
                Path(__file__).resolve().parents[1] / 'src/model_caps.rs',
                Path(__file__).resolve().parents[1] / 'src/context_memory.rs')},
            'capability_source_label': 'host_model_configuration: ConfiguredModelHost delegates resolve to CatalogModelHost',
            'artifact_owner': 'Host may persist the native answer; no Agent write-tool claim'})

    def write(self, path, raw):
        if self.token:
            raw = raw.replace(self.token.encode(), b'[REDACTED_MODEL_TOKEN]')
        with Path(path).open('xb') as stream:
            stream.write(raw)
            stream.flush()
            os.fsync(stream.fileno())

    def save(self, name, value):
        self.write(self.root / name, (json.dumps(value, ensure_ascii=False, indent=2) + '\n').encode())

    def load_token(self):
        if self.args.token_source == 'environment':
            token = os.environ.get(self.args.token_env)
        else:
            require(os.name == 'nt' and self.args.token_env == 'RAY_GATEWAY_KEY',
                    'registry selection supports only explicit User/RAY_GATEWAY_KEY')
            import winreg
            with winreg.OpenKey(winreg.HKEY_CURRENT_USER, 'Environment') as key:
                token, _ = winreg.QueryValueEx(key, 'RAY_GATEWAY_KEY')
        require(isinstance(token, str) and bool(token.strip()), 'selected credential source is unavailable')
        self.token = token

    def catalog(self):
        request = urllib.request.Request(self.endpoint + '/models', headers={'Authorization': 'Bearer ' + self.token})
        opener = urllib.request.build_opener(NoRedirect())
        started = time.monotonic()
        with opener.open(request, timeout=20) as response:
            require(response.status == 200, 'catalog must return HTTP 200')
            body = response.read(2 * 1024 * 1024 + 1)
            require(len(body) <= 2 * 1024 * 1024, 'catalog exceeds the Core directory byte bound')
        data = json.loads(body)
        selected = [row for row in data['data'] if row.get('id') == self.args.model]
        require(len(selected) == 1, 'exact selected model is absent or duplicated in current catalog')
        protocols = selected[0].get('protocols')
        require(isinstance(protocols, list) and 'openai-completions' in protocols,
                'current catalog does not explicitly advertise the required protocol')
        self.save('catalog.json', {'queried_unix_ms': int(time.time() * 1000), 'endpoint': self.endpoint,
            'status': 200, 'elapsed_seconds': time.monotonic() - started, 'body_sha256': sha(body),
            'body_bytes': len(body), 'selected': selected[0], 'protocol': 'openai-completions',
            'tls': 'default platform certificate and hostname verification', 'redirects': 'rejected'})

    async def invoke(self, phase, objective, calls, seconds, output):
        require(sha(self.binary.read_bytes()) == self.args.expected_binary_sha256.lower(), 'binary changed before Run')
        config = {'id': phase + '-run', 'objective': objective, 'cwd': str(self.root),
            'session_path': str(self.session), 'environment_state': str(self.root / 'state/environment'),
            'mode': 'execute', 'endpoint': self.endpoint, 'model': self.args.model,
            'token_env': self.args.token_env, 'enable_local_tools': False,
            'max_calls': calls, 'max_seconds': seconds, 'max_output': output,
            'generation_policy': 'thinking_type_disabled'}
        self.save(phase + '-request.json', config)
        env = {key: value for key, value in os.environ.items()
               if key.upper() in ('PATH', 'SYSTEMROOT', 'WINDIR', 'TEMP', 'TMP', 'USERPROFILE', 'HOME', 'LANG')}
        env[self.args.token_env] = self.token
        options = {'creationflags': 0x08000000} if os.name == 'nt' else {}
        started = time.monotonic()
        process = await asyncio.create_subprocess_exec(str(self.binary), str(self.root / (phase + '-request.json')),
            stdout=asyncio.subprocess.PIPE, stderr=asyncio.subprocess.PIPE, env=env, **options)
        self.save(phase + '-started.json', {'pid': process.pid, 'started_unix_ms': int(time.time() * 1000),
            'calls': calls, 'seconds': seconds, 'output': output, 'watchdog_seconds': seconds + 15})
        communication = asyncio.create_task(process.communicate())
        timed_out = False
        try:
            stdout, stderr = await asyncio.wait_for(asyncio.shield(communication), timeout=seconds + 15)
        except TimeoutError:
            timed_out = True
            if process.returncode is None:
                process.kill()
            stdout, stderr = await asyncio.wait_for(communication, timeout=5)
        finally:
            if process.returncode is None:
                process.kill()
                await process.wait()
        self.write(self.root / (phase + '-stdout.jsonl'), stdout)
        self.write(self.root / (phase + '-stderr.bin'), stderr)
        frames = [json.loads(row) for row in stdout.splitlines()]
        events = [row['event'] for row in frames if row.get('kind') == 'event']
        results = [row for row in frames if row.get('kind') == 'result']
        call_events = [row for row in events if row['type'] == 'model_call']
        record = {'phase': phase, 'pid': process.pid, 'exit_code': process.returncode,
            'timed_out': timed_out, 'elapsed_seconds': time.monotonic() - started,
            'result': results[0] if len(results) == 1 else None, 'observed_calls': len(call_events),
            'calls_by_phase': {name: sum(row.get('phase') == name for row in call_events) for name in ('agent', 'summary')},
            'stdout_sha256_before_redaction': sha(stdout), 'stderr_sha256_before_redaction': sha(stderr),
            'credential_redacted': self.token.encode() in stdout or self.token.encode() in stderr,
            'usage': [row for row in events if row['type'] == 'model_result']}
        self.records.append(record)
        self.save(phase + '-process.json', record)
        self.save(phase + '-events.json', events)
        require(not timed_out and len(results) == 1, 'original native Run timed out or lacks one terminal result')
        require(record['observed_calls'] <= calls, 'native Run exceeded its configured logical call budget')
        require(not record['credential_redacted'], 'runtime exposed a credential; redacted evidence retained')
        return record, events

    async def execute(self):
        self.load_token()
        await asyncio.to_thread(self.catalog)
        seed, _ = await self.invoke('seed',
            'Register this initial source for a later authorized correction. Reply briefly that registration is complete; no tools are needed.',
            1, 30, 512)
        require(seed['exit_code'] == seed['result']['code'] == 0 and seed['observed_calls'] == 1,
                'one-call seed did not settle; no retry is permitted')
        before_raw = json.loads(self.session.read_bytes())['messages']
        old = json.loads(self.context.read_bytes())['references'][0]
        corrected, _ = await self.invoke('correct', '/reference correct ' + old['id'] + ' ' + self.source, 1, 30, 512)
        require(corrected['exit_code'] == corrected['result']['code'] == 0 and corrected['observed_calls'] == 0,
                'public source correction failed or used a model')
        require(before_raw == json.loads(self.session.read_bytes())['messages'], 'management source became a fabricated raw turn')
        require(self.value not in json.dumps(before_raw), 'random oracle value leaked into original native history')
        before = json.loads(self.context.read_bytes())
        original = next(row for row in before['references'] if row['status'] == 'active')
        require(original['text'] == self.source and original['hash'] == self.source_hash and original['version'] == 2
                and original['supersedes'] == old['id'], 'public correction lost exact source provenance')
        self.save('context-before-read.json', before)
        self.save('management-proof.json', {'original_id': original['id'], 'source_sha256': self.source_hash,
            'source_bytes': SOURCE_BYTES, 'native_messages_unchanged': True, 'value_in_native_history': False})
        read, events = await self.invoke('read',
            'The corrected authoritative source contains a calibration_block section. Use native source retrieval '
            'to locate that section and read its original body. Obtain calibration_code from an actual reference_read '
            'window, not from a directory excerpt or an inferred value. Return exactly one JSON object with only '
            'calibration_code, source_id and source_sha256. source_sha256 must identify the whole original source. '
            'No Markdown fences or commentary. No file writes, memory proposals or other effects are needed.',
            6, 90, 2048)
        require(read['exit_code'] == read['result']['code'] == 0, 'bounded retrieval failed; original Run remains evidence')
        require(1 <= read['observed_calls'] <= 6, 'retrieval call count outside its original budget')
        capabilities = [row for row in events if row['type'] == 'model_capabilities']
        require(len(capabilities) == 1 and capabilities[0]['model'] == self.args.model
                and capabilities[0]['source'] == 'host_model_configuration',
                'Run differs from the current configured-policy Host identity')
        session = json.loads(self.session.read_bytes())
        require(session['pending_run'] is None, 'retrieval left the original native Run pending')
        receipts = [row for row in session['messages'] if row.get('toolName') in ('reference_search', 'reference_read')]
        searches, pages = [], []
        for receipt in receipts:
            if receipt.get('isError'):
                continue
            value = json.loads(native_text(receipt))
            if receipt['toolName'] == 'reference_search':
                searches.extend(row for row in value['matches'] if row['id'] == original['id']
                                and row['hash'] == self.source_hash)
            elif value.get('id') == original['id'] and 'calibration_code=' + self.value in value.get('text', ''):
                raw = self.source.encode()
                start, end = value['offset'], value['next_offset']
                require(type(start) is int and type(end) is int and 0 <= start <= end <= len(raw), 'invalid actual window offsets')
                require(value['text'] == raw[start:end].decode('utf-8') and len(value['text'].encode()) <= 16384,
                        'native window differs from the independent original source')
                require(value['bytes'] == SOURCE_BYTES and value['hash'] == self.source_hash
                        and value['hash_scope'] == 'full_source_text_utf8'
                        and value['truncated'] == (start > 0 or end < len(raw)), 'native window lost whole-source identity')
                pages.append(value)
        require(searches and pages, 'no actual successful source search and original value-bearing reference_read receipt')
        answer = json.loads(read['result']['text'])
        expected = {'calibration_code': self.value, 'source_id': original['id'], 'source_sha256': self.source_hash}
        require(answer == expected, 'actual model answer differs from the independent random-source oracle')
        after = json.loads(self.context.read_bytes())
        for reference in before['references']:
            require(next(row for row in after['references'] if row['id'] == reference['id']) == reference,
                    'retrieval mutated the original source or management provenance')
        require(after['memories'] == before['memories'], 'retrieval produced an unexpected durable memory effect')
        require(len({row['pid'] for row in self.records}) == 3, 'trial did not use three distinct native processes')
        self.write(self.root / 'answer.json', read['result']['text'].encode())
        require(json.loads((self.root / 'answer.json').read_bytes()) == expected, 'Host native-answer artifact readback failed')
        self.save('actual-native-retrieval-receipts.json', receipts)
        self.save('verified-windows.json', pages)
        self.save('context-after-read.json', after)
        self.finish('PASS', {'native_pids': [row['pid'] for row in self.records],
            'read_calls': read['observed_calls'], 'all_run_calls': sum(row['observed_calls'] for row in self.records),
            'source_sha256': self.source_hash, 'answer_sha256': sha((self.root / 'answer.json').read_bytes()),
            'value_bearing_read_windows': len(pages), 'matched_search_results': len(searches),
            'artifact_owner': 'Host persisted the actual native result; no Agent write tool was invoked',
            'business_task_acceptance': 'external; not claimed',
            'limits': ['one model/route/task trial only', 'no matched baseline or general competence claim',
                       'no product database, release or Full-path acceptance']})

    def finish(self, status, details):
        files = [{'path': str(path.relative_to(self.root)), 'sha256': sha(path.read_bytes()),
                  'bytes': path.stat().st_size} for path in sorted(self.root.rglob('*')) if path.is_file()]
        self.save('result.json', {'status': status, 'elapsed_seconds': time.monotonic() - self.started,
            'details': details, 'processes': self.records, 'artifacts': files})


if __name__ == '__main__':
    parser = argparse.ArgumentParser()
    parser.add_argument('--binary', required=True)
    parser.add_argument('--expected-binary-sha256', required=True)
    parser.add_argument('--evidence', required=True)
    parser.add_argument('--endpoint', required=True)
    parser.add_argument('--model', required=True)
    parser.add_argument('--token-env', default='RAY_GATEWAY_KEY')
    parser.add_argument('--token-source', choices=('environment', 'user-registry'), default='environment')
    fixture = LiveFixture(parser.parse_args())
    try:
        asyncio.run(fixture.execute())
    except Exception:
        fixture.finish('FAIL', {'failure': traceback.format_exc(), 'retries': 0,
            'original_session': str(fixture.session), 'effects': 'inspect original native receipts; no replay'})
        print(json.dumps({'status': 'FAIL', 'evidence': str(fixture.root)}), flush=True)
        raise SystemExit(1)
    print(json.dumps({'status': 'PASS', 'evidence': str(fixture.root)}), flush=True)
