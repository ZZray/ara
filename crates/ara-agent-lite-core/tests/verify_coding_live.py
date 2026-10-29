"""Opt-in real-model coding baseline through the independent business_host.

Default/--prepare creates deliberately faulty isolated projects and proves the
pre-run oracle fails. --live additionally invokes the native Host once per task.
Credentials are read only from explicitly named environment variables. Neither
this harness nor Core tools provide an OS sandbox. Use an authorized test host.
Hidden samples are created only while the model process is not running; model
answers, claimed test success and an exit code are never the correctness oracle.
"""
import argparse
import contextlib
import difflib
import hashlib
import http.server
import json
import os
from pathlib import Path
import shutil
import subprocess
import sys
import tempfile
import threading
import time
import urllib.error
import urllib.parse
import urllib.request


PYTHON_FILES = {
    'README.md': '''# Ledger repair exercise
The public interface is ledger.report.summarize(rows), returning an integer
number of cents. Each row has amount (decimal string) and status. Include only
posted rows, ignoring case in status; include negative refunds. Round EACH row
to cents with decimal ROUND_HALF_UP, then sum cents. Inputs are finite decimal
amounts, possibly zero, fractions of a cent, or large values. Do not change this
contract or project data. Repair the implementation and verify your work.
''',
    'ledger/__init__.py': '',
    'ledger/currency.py': '''def cents(amount):
    return round(float(amount) * 100)
''',
    'ledger/report.py': '''from .currency import cents


def summarize(rows):
    return sum(cents(row["amount"]) for row in rows
               if row["status"] == "posted" and float(row["amount"]) > 0)
''',
}
PYTHON_REFERENCE = {
    'ledger/currency.py': '''from decimal import Decimal, ROUND_HALF_UP


def cents(amount):
    return int((Decimal(amount) * 100).to_integral_value(rounding=ROUND_HALF_UP))
''',
    'ledger/report.py': '''from .currency import cents


def summarize(rows):
    return sum(cents(row["amount"]) for row in rows
               if row["status"].casefold() == "posted")
''',
}
POWERSHELL_FILES = {
    'README.md': '''# Text inventory repair exercise
The PowerShell entry point in this project accepts -Root (a directory path).
Return one JSON object: {"total_bytes": integer, "files": [{"path": relative
path using forward slashes, "bytes": integer}]}. Recursively include regular
.txt files (case insensitive extension), including empty files, and exclude
other extensions. Paths can contain Chinese text, spaces, and literal brackets.
File sizes are bytes, not character counts. The files array may be in any order.
Do not modify source data or the project contract. Repair and verify the script.
''',
    'inventory.ps1': '''param([Parameter(Mandatory=$true)][string]$Root)
$items = @(Get-ChildItem -Path $Root -File | Where-Object { $_.Extension -eq '.txt' } | ForEach-Object {
    @{path=$_.Name; bytes=(Get-Content -Path $_.FullName).Length}
})
@{total_bytes=($items | Measure-Object -Property bytes -Sum).Sum; files=$items} | ConvertTo-Json -Depth 5 -Compress
''',
}
POWERSHELL_REFERENCE = '''param([Parameter(Mandatory=$true)][string]$Root)
$base = (Get-Item -LiteralPath $Root).FullName.TrimEnd([char]'\\',[char]'/')
$items = @(Get-ChildItem -LiteralPath $base -File -Recurse | Where-Object { $_.Extension -ieq '.txt' } | ForEach-Object {
    @{path=$_.FullName.Substring($base.Length+1).Replace('\\','/'); bytes=[long]$_.Length}
})
[long]$total = 0
foreach ($item in $items) { $total += $item.bytes }
@{total_bytes=$total; files=$items} | ConvertTo-Json -Depth 5 -Compress
'''
TASKS = {
    'python-multifile': {
        'files': PYTHON_FILES,
        'objective': '这个小项目的账目汇总不符合项目说明。请自行检查项目、修复实现并实际运行验证。'
                     '保留接口，不修改说明或输入数据；完成后说明改动和验证结果。',
    },
    'powershell-unicode-path': {
        'files': POWERSHELL_FILES,
        'objective': '这个小项目的文本文件清单在中文、空格和特殊字符目录下统计错误。'
                     '请自行检查项目，按项目说明修复并实际运行验证，保留接口且不修改说明或输入数据。',
    },
}


def require(condition, message):
    if not condition:
        raise AssertionError(message)


def digest(data):
    return hashlib.sha256(data).hexdigest()


def write_json(path, value, secret=None):
    text = json.dumps(value, ensure_ascii=False, indent=2) + '\n'
    if secret:
        text = text.replace(secret, '[REDACTED]')
    path.write_text(text, encoding='utf-8', newline='\n')


