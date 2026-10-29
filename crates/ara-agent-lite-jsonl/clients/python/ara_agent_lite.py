"""Async Host callbacks for the shared JSONL runtime; no Agent loop or retry."""
from __future__ import annotations

import asyncio
from dataclasses import dataclass
import hashlib
import inspect
import json
import os
from pathlib import Path
import subprocess
import time
from typing import Any, Protocol

FRAME_BYTES = 262144
RESULT_BYTES = 131072


class ReconciliationRequired(RuntimeError):
    """Inspect original Task files/Host operations before any further action."""


class Host(Protocol):
    async def tool_call(self, request: dict, cancelled: asyncio.Event) -> dict: ...
    async def verify(self, candidate: dict, cancelled: asyncio.Event) -> dict: ...
    async def prepare(self, continuation: dict, cancelled: asyncio.Event) -> dict: ...


class BudgetStopHost(Host, Protocol):
    async def verify_budget_snapshot(self, snapshot: dict, cancelled: asyncio.Event) -> dict: ...
    async def prepare_budget_stop_continuation(self, continuation: dict, cancelled: asyncio.Event) -> dict: ...


class LengthStopHost(Host, Protocol):
    async def verify_stop_snapshot(self, snapshot: dict, cancelled: asyncio.Event) -> dict: ...
    async def prepare_stop_continuation(self, continuation: dict, cancelled: asyncio.Event) -> dict: ...


class ResumeHost(Host, Protocol):
    async def reconcile_resume(self, review: dict, cancelled: asyncio.Event) -> dict: ...


# Failure categories of the `reconcile_resume` callback (11.2); each is the
# first segment of the ReconciliationRequired message. No reply is written.
RECONCILE_FAILURES = ('reconcile_callback_exception', 'reconcile_receipt_invalid', 'reconcile_frame_too_large',
                      'reconcile_timeout')


class _ReconcileFailure(ReconciliationRequired):
    def __init__(self, category: str):
        super().__init__(f'{category}: Host reconcile_resume gave no valid settlement; no reply was written')
        self.category = category


def valid_resume_receipt(value: Any) -> bool:
    """Exact `ResumeReceipt` fields and types; Harness still checks the content."""
    if not isinstance(value, dict) or set(value) != {'committed', 'settlement', 'tool_calls', 'evidence'}:
        return False
    if type(value['committed']) is not bool or value['settlement'] not in ('Settled', 'Unknown'):
        return False
    if not isinstance(value['evidence'], list) or not all(isinstance(item, str) for item in value['evidence']):
        return False
    if not isinstance(value['tool_calls'], list):
        return False
    for call in value['tool_calls']:
        if not isinstance(call, dict) or set(call) != {'call_id', 'settlement'} or not isinstance(call['call_id'], str):
            return False
        settlement = call['settlement']
        if not isinstance(settlement, dict):
            return False
        if settlement.get('kind') == 'not_executed':
            if set(settlement) != {'kind'}:
                return False
        elif (settlement.get('kind') not in ('completed', 'failed') or set(settlement) != {'kind', 'output'}
              or not isinstance(settlement['output'], str)):
            return False
    return True


async def reconcile_reply(host, frame: dict, review: dict, cancelled: asyncio.Event,
                          deadline_unix_ms: int, grace_seconds: float) -> dict:
    """Call `host.reconcile_resume` once and build its `reconcile_reply`.

    At the Task deadline the signal is set and the callback gets the grace to
    return. Any failure raises `ReconciliationRequired` whose message starts
    with one of RECONCILE_FAILURES; it never contains the callback's error text.
    """
    call = asyncio.ensure_future(host.reconcile_resume(review, cancelled))
    done, _ = await asyncio.wait({call}, timeout=max(0.0, deadline_unix_ms / 1000 - time.time()))
    if not done:
        cancelled.set()
        done, _ = await asyncio.wait({call}, timeout=grace_seconds)
        if not done:
            call.cancel()
        raise _ReconcileFailure('reconcile_timeout')
    if cancelled.is_set() or time.time() * 1000 >= deadline_unix_ms:
        # The sidecar's callback_cancel or the deadline came first; a late reply is never written.
        raise _ReconcileFailure('reconcile_timeout')
    if call.cancelled() or call.exception() is not None:
        raise _ReconcileFailure('reconcile_callback_exception')
    receipt = call.result()
    if not valid_resume_receipt(receipt):
        raise _ReconcileFailure('reconcile_receipt_invalid')
    reply = {'kind': 'reconcile_reply', **{key: frame[key] for key in ('version', 'rpc_id', 'task_id', 'run_id')},
             'receipt': receipt}
    try:
        size = len(json.dumps(reply, ensure_ascii=False, separators=(',', ':'), allow_nan=False).encode('utf-8')) + 1
    except (TypeError, ValueError):
        raise _ReconcileFailure('reconcile_receipt_invalid') from None
    if size > FRAME_BYTES:
        raise _ReconcileFailure('reconcile_frame_too_large')
    return reply


