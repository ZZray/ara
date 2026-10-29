// Node client `reconcileResume` (requirement 12) against an explicit protocol
// fixture process. No Rust, model or product runs here; the actual sidecar and
// the cross-language comparison are in tests/test_resume_cross_language.py.
import test from 'node:test';
import assert from 'node:assert/strict';
import { promises as fs } from 'node:fs';
import os from 'node:os';
import path from 'node:path';
import { startTask } from './client.mjs';

const fixture = String.raw`
import {createInterface} from 'node:readline';
import {promises as fs} from 'node:fs';
import path from 'node:path';
import {createHash} from 'node:crypto';
const hash=x=>createHash('sha256').update(x).digest('hex');
const emit=x=>process.stdout.write(JSON.stringify(x)+'\n');
const input=createInterface({input:process.stdin,crlfDelay:Infinity});
let start;let done=false;
for await(const line of input){const frame=JSON.parse(line);
 if(frame.kind==='start_task'){
  start=frame;const mode=frame.run.objective;await fs.mkdir(start.task.evidence_dir);
  if(start.resume||mode==='unexpected'||mode==='wrong-run'){
   const run=mode==='wrong-run'?'run-never-started':start.run.run_id;
   emit({kind:'reconcile_request',version:1,rpc_id:'rpc-1',task_id:start.task.task_id,run_id:run,
    review:{task_id:start.task.task_id,interrupted_run:null,consumed_runs:1,consumed_calls:2}});
  }
 }else if(frame.kind==='reconcile_reply'){
  await fs.writeFile(path.join(start.task.evidence_dir,'fixture-reply.json'),line);
  const outcome={task_id:start.task.task_id,stop:'AwaitingHostAcceptance',task_accepted:false,runs:2,observed_calls:3,fence_presence:'Released',reconciliation_fence:null};
  const target=path.join(start.task.evidence_dir,'released.json');
  const bytes=Buffer.from(JSON.stringify(outcome));await fs.writeFile(target,bytes);
  emit({kind:'task_result',version:1,...outcome,outcome_ref:{path:target,sha256:hash(bytes)},local_callbacks_finished:true});
  done=true;input.close();process.stdin.destroy();break;
 }else if(frame.kind==='cancel'){
  await fs.writeFile(path.join(start.task.evidence_dir,'fixture-cancel.json'),line);process.exit(2);
 }
}
if(!done)process.exitCode=2;
`;
const TOKEN = 'fixture-private-token-never-log';
const receipt = () => ({ committed: true, settlement: 'Settled', tool_calls: [], evidence: ['host-log:checked'] });

async function setup(t, mode = 'ordinary', { resume = true, deadlineMs = 8000 } = {}) {
  const root = await fs.mkdtemp(path.join(os.tmpdir(), 'ara-node-resume-'));
  t.after(() => fs.rm(root, { recursive: true, force: true }));
  const script = path.join(root, 'fixture.mjs'); await fs.writeFile(script, fixture);
  const start = { kind: 'start_task', version: 1,
    task: { task_id: 'node-resume', criteria_revision: 'criteria-original', authorization_revision: 'authorization-original',
      evidence_dir: path.join(root, 'evidence'), deadline_unix_ms: Date.now() + deadlineMs },
    run: { run_id: 'run-resume', objective: mode, tools: [{ name: 'deploy' }] }, cancel_grace_ms: 1000 };
  if (resume) start.resume = { version: 1, fence_sha256: '0'.repeat(64) };
  return { root, binary: process.execPath, binaryArgs: [script], operationDirectory: path.join(root, 'ops'),
    modelToken: TOKEN, start };
}
const base = { tool: async () => { throw new Error('no tool call expected'); },
  verify: async () => { throw new Error('no review expected'); }, prepare: async () => { throw new Error('no continuation'); } };
const exists = (file) => fs.access(file).then(() => true, () => false);

test('a settled receipt is written back once with the request correlation', { timeout: 8000 }, async (t) => {
  const options = await setup(t); const calls = [];
  const result = await (await startTask({ ...options, callbacks: { ...base,
    reconcileResume: async (review, { signal, frame }) => { calls.push({ review, frame, aborted: signal.aborted }); return receipt(); } } })).wait();
  assert.equal(result.delivery, 'validated', result.fault);
  assert.equal(result.outcome.stop, 'AwaitingHostAcceptance');
  assert.equal(calls.length, 1);
  assert.equal(calls[0].review.consumed_calls, 2);
  const reply = JSON.parse(await fs.readFile(path.join(options.start.task.evidence_dir, 'fixture-reply.json'), 'utf8'));
  assert.deepEqual(reply, { kind: 'reconcile_reply', version: 1, rpc_id: calls[0].frame.rpc_id,
    task_id: calls[0].frame.task_id, run_id: calls[0].frame.run_id, receipt: receipt() });
});

