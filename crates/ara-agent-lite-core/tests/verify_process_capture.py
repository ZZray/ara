"""Native business_host process-output capture acceptance, controlled provider.

Tool choices are scripted; process effects, stream bytes, hashes and tool-end
identities are independently checked. This is not a real-model capability trial
or an OS sandbox. Raw evidence is private fixture material, with no user secrets.
The harness never builds, retries a failed operation, or reuses an evidence root.
"""
import argparse
import json
import os
from pathlib import Path
import subprocess
import time

from verify_versioned_edits import Provider, TOKEN, MODEL, digest, require, write_json


MARKER = '\n[process output capture]\n'
LIMIT = 8 * 1024 * 1024
FIXTURE_CONTEXT_WINDOW = 1_000_000
CASES = ('middle-diagnostic-repair', 'exec-nonzero', 'quota',
         'existing-directory', 'missing-parent', 'timeout')
BUG = b'def compute(value):\n    return value - 1\n'
FIXED = b'def compute(value):\n    return value + 1\n'
OUT_DIAGNOSTIC = b'ERROR_CAPTURE_MIDDLE expected=4 actual=2\n'
ERR_DIAGNOSTIC = b'TRACE_CAPTURE_MIDDLE bug.py:2 return value - 1\n'
PAD_OUT = b'out-padding-' + b'o' * 100 + b'\n'
PAD_ERR = b'err-padding-' + b'e' * 100 + b'\n'
LONG_OUT = PAD_OUT * 1500 + OUT_DIAGNOSTIC + PAD_OUT * 1500
LONG_ERR = PAD_ERR * 1500 + ERR_DIAGNOSTIC + PAD_ERR * 1500
VERIFY_OUT = b'CAPTURE_REPAIR_VERIFIED\n'
VERIFY_ERR = b'CAPTURE_VERIFY_STDERR\n'
TIMEOUT_OUT = b'before-timeout-stdout\n'
TIMEOUT_ERR = b'before-timeout-stderr\n'


def script_for_streams(stdout, stderr, tail=''):
    return ('import sys\n'
            f'sys.stdout.buffer.write({stdout!r}); sys.stdout.buffer.flush()\n'
            f'sys.stderr.buffer.write({stderr!r}); sys.stderr.buffer.flush()\n' + tail)


def receipt(text):
    require(isinstance(text, str) and MARKER in text, 'native tool result lacks capture receipt')
    payload = text.split(MARKER, 1)[1]
    value, _ = json.JSONDecoder().raw_decode(payload.lstrip())
    require(isinstance(value, dict), 'capture receipt must be a JSON object')
    require(value.get('schema') == 'ara.process-output.capture.v1', 'capture schema differs')
    require(value.get('encoding') == 'raw-bytes' and value.get('limit_bytes_per_stream') == LIMIT,
            'capture raw-byte/quota contract differs')
    require(all(isinstance(value.get(stream), dict) for stream in ('stdout', 'stderr')),
            'capture receipt lacks stdout/stderr objects')
    require(isinstance(value.get('process'), dict), 'capture process settlement missing')
    require(value['process'].get('effects_not_rolled_back') is True,
            'process receipt lost external-effect boundary')
    return value


def check_stream(value, expected_path, expected_bytes, observed=None, complete=True, quota=False):
    require(same_target(value.get('path'), expected_path), 'capture path differs')
    require(expected_path.is_file(), 'capture artifact does not exist')
    actual = expected_path.read_bytes()
    require(actual == expected_bytes, 'capture artifact bytes differ')
    require(value.get('stored_bytes') == len(actual), 'stored_bytes differs from disk')
    require(value.get('observed_bytes') == (len(expected_bytes) if observed is None else observed),
            'observed_bytes differs from actual emitted data')
    require(value.get('sha256') == digest(actual), 'capture SHA256 differs from actual file')
    require(value.get('complete') is complete and value.get('quota_exceeded') is quota,
            'stream completeness/quota state differs')
    require(value.get('eof') is True, 'settled normal stream did not report EOF')
    require(value.get('error') is None, 'unexpected capture stream error')


def same_target(actual, expected):
    """Compare existing object identity, including Windows extended paths."""
    require(isinstance(actual, str) and actual, 'capture path missing or invalid')
    path = Path(actual)
    require(path.exists() and expected.exists(), 'capture/expected path does not exist')
    return path.samefile(expected)