@dataclass(frozen=True)
class UnknownEffect:
    """One tool call whose external effects are unknown (outcome.json `unknown_effects`)."""
    run_id: str
    native_call_id: str
    tool: str
    receipt_ref: dict


def parse_unknown_effects(outcome: dict) -> list[UnknownEffect]:
    """Typed view of `outcome['unknown_effects']` in original order; absent means none.

    An invalid record raises instead of producing an object with default values.
    """
    records = outcome.get('unknown_effects', [])
    if not isinstance(records, list):
        raise ReconciliationRequired('invalid unknown effect record: unknown_effects')
    effects = []
    for record in records:
        if not isinstance(record, dict):
            raise ReconciliationRequired('invalid unknown effect record: record')
        for field in record:
            if field not in ('run_id', 'native_call_id', 'tool', 'receipt_ref'):
                raise ReconciliationRequired(f'invalid unknown effect record: {field}')
        for field in ('run_id', 'native_call_id', 'tool'):
            if not isinstance(record.get(field), str) or not record[field]:
                raise ReconciliationRequired(f'invalid unknown effect record: {field}')
        receipt = record.get('receipt_ref')
        if not isinstance(receipt, dict) or not isinstance(receipt.get('evidence'), dict) or not receipt['evidence']:
            raise ReconciliationRequired('invalid unknown effect record: receipt_ref')
        effects.append(UnknownEffect(record['run_id'], record['native_call_id'], record['tool'], receipt))
    return effects


def _comparison_path(path: Path) -> Path:
    # Rust canonical paths on Windows use the verbatim prefix. Compare the
    # resolved filesystem paths in one spelling, while retaining the original
    # path for I/O (including long paths).
    value = str(path)
    if os.name == 'nt':
        if value.upper().startswith('\\\\?\\UNC\\'):
            value = '\\\\' + value[8:]
        elif value.startswith('\\\\?\\'):
            value = value[4:]
    return Path(value)


def validate_candidate_identity(candidate: dict, frame: dict, task_spec: dict) -> None:
    basis = candidate.get('basis') if isinstance(candidate, dict) else None
    if not isinstance(basis, dict) or any(basis.get(key) != task_spec[key]
        for key in ('task_id', 'criteria_revision', 'authorization_revision')) or basis.get('run_id') != frame['run_id']:
        raise ReconciliationRequired('callback candidate identity differs from the original Task and Run')


def encoded(value: Any) -> bytes:
    payload = json.dumps(value, ensure_ascii=False, separators=(',', ':'), allow_nan=False).encode('utf-8') + b'\n'
    if len(payload) > FRAME_BYTES:
        raise ReconciliationRequired('JSONL frame exceeds the encoded byte limit')
    return payload


def read_reference(reference: dict, evidence_dir: Path) -> dict:
    try:
        if set(reference) != {'path', 'sha256'}:
            raise ReconciliationRequired('invalid artifact reference')
        root = evidence_dir.resolve(strict=True)
        path = Path(reference['path']).resolve(strict=True)
        if not _comparison_path(path).is_relative_to(_comparison_path(root)) or not path.is_file() or path.stat().st_size > 8 * 1024 * 1024:
            raise ReconciliationRequired('artifact reference is outside the Task evidence scope')
        raw = path.read_bytes()
        if hashlib.sha256(raw).hexdigest() != reference['sha256']:
            raise ReconciliationRequired('artifact reference bytes changed')
        return json.loads(raw.decode('utf-8'))
    except (OSError, ValueError, TypeError, KeyError):
        raise ReconciliationRequired('artifact reference could not be verified') from None


def _budget_termination(value):
    if (not isinstance(value, dict) or set(value) != {'version', 'cause', 'native_round_closed', 'model_final_stop'}
            or type(value['version']) is not int or value['version'] != 1
            or value['cause'] != 'call_budget_exhausted' or value['native_round_closed'] is not True
            or value['model_final_stop'] is not False):
        raise ReconciliationRequired('budget termination is not the closed non-final version 1 contract')


