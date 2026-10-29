// Ordinary Node IPC tests with an explicit protocol fixture process. These do
// not execute Rust, a model, TodoService, PostgreSQL or a deployed product.
import test from 'node:test';
import assert from 'node:assert/strict';
import { promises as fs } from 'node:fs';
import os from 'node:os';
import path from 'node:path';
import { createHash } from 'node:crypto';
import { startTask, readCheckedJsonReference } from './client.mjs';

const sha = (bytes) => createHash('sha256').update(bytes).digest('hex');
const delay = (ms) => new Promise((resolve) => setTimeout(resolve, ms));
const fixture = String.raw`
import {createInterface} from 'node:readline';
import {promises as fs} from 'node:fs';
import path from 'node:path';
import {createHash} from 'node:crypto';
const hash=x=>createHash('sha256').update(x).digest('hex');
const emit=x=>process.stdout.write(JSON.stringify(x)+'\n');
const input=createInterface({input:process.stdin,crlfDelay:Infinity});
let start;let mode;let callback;
const originalBasis=()=>({task_id:start.task.task_id,run_id:start.run.run_id,criteria_revision:start.task.criteria_revision,authorization_revision:start.task.authorization_revision});
async function finish(stop='AwaitingHostAcceptance'){
 const released=stop==='AwaitingHostAcceptance';
 const fence=released?null:path.join(path.dirname(start.task.evidence_dir),'fixture.fence');
 if(fence)await fs.writeFile(fence,'original fixture fence');
 const outcome={task_id:start.task.task_id,stop,task_accepted:false,runs:1,observed_calls:2,fence_presence:released?'Released':'Present',reconciliation_fence:fence};
 const target=path.join(start.task.evidence_dir,released?'released.json':'outcome.json');
 const bytes=Buffer.from(JSON.stringify(outcome));await fs.writeFile(target,bytes);
 emit({kind:'task_result',version:1,...outcome,outcome_ref:{path:target,sha256:hash(bytes)},local_callbacks_finished:true});
 input.close();process.stdin.destroy();if(mode==='exit7')process.exitCode=7;
}
for await(const line of input){const frame=JSON.parse(line);
 if(frame.kind==='start_task'){
  start=frame;mode=frame.run.objective;await fs.mkdir(start.task.evidence_dir);
  if(mode==='observer')emit({kind:'event',version:1,task_id:start.task.task_id,run_id:start.run.run_id,seq:1,event_type:'agent_start',delivery:'best_effort_not_complete_evidence'});
  callback={kind:'tool_call',version:1,rpc_id:'rpc-tool',task_id:start.task.task_id,run_id:start.run.run_id,native_call_id:'native-1',tool_name:'lookup',args_sha256:hash('{}'),arguments:{}};emit(callback);
  if(mode==='disconnect')setTimeout(()=>{process.stderr.write(process.env.ARA_AGENT_MODEL_TOKEN);process.exit(3);},60);
 }else if(frame.kind==='cancel'){
  if(mode.includes('wrong-')){await finish('ReconciliationRequired');break;}
  emit({kind:'callback_cancel',version:1,rpc_id:callback.rpc_id,task_id:callback.task_id,run_id:callback.run_id,reason:'cancelled'});
 }else if(frame.kind==='tool_reply'){
  for(const key of ['rpc_id','task_id','run_id','native_call_id','tool_name','args_sha256'])if(frame[key]!==callback[key])throw Error('correlation changed');
  if(mode==='cancel'){await finish('Cancelled');break;}
  const basis=originalBasis();
  if(mode==='verify-wrong-criteria')basis.criteria_revision='forged-criteria';
  if(mode==='verify-wrong-authorization')basis.authorization_revision='forged-authorization';
  emit({kind:'verify_request',version:1,rpc_id:'rpc-verify',task_id:start.task.task_id,run_id:start.run.run_id,candidate:{basis,output:'fixture',goal:null}});
 }else if(frame.kind==='verify_reply'){
  if(mode.startsWith('prepare-wrong-')){
   const basis=originalBasis();
   if(mode==='prepare-wrong-criteria')basis.criteria_revision='forged-criteria';
   if(mode==='prepare-wrong-authorization')basis.authorization_revision='forged-authorization';
   emit({kind:'prepare_request',version:1,rpc_id:'rpc-prepare',task_id:start.task.task_id,run_id:start.run.run_id,continuation:{previous:{basis,output:'fixture',goal:null},receipt:frame.receipt,next_run_number:2,remaining_calls:2,deadline_unix_ms:start.task.deadline_unix_ms}});
  }else{await finish();break;}
 }
}
`;

