// JSONL v1 transport only: no Agent loop, effect replay, Session replacement or Task acceptance.
import { spawn } from 'node:child_process';
import { createHash } from 'node:crypto';
import { promises as fs } from 'node:fs';
import path from 'node:path';
import { TextDecoder, isDeepStrictEqual } from 'node:util';

export const VERSION = 1;
export const FRAME_BYTES = 262144;
const RESULT_BYTES = 131072;
const hash = (bytes) => createHash('sha256').update(bytes).digest('hex');
const digest = (value) => hash(Buffer.from(JSON.stringify(value)));
const object = (value) => value !== null && typeof value === 'object' && !Array.isArray(value);
const requireValue = (condition, message) => { if (!condition) throw new Error(message); };
const exact = (value, keys) => object(value) && Object.keys(value).length === keys.length
  && keys.every((key) => Object.hasOwn(value, key));
const sha = (value) => typeof value === 'string' && /^[0-9a-f]{64}$/.test(value);
const nonempty = (value) => typeof value === 'string' && value.trim().length > 0;
const operationalPath = (value) => process.platform === 'win32'
  ? value.replace(/^\\\\\?\\UNC\\/i, '\\\\').replace(/^\\\\\?\\/, '') : value;
const inside = (root, file) => {
  const relative = path.relative(operationalPath(root), operationalPath(file));
  return relative !== '' && relative !== '..' && !relative.startsWith(`..${path.sep}`) && !path.isAbsolute(relative);
};

/** A ref is a byte/hash proof, not a capability or authorization declaration. */
export async function readCheckedJsonReference(ref, evidenceDirectory, maxBytes = 16 * 1024 * 1024) {
  requireValue(exact(ref, ['path', 'sha256']) && path.isAbsolute(ref.path) && sha(ref.sha256), 'invalid JSON reference');
  const root = await fs.realpath(evidenceDirectory);
  const lexical = path.resolve(ref.path);
  requireValue(inside(path.resolve(evidenceDirectory), lexical), 'reference is outside original evidence directory');
  requireValue(!(await fs.lstat(lexical)).isSymbolicLink(), 'reference symlink refused');
  const real = await fs.realpath(lexical);
  requireValue(inside(root, real), 'reference real path escaped original evidence directory');
  const handle = await fs.open(real, 'r');
  try {
    const stat = await handle.stat();
    requireValue(stat.isFile() && stat.size <= maxBytes, 'reference is not a bounded regular file');
    const parts = []; let size = 0;
    while (true) {
      const chunk = Buffer.alloc(Math.min(65536, maxBytes + 1 - size));
      const { bytesRead } = await handle.read(chunk, 0, chunk.length, null);
      if (!bytesRead) break;
      size += bytesRead; requireValue(size <= maxBytes, 'reference grew beyond byte bound');
      parts.push(chunk.subarray(0, bytesRead));
    }
    const bytes = Buffer.concat(parts, size);
    requireValue(bytes.length <= maxBytes && hash(bytes) === ref.sha256, 'reference bytes/hash mismatch');
    return JSON.parse(new TextDecoder('utf-8', { fatal: true }).decode(bytes));
  } finally { await handle.close(); }
}

class Receipts {
  static async create(directory, taskId, startDigest) {
    requireValue(path.isAbsolute(directory), 'absolute fresh operationDirectory required');
    await fs.mkdir(directory); // The Host chooses a fresh sibling, not Rust's new evidence_dir.
    const receipts = new Receipts(directory);
    await receipts.write('client-started', { task_id: taskId, start_sha256: startDigest });
    return receipts;
  }
  constructor(directory) { this.directory = directory; }
  async write(name, value) {
    const target = path.join(this.directory, `${name}.json`);
    const handle = await fs.open(target, 'wx');
    try { await handle.writeFile(`${JSON.stringify(value, null, 2)}\n`); await handle.sync(); }
    finally { await handle.close(); }
    return target;
  }
  async operation(frame, stage, extra = {}) {
    // Hash payloads rather than logging business content or credentials. Product
    // callbacks own the authoritative operation_receipt and its original body.
    return this.write(`rpc-${hash(frame.rpc_id)}-${stage}`, {
      rpc_id: frame.rpc_id, task_id: frame.task_id, run_id: frame.run_id,
      kind: frame.kind, request_sha256: digest(frame), ...extra,
    });
  }
}

const envelope = (frame, kind) => ({ kind, version: VERSION, rpc_id: frame.rpc_id,
  task_id: frame.task_id, run_id: frame.run_id });
const toolUnknown = (frame, receipt) => ({ ...envelope(frame, 'tool_reply'),
  native_call_id: frame.native_call_id, tool_name: frame.tool_name, args_sha256: frame.args_sha256,
  result: { value: { error: 'Host callback outcome unknown; inspect original receipts; no replay' },
    is_error: true, settlement: 'Unknown', operation_receipt: receipt } });