def _budget_file(value, evidence: Path, name: str, reference=None):
    """Check the exact persisted view even when its control payload is inline."""
    path = evidence / name
    if path.is_symlink() or not path.is_file() or path.stat().st_size > 8 * 1024 * 1024:
        raise ReconciliationRequired('budget record must be an original bounded file')
    real = path.resolve(strict=True)
    if not _comparison_path(real).is_relative_to(_comparison_path(evidence.resolve(strict=True))):
        raise ReconciliationRequired('budget record escaped original evidence directory')
    with real.open('rb') as stream:
        raw = stream.read(8 * 1024 * 1024 + 1)
    if len(raw) > 8 * 1024 * 1024:
        raise ReconciliationRequired('budget record grew beyond byte bound')
    actual = {'path': str(path.resolve(strict=True)), 'sha256': hashlib.sha256(raw).hexdigest()}
    if reference is not None and (_comparison_path(Path(reference['path']).resolve(strict=True))
            != _comparison_path(path.resolve(strict=True)) or reference['sha256'] != actual['sha256']):
        raise ReconciliationRequired('budget reference path or hash differs from its fixed original')
    stored = json.loads(raw.decode('utf-8'))
    if stored != value:
        raise ReconciliationRequired('budget record differs from its original callback payload')
    return actual


def _checked_snapshot(snapshot, frame, task_spec, evidence):
    if not isinstance(snapshot, dict) or set(snapshot) != {'basis', 'diagnostic_output', 'goal', 'termination'}:
        raise ReconciliationRequired('invalid budget snapshot fields')
    validate_candidate_identity(snapshot, frame, task_spec)
    _budget_termination(snapshot['termination'])
    number = snapshot['basis'].get('run_number')
    if type(number) is not int or number < 1 or not isinstance(snapshot['diagnostic_output'], str):
        raise ReconciliationRequired('invalid budget snapshot identity or diagnostic output')
    return _budget_file(snapshot, evidence, f'run-{number}-budget-snapshot.json')


def _checked_stop_snapshot(snapshot, frame, task_spec, evidence):
    """A closed Length stop snapshot (16.2); its output is diagnostic only."""
    if not isinstance(snapshot, dict) or set(snapshot) != {'basis', 'diagnostic_output', 'goal', 'evidence', 'truncated'}:
        raise ReconciliationRequired('invalid stop snapshot fields')
    validate_candidate_identity(snapshot, frame, task_spec)
    number = snapshot['basis'].get('run_number')
    if (type(number) is not int or number < 1 or not isinstance(snapshot['diagnostic_output'], str)
            or not isinstance(snapshot['evidence'], dict)):
        raise ReconciliationRequired('invalid stop snapshot identity, evidence or diagnostic output')
    return _budget_file(snapshot, evidence, f'run-{number}-stop-snapshot.json')


def _checked_budget_continuation(value, frame, task_spec, evidence, check_snapshot=_checked_snapshot,
                                 prefix='jsonl-budget-continuation'):
    fields = {'previous', 'receipt', 'next_run_number', 'remaining_calls', 'deadline_unix_ms'}
    if not isinstance(value, dict) or set(value) != fields:
        raise ReconciliationRequired('invalid budget continuation fields')
    snapshot_ref = check_snapshot(value['previous'], frame, task_spec, evidence)
    receipt = value['receipt']
    if (not isinstance(receipt, dict) or receipt.get('basis') != value['previous']['basis']
            or receipt.get('committed') is not True or receipt.get('settlement') != 'Settled'
            or not isinstance(receipt.get('verdict'), dict) or set(receipt['verdict']) != {'NeedsChanges'}):
        raise ReconciliationRequired('budget continuation requires its committed rejection receipt')
    number = value['next_run_number']
    if (type(number) is not int or number != value['previous']['basis']['run_number'] + 1
            or type(value['remaining_calls']) is not int or not 0 < value['remaining_calls'] <= task_spec['max_calls']
            or value['deadline_unix_ms'] != task_spec['deadline_unix_ms']):
        raise ReconciliationRequired('budget continuation changed the original Task bounds')
    continuation_ref = _budget_file(value, evidence, f'{prefix}-{number}.json')
    return snapshot_ref, continuation_ref