async function setup(t, objective = 'ordinary') {
  const root = await fs.mkdtemp(path.join(os.tmpdir(), 'ara-node-ipc-'));
  t.after(() => fs.rm(root, { recursive: true, force: true }));
  const script = path.join(root, 'fixture.mjs'); await fs.writeFile(script, fixture);
  return { root, binary: process.execPath, binaryArgs: [script], operationDirectory: path.join(root, 'client-operations'),
    modelToken: 'fixture-private-token-never-log', start: { kind: 'start_task', version: 1,
      task: { task_id: 'node-fixture',criteria_revision:'criteria-original',authorization_revision:'authorization-original', evidence_dir: path.join(root, 'evidence'), deadline_unix_ms: Date.now() + 8000 },
      run: { run_id: 'run-original', objective, tools: [{name:'lookup'}] }, cancel_grace_ms: 1000 } };
}
function callbacks(tool) {
  return { tool, verify: async (candidate) => ({ basis: candidate.basis, committed: true,
    settlement: 'Settled', verdict: 'Verified', evidence: ['fixture-only-review'] }),
    prepare: async () => { throw new Error('fixture did not request continuation'); } };
}

test('ordinary child IPC preserves exact tool correlation and validates original outcome ref', { timeout: 5000 }, async (t) => {
  const options = await setup(t); let executions = 0;
  const handle = await startTask({ ...options, callbacks: callbacks(async () => {
    executions += 1; return { value: { actual_fixture: true }, is_error: false,
      settlement: 'Settled', operation_receipt: 'fixture-op-original' };
  }) });
  t.after(() => handle.forceKill());
  const result = await handle.wait();
  assert.equal(result.delivery, 'validated'); assert.equal(result.reconciliation_required, false);
  assert.equal(result.outcome.stop, 'AwaitingHostAcceptance'); assert.equal(result.callbacks_finished, true);
  assert.equal(result.outcome.task_accepted, false); assert.equal(executions, 1);
  const files = await fs.readdir(options.operationDirectory);
  assert.ok(files.some((file) => file.endsWith('-started.json')));
  assert.ok(files.some((file) => file.endsWith('-settled.json')));
  for (const file of files) assert.ok(!(await fs.readFile(path.join(options.operationDirectory, file), 'utf8')).includes(options.modelToken));
});

test('cancel aborts the original callback but wait remains pending until it settles', { timeout: 5000 }, async (t) => {
  const options = await setup(t, 'cancel'); let release; let entered; let abortSeen = false; let joined = false;
  const ready = new Promise((resolve) => { entered = resolve; });
  const settle = new Promise((resolve) => { release = resolve; });
  const handle = await startTask({ ...options, callbacks: callbacks(async (_frame, { signal }) => {
    signal.addEventListener('abort', () => { abortSeen = true; }, { once: true }); entered();
    await settle; await delay(30); joined = true;
    return { value: { callback_joined: true }, is_error: false, settlement: 'Settled', operation_receipt: 'fixture-cancel-op' };
  }) });
  t.after(() => handle.forceKill()); await ready; handle.cancel();
  assert.equal(await Promise.race([handle.wait().then(() => 'finished'), delay(40).then(() => 'pending')]), 'pending');
  assert.equal(abortSeen, true); assert.equal(joined, false); release();
  const result = await handle.wait();
  assert.equal(joined, true); assert.equal(result.delivery, 'validated');
  assert.equal(result.outcome.stop, 'Cancelled'); assert.equal(result.outcome.fence_presence, 'Present');
  assert.equal(result.callbacks_finished, true); assert.equal(result.reconciliation_required, true);
});

test('disconnect is delivery unknown, retains original callback receipts and does not retry', { timeout: 5000 }, async (t) => {
  const options = await setup(t, 'disconnect'); let executions = 0; let joined = false;
  const handle = await startTask({ ...options, callbacks: callbacks(async (_frame, { signal }) => {
    executions += 1;
    await new Promise((resolve) => signal.addEventListener('abort', resolve, { once: true }));
    await delay(25); joined = true;
    return { value: { settled_after_disconnect: true }, is_error: false,
      settlement: 'Settled', operation_receipt: 'original-product-op' };
  }) });
  t.after(() => handle.forceKill());
  const result = await handle.wait();
  assert.equal(result.delivery, 'unknown'); assert.equal(result.outcome, null);
  assert.equal(result.callbacks_finished, true); assert.equal(joined, true); assert.equal(executions, 1);
  assert.ok(!result.stderr.preview.includes(options.modelToken));
  const files = await fs.readdir(options.operationDirectory);
  assert.ok(files.some((file) => file.endsWith('-transport-closed.json')));
  assert.ok(files.some((file) => file.endsWith('-reply-delivery-unknown.json')));
});