def scan_actual_token(root, token):
    """Scan raw artifacts without rewriting evidence or printing secret bytes.

    Exact UTF-8 and UTF-16 encodings of the explicitly supplied token are covered.
    This is not a detector for other credentials or arbitrary secret transforms.
    Unreadable files and redirected paths prevent publication just like a leak.
    """
    if not token:
        return {'status': 'NOT_APPLICABLE', 'files_scanned': 0, 'leaks': [], 'errors': []}
    if not root.is_dir() or root.is_symlink() or (getattr(root.lstat(), 'st_file_attributes', 0) & 0x400):
        return {'status': 'REJECTED', 'files_scanned': 0, 'leaks': [],
                'errors': [{'path': '.', 'error_type': 'MissingOrRedirectedEvidenceRoot'}]}
    patterns = [token.encode(encoding) for encoding in ('utf-8', 'utf-16-le', 'utf-16-be')]
    overlap = max(map(len, patterns)) - 1
    leaks, errors, count = [], [], 0
    for path in sorted(root.rglob('*')):
        relative = path.relative_to(root).as_posix()
        safe_name = relative.replace(token, '[REDACTED]')
        try:
            require(not path.is_symlink() and not (getattr(path.lstat(), 'st_file_attributes', 0) & 0x400),
                    'redirected artifact path')
            if not path.is_file():
                continue
            found = token in relative
            pin, tail = hashlib.sha256(), b''
            with path.open('rb') as stream:
                while True:
                    block = stream.read(65536)
                    if not block:
                        break
                    pin.update(block)
                    data = tail + block
                    found = found or any(pattern in data for pattern in patterns)
                    tail = data[-overlap:] if overlap else b''
            count += 1
            if found:
                leaks.append({'path': safe_name, 'sha256': pin.hexdigest()})
        except Exception as error:
            errors.append({'path': safe_name, 'error_type': type(error).__name__})
    return {'status': 'CLEAN' if not leaks and not errors else 'REJECTED', 'files_scanned': count,
            'leaks': leaks, 'errors': errors, 'coverage': 'exact supplied token in UTF-8/UTF-16LE/UTF-16BE; no other secret claim'}


def execution_feedback(events):
    """Require matching successful process receipts, not arbitrary tool effects.

    This establishes observed process feedback. Task correctness is independently
    checked by the oracle; a nonempty process output alone does not prove that a
    model's test design was adequate or that it interpreted every error correctly.
    """
    starts = {event.get('step_id'): event for event in events
              if event.get('type') == 'tool_start' and event.get('title') in ('exec', 'run_script')
              and isinstance(event.get('step_id'), str) and event['step_id']}
    receipts = []
    for event in events:
        start = starts.get(event.get('step_id'))
        if event.get('type') != 'tool_end' or event.get('status') != 'completed' or not start \
                or event.get('title') != start.get('title'):
            continue
        output = event.get('output')
        if not isinstance(output, str):
            continue
        substantive = output
        if event['title'] == 'run_script':
            substantive = '\n'.join(line for line in output.splitlines()
                                     if line.strip() not in ('stdout:', 'stderr:', 'stdout (truncated):', 'stderr (truncated):'))
        if not substantive.strip():
            continue
        receipts.append({'step_id': event['step_id'], 'tool': event['title'],
                         'output_sha256': digest(output.encode()), 'output_bytes': len(output.encode()),
                         'request_sha256': digest(json.dumps(start.get('detail'), sort_keys=True).encode())})
    return {'verified': bool(receipts), 'receipts': receipts,
            'criteria': 'matched completed exec/run_script and nonempty process output; oracle independently establishes correctness',
            'model_test_design_quality_proven': False}


def inventory(root):
    require(root.is_dir() and not root.is_symlink()
            and not (getattr(root.lstat(), 'st_file_attributes', 0) & 0x400), 'project root is missing or redirected')
    result = {}
    for path in sorted(root.rglob('*')):
        require(not path.is_symlink() and not (getattr(path.lstat(), 'st_file_attributes', 0) & 0x400),
                'project contains a symlink/reparse point: ' + str(path.relative_to(root)))
        if path.is_file():
            result[path.relative_to(root).as_posix()] = {'sha256': digest(path.read_bytes()), 'bytes': path.stat().st_size}
    return result


def write_project(work, files):
    work.mkdir(parents=True)
    for name, text in files.items():
        path = work / name
        path.parent.mkdir(parents=True, exist_ok=True)
        path.write_text(text, encoding='utf-8', newline='\n')


def protected_unchanged(work, files):
    """Only source files are editable; the original README is the contract."""
    inventory(work)  # Refuse links before following paths or executing code.
    for name, original in files.items():
        if name.endswith('.md'):
            require((work / name).read_bytes() == original.encode(), 'protected project contract was changed')


def command_result(command, cwd, timeout=12):
    started = time.monotonic()
    try:
        process = subprocess.run(command, cwd=cwd, capture_output=True, timeout=timeout)
        return {'exit_code': process.returncode, 'stdout': process.stdout.decode('utf-8-sig', errors='replace'),
                'stderr': process.stderr.decode('utf-8-sig', errors='replace'),
                'elapsed_seconds': round(time.monotonic() - started, 3)}
    except subprocess.TimeoutExpired as error:
        return {'exit_code': None, 'timeout': True,
                'stdout': (error.stdout or b'').decode('utf-8', errors='replace'),
                'stderr': (error.stderr or b'').decode('utf-8', errors='replace'),
                'elapsed_seconds': round(time.monotonic() - started, 3)}