def read_budget_review_reference(reference: dict, evidence_dir: str | Path, task_spec: dict, outcome: dict) -> dict:
    """Read the separate typed budget review; never modify the original outcome."""
    evidence = Path(evidence_dir)
    record = read_reference(reference, evidence)
    if (not isinstance(record, dict) or set(record) != {'version', 'kind', 'snapshot', 'receipt'}
            or type(record['version']) is not int or record['version'] != 1
            or record['kind'] != 'closed_budget_snapshot_review'):
        raise ReconciliationRequired('invalid typed budget review index')
    receipt = record['receipt']
    if (outcome.get('task_id') != task_spec['task_id'] or outcome.get('task_accepted') is not False or outcome.get('last_candidate') is not None
            or not isinstance(receipt, dict) or receipt != outcome.get('last_receipt')
            or receipt.get('committed') is not True or receipt.get('settlement') != 'Settled'
            or receipt.get('verdict') == 'Unknown'):
        raise ReconciliationRequired('budget review is not the exact current committed outcome receipt')
    basis = receipt['basis']
    frame = {'run_id': basis['run_id']}
    snapshot = read_reference(record['snapshot'], evidence)
    snapshot_ref = _checked_snapshot(snapshot, frame, task_spec, evidence)
    if snapshot['basis'] != basis or snapshot_ref['sha256'] != record['snapshot']['sha256']:
        raise ReconciliationRequired('budget index snapshot identity differs')
    _budget_file(snapshot, evidence, f"run-{basis['run_number']}-budget-snapshot.json", record['snapshot'])
    _budget_file(record, evidence, f"run-{basis['run_number']}-budget-review-index.json", reference)
    return record