def process_probe(pid):
    """Read process liveness; uncertainty never counts as a terminated child."""
    require(type(pid) is int and pid > 0 and pid != os.getpid(), 'invalid child PID')
    result = {'pid': pid, 'platform': os.name, 'running': None}
    if os.name == 'nt':
        import ctypes
        from ctypes import wintypes
        kernel = ctypes.WinDLL('kernel32', use_last_error=True)
        kernel.OpenProcess.argtypes = [wintypes.DWORD, wintypes.BOOL, wintypes.DWORD]
        kernel.OpenProcess.restype = wintypes.HANDLE
        kernel.GetExitCodeProcess.argtypes = [wintypes.HANDLE, ctypes.POINTER(wintypes.DWORD)]
        kernel.GetExitCodeProcess.restype = wintypes.BOOL
        kernel.CloseHandle.argtypes = [wintypes.HANDLE]
        kernel.CloseHandle.restype = wintypes.BOOL
        handle = kernel.OpenProcess(0x1000, False, pid)  # PROCESS_QUERY_LIMITED_INFORMATION only.
        if not handle:
            error = ctypes.get_last_error()
            result.update(api='OpenProcess', winerror=error)
            if error == 87:  # ERROR_INVALID_PARAMETER: the PID no longer exists.
                result.update(running=False, state='pid-not-found')
            else:
                result.update(state='unknown', error='process query denied or failed')
            return result
        try:
            code = wintypes.DWORD()
            if not kernel.GetExitCodeProcess(handle, ctypes.byref(code)):
                result.update(api='GetExitCodeProcess', winerror=ctypes.get_last_error(), state='unknown')
            else:
                result.update(api='GetExitCodeProcess', exit_code=code.value,
                              running=code.value == 259, state='active' if code.value == 259 else 'exited')
        finally:
            if not kernel.CloseHandle(handle):
                result['close_handle_error'] = ctypes.get_last_error()
        return result
    # This branch is never used on Windows: os.kill(pid, 0) is Unix-only here.
    stat = Path('/proc') / str(pid) / 'stat'
    try:
        if stat.is_file():
            fields = stat.read_text(encoding='ascii').rsplit(')', 1)[1].split()
            state = fields[0]
            result.update(proc_state=state)
            if state in ('Z', 'X', 'x'):
                result.update(running=False, state='zombie-not-running' if state == 'Z' else 'dead')
                return result
        os.kill(pid, 0)
        result.update(api='kill-zero', running=True, state='exists')
    except ProcessLookupError:
        result.update(api='kill-zero', running=False, state='pid-not-found')
    except (OSError, ValueError, IndexError) as error:
        result.update(state='unknown', error=type(error).__name__ + ': ' + str(error))
    return result