function validateToolResult(result) {
  requireValue(exact(result, ['value', 'is_error', 'settlement', 'operation_receipt']), 'invalid tool callback result fields');
  requireValue(typeof result.is_error === 'boolean' && ['Settled', 'Unknown'].includes(result.settlement)
    && nonempty(result.operation_receipt), 'invalid tool callback settlement');
  requireValue(Buffer.byteLength(JSON.stringify(result)) <= RESULT_BYTES, 'tool result exceeds protocol bound');
}
function validateReceipt(receipt, candidate) {
  requireValue(exact(receipt, ['basis', 'committed', 'settlement', 'verdict', 'evidence']), 'invalid review receipt fields');
  requireValue(digest(receipt.basis) === digest(candidate.basis), 'review basis changed');
  requireValue(typeof receipt.committed === 'boolean' && ['Settled', 'Unknown'].includes(receipt.settlement)
    && Array.isArray(receipt.evidence) && receipt.evidence.every(nonempty), 'invalid review settlement/evidence');
  const verdict = receipt.verdict;
  requireValue(verdict === 'Verified' || verdict === 'Unknown'
    || (exact(verdict, ['NeedsChanges']) && exact(verdict.NeedsChanges, ['feedback'])
      && typeof verdict.NeedsChanges.feedback === 'string'), 'invalid Harness verdict encoding');
}

function budgetTermination(value) {
  requireValue(exact(value, ['version','cause','native_round_closed','model_final_stop'])
    && value.version === 1 && value.cause === 'call_budget_exhausted'
    && value.native_round_closed === true && value.model_final_stop === false, 'invalid closed budget termination');
}

function budgetReceipt(receipt, snapshot) {
  requireValue(isDeepStrictEqual(receipt?.basis, snapshot?.basis), 'budget review basis changed');
  // Reuse the ordinary schema checks after the order-independent identity check.
  validateReceipt(receipt, { basis: receipt.basis });
}

async function budgetFile(value, evidenceDirectory, name, reference = null) {
  const file = path.join(evidenceDirectory, name);
  requireValue(!(await fs.lstat(file)).isSymbolicLink(), 'budget record symlink refused');
  const real = await fs.realpath(file); const stat = await fs.stat(real);
  requireValue(stat.isFile() && stat.size <= 16 * 1024 * 1024, 'budget record is not a bounded file');
  requireValue(inside(await fs.realpath(evidenceDirectory), real), 'budget record escaped original evidence directory');
  const handle = await fs.open(real, 'r');
  const parts = []; let size = 0;
  try {
    while (true) {
      const part = Buffer.alloc(Math.min(65536, 16 * 1024 * 1024 + 1 - size));
      const { bytesRead } = await handle.read(part, 0, part.length, null);
      if (!bytesRead) break;
      size += bytesRead;
      requireValue(size <= 16 * 1024 * 1024, 'budget record grew beyond bound');
      parts.push(part.subarray(0, bytesRead));
    }
  } finally { await handle.close(); }
  const bytes = Buffer.concat(parts, size);
  const actual = { path: real, sha256: hash(bytes) };
  if (reference) requireValue(exact(reference, ['path','sha256'])
    && operationalPath(await fs.realpath(reference.path)) === operationalPath(real)
    && reference.sha256 === actual.sha256, 'budget reference differs from its fixed original');
  const stored = await readCheckedJsonReference(actual, evidenceDirectory);
  requireValue(isDeepStrictEqual(stored, value), 'budget record differs from callback payload');
  return actual;
}

async function checkedBudgetSnapshot(snapshot, frame, task, evidenceDirectory) {
  requireValue(exact(snapshot, ['basis','diagnostic_output','goal','termination']), 'invalid budget snapshot fields');
  const basis = snapshot.basis;
  requireValue(object(basis) && basis.task_id === task.task_id && basis.run_id === frame.run_id
    && basis.criteria_revision === task.criteria_revision && basis.authorization_revision === task.authorization_revision
    && Number.isSafeInteger(basis.run_number) && basis.run_number > 0
    && typeof snapshot.diagnostic_output === 'string', 'budget snapshot immutable identity mismatch');
  budgetTermination(snapshot.termination);
  return budgetFile(snapshot, evidenceDirectory, `run-${basis.run_number}-budget-snapshot.json`);
}

async function checkedBudgetContinuation(value, frame, task, evidenceDirectory) {
  requireValue(exact(value, ['previous','receipt','next_run_number','remaining_calls','deadline_unix_ms']), 'invalid budget continuation fields');
  const snapshotRef = await checkedBudgetSnapshot(value.previous, frame, task, evidenceDirectory);
  budgetReceipt(value.receipt, value.previous);
  requireValue(value.receipt.committed === true && value.receipt.settlement === 'Settled'
    && object(value.receipt.verdict) && object(value.receipt.verdict.NeedsChanges), 'budget continuation requires committed rejection');
  requireValue(value.next_run_number === value.previous.basis.run_number + 1
    && Number.isSafeInteger(value.remaining_calls) && value.remaining_calls > 0 && value.remaining_calls <= task.max_calls
    && value.deadline_unix_ms === task.deadline_unix_ms, 'budget continuation changed original Task bounds');
  const continuationRef = await budgetFile(value, evidenceDirectory, `jsonl-budget-continuation-${value.next_run_number}.json`);
  return { snapshotRef, continuationRef };
}

/** A closed Length stop snapshot (16.2); its output is diagnostic only. */
async function checkedStopSnapshot(snapshot, frame, task, evidenceDirectory) {
  requireValue(exact(snapshot, ['basis','diagnostic_output','goal','evidence','truncated']), 'invalid stop snapshot fields');
  const basis = snapshot.basis;
  requireValue(object(basis) && basis.task_id === task.task_id && basis.run_id === frame.run_id
    && basis.criteria_revision === task.criteria_revision && basis.authorization_revision === task.authorization_revision
    && Number.isSafeInteger(basis.run_number) && basis.run_number >= 1
    && typeof snapshot.diagnostic_output === 'string' && object(snapshot.evidence), 'stop snapshot immutable identity mismatch');
  return budgetFile(snapshot, evidenceDirectory, `run-${basis.run_number}-stop-snapshot.json`);
}

