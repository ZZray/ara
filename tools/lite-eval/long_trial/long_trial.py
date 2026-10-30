"""Combined long-task reliability trial driver (requirement 35.1, 35.2; task 60.1).

`lite-eval long-trial` starts this driver with `--input <trial-input.json>` and
`--trial-dir <new directory>`. The input carries the Run_Policy items of
`budgets.LONG-TRIAL` (read by the Rust command from the fixed policy path) and
only the NAME of the credential variable; the key is read here from the
environment once and is passed to the sidecar by the Python_Client, never
written to disk.

One Task, one Session:

1. Process 1 is a JSONL_Sidecar started through the Python_Client with
   `length_stop_review=review_closed_length` and `emit_events=true`. This
   driver is the Host: confined `ws_*` file tools, `run_public_checks`, and the
   review callbacks (`verify`, `prepare`, `verify_stop_snapshot`,
   `prepare_stop_continuation`) that run the same public check.
2. At the start of every side-effecting tool (`ws_write_file`, `ws_edit_file`)
   the Host counts, from the sidecar's evidence files, completed `compact`
   events, sent `prepare_reply` frames (Host review continuations) and sent
   `prepare_stop_reply` frames (length-truncation continuations). Once each is
   at least 1, that tool writes its `started` receipt, applies the effect,
   writes its committed `applied` receipt, then writes `kill-marker.json` and
   stays open, so the ToolResult is never written.
3. The driver polls the marker and terminates the sidecar process at OS level
   (`Process.kill()`: TerminateProcess on Windows, no cleanup in the child).
4. Process 2 is a new sidecar started with `resume` over the same Task and
   Session. `reconcile_resume` settles every open call only from the Host's
   own operation receipts.

Everything is recorded in `trial-log.jsonl` with one driver sequence number
per line (callbacks, best-effort `event` frames, Host receipts, marker, kill,
reconcile reply), so `lite-eval long-trial` can order receipts against the
first upstream request after resume. Nothing here judges the trial; the Rust
checker does (task 60.2).
"""
from __future__ import annotations

import argparse
import asyncio
import copy
import hashlib
import json
import os
from pathlib import Path
import shutil
import subprocess
import sys
import threading
import time
import types

REPO = Path(__file__).resolve().parents[3]
sys.path.insert(0, str(REPO / 'crates' / 'ara-agent-lite-jsonl' / 'clients' / 'python'))
import ara_agent_lite as client  # noqa: E402  (path set above)

TRIAL_VERSION = 1
FEEDBACK_LIMIT = 8 * 1024
READ_LIMIT = 64 * 1024
PUBLIC_CHECK_TIMEOUT_S = 600
MARKER_POLL_S = 0.05
# Variables a Host-started process keeps (public checks). The Python_Client
# applies the same Windows basics to the sidecar; without SYSTEMDRIVE and the
# profile/ProgramData folders Windows tools write a literal `%SystemDrive%`
# folder into the working directory. No credential is ever in this list.
CHILD_ENV_KEYS = ('PATH', 'SYSTEMROOT', 'WINDIR', 'SYSTEMDRIVE', 'TEMP', 'TMP', 'PATHEXT', 'COMSPEC',
                  'PROGRAMDATA', 'ALLUSERSPROFILE', 'USERPROFILE', 'LOCALAPPDATA', 'APPDATA', 'HOME', 'LANG')
WRITE_TOOLS = ('ws_write_file', 'ws_edit_file')
OBJECTIVE = ('Complete the coding task in the workspace. First read the task description and the existing files with '
             'ws_list_dir and ws_read_file. Change files only with ws_write_file or ws_edit_file. Run '
             'run_public_checks to check your work and keep repairing until it passes, then finish with a short '
             'summary of what you changed.')
RESUME_OBJECTIVE = ('The previous process stopped while one of your tool calls was open; the Host settled that call '
                    'from its own operation receipt (see its tool result). Do not repeat an operation the result says '
                    'is applied. Continue the same coding task in the workspace, run run_public_checks, and finish '
                    'with a short summary.')


def _schema(properties: dict, required: list[str]) -> dict:
    return {'type': 'object', 'properties': properties, 'required': required, 'additionalProperties': False}