class CaptureProvider(Provider):
    def __init__(self, folder, name, work):
        super().__init__(folder, name, work / 'bug.py')
        # The shared fixture advertises 128000, which is intentionally too small
        # for this 128 KiB output-boundary test under Core's conservative byte
        # budgeting. Override only our catalog before the native Host is started.
        # Production budgets and the shared fixture remain unchanged.
        owner = self
        base_handler = self.server.RequestHandlerClass

        class CaptureHandler(base_handler):
            def do_GET(self):
                try:
                    self.authorize()
                    require(self.path == '/v1/models', 'wrong capture catalog path')
                    owner.catalogs.append(self.path)
                    body = json.dumps({'data': [{'id': MODEL, 'protocols': ['openai-completions'],
                        'context_window': FIXTURE_CONTEXT_WINDOW, 'max_tokens': 4096,
                        'omp': {'supportsTools': True, 'input': ['text'], 'reasoning': False}}]}).encode()
                    self.send_response(200)
                    self.send_header('Content-Type', 'application/json')
                    self.send_header('Content-Length', str(len(body)))
                    self.end_headers()
                    self.wfile.write(body)
                except Exception as error:
                    owner.errors.append(type(error).__name__ + ': ' + str(error))
                    self.send_error(500)

        self.server.RequestHandlerClass = CaptureHandler
        self.work = work
        self.capture_receipts = []
        self.expected_statuses = []

    def call(self, name, args, status='completed'):
        self.expected_statuses.append(status)
        return super().call(name, args)

    def captured(self, text, directory, out, err, *, observed_out=None, quota=False, exit_code=0):
        value = receipt(text)
        require(same_target(value.get('directory'), self.work / directory),
                'capture directory differs')
        require(value.get('complete') is (not quota), 'aggregate capture completeness differs')
        require(value['process'].get('wait_completed') is True and
                value['process'].get('exit_code') == exit_code and value['process'].get('error') is None,
                'actual process exit/wait state differs')
        check_stream(value['stdout'], self.work / directory / 'stdout.log', out,
                     observed=observed_out, complete=not quota, quota=quota)
        check_stream(value['stderr'], self.work / directory / 'stderr.log', err)
        self.capture_receipts.append(value)
        return value

    def finish(self):
        self.finished = True
        return {'content': 'PROCESS_CAPTURE_CASE_VERIFIED'}, 'stop'

    def next_action(self, request):
        step = len(self.requests)
        require(step <= 8, 'case exceeded eight model calls')
        tools = [m for m in request['messages'] if m.get('role') == 'tool']
        previous = self.previous(tools)
        if step == 1:
            advertised = {t['function']['name']: t['function'] for t in request.get('tools', [])}
            for tool in ('exec', 'run_script'):
                require('capture_dir' in advertised[tool]['parameters']['properties'],
                        tool + ' capture_dir schema missing')
            if self.name == 'middle-diagnostic-repair':
                # The diagnostic lies beyond both ends of the bounded preview.
                require(self.target.read_bytes() == BUG, 'seeded source differs before native execution')
                # Keep the script small rather than embedding the long output.
                source = ('import sys, os\nsys.dont_write_bytecode = True\nsys.path.insert(0, os.getcwd())\n'
                          'from bug import compute\nassert compute(3) == 2\n'
                          f'out = {PAD_OUT!r} * 1500 + {OUT_DIAGNOSTIC!r} + {PAD_OUT!r} * 1500\n'
                          f'err = {PAD_ERR!r} * 1500 + {ERR_DIAGNOSTIC!r} + {PAD_ERR!r} * 1500\n'
                          'sys.stdout.buffer.write(out); sys.stdout.buffer.flush()\n'
                          'sys.stderr.buffer.write(err); sys.stderr.buffer.flush()\nsys.exit(7)\n')
                return self.call('run_script', {'language': 'python', 'script': source,
                    'capture_dir': 'initial-output', 'output_limit': 131072}, 'failed')
            if self.name == 'exec-nonzero':
                command = ("[Console]::Out.Write('exec-out'); [Console]::Error.Write('exec-err'); exit 23"
                           if os.name == 'nt' else "printf 'exec-out'; printf 'exec-err' >&2; exit 23")
                return self.call('exec', {'cmd': command, 'capture_dir': 'exec-output'}, 'failed')
            if self.name == 'quota':
                source = ('import sys\nfrom pathlib import Path\n'
                          f'sys.stdout.buffer.write(b"Q" * {LIMIT + 4096}); sys.stdout.buffer.flush()\n'
                          'sys.stderr.buffer.write(b"quota-stderr\\n"); sys.stderr.buffer.flush()\n'
                          'Path("quota-completed.txt").write_text("completed", encoding="utf-8")\n')
                return self.call('run_script', {'language': 'python', 'script': source,
                    'capture_dir': 'quota-output', 'output_limit': 1024})
            if self.name in ('existing-directory', 'missing-parent'):
                return self.call('run_script', {'language': 'python',
                    'script': 'from pathlib import Path\nPath("MUST_NOT_START").write_text("ran")\n',
                    'capture_dir': 'existing-output' if self.name == 'existing-directory' else 'missing/output'},
                    'failed')
            require(self.name == 'timeout', 'unexpected case')
            source = ('import os, json\nfrom pathlib import Path\n'
                      'with Path("child-pid.json").open("w", encoding="utf-8") as pid_file:\n'
                      '    json.dump({"pid":os.getpid(),"case":"timeout"}, pid_file)\n'
                      '    pid_file.flush()\n' + script_for_streams(TIMEOUT_OUT, TIMEOUT_ERR,
                'import time\nfrom pathlib import Path\ntime.sleep(15)\nPath("MUST_NOT_FINISH").write_text("ran")\n')
                      )
            return self.call('run_script', {'language': 'python', 'script': source, 'timeout': 1,
                'capture_dir': 'timeout-output', 'output_limit': 1024}, 'failed')
        if self.name == 'middle-diagnostic-repair':
            if step == 2:
                require('code 7' in previous, 'initial process failure was lost')
                preview = previous.split(MARKER, 1)[0]
                require(OUT_DIAGNOSTIC.decode().strip() not in preview and
                        ERR_DIAGNOSTIC.decode().strip() not in preview, 'diagnostic leaked into bounded preview')
                self.captured(previous, 'initial-output', LONG_OUT, LONG_ERR, exit_code=7)
                return self.call('read_file', {'path': 'initial-output/stdout.log', 'offset': 1501, 'limit': 1})
            if step == 3:
                parsed = json.loads(previous)
                require(parsed.get('lines') == [{'line': 1501, 'text': OUT_DIAGNOSTIC.decode().strip(),
                                                 'truncated': False}],
                        'native stdout page differs from exact middle diagnostic')
                require(parsed.get('coverage', {}).get('complete') is True, 'stdout requested page is partial')
                return self.call('read_file', {'path': 'initial-output/stderr.log', 'offset': 1501, 'limit': 1})
            if step == 4:
                parsed = json.loads(previous)
                require(parsed.get('lines') == [{'line': 1501, 'text': ERR_DIAGNOSTIC.decode().strip(),
                                                 'truncated': False}],
                        'native stderr page differs from exact middle trace')
                require(parsed.get('coverage', {}).get('complete') is True, 'stderr requested page is partial')
                return self.call('edit_file', {'path': 'bug.py', 'old_text': 'return value - 1',
                    'new_text': 'return value + 1'})
            if step == 5:
                require(self.target.read_bytes() == FIXED, 'native edit did not change actual source')
                source = ('import sys, os\nsys.path.insert(0, os.getcwd())\n'
                          'from bug import compute\nfrom pathlib import Path\nimport json\n'
                          'assert compute(3) == 4\nassert compute(-2) == -1\n'
                          'Path("verified.json").write_text(json.dumps({"value":compute(3)}), encoding="utf-8")\n' +
                          script_for_streams(VERIFY_OUT, VERIFY_ERR))
                return self.call('run_script', {'language': 'python', 'script': source,
                    'capture_dir': 'verified-output', 'output_limit': 1024})
            require(step == 6, 'unexpected diagnostic-repair sequence')
            self.captured(previous, 'verified-output', VERIFY_OUT, VERIFY_ERR)
            require(json.loads((self.work / 'verified.json').read_text(encoding='utf-8')) == {'value': 4},
                    'actual verification artifact differs')
            require(self.target.read_bytes() == FIXED, 'final source differs')
            return self.finish()
        require(step == 2, 'unexpected extra tool/model turn')
        if self.name == 'exec-nonzero':
            require('code 23' in previous, 'exec nonzero process exit was lost')
            self.captured(previous, 'exec-output', b'exec-out', b'exec-err', exit_code=23)
        elif self.name == 'quota':
            self.captured(previous, 'quota-output', b'Q' * LIMIT, b'quota-stderr\n',
                          observed_out=LIMIT + 4096, quota=True)
            require((self.work / 'quota-completed.txt').read_bytes() == b'completed',
                    'quota process did not finish actual work')
        elif self.name in ('existing-directory', 'missing-parent'):
            require(previous and not (self.work / 'MUST_NOT_START').exists(),
                    'invalid capture target allowed command side effect')
            require('capture' in previous.lower(), 'failure did not identify capture preflight')
            require(MARKER not in previous, 'rejected capture target claims launched-process capture')
            if self.name == 'existing-directory':
                require('exist' in previous.lower(), 'existing-directory preflight reason missing')
                require((self.work / 'existing-output' / 'sentinel').read_bytes() == b'preserve',
                        'existing capture directory was altered')
                require(sorted(p.name for p in (self.work / 'existing-output').iterdir()) == ['sentinel'],
                        'capture files were added to existing directory')
            else:
                require(any(word in previous.lower() for word in ('parent', 'not found', 'no such')),
                        'missing-parent preflight reason missing')
                require(not (self.work / 'missing').exists(), 'missing parent was created')
        else:
            require('timed out' in previous.lower(), 'native timeout result not reported')
            value = receipt(previous)
            require(same_target(value.get('directory'), self.work / 'timeout-output'),
                    'timeout capture directory differs')
            process = value['process']
            require(process.get('wait_completed') is False and process.get('exit_code') is None and
                    'timed out' in str(process.get('error', '')).lower(),
                    'timeout claimed successful/known process completion')
            # Termination may leave EOF or an explicit incomplete stream, but the
            # real already-flushed bytes and their hashes must survive settlement.
            for stream, expected in (('stdout', TIMEOUT_OUT), ('stderr', TIMEOUT_ERR)):
                item = value[stream]
                path = self.work / 'timeout-output' / (stream + '.log')
                require(path.read_bytes() == expected, 'timeout lost flushed ' + stream)
                require(item.get('stored_bytes') == len(expected) and item.get('observed_bytes') == len(expected),
                        'timeout stream byte counts differ')
                require(item.get('sha256') == digest(expected) and same_target(item.get('path'), path),
                        'timeout path/hash differs')
                require(item.get('quota_exceeded') is False, 'timeout mislabeled quota exceeded')
            require(not (self.work / 'MUST_NOT_FINISH').exists(), 'timed-out child reached later side effect')
            self.capture_receipts.append(value)
        return self.finish()