def python_oracle(work):
    # Samples live in verifier memory, outside the model's project and prompts.
    # The two source modules execute in a new isolated interpreter, never inside
    # this verifier. This isolation is not a security sandbox for hostile code.
    samples = [
        ([{'amount': '1.005', 'status': 'posted'}, {'amount': '-0.005', 'status': 'POSTED'},
          {'amount': '90.0', 'status': 'void'}], 100),
        ([{'amount': '2.675', 'status': 'Posted'}, {'amount': '0.015', 'status': 'posted'},
          {'amount': '-1.225', 'status': 'posted'}], 147),
        ([{'amount': '99999999999999.99', 'status': 'posted'},
          {'amount': '-0.01', 'status': 'posted'}], 9999999999999998),
        ([], 0),
        ([{'amount': '-3.335', 'status': 'pOsTeD'}, {'amount': '0', 'status': 'posted'},
          {'amount': '1.004', 'status': 'pending'}], -334),
    ]
    script = ("import sys,json;sys.path.insert(0,sys.argv[1]);"
              "from ledger.report import summarize;"
              "print(json.dumps([summarize(x) for x in json.load(sys.stdin)]))")
    started = time.monotonic()
    try:
        process = subprocess.run([sys.executable, '-I', '-B', '-c', script, str(work)],
            cwd=work, input=json.dumps([s[0] for s in samples]).encode(), capture_output=True, timeout=10)
        stdout = process.stdout.decode('utf-8-sig', errors='replace')
        stderr = process.stderr.decode('utf-8-sig', errors='replace')
        actual = json.loads(stdout) if process.returncode == 0 else None
        expected = [s[1] for s in samples]
        passed = process.returncode == 0 and isinstance(actual, list) and len(actual) == len(expected) \
            and all(type(v) is int and v == want for v, want in zip(actual, expected))
        return {'status': 'PASS' if passed else 'FAIL', 'exit_code': process.returncode,
                'sample_count': len(samples), 'expected': expected, 'actual': actual,
                'stdout': stdout, 'stderr': stderr, 'elapsed_seconds': round(time.monotonic()-started, 3)}
    except Exception as error:
        return {'status': 'FAIL', 'error': type(error).__name__ + ': ' + str(error),
                'elapsed_seconds': round(time.monotonic()-started, 3)}


def powershell_oracle(work, executable):
    if os.name != 'nt':
        return {'status': 'NOT_RUN', 'reason': 'Windows-specific path behavior requires Windows'}
    if not executable:
        return {'status': 'NOT_RUN', 'reason': 'PowerShell executable unavailable'}
    results = []
    with tempfile.TemporaryDirectory(prefix='ara-hidden-oracle-') as temporary:
        for index, files in enumerate([
            {'中文 报告.txt': '你好\n'.encode(), '子 目录/round [1].TXT': b'12345',
             '子 目录/empty.txt': b'', 'ignore.json': b'{"value":1}'},
            {},
            {'[literal] 甲/乙.txt': '科技 AI\r\n'.encode(), 'nested/data.txt': bytes(range(1, 9)),
             'nested/not.txt.bak': b'ignore'},
        ]):
            root = Path(temporary) / ('输入 [验收] ' + str(index))
            root.mkdir()
            for relative, data in files.items():
                path = root / relative
                path.parent.mkdir(parents=True, exist_ok=True)
                path.write_bytes(data)
            before = inventory(root)
            # PS 5.1 defaults to an OEM pipe encoding. Set only output encoding
            # in this verifier wrapper so correct Unicode JSON is not rejected.
            quote = lambda value: "'" + str(value).replace("'", "''") + "'"
            invocation = ('[Console]::OutputEncoding = [System.Text.UTF8Encoding]::new($false); & '
                          + quote(work / 'inventory.ps1') + ' -Root ' + quote(root))
            result = command_result([executable, '-NoLogo', '-NoProfile', '-NonInteractive',
                '-ExecutionPolicy', 'Bypass', '-Command', invocation], work)
            expected = {name: len(data) for name, data in files.items() if name.lower().endswith('.txt')}
            try:
                actual = json.loads(result['stdout'])
                require(isinstance(actual, dict) and type(actual.get('total_bytes')) is int,
                        'output must contain integer total_bytes')
                require(isinstance(actual.get('files'), list), 'files must be an array even for zero or one entry')
                rows = actual['files']
                require(all(isinstance(row, dict) and isinstance(row.get('path'), str)
                            and type(row.get('bytes')) is int for row in rows), 'invalid file entry')
                values = {row['path']: row['bytes'] for row in rows}
                passed = result['exit_code'] == 0 and values == expected and len(rows) == len(values) \
                    and actual['total_bytes'] == sum(expected.values()) and before == inventory(root)
                result.update(status='PASS' if passed else 'FAIL', expected=expected, actual=actual)
            except Exception as error:
                result.update(status='FAIL', error=type(error).__name__ + ': ' + str(error))
            results.append(result)
    return {'status': 'PASS' if all(row['status'] == 'PASS' for row in results) else 'FAIL',
            'sample_count': len(results), 'samples': results}


def oracle(name, work, ps):
    protected_unchanged(work, TASKS[name]['files'])
    try:
        return python_oracle(work) if name == 'python-multifile' else powershell_oracle(work, ps)
    finally:
        # Candidate imports/scripts run native code. A precheck alone cannot
        # prove they preserved the contract while producing correct outputs.
        protected_unchanged(work, TASKS[name]['files'])


class NoRedirect(urllib.request.HTTPRedirectHandler):
    def redirect_request(self, *args, **kwargs):
        return None