test('reference hash and original evidence containment are mandatory', async (t) => {
  const { root } = await setup(t); const evidence = path.join(root, 'refs'); await fs.mkdir(evidence);
  const file = path.join(evidence, 'one.json'); const bytes = Buffer.from('{"actual":true}'); await fs.writeFile(file, bytes);
  assert.deepEqual(await readCheckedJsonReference({ path: file, sha256: sha(bytes) }, evidence), { actual: true });
  await assert.rejects(readCheckedJsonReference({ path: file, sha256: '0'.repeat(64) }, evidence));
  const outside = path.join(root, 'outside.json'); await fs.writeFile(outside, bytes);
  await assert.rejects(readCheckedJsonReference({ path: outside, sha256: sha(bytes) }, evidence));
  await assert.rejects(readCheckedJsonReference({ path: file, sha256: sha(bytes) }, evidence, 2));
});

test('an async UI observer rejection does not become unhandled or control the task', { timeout: 5000 }, async (t) => {
  const options = await setup(t, 'observer'); let observations = 0;
  const handle = await startTask({ ...options, onEvent: async () => {
    observations += 1; throw new Error('fixture observer rejected');
  }, callbacks: callbacks(async () => ({ value:{written:true},is_error:false,
    settlement:'Settled',operation_receipt:'fixture-observer-op' })) });
  t.after(() => handle.forceKill());
  const result = await handle.wait();
  assert.equal(observations,1); assert.equal(result.delivery,'validated');
  assert.equal(result.outcome.stop,'AwaitingHostAcceptance'); assert.equal(result.process.exit_code,0);
});

test('a valid terminal plus nonzero process exit preserves the original outcome but is not validated delivery', { timeout: 5000 }, async (t) => {
  const options = await setup(t, 'exit7');
  const handle = await startTask({ ...options, callbacks: callbacks(async () => ({ value:{written:true},is_error:false,
    settlement:'Settled',operation_receipt:'fixture-nonzero-op' })) });
  t.after(() => handle.forceKill());
  const result = await handle.wait();
  assert.equal(result.delivery,'unknown'); assert.equal(result.fault,'process_exit_nonzero');
  assert.equal(result.reconciliation_required,true); assert.equal(result.process.exit_code,7);
  assert.equal(result.outcome.stop,'AwaitingHostAcceptance'); assert.equal(result.outcome.fence_presence,'Released');
  assert.equal(result.outcome.reconciliation_fence,null); assert.equal(result.outcome.task_accepted,false);
  assert.equal(result.callbacks_finished,true); assert.equal(result.process.forced,false);
  const files=await fs.readdir(options.operationDirectory);
  assert.ok(files.some((file)=>file.endsWith('-settled.json')));
});

test('verify and continuation reject changed original criteria or authorization before invoking Host callbacks', { timeout: 8000 }, async (t) => {
  for (const mode of ['verify-wrong-criteria','verify-wrong-authorization','prepare-wrong-criteria','prepare-wrong-authorization']) {
    const options=await setup(t,mode);let reviews=0;let prepares=0;
    const handle=await startTask({...options,callbacks:{
      tool:async()=>({value:{written:true},is_error:false,settlement:'Settled',operation_receipt:'fixture-identity-op'}),
      verify:async(candidate)=>{reviews+=1;return {basis:candidate.basis,committed:true,settlement:'Settled',
        verdict:{NeedsChanges:{feedback:'Fixture requires original-source continuation.'}},evidence:['fixture-identity-review']};},
      prepare:async()=>{prepares+=1;return {run_id_next:'must-not-start',objective:'must not enter wrong-basis callback'};},
    }});
    t.after(()=>handle.forceKill());
    const result=await handle.wait();
    assert.equal(result.delivery,'unknown',mode);assert.equal(result.reconciliation_required,true,mode);
    assert.equal(result.outcome.stop,'ReconciliationRequired',mode);
    assert.equal(reviews,mode.startsWith('verify-')?0:1,mode);assert.equal(prepares,0,mode);
    assert.equal(result.outcome.fence_presence,'Present',mode);
  }
});