TOOLS = [
    {'name': 'ws_read_file', 'description': 'Read a text file of the task workspace. `offset` is the first line '
     '(1-based, default 1) and `limit` the line count (default 400). Paths are relative to the workspace root; '
     'paths outside it are rejected with path_outside_workspace.',
     'parameters': _schema({'path': {'type': 'string'}, 'offset': {'type': 'integer'}, 'limit': {'type': 'integer'}},
                           ['path'])},
    {'name': 'ws_list_dir', 'description': 'List a directory of the task workspace (default: the root).',
     'parameters': _schema({'path': {'type': 'string'}}, [])},
    {'name': 'ws_search_files', 'description': 'Search text files under a workspace directory for a literal '
     'substring; returns at most 100 matching lines.',
     'parameters': _schema({'pattern': {'type': 'string'}, 'path': {'type': 'string'}}, ['pattern'])},
    {'name': 'ws_write_file', 'description': 'Write a whole text file in the task workspace (creates or replaces it).',
     'parameters': _schema({'path': {'type': 'string'}, 'content': {'type': 'string'}}, ['path', 'content'])},
    {'name': 'ws_edit_file', 'description': 'Replace the single occurrence of `old_text` with `new_text` in a '
     'workspace file. Fails without changing the file when `old_text` occurs zero or several times.',
     'parameters': _schema({'path': {'type': 'string'}, 'old_text': {'type': 'string'}, 'new_text': {'type': 'string'}},
                           ['path', 'old_text', 'new_text'])},
    {'name': 'run_public_checks', 'description': "Run this task's fixed public checks on a fresh copy of the "
     'workspace and return the exit status and output tail. Takes no arguments.',
     'parameters': _schema({}, [])},
]


def now_ms() -> int:
    return int(time.time() * 1000)


def sha256_bytes(data: bytes) -> str:
    return hashlib.sha256(data).hexdigest()


def tail(text: str, limit: int) -> str:
    data = text.encode('utf-8')
    return text if len(data) <= limit else data[-limit:].decode('utf-8', 'ignore')


class ToolError(Exception):
    """A known tool failure with no effect (settled, not unknown)."""

    def __init__(self, code: str, detail: str = ''):
        super().__init__(code)
        self.code, self.detail = code, detail


def confine(root: Path, raw, write: bool = False) -> Path:
    """`raw` resolved inside the canonical workspace `root`, or ToolError."""
    if not isinstance(raw, str) or not raw.strip():
        raise ToolError('empty_path')
    candidate = Path(raw)
    target = candidate if candidate.is_absolute() else root / candidate
    resolved = target.resolve(strict=False)
    try:
        inside = os.path.normcase(os.path.commonpath([str(resolved), str(root)])) == os.path.normcase(str(root))
    except ValueError:  # different drives
        inside = False
    if not inside:
        raise ToolError('path_outside_workspace', str(resolved))
    if target.is_symlink() or (write and resolved.is_file() and os.stat(resolved).st_nlink != 1):
        raise ToolError('path_outside_workspace', f'{resolved} is a link')
    return resolved


class StderrTee:
    """Keeps the sidecar stderr the Python_Client reads and discards (its log)."""

    def __init__(self, inner):
        self.inner, self.data = inner, bytearray()

    async def read(self, n: int = -1) -> bytes:
        chunk = await self.inner.read(n)
        self.data.extend(chunk)
        return chunk


class ClientAsyncio(types.ModuleType):
    """`asyncio` as seen by the Python_Client, with the sidecar process handed
    to the driver so it can terminate it at OS level. Nothing else changes."""

    def __init__(self):
        super().__init__('asyncio')
        self.sink = None

    def __getattr__(self, name):
        return getattr(asyncio, name)

    async def create_subprocess_exec(self, *args, **kwargs):
        proc = await asyncio.create_subprocess_exec(*args, **kwargs)
        if self.sink is not None:
            self.sink(proc)
        return proc


class TrialLog:
    """`trial-log.jsonl`: one fsynced line per record with a driver sequence."""

    def __init__(self, path: Path, token: str):
        self.token, self.seq = token, 0
        self.lock = threading.Lock()  # public checks and snapshots run in worker threads
        self.stream = path.open('x', encoding='utf-8')

    def write(self, kind: str, **fields) -> int:
        with self.lock:
            self.seq += 1
            line = json.dumps({'seq': self.seq, 'at_unix_ms': now_ms(), 'kind': kind, **fields}, ensure_ascii=False)
            if self.token:
                line = line.replace(self.token, '[REDACTED_CREDENTIAL]')
            self.stream.write(line + '\n')
            self.stream.flush()
            os.fsync(self.stream.fileno())
            return self.seq

    def close(self):
        self.stream.close()