class Relay:
    """Exact provider body forwarding, with local evidence and no credential logs.

    The relay endpoint is loopback; the original HTTPS route and selected model
    remain explicit. The relay neither inserts tool calls nor supplies answers.
    Its process boundary slightly changes network timing; record that limitation.
    """
    def __init__(self, endpoint, token, model, output, max_calls=8):
        self.calls, self.catalogs, self.errors = [], [], []
        self.endpoint, self.token, self.model, self.output = endpoint.rstrip('/'), token, model, output
        self.lock = threading.Lock()
        self.max_calls = max_calls
        owner = self
        handlers = [NoRedirect()]
        if urllib.parse.urlsplit(endpoint).hostname in ('127.0.0.1', '::1'):
            handlers.append(urllib.request.ProxyHandler({}))
        opener = urllib.request.build_opener(*handlers)

        class Handler(http.server.BaseHTTPRequestHandler):
            def log_message(self, *args):
                pass

            def do_GET(self):
                self.forward(False)

            def do_POST(self):
                self.forward(True)

            def forward(self, post):
                received, chunks = time.monotonic(), []
                entry = {'method': self.command, 'path': self.path, 'status': 'started', 'usage': None}
                body, number = None, None
                try:
                    require(self.path == ('/v1/chat/completions' if post else '/v1/models'), 'unexpected relay path')
                    require(self.headers.get('Authorization') == 'Bearer ' + owner.token, 'relay credential mismatch')
                    if post:
                        size = int(self.headers.get('Content-Length', '0'))
                        require(0 < size <= 4 * 1024 * 1024, 'invalid provider request length')
                        body = self.rfile.read(size)
                        packet = json.loads(body)
                        require(packet.get('model') == owner.model, 'provider request changed selected model')
                        with owner.lock:
                            number = len(owner.calls) + 1
                            owner.calls.append(entry)
                        require(number <= owner.max_calls, 'provider attempted more than the configured call budget')
                        entry.update(number=number, model=packet['model'])
                        (output / ('call-%02d.request.native.json' % number)).write_bytes(body)
                        owner.write_safe(output / ('call-%02d.request.json' % number), body)
                    else:
                        with owner.lock:
                            owner.catalogs.append(entry)
                    target = owner.endpoint + ('/chat/completions' if post else '/models')
                    request = urllib.request.Request(target, data=body, method=self.command,
                        headers={'Authorization': 'Bearer ' + owner.token, 'Content-Type': 'application/json',
                                 'Accept': 'text/event-stream' if post else 'application/json'})
                    try:
                        response = opener.open(request, timeout=45)
                    except urllib.error.HTTPError as error:
                        response = error
                    with response:
                        self.send_response(response.status)
                        self.send_header('Content-Type', response.headers.get('Content-Type', 'application/json'))
                        self.send_header('Connection', 'close')
                        self.end_headers()
                        entry['http_status'] = response.status
                        total = 0
                        while True:
                            require(time.monotonic() - received < 50, 'relay response wall deadline exceeded')
                            chunk = response.readline(1024 * 1024)
                            if not chunk:
                                break
                            chunks.append(chunk)
                            if len(chunks) == 1:
                                entry['first_byte_seconds'] = round(time.monotonic() - received, 3)
                            total += len(chunk)
                            require(total <= 16 * 1024 * 1024, 'provider response exceeds evidence bound')
                            self.wfile.write(chunk)
                            self.wfile.flush()
                        entry['status'] = 'received'
                except (BrokenPipeError, ConnectionResetError, ConnectionAbortedError) as error:
                    entry.update(status='client_disconnected', error=type(error).__name__)
                except Exception as error:
                    entry.update(status='failed', error=owner.safe(type(error).__name__ + ': ' + str(error)))
                    owner.errors.append(entry['error'])
                    with contextlib.suppress(Exception):
                        self.send_error(502, 'local evidence relay failed; inspect local redacted receipt')
                finally:
                    raw = b''.join(chunks)
                    if post and number:
                        (output / ('call-%02d.response.native.bin' % number)).write_bytes(raw)
                        path = output / ('call-%02d.response.txt' % number)
                        owner.write_safe(path, raw)
                        entry['response_sha256'] = digest(path.read_bytes())
                        # Preserve absence/zero as reported; never invent usage.
                        observations = []
                        response_models = set()
                        for line in raw.splitlines():
                            if line.startswith(b'data: '):
                                with contextlib.suppress(ValueError, TypeError):
                                    value = json.loads(line[6:])
                                    if isinstance(value, dict) and isinstance(value.get('model'), str):
                                        response_models.add(value['model'])
                                    if isinstance(value, dict) and isinstance(value.get('usage'), dict):
                                        observations.append(value['usage'])
                        entry['usage'] = observations[-1] if observations else None
                        entry['usage_observations'] = observations
                        entry['response_models'] = sorted(response_models)
                    elif not post:
                        (output / 'catalog.response.native.bin').write_bytes(raw)
                        owner.write_safe(output / 'catalog.response.json', raw)
                        entry['response_sha256'] = digest(owner.safe(raw.decode('utf-8', errors='replace')).encode())
                    entry['elapsed_seconds'] = round(time.monotonic() - received, 3)

        self.server = http.server.ThreadingHTTPServer(('127.0.0.1', 0), Handler)
        # server_close waits for request threads, so reports never snapshot a
        # partially written call. Every upstream socket has a bounded timeout.
        self.server.daemon_threads = False
        self.thread = threading.Thread(target=self.server.serve_forever, daemon=True)
        self.thread.start()
        self.local_endpoint = 'http://127.0.0.1:%d/v1' % self.server.server_port

    def safe(self, value):
        return value.replace(self.token, '[REDACTED]')

    def write_safe(self, path, data):
        path.write_bytes(self.safe(data.decode('utf-8', errors='replace')).encode())

    def close(self):
        self.server.shutdown()
        self.server.server_close()
        self.thread.join(timeout=2)