def source_pins():
    core = Path(__file__).resolve().parents[1]
    paths = sorted(path for path in (core / 'src').rglob('*') if path.is_file())
    paths += [core / 'Cargo.toml', core / 'Cargo.lock', core / 'examples' / 'business_host.rs',
              Path(__file__).resolve(), Path(__file__).with_name('verify_versioned_edits.py').resolve()]
    return {str(path.relative_to(core)): digest(path.read_bytes()) for path in paths if path.is_file()}


def run_case(binary, root, name):
    folder = root / name
    folder.mkdir()
    work = folder / 'project'
    work.mkdir()
    (work / 'bug.py').write_bytes(BUG)
    if name == 'existing-directory':
        (work / 'existing-output').mkdir()
        (work / 'existing-output' / 'sentinel').write_bytes(b'preserve')
    provider = CaptureProvider(folder, name, work)
    config = {'id': 'capture-' + name, 'objective': 'Controlled native capture acceptance: ' + name,
        'cwd': str(work), 'session_path': str(folder / 'session.json'),
        'environment_state': str(folder / 'environment-state'), 'endpoint': provider.endpoint,
        'model': MODEL, 'token_env': 'ARA_LITE_CAPTURE_FIXTURE_KEY', 'mode': 'execute', 'enable_local_tools': True}
    write_json(folder / 'request.json', config)
    environment = os.environ.copy()
    environment.update(ARA_LITE_CAPTURE_FIXTURE_KEY=TOKEN, NO_PROXY='127.0.0.1,localhost')
    case = {'name': name, 'status': 'FAIL', 'effects_unknown': False}
    started = time.monotonic()
    try:
        try:
            process = subprocess.run([str(binary), str(folder / 'request.json')], cwd=work,
                env=environment, capture_output=True, timeout=55)
            code, stdout, stderr = process.returncode, process.stdout, process.stderr
        except subprocess.TimeoutExpired as error:
            code, stdout, stderr = None, error.stdout or b'', error.stderr or b''
            case.update(timeout=True, effects_unknown=True)
        (folder / 'host.stdout.jsonl').write_bytes(stdout)
        (folder / 'host.stderr.txt').write_bytes(stderr)
        case['host_process_exit'] = code
        require(code is not None, 'outer native process timed out; retain evidence, do not replay')
        packets = [json.loads(line) for line in stdout.decode('utf-8').splitlines() if line.strip()]
        events = [packet['event'] for packet in packets if packet.get('kind') == 'event']
        starts = [event for event in events if event.get('type') == 'tool_start']
        ends = [event for event in events if event.get('type') == 'tool_end']
        write_json(folder / 'tool-end.json', ends)
        require(not provider.errors, 'provider assertions failed: ' + repr(provider.errors))
        require(provider.finished, 'provider did not finish all actual checks')
        require(code == 0 and packets[-1].get('kind') == 'result' and packets[-1].get('code') == 0,
                'native Run did not settle successfully after expected tool errors')
        calls = [e for e in events if e.get('type') == 'model_call']
        caps = [e for e in events if e.get('type') == 'model_capabilities']
        require(len(caps) == 1 and caps[0].get('declared', {}).get('context_window') == FIXTURE_CONTEXT_WINDOW,
                'native Host did not observe the capture fixture context declaration')
        require(0 < len(provider.requests) <= 8 and len(calls) == len(provider.requests),
                'model call bounds/native request identity differ')
        require(len(starts) == len(ends) == len(provider.actions), 'native tool event count differs')
        capture_index = 0
        for start, end, action, expected in zip(starts, ends, provider.actions, provider.expected_statuses):
            for event in (start, end):
                require(event.get('step_id') == action['id'] and event.get('title') == action['name'],
                        'native tool event identity differs')
            require(end.get('status') == expected, 'native tool error/completed state differs')
            require(isinstance(end.get('output'), str) and len(end['output'].encode('utf-8')) <= 128 * 1024,
                    'native tool_end output exceeds its 128 KiB UTF-8 budget')
            if action['name'] in ('exec', 'run_script') and name not in ('existing-directory', 'missing-parent'):
                # Require the event stream, not only the provider transcript, to
                # retain the receipt beyond the bounded preview.
                require(receipt(end.get('output', '')) == provider.capture_receipts[capture_index],
                        'tool_end capture receipt differs from provider-observed result')
                capture_index += 1
        require(capture_index == len(provider.capture_receipts), 'unmatched capture receipts')
        if name == 'timeout':
            require(time.monotonic() - started >= 0.7, 'timeout fixture advanced before real timeout')
            pid_bytes = (work / 'child-pid.json').read_bytes()
            pid_record = json.loads(pid_bytes.decode('utf-8'))
            require(pid_record.get('case') == 'timeout', 'timeout PID file identity differs')
            probe = process_probe(pid_record.get('pid'))
            case.update(child_pid_record=pid_record, child_pid_artifact_sha256=digest(pid_bytes),
                        child_process_probe=probe)
            write_json(folder / 'child-process-probe.json', probe)
            require(probe.get('running') is False, 'timed-out child is running or liveness is unknown')
        case.update(status='PASS', native_model_calls=len(calls), native_tool_ends=len(ends),
                    capture_receipts=provider.capture_receipts)
    except Exception as error:
        case['error'] = type(error).__name__ + ': ' + str(error)
    finally:
        provider.close()
        write_json(folder / 'provider-errors.json', provider.errors)
        write_json(folder / 'scripted-actions.json', provider.actions)
        write_json(folder / 'provider-receipts.json', provider.capture_receipts)
        case['elapsed_seconds'] = round(time.monotonic() - started, 3)
        write_json(folder / 'case.json', case)
    return case


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--binary', type=Path, required=True)
    parser.add_argument('--output', type=Path, required=True)
    args = parser.parse_args()
    binary, root = args.binary.resolve(), args.output.resolve()
    require(binary.is_file(), '--binary must already exist; no implicit build')
    root.mkdir(parents=True, exist_ok=False)
    before = source_pins()
    result = {'schema': 'ara-lite-process-capture-native-v1', 'status': 'FAIL',
        'scope': 'controlled provider; real native process/files/receipts; not real-model intelligence or OS sandbox',
        'raw_evidence': 'private controlled fixture material; no user secrets; not a publication approval',
        'binary_path': str(binary), 'binary_sha256': digest(binary.read_bytes()),
        'harness_sha256': digest(Path(__file__).read_bytes()), 'source_pins': before,
        'fixture_context_window': FIXTURE_CONTEXT_WINDOW,
        'budget_per_case': {'model_calls': 8, 'host_seconds': 45, 'outer_seconds': 55}, 'cases': []}
    started = time.monotonic()
    try:
        for name in CASES:
            result['cases'].append(run_case(binary, root, name))
        result['source_pins_unchanged'] = before == source_pins()
        result['binary_unchanged'] = result['binary_sha256'] == digest(binary.read_bytes())
        result['status'] = ('PASS' if result['source_pins_unchanged'] and result['binary_unchanged']
                            and all(case['status'] == 'PASS' for case in result['cases']) else 'FAIL')
    finally:
        result['elapsed_seconds'] = round(time.monotonic() - started, 3)
        write_json(root / 'result.json', result)
        print(json.dumps(result, ensure_ascii=False, indent=2))
    require(result['status'] == 'PASS', 'native capture acceptance failed; inspect evidence without replay')


if __name__ == '__main__':
    main()