async def run_task(binary: str | Path, config: dict, host: Host, *, token: str,
                   operation_directory: str | Path, cancellation: asyncio.Event | None = None,
                   on_event=None) -> dict:
    """Run one Task. Host owns operation receipts and external-effect settlement.

    Host callbacks must be async/cooperative. Use cancellation instead of dropping
    the coroutine. A failed connection is never retried or given a new Session.
    """
    executable = Path(binary).resolve(strict=True)
    if not executable.is_file() or not isinstance(token, str) or not token:
        raise ValueError('an existing executable and a nonempty model grant are required')
    if config.get('kind') != 'start_task' or config.get('version') != 1:
        raise ValueError('start_task protocol version 1 is required')
    budget_policy = config.get('budget_stop_review', 'reject')
    if budget_policy not in ('reject', 'review_closed_tool_round'):
        raise ValueError('invalid budget_stop_review policy')
    budget_enabled = budget_policy == 'review_closed_tool_round'
    if budget_enabled and (config['run'].get('mode') != 'execute' or any(not callable(getattr(host, name, None))
            for name in ('verify_budget_snapshot', 'prepare_budget_stop_continuation'))):
        raise ValueError('Execute budget opt-in requires both dedicated Host callbacks')
    length_policy = config.get('length_stop_review', 'reject')
    if length_policy not in ('reject', 'review_closed_length'):
        raise ValueError('invalid length_stop_review policy')
    length_enabled = length_policy == 'review_closed_length'
    if length_enabled and (config['run'].get('mode') != 'execute' or any(not callable(getattr(host, name, None))
            for name in ('verify_stop_snapshot', 'prepare_stop_continuation'))):
        raise ValueError('Execute length-stop opt-in requires both dedicated Host callbacks')
    mcp_enabled = 'mcp_servers' in config
    resume_enabled = 'resume' in config
    if resume_enabled and not callable(getattr(host, 'reconcile_resume', None)):
        raise ValueError('resume requires a reconcile_resume callback')
    task_id = config['task']['task_id']
    evidence = Path(config['task']['evidence_dir'])
    first_run = config['run']['run_id']
    allowed_runs = {first_run}
    allowed_tools = {entry['name'] for entry in config['run']['tools']}
    start_line = encoded(config)
    operations = Path(operation_directory)
    if not operations.is_absolute() or not operations.parent.is_dir():
        raise ValueError('a fresh absolute Host operation directory with existing parent is required')
    operations.mkdir()

    def record(frame, stage, **details):
        name = hashlib.sha256(frame['rpc_id'].encode('utf-8')).hexdigest() + '-' + stage + '.json'
        metadata = {key: frame[key] for key in ('kind', 'version', 'task_id', 'run_id', 'rpc_id',
                     'native_call_id', 'tool_name', 'args_sha256') if key in frame}
        path = operations / name
        with path.open('x', encoding='utf-8') as stream:
            json.dump({'correlation': metadata, 'stage': stage, **details}, stream, ensure_ascii=False)
            stream.write('\n')
            stream.flush()
            os.fsync(stream.fileno())
        return str(path)
    env = {key: value for key, value in os.environ.items()
           # Windows basics that child tools and MCP servers (npx, uvx, Python)
           # need to resolve profile and ProgramData folders; none are secrets.
           if key.upper() in ('PATH', 'SYSTEMROOT', 'WINDIR', 'TEMP', 'TMP', 'USERPROFILE', 'HOME', 'LANG',
                              'SYSTEMDRIVE', 'PROGRAMDATA', 'ALLUSERSPROFILE', 'LOCALAPPDATA', 'APPDATA',
                              'PATHEXT', 'COMSPEC')}
    env['ARA_AGENT_MODEL_TOKEN'] = token
    kwargs = {'creationflags': subprocess.CREATE_NO_WINDOW} if os.name == 'nt' else {}
    proc = await asyncio.create_subprocess_exec(str(executable), stdin=asyncio.subprocess.PIPE,
                stdout=asyncio.subprocess.PIPE, stderr=asyncio.subprocess.PIPE,
                env=env, limit=FRAME_BYTES, **kwargs)
    send_lock = asyncio.Lock()
    pending: dict[str, tuple[asyncio.Event, asyncio.Task]] = {}
    callback_runs: dict[str, str] = {}
    callback_kinds: dict[str, str] = {}
    seen: set[str] = set()
    ui_tasks: set[asyncio.Task] = set()
    grace = config.get('cancel_grace_ms', 5000) / 1000
    # Category of a refused resume settlement; it names the final error (11.2, 11.5).
    reconcile_failure: list[str] = []
    reconcile_cancel_sent: list[bool] = []

    async def send(value):
        data = encoded(value)
        async with send_lock:
            proc.stdin.write(data)
            await asyncio.wait_for(proc.stdin.drain(), timeout=5)

    async def discard_stderr():
        # Protocol errors are reported without replaying potentially secret stderr.
        while await proc.stderr.read(8192):
            pass

    async def watch_cancel():
        if cancellation is not None:
            await cancellation.wait()
            await send({'kind': 'cancel', 'version': 1, 'task_id': task_id})

    def body(frame, name):
        inline, ref = name in frame, name + '_ref' in frame
        if inline == ref:
            raise ReconciliationRequired('callback requires exactly one inline body or reference')
        return frame[name] if inline else read_reference(frame[name + '_ref'], evidence)

    async def callback(frame, signal):
        kind = frame['kind']
        base = {key: frame[key] for key in ('version', 'rpc_id', 'task_id', 'run_id')}
        try:
            if kind == 'tool_call':
                result = await host.tool_call(frame, signal)
                if set(result) != {'value', 'is_error', 'settlement', 'operation_receipt'}:
                    raise ValueError('tool result fields do not match the protocol')
                if (type(result['is_error']) is not bool or result['settlement'] not in ('Settled', 'Unknown')
                        or not isinstance(result['operation_receipt'], str) or not result['operation_receipt'].strip()):
                    raise ValueError('tool effect settlement and operation receipt are required')
                if len(json.dumps(result, ensure_ascii=False, allow_nan=False).encode()) > RESULT_BYTES:
                    raise ValueError('tool result payload exceeds the encoded byte limit')
                reply = {'kind': 'tool_reply', **base,
                         **{key: frame[key] for key in ('native_call_id', 'tool_name', 'args_sha256')}, 'result': result}
            elif kind == 'verify_request':
                candidate = body(frame, 'candidate')
                validate_candidate_identity(candidate, frame, config['task'])
                receipt = await host.verify(candidate, signal)
                if receipt.get('basis') != candidate['basis']:
                    raise ValueError('Host review basis changed')
                reply = {'kind': 'verify_reply', **base, 'receipt': receipt}
            elif kind == 'verify_budget_request':
                snapshot = body(frame, 'snapshot')
                before = _checked_snapshot(snapshot, frame, config['task'], evidence)
                if 'snapshot_ref' in frame:
                    _budget_file(snapshot, evidence, f"run-{snapshot['basis']['run_number']}-budget-snapshot.json", frame['snapshot_ref'])
                receipt = await host.verify_budget_snapshot(snapshot, signal)
                after = _checked_snapshot(snapshot, frame, config['task'], evidence)
                if before != after or receipt.get('basis') != snapshot['basis']:
                    raise ValueError('budget snapshot changed during verification')
                record(frame, 'budget-snapshot-checked', snapshot_ref=before)
                reply = {'kind': 'verify_budget_reply', **base, 'receipt': receipt}
            elif kind == 'verify_stop_request':
                snapshot = body(frame, 'snapshot')
                before = _checked_stop_snapshot(snapshot, frame, config['task'], evidence)
                if 'snapshot_ref' in frame:
                    _budget_file(snapshot, evidence, f"run-{snapshot['basis']['run_number']}-stop-snapshot.json", frame['snapshot_ref'])
                receipt = await host.verify_stop_snapshot(snapshot, signal)
                after = _checked_stop_snapshot(snapshot, frame, config['task'], evidence)
                if before != after or receipt.get('basis') != snapshot['basis']:
                    raise ValueError('stop snapshot changed during verification')
                record(frame, 'stop-snapshot-checked', snapshot_ref=before)
                reply = {'kind': 'verify_stop_reply', **base, 'receipt': receipt}
            elif kind == 'prepare_stop_request':
                continuation = body(frame, 'stop_continuation')
                check = lambda: _checked_budget_continuation(continuation, frame, config['task'], evidence,
                                                             _checked_stop_snapshot, 'jsonl-stop-continuation')
                before = check()
                if 'stop_continuation_ref' in frame:
                    _budget_file(continuation, evidence, f"jsonl-stop-continuation-{continuation['next_run_number']}.json", frame['stop_continuation_ref'])
                prepared = await host.prepare_stop_continuation(continuation, signal)
                if before != check():
                    raise ValueError('stop continuation changed during preparation')
                record(frame, 'stop-continuation-checked', snapshot_ref=before[0], continuation_ref=before[1])
                if set(prepared) != {'run_id_next', 'objective'}:
                    raise ValueError('prepare may only change Run ID and objective')
                if (not isinstance(prepared['run_id_next'], str) or not prepared['run_id_next'].strip()
                        or prepared['run_id_next'] in allowed_runs or not isinstance(prepared['objective'], str)
                        or not prepared['objective'].strip()):
                    raise ValueError('new Run identity and nonempty correction objective are required')
                allowed_runs.add(prepared['run_id_next'])
                reply = {'kind': 'prepare_stop_reply', **base, **prepared}
            else:
                budget = kind == 'prepare_budget_request'
                continuation = body(frame, 'budget_continuation' if budget else 'continuation')
                if budget:
                    before = _checked_budget_continuation(continuation, frame, config['task'], evidence)
                    if 'budget_continuation_ref' in frame:
                        _budget_file(continuation, evidence, f"jsonl-budget-continuation-{continuation['next_run_number']}.json", frame['budget_continuation_ref'])
                    prepared = await host.prepare_budget_stop_continuation(continuation, signal)
                    if before != _checked_budget_continuation(continuation, frame, config['task'], evidence):
                        raise ValueError('budget continuation changed during preparation')
                    record(frame, 'budget-continuation-checked', snapshot_ref=before[0], continuation_ref=before[1])
                else:
                    validate_candidate_identity(continuation.get('previous'), frame, config['task'])
                    prepared = await host.prepare(continuation, signal)
                if set(prepared) != {'run_id_next', 'objective'}:
                    raise ValueError('prepare may only change Run ID and objective')
                if (not isinstance(prepared['run_id_next'], str) or not prepared['run_id_next'].strip()
                        or prepared['run_id_next'] in allowed_runs or not isinstance(prepared['objective'], str)
                        or not prepared['objective'].strip()):
                    raise ValueError('new Run identity and nonempty correction objective are required')
                allowed_runs.add(prepared['run_id_next'])
                reply = {'kind': 'prepare_budget_reply' if budget else 'prepare_reply', **base, **prepared}
            record(frame, 'host-returned', settlement=reply.get('result', {}).get('settlement', 'HostReceipt'),
                   operation_receipt=reply.get('result', {}).get('operation_receipt'))
            await send(reply)
        except Exception:
            failure_receipt = record(frame, 'failed-or-unknown', effects='unknown_unless_original_host_receipt_settles')
            if kind == 'tool_call':
                # Exceptions may follow partial writes: never assume no effect.
                try:
                    await send({'kind': 'tool_reply', **base,
                        **{key: frame[key] for key in ('native_call_id', 'tool_name', 'args_sha256')},
                        'result': {'value': None, 'is_error': True, 'settlement': 'Unknown',
                                   'operation_receipt': failure_receipt}})
                    return
                except Exception:
                    pass
            try:
                await send({'kind': 'cancel', 'version': 1, 'task_id': task_id})
            except Exception:
                pass
            raise ReconciliationRequired('Host callback failed; inspect original operation receipts') from None

    async def reconcile(frame, signal):
        try:
            review = body(frame, 'review')
            reply = await reconcile_reply(host, frame, review, signal, config['task']['deadline_unix_ms'], grace)
        except Exception as error:
            reconcile_failure.append(error.category if isinstance(error, _ReconcileFailure) else 'reconcile_request_invalid')
            record(frame, 'failed-or-unknown', effects='reconcile_not_replied')
            if not reconcile_cancel_sent:
                reconcile_cancel_sent.append(True)
                try:
                    await send({'kind': 'cancel', 'version': 1, 'task_id': task_id})
                except Exception:
                    pass
            raise _ReconcileFailure(reconcile_failure[0]) from None
        record(frame, 'host-returned', settlement='HostReceipt', operation_receipt=None)
        await send(reply)

    async def observe_event(frame):
        try:
            result = on_event(frame)
            if inspect.isawaitable(result):
                await result
        except Exception:
            pass  # UI observation is not Core evidence or Task authority.

    stderr_task = asyncio.create_task(discard_stderr())
    cancel_task = asyncio.create_task(watch_cancel())
    terminal = None
    try:
        async with send_lock:
            proc.stdin.write(start_line)
            await asyncio.wait_for(proc.stdin.drain(), timeout=5)
        deadline_seconds = max(0, config['task']['deadline_unix_ms'] / 1000 - time.time())
        async with asyncio.timeout(deadline_seconds + grace + 20):
            while True:
                raw = await proc.stdout.readline()
                if not raw or len(raw) > FRAME_BYTES or not raw.endswith(b'\n'):
                    raise ReconciliationRequired('runtime disconnected or sent an incomplete/oversized frame')
                frame = json.loads(raw.decode('utf-8'))
                if frame.get('version') != 1 or frame.get('task_id') != task_id:
                    raise ReconciliationRequired('runtime frame version or Task identity mismatch')
                kind = frame.get('kind')
                if kind == 'mcp_status':
                    if not mcp_enabled or set(frame) != {'kind', 'version', 'task_id', 'servers', 'excluded'}:
                        raise ReconciliationRequired('unsolicited or invalid mcp_status frame')
                    if on_event is not None and len(ui_tasks) < 16:
                        item = asyncio.create_task(observe_event(frame))
                        ui_tasks.add(item)
                        item.add_done_callback(ui_tasks.discard)
                    continue
                if kind == 'event':
                    if on_event is not None and len(ui_tasks) < 16:
                        item = asyncio.create_task(observe_event(frame))
                        ui_tasks.add(item)
                        item.add_done_callback(ui_tasks.discard)
                    continue
                if kind == 'callback_cancel':
                    current = pending.get(frame.get('rpc_id'))
                    if current is None or frame['run_id'] != callback_runs.get(frame.get('rpc_id')):
                        raise ReconciliationRequired('cancel targets an unknown Host callback')
                    if not current[1].done():
                        current[0].set()
                    continue
                if kind == 'task_result':
                    terminal = read_reference(frame['outcome_ref'], evidence)
                    for field in ('task_id', 'stop', 'task_accepted', 'runs', 'observed_calls',
                                  'fence_presence', 'reconciliation_fence'):
                        if frame[field] != terminal[field]:
                            raise ReconciliationRequired('terminal summary differs from its original Task artifact')
                    if terminal['task_accepted'] is not False:
                        raise ReconciliationRequired('the shared runtime cannot accept a business Task')
                    if 'budget_review_ref' in frame:
                        if not budget_enabled:
                            raise ReconciliationRequired('unsolicited budget review reference')
                        review = read_budget_review_reference(frame['budget_review_ref'], evidence, config['task'], terminal)
                        with (operations / 'budget-review.json').open('x', encoding='utf-8') as stream:
                            json.dump({'budget_review_ref': frame['budget_review_ref'], 'record': review}, stream, ensure_ascii=False)
                            stream.write('\n')
                            stream.flush()
                            os.fsync(stream.fileno())
                    break
                if kind == 'reconcile_request':
                    # Only with `resume`, only once per run_task (11.1, 11.5); the callback is not called otherwise.
                    rpc_id = frame.get('rpc_id')
                    if not resume_enabled or any(entry_kind == 'reconcile_request' for entry_kind in callback_kinds.values()):
                        reconcile_failure.append('reconcile_request_unexpected')
                        raise ReconciliationRequired('reconcile_request_unexpected')
                    header = {'kind', 'version', 'rpc_id', 'task_id', 'run_id'}
                    if (set(frame) not in (header | {'review'}, header | {'review_ref'}) or not isinstance(rpc_id, str)
                            or not rpc_id or rpc_id in seen or frame.get('run_id') != first_run):
                        reconcile_failure.append('reconcile_request_invalid')
                        raise ReconciliationRequired('reconcile_request_invalid')
                    seen.add(rpc_id)
                    callback_runs[rpc_id] = frame['run_id']
                    callback_kinds[rpc_id] = kind
                    record(frame, 'started', external_effects='not_yet_settled')
                    signal = asyncio.Event()
                    pending[rpc_id] = (signal, asyncio.create_task(reconcile(frame, signal)))
                    continue
                if kind not in ('tool_call', 'verify_request', 'prepare_request', 'verify_budget_request', 'prepare_budget_request',
                                'verify_stop_request', 'prepare_stop_request'):
                    raise ReconciliationRequired('unexpected runtime control frame')
                if kind in ('verify_budget_request', 'prepare_budget_request') and not budget_enabled:
                    raise ReconciliationRequired('budget callback without explicit opt-in')
                if kind in ('verify_stop_request', 'prepare_stop_request') and not length_enabled:
                    raise ReconciliationRequired('stop snapshot callback without explicit opt-in')
                rpc_id = frame.get('rpc_id')
                if not isinstance(rpc_id, str) or not rpc_id or rpc_id in seen or frame.get('run_id') not in allowed_runs:
                    raise ReconciliationRequired('duplicate callback or unknown Run identity')
                if kind == 'tool_call' and frame.get('tool_name') not in allowed_tools:
                    raise ReconciliationRequired('runtime requested a tool outside the initial registry')
                seen.add(rpc_id)
                callback_runs[rpc_id] = frame['run_id']
                record(frame, 'started', external_effects='not_yet_settled')
                signal = asyncio.Event()
                task = asyncio.create_task(callback(frame, signal))
                pending[rpc_id] = (signal, task)
            active = {task for _, task in pending.values() if not task.done()}
            if active:
                _, unjoined = await asyncio.wait(active, timeout=grace)
                if unjoined:
                    raise ReconciliationRequired('external Host callbacks still pending; original effects are unknown')
            for _, task in pending.values():
                if task.done():
                    task.result()
            code = await asyncio.wait_for(proc.wait(), timeout=5)
            if code != 0:
                raise ReconciliationRequired('runtime exit failed; inspect the persisted Task outcome')
    except (asyncio.CancelledError, Exception):
        for signal, _ in pending.values():
            signal.set()
        if not reconcile_cancel_sent:
            # A refused settlement has already asked the sidecar to stop once.
            reconcile_cancel_sent.append(True)
            try:
                await send({'kind': 'cancel', 'version': 1, 'task_id': task_id})
            except Exception:
                pass
        # No automatic replay. A process watchdog ending the Rust peer cannot
        # establish that an external Host operation has stopped or rolled back.
        active = {task for _, task in pending.values() if not task.done()}
        if active:
            await asyncio.wait(active, timeout=grace)
        for _, task in pending.values():
            if task.done() and not task.cancelled():
                task.exception()  # Retrieved here; the error below names the failure.
        if reconcile_failure:
            raise ReconciliationRequired(f'{reconcile_failure[0]}: resume refused; the fence and original '
                                         'receipts are unchanged') from None
        raise ReconciliationRequired('Task transport/callback interrupted; query original receipts before continuing') from None
    finally:
        cancel_task.cancel()
        await asyncio.gather(cancel_task, return_exceptions=True)
        if proc.stdin is not None:
            proc.stdin.close()
        try:
            await asyncio.wait_for(proc.wait(), timeout=grace + 5)
        except asyncio.TimeoutError:
            proc.kill()
            await proc.wait()
        await asyncio.gather(stderr_task, return_exceptions=True)
        for task in ui_tasks:
            task.cancel()
        if ui_tasks:
            await asyncio.gather(*ui_tasks, return_exceptions=True)
    # 23.2/23.7: reject an invalid record; the caller still gets the original outcome dict.
    parse_unknown_effects(terminal)
    return terminal