class Trial:
    def __init__(self, spec: dict, trial_dir: Path, token: str):
        self.spec, self.dir, self.token = spec, trial_dir, token
        self.task_id = spec['task_id']
        self.workspace = trial_dir / 'workspace'
        self.state = trial_dir / 'state'
        self.session = self.state / 'session.json'
        self.receipts = trial_dir / 'host-receipts'
        self.private = trial_dir / 'host-private'
        self.marker = trial_dir / 'kill-marker.json'
        self.evidence = [trial_dir / 'evidence-1', trial_dir / 'evidence-2']
        self.operations = [trial_dir / 'ops-1', trial_dir / 'ops-2']
        self.log = TrialLog(trial_dir / 'trial-log.jsonl', token)
        self.process = 0
        self.kill_requested = False
        self.kill = None
        self.reconciled = False
        self.fences: dict[str, str | None] = {'start': None, 'before_resume': None, 'after_resume': None}
        self.journal_snapshots: list[str] = []
        self.counter = 0
        self.receipt_index: dict[str, list[tuple[Path, dict]]] = {}
        self.seen_calls: dict[str, str] = {}
        self.processes: list[dict] = []

    # ----- evidence files -------------------------------------------------

    def write_new(self, path: Path, value) -> Path:
        text = json.dumps(value, ensure_ascii=False, indent=2)
        if self.token:
            text = text.replace(self.token, '[REDACTED_CREDENTIAL]')
        with path.open('x', encoding='utf-8') as stream:
            stream.write(text + '\n')
            stream.flush()
            os.fsync(stream.fileno())
        return path

    def next_number(self) -> int:
        with self.log.lock:
            self.counter += 1
            return self.counter

    def fence_path(self) -> Path | None:
        fences = sorted(self.state.glob('.ara-lite-harness-*.fence.json'))
        return fences[0] if len(fences) == 1 else None

    def snapshot_fence(self, label: str) -> None:
        if self.fences[label] is not None:
            return
        fence = self.fence_path()
        if fence is None:
            return
        target = self.dir / f'fence-{label.replace("_", "-")}.json'
        target.write_bytes(fence.read_bytes())
        self.fences[label] = str(target)
        self.log.write('fence_snapshot', process=self.process, label=label, path=str(target),
                       sha256=sha256_bytes(target.read_bytes()))

    def snapshot_journal(self, label: str) -> None:
        directory = self.session.with_suffix('.journal')
        records, count = [], 0
        head = directory / 'head.json'
        if head.is_file():
            count = json.loads(head.read_bytes())['head']['raw_count']
            for segment in sorted(directory.glob('segment-*.jsonl')):
                for line in segment.read_bytes().split(b'\n'):
                    try:
                        row = json.loads(line)
                    except ValueError:
                        continue  # empty or not yet complete line
                    if row['index'] < count:
                        records.append({'index': row['index'], 'hash': row['hash'], 'chain': row['chain']})
        path = self.write_new(self.dir / f'journal-{len(self.journal_snapshots) + 1:03d}-{label}.json',
                              {'label': label, 'process': self.process, 'raw_count': count, 'records': records})
        self.journal_snapshots.append(str(path))
        self.log.write('journal_snapshot', process=self.process, label=label, path=str(path), raw_count=count)

    def receipt(self, request: dict, stage: str, committed: bool, **details) -> Path:
        number = self.next_number()
        native = request['native_call_id']
        path = self.receipts / f'{number:04d}-{stage}-{sha256_bytes(native.encode())[:12]}.json'
        record = {'version': 1, 'kind': 'host_operation', 'stage': stage, 'committed': committed,
                  'native_call_id': native, 'rpc_id': request['rpc_id'], 'run_id': request['run_id'],
                  'task_id': request['task_id'], 'tool_name': request['tool_name'],
                  'args_sha256': request['args_sha256'], 'process': self.process, 'at_unix_ms': now_ms(), **details}
        self.write_new(path, record)
        self.receipt_index.setdefault(native, []).append((path, record))
        self.log.write('host_receipt', process=self.process, path=str(path), stage=stage, committed=committed,
                       native_call_id=native, rpc_id=request['rpc_id'], tool_name=request['tool_name'],
                       args_sha256=request['args_sha256'])
        return path

    def counts(self) -> dict:
        """The three trigger counts from the first sidecar's evidence files."""
        evidence = self.evidence[0]
        compactions = 0
        for events in evidence.glob('run-*-events.jsonl'):
            for line in events.read_bytes().split(b'\n'):
                try:
                    event = json.loads(line)['event']
                except (ValueError, KeyError, TypeError):
                    continue
                if isinstance(event, dict) and event.get('type') == 'compact' and event.get('status') == 'completed':
                    compactions += 1
        kinds = []
        for reply in evidence.glob('jsonl-*-reply.json'):
            try:
                kinds.append(json.loads(reply.read_bytes()).get('kind'))
            except ValueError:
                continue
        return {'compaction': compactions, 'host_review_continuation': kinds.count('prepare_reply'),
                'length_continuation': kinds.count('prepare_stop_reply')}

    # ----- public check ---------------------------------------------------

    def child_env(self) -> dict:
        return {key: value for key, value in os.environ.items()
                if key.upper() in CHILD_ENV_KEYS and key != self.spec['credential_env']}

    def public_check(self, label: str) -> tuple[bool, str, Path]:
        number = self.next_number()
        copy_dir = self.private / f'{label}-{number}'
        shutil.copytree(self.workspace, copy_dir)
        command = self.spec['public_check']
        flags = {'creationflags': subprocess.CREATE_NO_WINDOW} if os.name == 'nt' else {}
        try:
            done = subprocess.run(command, cwd=copy_dir, env=self.child_env(), capture_output=True,
                                  timeout=PUBLIC_CHECK_TIMEOUT_S, **flags)
            passed, exit_text = done.returncode == 0, f'exit {done.returncode}'
            output = (done.stdout + done.stderr).decode('utf-8', 'replace')
        except subprocess.TimeoutExpired:
            passed, exit_text, output = False, 'timed out', ''
        except OSError as error:
            passed, exit_text, output = False, f'could not start: {error}', ''
        feedback = tail(f"public checks {'passed' if passed else 'failed'} ({exit_text}):\n{output}", FEEDBACK_LIMIT)
        receipt = self.write_new(self.private / f'{label}-{number}.json',
                                 {'label': label, 'command': command, 'passed': passed, 'exit': exit_text,
                                  'output_sha256': sha256_bytes(output.encode('utf-8')), 'feedback': feedback,
                                  'committed': True, 'process': self.process, 'at_unix_ms': now_ms()})
        self.log.write('public_check', process=self.process, label=label, passed=passed, receipt=str(receipt))
        return passed, feedback, receipt

    # ----- trial ----------------------------------------------------------

    def start_config(self, deadline_unix_ms: int) -> dict:
        model, budget = self.spec['model'], self.spec['budget']
        return {
            'kind': 'start_task', 'version': 1,
            'task': {'task_id': self.task_id, 'criteria_revision': f'long-trial-{self.task_id}-v1',
                     'authorization_revision': 'lite-eval-long-trial-v1', 'kind': self.spec['kind'],
                     'evidence_dir': str(self.evidence[0]), 'max_runs': budget['max_runs'],
                     'max_calls': budget['max_calls'], 'deadline_unix_ms': deadline_unix_ms},
            'run': {'run_id': f'{self.task_id}-run-1', 'objective': OBJECTIVE, 'cwd': str(self.workspace),
                    'session_path': str(self.session), 'environment_state': str(self.state / 'environment'),
                    'device_fp': 'lite-eval-long-trial', 'session_key': f'long-trial/{self.task_id}',
                    'mode': 'execute', 'endpoint': model['endpoint'], 'model': model['id'],
                    'max_calls': budget['run_max_calls'], 'max_seconds': budget['run_max_seconds'],
                    'max_output': self.spec['max_output'], 'model_resolution': {'kind': 'catalog'},
                    'generation_policy': model['generation_policy'], 'length_recovery': model['length_recovery'],
                    'tools': TOOLS},
            'length_stop_review': 'review_closed_length',
            'emit_events': True,
            'cancel_grace_ms': 5000,
        }

    async def watch_marker(self, spawned: list) -> None:
        while not self.marker.exists():
            await asyncio.sleep(MARKER_POLL_S)
        while not spawned:
            await asyncio.sleep(MARKER_POLL_S)
        proc = spawned[-1]
        if proc.returncode is None:
            proc.kill()  # TerminateProcess on Windows: the sidecar runs no cleanup.
            method = 'TerminateProcess' if os.name == 'nt' else 'SIGKILL'
        else:
            method = f'already exited ({proc.returncode})'
        self.kill = {**(self.kill or {}), 'pid': proc.pid, 'method': method, 'at_unix_ms': now_ms()}
        self.kill['log_seq'] = self.log.write('kill', process=self.process, pid=proc.pid, method=method)

    async def run_process(self, number: int, config: dict, host, shim: ClientAsyncio) -> dict:
        self.process = number
        spawned: list = []
        tees: list[StderrTee] = []

        def sink(proc):
            tee = StderrTee(proc.stderr)
            proc.stderr = tee
            tees.append(tee)
            spawned.append(proc)
            self.log.write('sidecar_spawned', process=number, pid=proc.pid)

        shim.sink = sink
        start_path = self.write_new(self.dir / f'start-{number}.json', config)
        self.log.write('sidecar_start', process=number, evidence_dir=config['task']['evidence_dir'],
                       resume='resume' in config, start_frame=str(start_path))
        watcher = asyncio.create_task(self.watch_marker(spawned)) if number == 1 else None
        record = {'process': number, 'start_frame': str(start_path), 'outcome': None, 'stop': None, 'error': None}
        try:
            outcome = await client.run_task(Path(self.spec['sidecar']), config, host, token=self.token,
                                            operation_directory=self.operations[number - 1],
                                            on_event=lambda frame: self.on_event(number, frame))
            record['outcome'], record['stop'] = outcome, outcome.get('stop')
        except Exception as error:  # ReconciliationRequired, ValueError, OSError: recorded, never retried
            record['error'] = f'{type(error).__name__}: {error}'
        finally:
            shim.sink = None
            if watcher is not None:
                watcher.cancel()
                await asyncio.gather(watcher, return_exceptions=True)
        proc = spawned[-1] if spawned else None
        record['pid'] = proc.pid if proc else None
        record['exit_code'] = proc.returncode if proc else None
        stderr = bytes(tees[-1].data).decode('utf-8', 'replace') if tees else ''
        if self.token:
            stderr = stderr.replace(self.token, '[REDACTED_CREDENTIAL]')
        log_path = self.dir / f'sidecar-{number}.log'
        header = {'process': number, 'pid': record['pid'], 'exit_code': record['exit_code'],
                  'binary': self.spec['sidecar'], 'evidence_dir': config['task']['evidence_dir'],
                  'error': record['error'], 'stop': record['stop']}
        with log_path.open('x', encoding='utf-8') as stream:
            stream.write(json.dumps(header, ensure_ascii=False) + '\n--- stderr ---\n' + stderr)
        record['log'] = str(log_path)
        self.log.write('sidecar_end', process=number, pid=record['pid'], exit_code=record['exit_code'],
                       stop=record['stop'], error=record['error'])
        self.processes.append({key: value for key, value in record.items() if key != 'outcome'})
        return record

    def on_event(self, number: int, frame: dict) -> None:
        if frame.get('kind') == 'event':
            self.log.write('event', process=number, run_id=frame.get('run_id'), event_seq=frame.get('seq'),
                           event_type=frame.get('event_type'))
        else:
            self.log.write('frame', process=number, frame_kind=frame.get('kind'))