def environment_probe():
    tools = {name: shutil.which(name) for name in
             ('python', 'pwsh', 'powershell', 'llvm-objdump', 'llvm-nm', 'clang', 'ghidra', 'analyzeHeadless')}
    return {'platform': sys.platform, 'python': sys.version, 'tools': tools,
            'reverse': {'status': 'NOT_RUN', 'reason': 'backend discovery only; no compiled stripped sample task implemented',
                        'llvm_backend_on_path': bool(tools['llvm-objdump'] and tools['llvm-nm'])}}


def live_run(name, case, binary, endpoint, model, token_env, token):
    work, folder = case['work'], case['folder']
    relay = Relay(endpoint, token, model, folder)
    request = {'id': 'coding-' + name, 'objective': TASKS[name]['objective'], 'cwd': str(work),
        'session_path': str(folder / 'native-session.json'), 'environment_state': str(folder / 'environment-state'),
        'endpoint': relay.local_endpoint, 'model': model, 'token_env': token_env, 'mode': 'execute',
        'enable_local_tools': True, 'enable_worker': True}
    write_json(folder / 'request.json', request)
    started = time.monotonic()
    try:
        process = subprocess.run([str(binary), str(folder / 'request.json')], cwd=work,
                                 capture_output=True, timeout=65)
        code, stdout, stderr, timeout = process.returncode, process.stdout, process.stderr, False
    except subprocess.TimeoutExpired as error:
        code, stdout, stderr, timeout = None, error.stdout or b'', error.stderr or b'', True
    finally:
        relay.close()
    (folder / 'host.stdout.native.jsonl').write_bytes(stdout)
    (folder / 'host.stderr.native.txt').write_bytes(stderr)
    relay.write_safe(folder / 'host.stdout.jsonl', stdout)
    relay.write_safe(folder / 'host.stderr.txt', stderr)
    packets, parse_errors = [], []
    for index, line in enumerate(relay.safe(stdout.decode('utf-8', errors='replace')).splitlines()):
        try:
            packet = json.loads(line)
            require(isinstance(packet, dict), 'native stdout JSON must be an object')
            packets.append(packet)
        except (ValueError, AssertionError):
            parse_errors.append(index + 1)
    events = [packet['event'] for packet in packets if packet.get('kind') == 'event']
    final = next((packet for packet in reversed(packets) if packet.get('kind') == 'result'), None)
    native_calls = [event for event in events if event.get('type') == 'model_call']
    native_failures = [event for event in events if event.get('status') in ('failed', 'error')]
    feedback = execution_feedback(events)
    write_json(folder / 'provider-receipts.json', {'calls': relay.calls, 'catalogs': relay.catalogs, 'errors': relay.errors})
    budget = any('budget' in json.dumps(event).lower() and
                 any(word in json.dumps(event).lower() for word in ('exhaust', 'limit_reached')) for event in events)
    if final:
        budget = budget or any(word in json.dumps(final).lower() for word in ('budget', 'limit_reached', 'deadline'))
    return {'exit_code': code, 'timeout': timeout, 'elapsed_seconds': round(time.monotonic()-started, 3),
        'native_result': final, 'native_model_call_count': len(native_calls), 'http_model_call_count': len(relay.calls),
        'native_tool_receipt_count': sum(event.get('type') == 'tool_end' for event in events),
        'execution_feedback_verified': feedback['verified'], 'execution_feedback': feedback,
        'native_failed_events': native_failures, 'budget_exhausted_observed': budget,
        'model_capabilities_events': [event for event in events if event.get('type') == 'model_capabilities'],
        'usage_by_call': [entry['usage'] for entry in relay.calls], 'parse_error_lines': parse_errors,
        'relay_errors': relay.errors,
        'effects_unknown': timeout or final is None,
        'native_execution_success': code == 0 and final is not None and final.get('code') == 0 and not timeout
                                    and 0 < len(relay.calls) <= 8 and not parse_errors and not relay.errors}