test('a thrown callback is refused without its message or the model token', { timeout: 8000 }, async (t) => {
  const options = await setup(t); let calls = 0;
  const result = await (await startTask({ ...options, callbacks: { ...base,
    reconcileResume: async () => { calls += 1; throw new Error(`Host failure leaking ${TOKEN}`); } } })).wait();
  assert.equal(result.fault, 'reconcile_callback_threw');
  assert.equal(result.delivery, 'unknown');
  assert.equal(calls, 1);
  assert.ok(!JSON.stringify(result).includes(TOKEN) && !JSON.stringify(result).includes('Host failure'));
  assert.equal(await exists(path.join(options.start.task.evidence_dir, 'fixture-reply.json')), false);
  assert.equal(await exists(path.join(options.start.task.evidence_dir, 'fixture-cancel.json')), true);
  for (const file of await fs.readdir(options.operationDirectory)) {
    assert.ok(!(await fs.readFile(path.join(options.operationDirectory, file), 'utf8')).includes(TOKEN));
  }
});

test('an invalid receipt is refused and never written', { timeout: 8000 }, async (t) => {
  for (const value of [{ committed: true }, { ...receipt(), extra: 1 }, { ...receipt(), evidence: 'host-log' },
    { ...receipt(), tool_calls: [{ call_id: 'c', settlement: { kind: 'not_executed', output: 'x' } }] }]) {
    const options = await setup(t);
    const result = await (await startTask({ ...options, callbacks: { ...base, reconcileResume: async () => value } })).wait();
    assert.equal(result.fault, 'reconcile_receipt_invalid');
    assert.equal(await exists(path.join(options.start.task.evidence_dir, 'fixture-reply.json')), false);
  }
});

test('resume without the callback fails before spawn; without resume the callback is optional', { timeout: 8000 }, async (t) => {
  const options = await setup(t);
  await assert.rejects(startTask({ ...options, callbacks: base }), (error) => error.message === 'reconcile_callback_missing');
  assert.equal(await exists(options.operationDirectory), false, 'no operation ledger and no process');
  assert.equal(await exists(options.start.task.evidence_dir), false);
  const plain = await setup(t, 'no-request', { resume: false });
  const handle = await startTask({ ...plain, callbacks: base });
  handle.cancel();
  const result = await handle.wait();
  assert.notEqual(result.fault, 'reconcile_callback_missing');
});

test('a request without resume, or for a Run never started, never calls the callback', { timeout: 8000 }, async (t) => {
  for (const [mode, resume, fault] of [['unexpected', false, 'reconcile_request_unexpected'], ['wrong-run', true, 'invalid_stdout_protocol']]) {
    const options = await setup(t, mode, { resume }); let calls = 0;
    const result = await (await startTask({ ...options, callbacks: { ...base, reconcileResume: async () => { calls += 1; return receipt(); } } })).wait();
    assert.equal(result.fault, fault, mode);
    assert.equal(calls, 0, mode);
    assert.equal(await exists(path.join(options.start.task.evidence_dir, 'fixture-reply.json')), false);
  }
});

for (const trigger of ['cancel', 'deadline']) {
  test(`${trigger} aborts the pending callback within 1000 ms and it is not called again`, { timeout: 10000 }, async (t) => {
    const options = await setup(t, 'ordinary', { deadlineMs: trigger === 'deadline' ? 1500 : 8000 });
    let calls = 0; let entered; let aborted;
    const ready = new Promise((resolve) => { entered = resolve; });
    const seen = new Promise((resolve) => { aborted = resolve; });
    const handle = await startTask({ ...options, callbacks: { ...base, reconcileResume: async (review, { signal }) => {
      calls += 1; entered();
      await new Promise((resolve) => signal.addEventListener('abort', () => { aborted(Date.now()); resolve(); }, { once: true }));
      return receipt();
    } } });
    await ready;
    const from = trigger === 'cancel' ? Date.now() : options.start.task.deadline_unix_ms;
    if (trigger === 'cancel') handle.cancel();
    const at = await seen;
    assert.ok(at - from <= 1000, `aborted ${at - from} ms after ${trigger}`);
    const result = await handle.wait();
    assert.equal(calls, 1);
    assert.equal(result.delivery, 'unknown');
    assert.equal(await exists(path.join(options.start.task.evidence_dir, 'fixture-reply.json')), false);
  });
}