class TrialHost:
    """The Host of both sidecar processes. Its tools reach only the workspace."""

    def __init__(self, trial: Trial):
        self.t = trial
        self.root = trial.workspace.resolve(strict=True)

    def log_callback(self, frame_kind: str, **fields) -> None:
        self.t.log.write('callback', process=self.t.process, frame_kind=frame_kind, **fields)
        if self.t.process == 1:
            self.t.snapshot_fence('start')
        elif self.t.reconciled:
            self.t.snapshot_fence('after_resume')

    async def tool_call(self, request: dict, cancelled: asyncio.Event) -> dict:
        name, args = request['tool_name'], request['arguments']
        self.log_callback('tool_call', rpc_id=request['rpc_id'], run_id=request['run_id'],
                          native_call_id=request['native_call_id'], tool_name=name, args_sha256=request['args_sha256'])
        self.t.seen_calls[request['native_call_id']] = name
        rejected = f"rejected-no-effect:{request['rpc_id']}"
        try:
            if name in WRITE_TOOLS:
                return await self.side_effect(request, args, cancelled)
            if name == 'run_public_checks':
                passed, feedback, receipt = await asyncio.to_thread(self.t.public_check, 'public-check')
                return {'value': {'passed': passed, 'output': feedback}, 'is_error': False, 'settlement': 'Settled',
                        'operation_receipt': str(receipt)}
            try:
                value = self.read_only(name, args)
            except (ValueError, TypeError, OSError) as error:  # bad argument or unreadable file: no effect
                raise ToolError('invalid_request', f'{type(error).__name__}: {error}') from None
            return {'value': value, 'is_error': False, 'settlement': 'Settled',
                    'operation_receipt': f"read-only:{request['rpc_id']}"}
        except ToolError as error:
            return {'value': {'error': error.code, 'detail': error.detail}, 'is_error': True, 'settlement': 'Settled',
                    'operation_receipt': rejected}

    def relative(self, path: Path) -> str:
        return os.path.relpath(path, self.root).replace('\\', '/')

    def read_only(self, name: str, args: dict):
        if name == 'ws_read_file':
            path = confine(self.root, args.get('path'))
            if not path.is_file():
                raise ToolError('not_a_file', self.relative(path))
            lines = path.read_text(encoding='utf-8', errors='replace').splitlines()
            offset = max(1, int(args.get('offset') or 1))
            limit = max(1, min(int(args.get('limit') or 400), 2000))
            text = '\n'.join(f'{number}: {line}' for number, line in
                             enumerate(lines[offset - 1:offset - 1 + limit], start=offset))
            data = text.encode('utf-8')
            return {'path': self.relative(path), 'total_lines': len(lines),
                    'content': data[:READ_LIMIT].decode('utf-8', 'ignore'), 'truncated': len(data) > READ_LIMIT}
        if name == 'ws_list_dir':
            path = confine(self.root, args.get('path') or '.')
            if not path.is_dir():
                raise ToolError('not_a_directory', self.relative(path))
            entries = sorted(item.name + ('/' if item.is_dir() else '') for item in path.iterdir())
            return {'path': self.relative(path), 'entries': entries[:500], 'truncated': len(entries) > 500}
        if name == 'ws_search_files':
            pattern = args.get('pattern')
            if not isinstance(pattern, str) or not pattern:
                raise ToolError('empty_pattern')
            base = confine(self.root, args.get('path') or '.')
            matches = []
            for file in sorted(base.rglob('*')):
                if len(matches) >= 100:
                    break
                if not file.is_file() or file.is_symlink() or file.stat().st_size > 2 * 1024 * 1024:
                    continue
                for number, line in enumerate(file.read_text(encoding='utf-8', errors='ignore').splitlines(), 1):
                    if pattern in line:
                        matches.append({'path': self.relative(file), 'line': number, 'text': line[:400]})
                        if len(matches) >= 100:
                            break
            return {'pattern': pattern, 'matches': matches, 'truncated': len(matches) >= 100}
        raise ToolError('unknown_tool', name)

    async def side_effect(self, request: dict, args: dict, cancelled: asyncio.Event) -> dict:
        path = confine(self.root, args.get('path'), write=True)
        if request['tool_name'] == 'ws_write_file':
            content = args.get('content')
            if not isinstance(content, str):
                raise ToolError('content_required')
            data = content.encode('utf-8')
        else:
            old, new = args.get('old_text'), args.get('new_text')
            if not isinstance(old, str) or not old or not isinstance(new, str):
                raise ToolError('old_text_and_new_text_required')
            if not path.is_file():
                raise ToolError('not_a_file', self.relative(path))
            try:
                text = path.read_text(encoding='utf-8')
            except (OSError, ValueError) as error:  # before any effect
                raise ToolError('unreadable_file', f'{type(error).__name__}: {error}') from None
            if text.count(old) != 1:
                raise ToolError('old_text_not_unique', f'old_text occurs {text.count(old)} times')
            data = text.replace(old, new, 1).encode('utf-8')
        trial = self.t
        trigger = None
        if trial.process == 1 and not trial.kill_requested:
            counts = trial.counts()
            if all(value >= 1 for value in counts.values()):
                trigger = counts
        target = self.relative(path)
        trial.receipt(request, 'started', False, target=target, intended_sha256=sha256_bytes(data),
                      trial_kill_point=trigger is not None)
        path.parent.mkdir(parents=True, exist_ok=True)
        with path.open('wb') as stream:
            stream.write(data)
            stream.flush()
            os.fsync(stream.fileno())
        applied = trial.receipt(request, 'applied', True, target=target, result_sha256=sha256_bytes(path.read_bytes()),
                                bytes=len(data))
        if trigger is not None:
            # 35.2: the effect has started (and is committed in the Host's own
            # receipt); the ToolResult is never written. The marker comes after
            # the committed receipt so the kill cannot race the effect.
            trial.kill_requested = True
            trial.kill = {'native_call_id': request['native_call_id'], 'rpc_id': request['rpc_id'],
                          'run_id': request['run_id'], 'tool_name': request['tool_name'],
                          'args_sha256': request['args_sha256'], 'receipt': str(applied), 'counts': trigger}
            trial.write_new(trial.marker, {**trial.kill, 'at_unix_ms': now_ms()})
            trial.kill['marker_log_seq'] = trial.log.write('marker', process=trial.process, path=str(trial.marker),
                                                           native_call_id=request['native_call_id'], counts=trigger)
            await cancelled.wait()
            raise RuntimeError('sidecar terminated while this Host tool was open')
        return {'value': {'written': target, 'bytes': len(data), 'sha256': sha256_bytes(data)}, 'is_error': False,
                'settlement': 'Settled', 'operation_receipt': str(applied)}

    def review_receipt(self, kind: str, basis: dict, passed: bool, feedback: str, check: Path) -> dict:
        verdict = 'Verified' if passed else {'NeedsChanges': {'feedback': feedback}}
        number = self.t.next_number()
        path = self.t.write_new(self.t.receipts / f'{number:04d}-{kind}.json',
                                {'version': 1, 'kind': kind, 'committed': True, 'run_id': basis.get('run_id'),
                                 'run_number': basis.get('run_number'), 'verdict': verdict,
                                 'public_check_receipt': str(check), 'process': self.t.process,
                                 'at_unix_ms': now_ms()})
        self.t.log.write('host_receipt', process=self.t.process, path=str(path), stage=kind, committed=True,
                         native_call_id=None, run_id=basis.get('run_id'))
        return {'basis': basis, 'committed': True, 'settlement': 'Settled', 'verdict': verdict, 'evidence': [str(path)]}

    async def review(self, kind: str, basis: dict) -> dict:
        self.log_callback(kind, run_id=basis.get('run_id'), run_number=basis.get('run_number'))
        await asyncio.to_thread(self.t.snapshot_journal, f'{kind}-run-{basis.get("run_number")}')
        passed, feedback, check = await asyncio.to_thread(self.t.public_check, kind)
        return self.review_receipt(kind, basis, passed, feedback, check)

    async def verify(self, candidate: dict, cancelled: asyncio.Event) -> dict:
        return await self.review('verify', candidate['basis'])

    async def verify_stop_snapshot(self, snapshot: dict, cancelled: asyncio.Event) -> dict:
        receipt = await self.review('verify_stop', snapshot['basis'])
        if receipt['verdict'] != 'Verified':
            cause = snapshot.get('evidence', {}).get('cause')
            feedback = receipt['verdict']['NeedsChanges']['feedback']
            receipt['verdict'] = {'NeedsChanges': {'feedback': f'The previous Run stopped ({cause}). ' + feedback}}
        return receipt

    def next_run(self, kind: str, continuation: dict, prefix: str) -> dict:
        number = continuation['next_run_number']
        run_id = f'{self.t.task_id}-run-{number}'
        feedback = continuation['receipt']['verdict']['NeedsChanges']['feedback']
        self.log_callback(kind, run_id=continuation['previous']['basis']['run_id'], run_id_next=run_id,
                          next_run_number=number)
        return {'run_id_next': run_id, 'objective': f'{prefix}\n\n{feedback}\n\nRepair the workspace files, run '
                'run_public_checks, then finish with a short summary.'}

    async def prepare(self, continuation: dict, cancelled: asyncio.Event) -> dict:
        return self.next_run('prepare', continuation,
                             "The Host ran this task's public checks on your workspace and they did not pass.")

    async def prepare_stop_continuation(self, continuation: dict, cancelled: asyncio.Event) -> dict:
        return self.next_run('prepare_stop', continuation,
                             'Your previous reply was cut at the output limit. Work in smaller steps and keep each '
                             'reply short; write large files in several edits.')

    async def reconcile_resume(self, review: dict, cancelled: asyncio.Event) -> dict:
        trial = self.t
        interrupted = review.get('interrupted_run') or {}
        trial.log.write('callback', process=trial.process, frame_kind='reconcile_request',
                        interrupted_run=interrupted.get('run_id'), consumed_runs=review.get('consumed_runs'),
                        consumed_calls=review.get('consumed_calls'),
                        open_calls=[call.get('id') for call in interrupted.get('open_tool_calls', [])])
        trial.write_new(trial.dir / 'resume-review.json', review)
        calls, evidence = [], []
        for call in interrupted.get('open_tool_calls', []):
            records = trial.receipt_index.get(call['id'], [])
            applied = [(path, record) for path, record in records if record['committed']]
            started = [(path, record) for path, record in records if record['stage'] == 'started']
            if applied:
                path, record = applied[-1]
                output = (f"Host operation receipt {path.name}: {call['name']} on {record['target']} was applied once "
                          f"(sha256 {record['result_sha256']}). The effect is in place; do not repeat it.")
                settlement = {'kind': 'completed', 'output': output}
                evidence.append(str(path))
            elif started:
                # Started, never committed: settle from the file itself.
                path, record = started[-1]
                target = self.root / record['target']
                actual = sha256_bytes(target.read_bytes()) if target.is_file() else None
                done = actual == record['intended_sha256']
                settlement = {'kind': 'completed' if done else 'failed',
                              'output': f"Host receipt {path.name}: {call['name']} on {record['target']} "
                                        f"{'is in place' if done else 'was not applied'} (file sha256 {actual})."}
                evidence.append(str(path))
            elif call['id'] in trial.seen_calls:
                settlement = {'kind': 'failed', 'output': f"{call['name']} was interrupted; it has no external effect. "
                              'Repeat it if you still need its result.'}
            else:
                settlement = {'kind': 'not_executed'}
            calls.append({'call_id': call['id'], 'settlement': settlement})
        number = trial.next_number()
        settlement_path = trial.write_new(trial.receipts / f'{number:04d}-reconcile.json',
                                          {'version': 1, 'kind': 'reconcile_settlement', 'committed': True,
                                           'interrupted_run': interrupted.get('run_id'), 'tool_calls': calls,
                                           'evidence': list(evidence), 'process': trial.process, 'at_unix_ms': now_ms()})
        trial.log.write('host_receipt', process=trial.process, path=str(settlement_path), stage='reconcile',
                        committed=True, native_call_id=None)
        evidence.append(str(settlement_path))
        trial.reconciled = True
        trial.log.write('reconcile_reply', process=trial.process, tool_calls=calls, evidence=evidence)
        return {'committed': True, 'settlement': 'Settled', 'tool_calls': calls, 'evidence': evidence}