def relay_self_test():
    """Controlled network only: prove forwarding, receipts and redaction, not AI."""
    secret = 'coding-relay-fixture-secret'
    observed = []

    class Fixture(http.server.BaseHTTPRequestHandler):
        def log_message(self, *args):
            pass

        def do_GET(self):
            observed.append({'method': 'GET', 'path': self.path,
                             'authorized': self.headers.get('Authorization') == 'Bearer ' + secret})
            body = json.dumps({'data': [{'id': 'fixture-model'}]}).encode()
            self.send_response(200)
            self.end_headers()
            self.wfile.write(body)

        def do_POST(self):
            body = self.rfile.read(int(self.headers['Content-Length']))
            observed.append({'method': 'POST', 'path': self.path, 'body': body,
                             'authorized': self.headers.get('Authorization') == 'Bearer ' + secret})
            response = {'model': 'fixture-model', 'choices': [], 'text': secret}
            if len([row for row in observed if row['method'] == 'POST']) == 1:
                response['usage'] = {'prompt_tokens': 5, 'completion_tokens': 2, 'total_tokens': 7}
            self.send_response(200)
            self.send_header('Content-Type', 'text/event-stream')
            self.end_headers()
            self.wfile.write(('data: ' + json.dumps(response) + '\n\ndata: [DONE]\n\n').encode())

    server = http.server.ThreadingHTTPServer(('127.0.0.1', 0), Fixture)
    server.daemon_threads = True
    thread = threading.Thread(target=server.serve_forever, daemon=True)
    thread.start()
    try:
        with tempfile.TemporaryDirectory(prefix='ara-coding-relay-selftest-') as temporary:
            folder = Path(temporary)
            relay = Relay('http://127.0.0.1:%d/v1' % server.server_port, secret, 'fixture-model', folder)
            try:
                for method in ('GET', 'POST', 'POST'):
                    body = json.dumps({'model': 'fixture-model', 'messages': []}).encode() if method == 'POST' else None
                    request = urllib.request.Request(relay.local_endpoint + ('/models' if method == 'GET' else '/chat/completions'),
                        method=method, data=body, headers={'Authorization': 'Bearer ' + secret})
                    with urllib.request.urlopen(request, timeout=5) as response:
                        response.read()
            finally:
                relay.close()
            require(len(relay.calls) == 2 and len(relay.catalogs) == 1 and not relay.errors, 'relay receipt mismatch')
            require(all(row['authorized'] for row in observed), 'relay did not forward explicit Authorization')
            require(all(row['path'] in ('/v1/models', '/v1/chat/completions') for row in observed), 'relay path changed')
            require(relay.calls[0]['usage']['total_tokens'] == 7 and relay.calls[1]['usage'] is None,
                    'unknown usage became a fabricated count')
            require(all(secret.encode() not in path.read_bytes() for path in folder.iterdir()
                        if '.native.' not in path.name), 'relay redacted derivative leaked token')
            scan = scan_actual_token(folder, secret)
            require(scan['status'] == 'REJECTED' and scan['leaks'], 'raw provider token leak escaped publication gate')
            require(all(json.loads(row['body']) == {'model': 'fixture-model', 'messages': []}
                        for row in observed if row['method'] == 'POST'), 'relay changed provider request body')
    finally:
        server.shutdown()
        server.server_close()
        thread.join(timeout=2)
    return {'name': 'controlled-relay', 'status': 'PASS', 'exact_body_forwarded': True,
            'authorization_header_not_logged': True, 'raw_echo_leak_rejected_without_rewriting': True,
            'unknown_usage_is_null': True,
            'response_model_and_call_receipts_recorded': True}


def evidence_gate_self_test():
    secret = 'explicit-fixture-token-for-private-scan'
    with tempfile.TemporaryDirectory(prefix='ara-coding-evidence-gate-') as temporary:
        root = Path(temporary)
        path = root / 'environment-state/jobs/one/session-journal.bin'
        path.parent.mkdir(parents=True)
        # Deliberately straddle the scanner read boundary in a nested raw file.
        original = b'x' * (65536 - 5) + secret.encode() + b'raw receipt tail'
        path.write_bytes(original)
        scan = scan_actual_token(root, secret)
        require(scan['status'] == 'REJECTED' and len(scan['leaks']) == 1, 'deep raw token leak not rejected')
        require(path.read_bytes() == original, 'secret scanner rewrote native evidence')
        path.write_bytes(secret.encode('utf-16-le'))
        require(scan_actual_token(root, secret)['status'] == 'REJECTED', 'UTF-16 raw token leak not rejected')
        path.write_bytes(b'actual receipt with no supplied token')
        require(scan_actual_token(root, secret)['status'] == 'CLEAN', 'clean receipts rejected')
    def start(tool='exec', step='one'):
        return {'type': 'tool_start', 'title': tool, 'step_id': step, 'detail': {'cmd': 'python verify.py'}}
    def end(tool='exec', step='one', status='completed', output='checks passed'):
        return {'type': 'tool_end', 'title': tool, 'step_id': step, 'status': status, 'output': output}
    for events in ([start('write_file'), end('write_file')],
                   [start(), end(status='failed')], [start(), end(output='  ')],
                   [start(), end(step='different')],
                   [start('run_script'), end('run_script', output='stdout:\n\nstderr:\n')]):
        require(not execution_feedback(events)['verified'], 'non-execution/failed/empty/unmatched feedback accepted')
    require(execution_feedback([start(), end()])['verified'], 'successful exec feedback rejected')
    require(execution_feedback([start('run_script'), end('run_script', output='stdout:\n3 checks passed\nstderr:\n')])['verified'],
            'successful script feedback rejected')
    return {'name': 'evidence-publication-and-execution-gates', 'status': 'PASS',
            'deep_and_boundary_token_leak_rejected': True, 'native_receipt_bytes_preserved': True,
            'utf16_token_leak_rejected': True, 'clean_scan_accepted': True,
            'write_only_failed_empty_unmatched_feedback_rejected': True, 'successful_process_feedback_accepted': True}