async function checkedStopContinuation(value, frame, task, evidenceDirectory) {
  requireValue(exact(value, ['previous','receipt','next_run_number','remaining_calls','deadline_unix_ms']), 'invalid stop continuation fields');
  const snapshotRef = await checkedStopSnapshot(value.previous, frame, task, evidenceDirectory);
  budgetReceipt(value.receipt, value.previous);
  requireValue(value.receipt.committed === true && value.receipt.settlement === 'Settled'
    && object(value.receipt.verdict) && object(value.receipt.verdict.NeedsChanges), 'stop continuation requires committed rejection');
  requireValue(value.next_run_number === value.previous.basis.run_number + 1
    && Number.isSafeInteger(value.remaining_calls) && value.remaining_calls > 0 && value.remaining_calls <= task.max_calls
    && value.deadline_unix_ms === task.deadline_unix_ms, 'stop continuation changed original Task bounds');
  const continuationRef = await budgetFile(value, evidenceDirectory, `jsonl-stop-continuation-${value.next_run_number}.json`);
  return { snapshotRef, continuationRef };
}

/**
 * Typed, frozen view of `outcome.unknown_effects` in original order (23.3).
 * An invalid record throws an Error whose `field` names the bad field.
 */
export function parseUnknownEffects(outcome) {
  const invalid = (field) => Object.assign(new Error(`invalid unknown effect record: ${field}`), { field });
  if (!object(outcome) || !Object.hasOwn(outcome, 'unknown_effects')) return Object.freeze([]);
  if (!Array.isArray(outcome.unknown_effects)) throw invalid('unknown_effects');
  return Object.freeze(outcome.unknown_effects.map((record) => {
    if (!object(record)) throw invalid('record');
    const extra = Object.keys(record).find((key) => !['run_id','native_call_id','tool','receipt_ref'].includes(key));
    if (extra !== undefined) throw invalid(extra);
    for (const field of ['run_id', 'native_call_id', 'tool']) {
      if (typeof record[field] !== 'string' || record[field].length === 0) throw invalid(field);
    }
    const receipt = record.receipt_ref;
    if (!object(receipt) || !object(receipt.evidence) || Object.keys(receipt.evidence).length === 0) throw invalid('receipt_ref');
    return Object.freeze({ run_id: record.run_id, native_call_id: record.native_call_id, tool: record.tool,
      receipt_ref: structuredClone(receipt) });
  }));
}

const reconcileError = (category) => Object.assign(new Error(category), { category });

/**
 * Build the `reconcile_reply` for a `reconcileResume` return value (12.1, 12.2).
 * Exact `ResumeReceipt` fields and types, whole frame within FRAME_BYTES;
 * otherwise throws an Error whose message and `category` are `reconcile_receipt_invalid`.
 */
export function reconcileReplyFor(frame, receipt) {
  const settlement = (value) => object(value) && (value.kind === 'not_executed' ? exact(value, ['kind'])
    : ['completed', 'failed'].includes(value.kind) && exact(value, ['kind', 'output']) && typeof value.output === 'string');
  const valid = exact(receipt, ['committed', 'settlement', 'tool_calls', 'evidence'])
    && typeof receipt.committed === 'boolean' && ['Settled', 'Unknown'].includes(receipt.settlement)
    && Array.isArray(receipt.evidence) && receipt.evidence.every((item) => typeof item === 'string')
    && Array.isArray(receipt.tool_calls) && receipt.tool_calls.every((call) => exact(call, ['call_id', 'settlement'])
      && typeof call.call_id === 'string' && settlement(call.settlement));
  if (!valid) throw reconcileError('reconcile_receipt_invalid');
  const reply = { ...envelope(frame, 'reconcile_reply'), receipt };
  let bytes;
  try { bytes = Buffer.byteLength(`${JSON.stringify(reply)}\n`); } catch { throw reconcileError('reconcile_receipt_invalid'); }
  if (bytes > FRAME_BYTES) throw reconcileError('reconcile_receipt_invalid');
  return reply;
}

/**
 * Call `reconcileResume` once and build its reply. A thrown callback becomes
 * `reconcile_callback_threw`; the original message is never kept.
 */
export async function settleReconcile(reconcileResume, frame, review, signal) {
  let receipt;
  try { receipt = await reconcileResume(review, { signal, frame }); }
  catch { throw reconcileError('reconcile_callback_threw'); }
  return reconcileReplyFor(frame, receipt);
}

/** Read the separate committed budget-review index without changing outcome. */
export async function readBudgetReviewReference(reference, evidenceDirectory, task, outcome) {
  const record = await readCheckedJsonReference(reference, evidenceDirectory);
  requireValue(exact(record, ['version','kind','snapshot','receipt']) && record.version === 1
    && record.kind === 'closed_budget_snapshot_review', 'invalid budget review index');
  requireValue(outcome.task_id === task.task_id && outcome.task_accepted === false && outcome.last_candidate === null
    && isDeepStrictEqual(record.receipt, outcome.last_receipt), 'budget index is not the current outcome receipt');
  const basis = record.receipt.basis;
  const snapshot = await readCheckedJsonReference(record.snapshot, evidenceDirectory);
  budgetReceipt(record.receipt, snapshot);
  requireValue(record.receipt.committed === true && record.receipt.settlement === 'Settled'
    && record.receipt.verdict !== 'Unknown', 'budget index is not committed and settled');
  const snapshotRef = await checkedBudgetSnapshot(snapshot, { run_id: basis.run_id }, task, evidenceDirectory);
  requireValue(snapshotRef.sha256 === record.snapshot.sha256, 'budget index snapshot hash changed');
  await budgetFile(snapshot, evidenceDirectory, `run-${basis.run_number}-budget-snapshot.json`, record.snapshot);
  await budgetFile(record, evidenceDirectory, `run-${basis.run_number}-budget-review-index.json`, reference);
  return record;
}