async def run_trial(spec: dict, trial_dir: Path, token: str) -> dict:
    trial = Trial(spec, trial_dir, token)
    for directory in (trial.state, trial.receipts, trial.private):
        directory.mkdir()
    shutil.copytree(spec['fixture'], trial.workspace)
    shim = ClientAsyncio()
    client.asyncio = shim  # only the client's view; see ClientAsyncio
    host = TrialHost(trial)
    started = now_ms()
    deadline = started + spec['budget']['deadline_seconds'] * 1000
    config = trial.start_config(deadline)
    error, resume = None, None
    trial.log.write('trial_start', task_id=trial.task_id, deadline_unix_ms=deadline, max_calls=spec['budget']['max_calls'])
    first = await trial.run_process(1, config, host, shim)
    final = first
    if trial.kill is not None and 'pid' in trial.kill:
        fence = trial.fence_path()
        trial.snapshot_fence('before_resume')
        await asyncio.to_thread(trial.snapshot_journal, 'before-resume')
        if fence is None:
            error = 'no single Harness fence file after the kill; resume not attempted'
            resume = {'attempted': False, 'refused': False, 'error': error, 'rejected': None}
        else:
            config2 = copy.deepcopy(config)
            config2['task']['evidence_dir'] = str(trial.evidence[1])
            config2['run']['run_id'] = f'{trial.task_id}-resume'
            config2['run']['objective'] = RESUME_OBJECTIVE
            config2['resume'] = {'version': 1, 'fence_sha256': sha256_bytes(fence.read_bytes())}
            trial.log.write('resume_start', fence_sha256=config2['resume']['fence_sha256'])
            second = await trial.run_process(2, config2, host, shim)
            final = second
            rejected_path = trial.evidence[1] / 'jsonl-resume-rejected.json'
            rejected = json.loads(rejected_path.read_bytes()) if rejected_path.is_file() else None
            refused = rejected is not None or (second['error'] is not None and not trial.reconciled)
            resume = {'attempted': True, 'refused': refused, 'error': second['error'], 'rejected': rejected}
            if refused:
                trial.log.write('resume_refused', error=second['error'], rejected=rejected)
    elif trial.kill is not None:
        error = 'the kill marker was written but the sidecar was never terminated'
    await asyncio.to_thread(trial.snapshot_journal, 'end')
    ended_by = 'task_result' if final['outcome'] is not None else 'error'
    trial.log.write('trial_end', ended_by=ended_by, stop=final['stop'], error=final['error'] or error)
    record = {
        'version': TRIAL_VERSION, 'task_id': trial.task_id, 'started_at_unix_ms': started,
        'finished_at_unix_ms': now_ms(), 'deadline_unix_ms': deadline, 'max_calls': spec['budget']['max_calls'],
        'session_path': str(trial.session), 'workspace': str(trial.workspace),
        'trial_log': str(trial.dir / 'trial-log.jsonl'), 'host_receipts': str(trial.receipts),
        'evidence_dirs': [str(path) for path in trial.evidence if path.exists()],
        'operation_dirs': [str(path) for path in trial.operations if path.exists()],
        'sidecar_logs': [process['log'] for process in trial.processes],
        'fences': trial.fences, 'journal_snapshots': trial.journal_snapshots,
        'tools': [tool['name'] for tool in TOOLS], 'kill': trial.kill, 'resume': resume,
        'processes': trial.processes, 'ended_by': ended_by, 'stop': final['stop'], 'error': error,
    }
    trial.write_new(trial.dir / 'trial.json', record)
    trial.log.close()
    return record


def main(argv=None) -> int:
    parser = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    parser.add_argument('--input', required=True, type=Path, help='trial-input.json written by lite-eval long-trial')
    parser.add_argument('--trial-dir', required=True, type=Path, help='new trial directory (already created)')
    args = parser.parse_args(argv)
    spec = json.loads(args.input.read_text(encoding='utf-8'))
    trial_dir = args.trial_dir.resolve(strict=True)
    token = os.environ.get(spec['credential_env'], '')
    if not token:
        print(f"credential variable {spec['credential_env']} is not set; nothing was started", file=sys.stderr)
        return 2
    if not Path(spec['sidecar']).is_file():
        print(f"sidecar binary {spec['sidecar']} does not exist; nothing was started", file=sys.stderr)
        return 2
    record = asyncio.run(run_trial(spec, trial_dir, token))
    print(json.dumps({'trial': str(trial_dir / 'trial.json'), 'ended_by': record['ended_by'], 'stop': record['stop'],
                      'killed': record['kill'] is not None, 'resume': record['resume']}, ensure_ascii=False))
    return 0


if __name__ == '__main__':
    sys.exit(main())