def self_test(ps):
    results = []
    with tempfile.TemporaryDirectory(prefix='ara-coding-oracle-selftest-') as temporary:
        for name, task in TASKS.items():
            work = Path(temporary) / name
            write_project(work, task['files'])
            before = oracle(name, work, ps)
            if before['status'] == 'NOT_RUN':
                results.append({'name': name, 'status': 'NOT_RUN', 'reason': before['reason']})
                continue
            require(before['status'] == 'FAIL', name + ': buggy baseline unexpectedly passed')
            replacements = PYTHON_REFERENCE if name == 'python-multifile' else {'inventory.ps1': POWERSHELL_REFERENCE}
            for relative, source in replacements.items():
                (work / relative).write_text(source, encoding='utf-8', newline='\n')
            after = oracle(name, work, ps)
            require(after['status'] == 'PASS', name + ': independent oracle rejects reference: ' + repr(after))
            (work / 'README.md').write_text('claims everything passed', encoding='utf-8', newline='\n')
            rejected = False
            try:
                oracle(name, work, ps)
            except AssertionError:
                rejected = True
            require(rejected, 'contract pollution accepted')
            (work / 'README.md').write_text(task['files']['README.md'], encoding='utf-8', newline='\n')
            if name == 'python-multifile':
                mutated = ('from pathlib import Path\n'
                           'Path(__file__).resolve().parents[1].joinpath("README.md").write_text('
                           '"altered contract", encoding="utf-8")\n' + PYTHON_REFERENCE['ledger/report.py'])
                (work / 'ledger/report.py').write_text(mutated, encoding='utf-8', newline='\n')
            else:
                mutated = POWERSHELL_REFERENCE.replace(
                    '$base = ',
                    'Set-Content -LiteralPath (Join-Path $PSScriptRoot \'README.md\') '
                    '-Value \'altered contract\' -Encoding utf8 -NoNewline\n$base = ', 1)
                (work / 'inventory.ps1').write_text(mutated, encoding='utf-8', newline='\n')
            rejected = False
            try:
                oracle(name, work, ps)
            except AssertionError:
                rejected = True
            require(rejected, 'candidate mutated project contract during oracle execution and was accepted')
            require((work / 'README.md').read_text(encoding='utf-8-sig') == 'altered contract',
                    'mutation counterexample did not execute its top-level effect')
            (work / 'README.md').write_text(task['files']['README.md'], encoding='utf-8', newline='\n')
            if name == 'python-multifile':
                (work / 'ledger/report.py').write_text('print("PASS")\ndef summarize(rows): return 0\n',
                                                       encoding='utf-8', newline='\n')
            else:
                (work / 'inventory.ps1').write_text('Write-Output \'{"status":"PASS"}\'\n',
                                                   encoding='utf-8', newline='\n')
            require(oracle(name, work, ps)['status'] == 'FAIL', 'claimed success without correct artifacts accepted')
            results.append({'name': name, 'status': 'PASS', 'buggy_rejected': True,
                            'reference_accepted': True, 'contract_pollution_rejected': True,
                            'contract_mutation_during_oracle_rejected': True,
                            'printed_success_without_correct_output_rejected': True})
    results.append(relay_self_test())
    results.append(evidence_gate_self_test())
    return results


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    mode = parser.add_mutually_exclusive_group()
    mode.add_argument('--prepare', action='store_true', help='default: no network/model requests')
    mode.add_argument('--live', action='store_true', help='explicitly authorize bounded real-model requests')
    parser.add_argument('--self-test', action='store_true', help='test independent oracles; never contact a provider')
    parser.add_argument('--output', type=Path)
    parser.add_argument('--binary', type=Path, help='previously built independent business_host executable')
    parser.add_argument('--endpoint-env', default='ARA_LITE_BASELINE_ENDPOINT')
    parser.add_argument('--model-env', default='ARA_LITE_BASELINE_MODEL')
    parser.add_argument('--token-env', default='ARA_LITE_BASELINE_TOKEN')
    args = parser.parse_args()
    require(not (args.self_test and args.live), '--self-test never permits --live')
    ps = shutil.which('pwsh') or shutil.which('powershell')
    if args.self_test:
        print(json.dumps({'status': 'PASS', 'scope': 'oracle self-tests only; not Agent intelligence',
                          'cases': self_test(ps)}, ensure_ascii=False, indent=2))
        return
    require(args.output is not None, '--output is required')
    endpoint = model = token = None
    binary = args.binary.resolve() if args.binary else None
    if args.live:
        require(binary is not None and binary.is_file(), '--live requires an existing --binary')
        endpoint, model, token = (os.environ.get(name, '') for name in (args.endpoint_env, args.model_env, args.token_env))
        require(all((endpoint, model, token)), 'set the explicitly named endpoint/model/token environment variables')
        url = urllib.parse.urlsplit(endpoint)
        require(not url.username and not url.password and not url.query and not url.fragment,
                'endpoint must not embed credentials, query, or fragment')
        require(url.scheme == 'https' or (url.scheme == 'http' and url.hostname in ('127.0.0.1', '::1')),
                'upstream must be HTTPS or literal loopback development endpoint')
    root = args.output.resolve()
    root.mkdir(parents=True, exist_ok=False)
    report = {'schema': 'ara-lite-coding-baseline-v1', 'status': 'NOT_RUN',
        'scope': 'two coding tasks; no reverse intelligence claim; no OS sandbox',
        'mode': 'live' if args.live else 'prepare', 'environment': environment_probe(), 'cases': [],
        'planned_tasks': list(TASKS),
        'budget_per_task': {'model_calls': 8, 'host_seconds': 45, 'max_output_tokens_per_call': 2048,
                            'outer_process_seconds': 65, 'source': 'examples/business_host.rs'},
        'route': {'endpoint': endpoint, 'model': model, 'protocol': 'openai-completions'} if args.live else None,
        'binary_sha256': digest(binary.read_bytes()) if binary else None,
        'harness_sha256': digest(Path(__file__).read_bytes()),
        'limits': ['real tools are not an OS sandbox', 'new tasks and new Sessions, not resume/production tests',
                   'relay observes provider bodies and changes network timing', 'no token/cost improvement claim',
                   'unreported usage remains null; no fabricated zero totals',
                   'all raw native/provider receipts remain private; only scan-clean publication-summary.json is a submission candidate',
                   'process feedback gate proves observed execution output, not general model test-design quality']}
    try:
        for name, task in TASKS.items():
            folder = root / name
            work = folder / 'project'
            write_project(work, task['files'])
            case = {'name': name, 'status': 'NOT_RUN'}
            report['cases'].append(case)
            before = inventory(work)
            write_json(folder / 'before-inventory.json', before)
            baseline = oracle(name, work, ps)
            case['before_oracle_status'] = baseline['status']
            if baseline['status'] == 'NOT_RUN':
                write_json(folder / 'before-oracle.json', baseline)
                case['reason'] = baseline['reason']
                continue
            require(baseline['status'] == 'FAIL', 'deliberately faulty initial implementation passed oracle')
            if not args.live:
                write_json(folder / 'before-oracle.json', baseline)
                case.update(status='PREPARED', reason='fault reproduced; model not requested')
                continue
            execution = live_run(name, {'folder': folder, 'work': work}, binary, endpoint, model, args.token_env, token)
            # Do not put hidden expected outputs on disk while the Agent runs.
            write_json(folder / 'before-oracle.json', baseline)
            case['execution'] = execution
            try:
                verified = oracle(name, work, ps)
            except Exception as error:
                verified = {'status': 'FAIL', 'error': type(error).__name__ + ': ' + str(error)}
            write_json(folder / 'after-oracle.json', verified, token)
            case['after_oracle_status'] = verified['status']
            after = inventory(work)
            write_json(folder / 'after-inventory.json', after)
            patches = []
            patch_paths = set(task['files']) | {path for path in after if path.endswith(('.py', '.ps1'))}
            for relative in sorted(patch_paths):
                original = task['files'].get(relative, '')
                path = work / relative
                updated = path.read_text(encoding='utf-8') if path.is_file() else ''
                patches.extend(difflib.unified_diff(original.splitlines(True), updated.splitlines(True),
                                                   fromfile='before/' + relative, tofile='after/' + relative))
            patch = ''.join(patches).replace(token, '[REDACTED]') if token else ''.join(patches)
            (folder / 'changes.patch').write_text(patch, encoding='utf-8', newline='\n')
            case['changed_paths'] = [path for path in sorted(set(before) | set(after)) if before.get(path) != after.get(path)]
            case['execution_feedback_verified'] = execution['execution_feedback_verified']
            case['status'] = 'PASS' if execution['native_execution_success'] and verified['status'] == 'PASS' \
                and execution['execution_feedback_verified'] and case['changed_paths'] else 'FAIL'
            case['patch_scope'] = 'all initial project files and added Python/PowerShell source; all other files hash-inventoried'
            case['artifact_hashes'] = inventory(folder)
            case['secret_scan'] = scan_actual_token(folder, token)
            case['evidence_private'] = case['secret_scan']['status'] != 'CLEAN'
            case['raw_evidence_private'] = True
            if case['evidence_private']:
                case['status'] = 'FAIL'
        statuses = [case['status'] for case in report['cases']]
        report['status'] = ('PASS' if statuses and all(status == 'PASS' for status in statuses) else
                            'FAIL' if 'FAIL' in statuses else 'PREPARED' if not args.live else 'PARTIAL')
    except Exception as error:
        report.update(status='FAIL', error=type(error).__name__ + ': ' + str(error))
        for name in TASKS:
            if not any(case['name'] == name for case in report['cases']):
                report['cases'].append({'name': name, 'status': 'NOT_RUN', 'reason': 'earlier harness failure; no replay'})
    finally:
        # Cover partial/failed Runs too. Never rewrite native sessions/journals,
        # raw stdout/provider packets, model-written files or state to hide leaks.
        report['secret_scan'] = scan_actual_token(root, token)
        report['raw_evidence_private'] = True
        report['evidence_private'] = bool(token) and report['secret_scan']['status'] != 'CLEAN'
        if report['evidence_private']:
            report['status'] = 'FAIL'
        report['public_summary_eligible'] = args.live and not report['evidence_private']
        if report['public_summary_eligible']:
            summary = {'schema': report['schema'], 'status': report['status'], 'route': report['route'],
                'binary_sha256': report['binary_sha256'], 'harness_sha256': report['harness_sha256'],
                'budget_per_task': report['budget_per_task'], 'raw_evidence_private': True,
                'secret_scan': {'status': report['secret_scan']['status'], 'files_scanned': report['secret_scan']['files_scanned']},
                'cases': [{key: case[key] for key in ('name', 'status', 'before_oracle_status', 'after_oracle_status',
                          'execution_feedback_verified') if key in case} for case in report['cases']]}
            write_json(root / 'publication-summary.json', summary, token)
        # Only explicit token bytes are redacted; no private configuration is read.
        serialized = json.dumps(report, ensure_ascii=False, indent=2) + '\n'
        if token:
            serialized = serialized.replace(token, '[REDACTED]')
        (root / 'result.json').write_text(serialized, encoding='utf-8', newline='\n')
        print(serialized)
    require(report['status'] in ('PASS', 'PREPARED'), 'acceptance incomplete/failed; inspect retained local evidence')


if __name__ == '__main__':
    main()