/**
 * Spawn one hidden sidecar, submit one start_task, then service its original RPCs.
 * callbacks.tool(frame, {signal}), verify(candidate, {signal,frame}) and
 * prepare(continuation, {signal,frame}) must settle their own external effects.
 * A thrown tool callback is Unknown, not a known no-effect rejection.
 * With `start.resume`, callbacks.reconcileResume(review, {signal,frame}) is
 * required (else this throws `reconcile_callback_missing` before spawn) and
 * is called once; a refused settlement sets `fault` to its category.
 */
export async function startTask({ binary, start, modelToken, operationDirectory,
  callbacks, onEvent = () => {}, env = {}, binaryArgs = [] }) {
  requireValue(path.isAbsolute(binary) && (await fs.stat(binary)).isFile(), 'absolute sidecar executable required');
  requireValue(Array.isArray(binaryArgs) && binaryArgs.length <= 16 && binaryArgs.every((arg) => typeof arg === 'string' && arg.length <= 4096), 'bounded Host-selected executable arguments required');
  requireValue(object(start) && start.kind === 'start_task' && start.version === VERSION
    && object(start.task) && object(start.run), 'invalid start_task envelope');
  requireValue(nonempty(start.task.task_id) && nonempty(start.run.run_id)
    && path.isAbsolute(start.task.evidence_dir), 'invalid original task identity/evidence directory');
  requireValue(nonempty(start.task.criteria_revision) && nonempty(start.task.authorization_revision), 'original criteria and authorization revisions required');
  requireValue(Array.isArray(start.run.tools) && start.run.tools.every((tool) => object(tool) && nonempty(tool.name)), 'initial Host tool registry required');
  const allowedTools = new Set(start.run.tools.map((tool) => tool.name));
  requireValue(allowedTools.size === start.run.tools.length, 'duplicate initial tool names');
  requireValue(nonempty(modelToken), 'model credential required through fixed child environment only');
  requireValue(!Object.hasOwn(start.run, 'token') && !Object.hasOwn(start.run, 'api_key')
    && !Object.hasOwn(start.run, 'apiKey'), 'credentials must not be sent over stdin');
  requireValue(object(callbacks) && ['tool', 'verify', 'prepare'].every((key) => typeof callbacks[key] === 'function'), 'all Host callbacks required');
  const budgetPolicy = start.budget_stop_review ?? 'reject';
  requireValue(!Object.hasOwn(start, 'budget_stop_review') || ['reject','review_closed_tool_round'].includes(start.budget_stop_review), 'invalid budget_stop_review policy');
  const budgetEnabled = budgetPolicy === 'review_closed_tool_round';
  requireValue(!budgetEnabled || (start.run.mode === 'execute'
    && ['verify_budget_snapshot','prepare_budget_stop_continuation'].every((key) => typeof callbacks[key] === 'function')),
  'Execute budget opt-in requires both dedicated callbacks');
  requireValue(!Object.hasOwn(start, 'length_stop_review') || ['reject','review_closed_length'].includes(start.length_stop_review), 'invalid length_stop_review policy');
  const lengthEnabled = start.length_stop_review === 'review_closed_length';
  requireValue(!lengthEnabled || (start.run.mode === 'execute'
    && ['verifyStopSnapshot','prepareStopContinuation'].every((key) => typeof callbacks[key] === 'function')),
  'Execute length-stop opt-in requires both dedicated callbacks');
  const mcpEnabled = Object.hasOwn(start, 'mcp_servers');
  // 12.1: `resume` needs the settlement callback; without `resume` it is optional and unused.
  const resumeEnabled = Object.hasOwn(start, 'resume');
  if (resumeEnabled && typeof callbacks.reconcileResume !== 'function') throw new Error('reconcile_callback_missing');
  let reconcileSeen = false;
  requireValue(Buffer.byteLength(`${JSON.stringify(start)}\n`) <= FRAME_BYTES, 'start frame exceeds protocol bound');
  const grace = start.cancel_grace_ms ?? 5000;
  requireValue(Number.isInteger(grace) && grace >= 1000 && grace <= 15000, 'invalid cancellation grace');
  requireValue(Number.isSafeInteger(start.task.deadline_unix_ms), 'integer original deadline required');
  const receipts = await Receipts.create(operationDirectory, start.task.task_id, digest(start));
  const childEnv = {};
  // Windows basics that child tools and MCP servers need to resolve profile
  // and ProgramData folders; none are secrets.
  for (const key of ['PATH', 'SYSTEMROOT', 'SystemRoot', 'TEMP', 'TMP', 'HOME', 'USERPROFILE', 'LANG', 'LC_ALL',
    'SystemDrive', 'ProgramData', 'ALLUSERSPROFILE', 'LOCALAPPDATA', 'APPDATA', 'PATHEXT', 'ComSpec']) {
    const value = env[key] ?? process.env[key]; if (typeof value === 'string') childEnv[key] = value;
  }
  childEnv.ARA_AGENT_MODEL_TOKEN = modelToken;
  const child = spawn(binary, binaryArgs, { windowsHide: true, stdio: ['pipe', 'pipe', 'pipe'], env: childEnv });
  const state = { phase: 'running', cancelled: false, forced: false, fault: null, terminal: null,
    exit_code: null, exit_signal: null, closed: false, pending_callbacks: 0 };
  const active = new Map(); const seen = new Set(); const startedRuns = new Set([start.run.run_id]);
  const journalTasks = []; const stderrHash = createHash('sha256');
  let stderrPreview = Buffer.alloc(0); let stderrBytes = 0; let buffered = Buffer.alloc(0);
  let queuedBytes = 0; let queue = Promise.resolve(); let terminalCount = 0;
  const persist = (promise) => { journalTasks.push(promise); promise.catch(() => { state.fault ??= 'client_receipt_write_failed'; }); return promise; };
  const redacted = (text) => String(text).split(modelToken).join('[redacted]');
  const send = (frame) => {
    const bytes = Buffer.from(`${JSON.stringify(frame)}\n`);
    if (bytes.length > FRAME_BYTES || queuedBytes + bytes.length > FRAME_BYTES * 8) return Promise.reject(new Error('control write byte bound'));
    queuedBytes += bytes.length;
    const sent = queue.then(() => new Promise((resolve, reject) => {
      if (state.closed || child.stdin.destroyed) return reject(new Error('sidecar stdin closed'));
      child.stdin.write(bytes, (error) => error ? reject(error) : resolve());
    })).finally(() => { queuedBytes -= bytes.length; });
    queue = sent.catch(() => {});
    return sent;
  };
  const abortCallbacks = () => { for (const item of active.values()) item.controller.abort(); };
  const cancel = () => {
    if (state.cancelled || state.closed) return;
    state.cancelled = true; abortCallbacks();
    send({ kind: 'cancel', version: VERSION, task_id: start.task.task_id }).catch(() => { state.fault ??= 'cancel_delivery_unknown'; });
  };
  const forceKill = () => {
    state.forced = true; state.fault ??= 'forced_process_termination'; abortCallbacks();
    if (!state.closed) child.kill('SIGKILL'); // No claim about descendants or callback rollback.
  };
  const fault = (category) => { state.fault ??= category; cancel(); };
  const correlation = (frame) => {
    requireValue(frame.version === VERSION && frame.task_id === start.task.task_id
      && nonempty(frame.rpc_id) && nonempty(frame.run_id) && startedRuns.has(frame.run_id), 'RPC correlation mismatch');
  };
  const candidateOf = async (frame, key) => {
    const refKey = `${key}_ref`;
    requireValue(Object.hasOwn(frame, key) !== Object.hasOwn(frame, refKey), 'exactly one inline/reference payload required');
    return Object.hasOwn(frame, key) ? frame[key] : readCheckedJsonReference(frame[refKey], start.task.evidence_dir);
  };
  const checkBasisIdentity = (basis, frame) => {
    requireValue(object(basis) && basis.task_id === frame.task_id && basis.run_id === frame.run_id
      && basis.criteria_revision === start.task.criteria_revision
      && basis.authorization_revision === start.task.authorization_revision, 'candidate immutable identity mismatch');
  };
  const executeCallback = async (item) => {
    const frame = item.frame; let reply; let stage = 'settled';
    const started = await receipts.operation(frame, 'started');
    try {
      if (frame.kind === 'tool_call') {
        const result = await callbacks.tool(frame, { signal: item.controller.signal });
        validateToolResult(result);
        reply = { ...envelope(frame, 'tool_reply'), native_call_id: frame.native_call_id,
          tool_name: frame.tool_name, args_sha256: frame.args_sha256, result };
        if (result.settlement === 'Unknown') stage = 'unknown';
      } else if (frame.kind === 'verify_request') {
        const candidate = await candidateOf(frame, 'candidate');
        checkBasisIdentity(candidate?.basis, frame);
        item.candidate = candidate;
        const receipt = await callbacks.verify(candidate, { signal: item.controller.signal, frame });
        validateReceipt(receipt, candidate);
        reply = { ...envelope(frame, 'verify_reply'), receipt };
        if (receipt.settlement === 'Unknown') stage = 'unknown';
      } else if (frame.kind === 'verify_budget_request') {
        const snapshot = await candidateOf(frame, 'snapshot');
        const before = await checkedBudgetSnapshot(snapshot, frame, start.task, start.task.evidence_dir);
        if (frame.snapshot_ref) await budgetFile(snapshot, start.task.evidence_dir,
          `run-${snapshot.basis.run_number}-budget-snapshot.json`, frame.snapshot_ref);
        item.budgetSnapshot = snapshot;
        const receipt = await callbacks.verify_budget_snapshot(snapshot, { signal: item.controller.signal, frame });
        budgetReceipt(receipt, snapshot);
        const after = await checkedBudgetSnapshot(snapshot, frame, start.task, start.task.evidence_dir);
        requireValue(digest(before) === digest(after), 'budget snapshot changed during verification');
        await receipts.operation(frame, 'budget-snapshot-checked', { snapshot_ref: before });
        reply = { ...envelope(frame, 'verify_budget_reply'), receipt };
        if (receipt.settlement === 'Unknown') stage = 'unknown';
      } else if (frame.kind === 'verify_stop_request') {
        const snapshot = await candidateOf(frame, 'snapshot');
        const before = await checkedStopSnapshot(snapshot, frame, start.task, start.task.evidence_dir);
        if (frame.snapshot_ref) await budgetFile(snapshot, start.task.evidence_dir,
          `run-${snapshot.basis.run_number}-stop-snapshot.json`, frame.snapshot_ref);
        item.stopSnapshot = snapshot;
        const receipt = await callbacks.verifyStopSnapshot(snapshot, { signal: item.controller.signal, frame });
        budgetReceipt(receipt, snapshot);
        const after = await checkedStopSnapshot(snapshot, frame, start.task, start.task.evidence_dir);
        requireValue(digest(before) === digest(after), 'stop snapshot changed during verification');
        await receipts.operation(frame, 'stop-snapshot-checked', { snapshot_ref: before });
        reply = { ...envelope(frame, 'verify_stop_reply'), receipt };
        if (receipt.settlement === 'Unknown') stage = 'unknown';
      } else if (frame.kind === 'prepare_stop_request') {
        const continuation = await candidateOf(frame, 'stop_continuation');
        const before = await checkedStopContinuation(continuation, frame, start.task, start.task.evidence_dir);
        if (frame.stop_continuation_ref) await budgetFile(continuation, start.task.evidence_dir,
          `jsonl-stop-continuation-${continuation.next_run_number}.json`, frame.stop_continuation_ref);
        const prepared = await callbacks.prepareStopContinuation(continuation, { signal: item.controller.signal, frame });
        const after = await checkedStopContinuation(continuation, frame, start.task, start.task.evidence_dir);
        requireValue(digest(before) === digest(after), 'stop continuation changed during preparation');
        await receipts.operation(frame, 'stop-continuation-checked', { snapshot_ref: before.snapshotRef,
          continuation_ref: before.continuationRef });
        requireValue(exact(prepared, ['run_id_next', 'objective']) && nonempty(prepared.run_id_next)
          && nonempty(prepared.objective) && !startedRuns.has(prepared.run_id_next), 'invalid continuation or repeated Run ID');
        reply = { ...envelope(frame, 'prepare_stop_reply'), ...prepared };
        startedRuns.add(prepared.run_id_next);
      } else {
        const budget = frame.kind === 'prepare_budget_request';
        const continuation = await candidateOf(frame, budget ? 'budget_continuation' : 'continuation');
        let before;
        if (budget) {
          before = await checkedBudgetContinuation(continuation, frame, start.task, start.task.evidence_dir);
          if (frame.budget_continuation_ref) await budgetFile(continuation, start.task.evidence_dir,
            `jsonl-budget-continuation-${continuation.next_run_number}.json`, frame.budget_continuation_ref);
        } else checkBasisIdentity(continuation?.previous?.basis, frame);
        const prepared = budget
          ? await callbacks.prepare_budget_stop_continuation(continuation, { signal: item.controller.signal, frame })
          : await callbacks.prepare(continuation, { signal: item.controller.signal, frame });
        if (budget) {
          const after = await checkedBudgetContinuation(continuation, frame, start.task, start.task.evidence_dir);
          requireValue(digest(before) === digest(after), 'budget continuation changed during preparation');
          await receipts.operation(frame, 'budget-continuation-checked', { snapshot_ref: before.snapshotRef,
            continuation_ref: before.continuationRef });
        }
        requireValue(exact(prepared, ['run_id_next', 'objective']) && nonempty(prepared.run_id_next)
          && nonempty(prepared.objective) && !startedRuns.has(prepared.run_id_next), 'invalid continuation or repeated Run ID');
        reply = { ...envelope(frame, budget ? 'prepare_budget_reply' : 'prepare_reply'), ...prepared };
        startedRuns.add(prepared.run_id_next);
      }
    } catch {
      // No exception message is sent or persisted: it can contain provider keys.
      stage = 'unknown';
      if (frame.kind === 'tool_call') reply = toolUnknown(frame, started);
      else if (frame.kind === 'verify_request' && item.candidate) reply = { ...envelope(frame, 'verify_reply'),
        receipt: { basis: item.candidate.basis, committed: false, settlement: 'Unknown', verdict: 'Unknown', evidence: [started] } };
      else if (frame.kind === 'verify_budget_request' && item.budgetSnapshot) reply = { ...envelope(frame, 'verify_budget_reply'),
        receipt: { basis: item.budgetSnapshot.basis, committed: false, settlement: 'Unknown', verdict: 'Unknown', evidence: [started] } };
      else if (frame.kind === 'verify_stop_request' && item.stopSnapshot) reply = { ...envelope(frame, 'verify_stop_reply'),
        receipt: { basis: item.stopSnapshot.basis, committed: false, settlement: 'Unknown', verdict: 'Unknown', evidence: [started] } };
      else fault('host_callback_unknown');
    }
    item.settled = true;
    await receipts.operation(frame, stage, { reply_sha256: reply ? digest(reply) : null });
    if (reply) {
      try { await send(reply); item.replied = true; }
      catch { await receipts.operation(frame, 'reply-delivery-unknown'); state.fault ??= 'callback_reply_delivery_unknown'; }
    }
  };
  const executeReconcile = async (item) => {
    const frame = item.frame; let reply = null; let category = null;
    await receipts.operation(frame, 'started');
    try {
      const review = await candidateOf(frame, 'review');
      const built = await settleReconcile(callbacks.reconcileResume, frame, review, item.controller.signal);
      // An aborted settlement is never written back, even if the callback still returned.
      if (item.controller.signal.aborted) {
        category = Date.now() >= start.task.deadline_unix_ms ? 'reconcile_timeout' : 'reconcile_aborted';
      } else reply = built;
    } catch (error) { category = error?.category ?? 'reconcile_request_invalid'; }
    item.settled = true;
    await receipts.operation(frame, reply ? 'settled' : 'unknown', { reply_sha256: reply ? digest(reply) : null, category });
    if (!reply) { fault(category); return; }
    try { await send(reply); item.replied = true; }
    catch { await receipts.operation(frame, 'reply-delivery-unknown'); state.fault ??= 'callback_reply_delivery_unknown'; }
  };
  const dispatch = (frame) => {
    requireValue(object(frame) && frame.version === VERSION && frame.task_id === start.task.task_id, 'invalid server frame');
    if (frame.kind === 'reconcile_request') {
      requireValue(!state.terminal && !state.fault && active.size < 8, 'out-of-state reconcile request');
      correlation(frame);
      requireValue(!seen.has(frame.rpc_id), 'duplicate RPC request');
      requireValue(exact(frame, ['kind','version','rpc_id','task_id','run_id','review'])
        || exact(frame, ['kind','version','rpc_id','task_id','run_id','review_ref']), 'invalid reconcile_request fields');
      seen.add(frame.rpc_id);
      // Only with `resume` and only once; the callback is not called otherwise.
      if (typeof callbacks.reconcileResume !== 'function') { fault('reconcile_callback_missing'); return; }
      if (!resumeEnabled || reconcileSeen) { fault('reconcile_request_unexpected'); return; }
      reconcileSeen = true;
      const item = { frame, controller: new AbortController(), settled: false, replied: false };
      active.set(frame.rpc_id, item); state.pending_callbacks += 1;
      if (state.cancelled) item.controller.abort();
      item.promise = executeReconcile(item).catch(() => { fault('callback_receipt_or_transport_failure'); })
        .finally(() => { active.delete(frame.rpc_id); state.pending_callbacks -= 1; });
      journalTasks.push(item.promise);
      return;
    }
    if (frame.kind === 'mcp_status') {
      requireValue(mcpEnabled && exact(frame, ['kind','version','task_id','servers','excluded'])
        && object(frame.servers) && Array.isArray(frame.excluded), 'unsolicited or invalid mcp_status frame');
      try { void Promise.resolve(onEvent(frame)).catch(() => {}); }
      catch { /* Connection report only; never controls execution. */ }
      return;
    }
    if (frame.kind === 'event') {
      requireValue(exact(frame, ['kind','version','task_id','run_id','seq','event_type','delivery'])
        && frame.delivery === 'best_effort_not_complete_evidence', 'invalid event delivery marker');
      try { void Promise.resolve(onEvent(frame)).catch(() => {}); }
      catch { /* UI observer never controls execution or backpressures Core. */ }
      return;
    }
    if (frame.kind === 'task_result') {
      const keys = ['kind','version','task_id','stop','task_accepted','runs','observed_calls','fence_presence','reconciliation_fence','outcome_ref','local_callbacks_finished'];
      requireValue((exact(frame, keys) || (budgetEnabled && exact(frame, [...keys,'budget_review_ref'])))
        && ++terminalCount === 1 && frame.task_accepted === false, 'duplicate/accepted terminal refused');
      state.terminal = frame; state.phase = 'terminal_received'; return;
    }
    if (frame.kind === 'callback_cancel') {
      requireValue(exact(frame, ['kind','version','rpc_id','task_id','run_id','reason']), 'invalid callback_cancel fields');
      correlation(frame);
      const item = active.get(frame.rpc_id);
      requireValue(item && item.frame.run_id === frame.run_id && !item.replied, 'unsolicited callback cancellation');
      item.controller.abort(); return;
    }
    requireValue(['tool_call', 'verify_request', 'prepare_request', 'verify_budget_request', 'prepare_budget_request',
      'verify_stop_request', 'prepare_stop_request'].includes(frame.kind)
      && !state.terminal && !state.fault && active.size < 8, 'unknown or out-of-state callback');
    correlation(frame);
    requireValue(!['verify_budget_request','prepare_budget_request'].includes(frame.kind) || budgetEnabled, 'budget callback without opt-in');
    requireValue(!['verify_stop_request','prepare_stop_request'].includes(frame.kind) || lengthEnabled, 'stop snapshot callback without opt-in');
    requireValue(!seen.has(frame.rpc_id), 'duplicate RPC request');
    if (frame.kind === 'tool_call') requireValue(exact(frame, ['kind','version','rpc_id','task_id','run_id','native_call_id','tool_name','args_sha256','arguments'])
      && nonempty(frame.native_call_id) && nonempty(frame.tool_name)
      && allowedTools.has(frame.tool_name) && sha(frame.args_sha256) && object(frame.arguments), 'invalid or ungranted tool correlation/arguments');
    if (['verify_request','prepare_request','verify_budget_request','prepare_budget_request','verify_stop_request','prepare_stop_request'].includes(frame.kind)) {
      const payload = ({ verify_request:'candidate',prepare_request:'continuation',verify_budget_request:'snapshot',prepare_budget_request:'budget_continuation',
        verify_stop_request:'snapshot',prepare_stop_request:'stop_continuation' })[frame.kind];
      requireValue(exact(frame, ['kind','version','rpc_id','task_id','run_id',payload])
        || exact(frame, ['kind','version','rpc_id','task_id','run_id',`${payload}_ref`]), 'invalid callback payload fields');
    }
    seen.add(frame.rpc_id);
    const item = { frame, controller: new AbortController(), settled: false, replied: false };
    active.set(frame.rpc_id, item); state.pending_callbacks += 1;
    if (state.cancelled) item.controller.abort();
    item.promise = executeCallback(item).catch(() => { fault('callback_receipt_or_transport_failure'); })
      .finally(() => { active.delete(frame.rpc_id); state.pending_callbacks -= 1; });
    journalTasks.push(item.promise);
  };
  child.stdout.on('data', (chunk) => {
    try {
      let offset = 0;
      while (offset < chunk.length) {
        const newline = chunk.indexOf(10, offset); const end = newline < 0 ? chunk.length : newline + 1;
        requireValue(buffered.length + end - offset <= FRAME_BYTES, 'server frame byte bound');
        buffered = Buffer.concat([buffered, chunk.subarray(offset, end)]); offset = end;
        if (newline >= 0) {
          const line = new TextDecoder('utf-8', { fatal: true }).decode(buffered.subarray(0, -1));
          buffered = Buffer.alloc(0); dispatch(JSON.parse(line));
        }
      }
    } catch { fault('invalid_stdout_protocol'); }
  });
  child.stderr.on('data', (chunk) => {
    stderrHash.update(chunk); stderrBytes += chunk.length;
    if (stderrPreview.length < 65536) stderrPreview = Buffer.concat([stderrPreview, chunk.subarray(0, 65536 - stderrPreview.length)]);
  });
  child.stdin.on('error', () => { fault('stdin_transport_failure'); });
  child.stdout.on('error', () => { fault('stdout_transport_failure'); });
  child.stderr.on('error', () => { state.fault ??= 'stderr_transport_failure'; });
  const closed = new Promise((resolve) => {
    child.once('error', () => { state.fault ??= 'spawn_or_process_error'; });
    child.once('close', (code, signal) => {
      state.closed = true; state.exit_code = code; state.exit_signal = signal;
      if (signal !== null) state.fault ??= 'process_exited_by_signal';
      else if (code !== 0) state.fault ??= Number.isInteger(code) ? 'process_exit_nonzero' : 'process_exit_status_unknown';
      if (buffered.length) state.fault ??= 'unterminated_stdout_frame';
      if (!state.terminal) state.fault ??= 'terminal_delivery_unknown';
      for (const item of active.values()) {
        item.controller.abort();
        persist(receipts.operation(item.frame, 'transport-closed', { callback_settled_at_close: item.settled }));
      }
      resolve();
    });
  });
  // This deadline only requests cancellation. A later emergency kill remains
  // explicitly unknown and never establishes remote callback settlement.
  const timeout = Math.max(0, start.task.deadline_unix_ms - Date.now());
  const deadlineTimer = setTimeout(cancel, Math.min(timeout, 2147483647)); deadlineTimer.unref();
  const killTimer = setTimeout(forceKill, Math.min(timeout + grace + 4000, 2147483647)); killTimer.unref();
  send(start).catch(() => { fault('start_delivery_unknown'); });
  const waiting = (async () => {
    await closed;
    // The callbacks are never detached on normal cancellation or transport loss.
    // A Host callback that ignores its signal can therefore keep wait pending.
    await Promise.allSettled(journalTasks);
    clearTimeout(deadlineTimer); clearTimeout(killTimer);
    let outcome = null;
    try {
      requireValue(state.terminal && state.terminal.local_callbacks_finished === true, 'missing validated terminal');
      outcome = await readCheckedJsonReference(state.terminal.outcome_ref, start.task.evidence_dir);
      requireValue(outcome.task_id === start.task.task_id && outcome.task_accepted === false, 'outcome identity mismatch');
      for (const key of ['stop', 'runs', 'observed_calls', 'fence_presence', 'reconciliation_fence']) {
        requireValue(JSON.stringify(outcome[key]) === JSON.stringify(state.terminal[key]), 'terminal/outcome disagreement');
      }
      if (state.terminal.budget_review_ref) {
        requireValue(budgetEnabled, 'unsolicited budget review reference');
        const record = await readBudgetReviewReference(state.terminal.budget_review_ref, start.task.evidence_dir, start.task, outcome);
        await receipts.write('budget-review', { budget_review_ref: state.terminal.budget_review_ref, record });
      }
    } catch { state.fault ??= 'terminal_reference_unverified'; }
    let unknownEffects = null;
    if (outcome) {
      try { unknownEffects = parseUnknownEffects(outcome); }
      catch (error) { state.fault ??= `invalid_unknown_effect_record:${error.field}`; }
    }
    const result = { delivery: state.fault ? 'unknown' : 'validated', reconciliation_required: Boolean(state.fault)
      || outcome?.stop === 'ReconciliationRequired' || outcome?.fence_presence !== 'Released',
      terminal: state.terminal, outcome, unknownEffects, process: { exit_code: state.exit_code, signal: state.exit_signal,
        forced: state.forced, closed: state.closed }, callbacks_finished: state.pending_callbacks === 0,
      fault: state.fault, stderr: { bytes: stderrBytes, sha256: stderrHash.digest('hex'),
        preview: redacted(stderrPreview.toString('utf8')) }, operation_directory: operationDirectory };
    await receipts.write('client-finished', { delivery: result.delivery, fault: result.fault,
      process: result.process, callbacks_finished: result.callbacks_finished,
      outcome_sha256: outcome ? digest(outcome) : null, stderr_bytes: stderrBytes });
    state.phase = 'finished'; return result;
  })();
  return { wait: () => waiting, cancel, forceKill,
    status: () => ({ phase: state.phase, cancelled: state.cancelled, forced: state.forced,
      closed: state.closed, pending_callbacks: state.pending_callbacks, pid: child.pid ?? null }) };
}

export async function runTask(options) { return (await startTask(options)).wait(); }
