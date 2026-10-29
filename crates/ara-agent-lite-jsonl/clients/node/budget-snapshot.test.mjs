// SDK reference/preflight checks only; actual Rust+HTTP coverage lives in
// tests/test_budget_snapshot.py using the ordinary shared sidecar.
import test from 'node:test';
import assert from 'node:assert/strict';
import { promises as fs } from 'node:fs';
import path from 'node:path';
import os from 'node:os';
import { createHash } from 'node:crypto';
import { startTask, readBudgetReviewReference } from './client.mjs';

const hash = (bytes) => createHash('sha256').update(bytes).digest('hex');
async function fixture(t) {
  const root = await fs.mkdtemp(path.join(os.tmpdir(), 'ara-budget-sdk-'));
  t.after(() => fs.rm(root, { recursive: true, force: true }));
  const evidence = path.join(root, 'evidence'); await fs.mkdir(evidence);
  const task = { task_id:'budget-task',criteria_revision:'criteria-1',authorization_revision:'authority-1',max_calls:1,deadline_unix_ms:Date.now()+10000,evidence_dir:evidence };
  const basis = { task_id:task.task_id,run_id:'run-1',criteria_revision:task.criteria_revision,
    authorization_revision:task.authorization_revision,run_number:1 };
  const snapshot = { basis,diagnostic_output:'not a final model answer',goal:null,
    termination:{version:1,cause:'call_budget_exhausted',native_round_closed:true,model_final_stop:false} };
  const write = async (name,value) => {
    const target=path.join(evidence,name); const bytes=Buffer.from(JSON.stringify(value));await fs.writeFile(target,bytes);
    return {path:target,sha256:hash(bytes)};
  };
  const snapshotRef=await write('run-1-budget-snapshot.json',snapshot);
  const receipt={basis,committed:true,settlement:'Settled',verdict:'Verified',evidence:['actual-file-check']};
  const record={version:1,kind:'closed_budget_snapshot_review',snapshot:snapshotRef,receipt};
  const reference=await write('run-1-budget-review-index.json',record);
  const outcome={task_id:task.task_id,task_accepted:false,last_candidate:null,last_receipt:receipt};
  return {root,evidence,task,snapshot,snapshotRef,receipt,record,reference,outcome,write};
}

test('budget index is checked without adding fields to outcome, regardless of JSON key order', async(t)=>{
  const f=await fixture(t);const original=JSON.stringify(f.outcome);
  const reordered={...f.record,receipt:{evidence:f.receipt.evidence,verdict:'Verified',settlement:'Settled',committed:true,basis:{...f.receipt.basis}}};
  const ref=await f.write('run-1-budget-review-index.json',reordered);
  assert.deepEqual(await readBudgetReviewReference(ref,f.evidence,f.task,f.outcome),reordered);
  assert.equal(JSON.stringify(f.outcome),original);
});

test('budget index cannot bind an older receipt or a forged authorization', async(t)=>{
  const f=await fixture(t);
  await assert.rejects(readBudgetReviewReference(f.reference,f.evidence,f.task,{...f.outcome,last_receipt:{...f.receipt,basis:{...f.receipt.basis,run_number:2}}}));
  await assert.rejects(readBudgetReviewReference(f.reference,f.evidence,{...f.task,authorization_revision:'forged'},f.outcome));
  await assert.rejects(readBudgetReviewReference(f.reference,f.evidence,f.task,{...f.outcome,last_candidate:{basis:f.receipt.basis}}));
});

test('budget snapshot hash and exact termination are mandatory',async(t)=>{
  const f=await fixture(t);
  await fs.writeFile(f.snapshotRef.path,JSON.stringify({...f.snapshot,diagnostic_output:'changed'}));
  await assert.rejects(readBudgetReviewReference(f.reference,f.evidence,f.task,f.outcome));
  const wrong={...f.snapshot,termination:{...f.snapshot.termination,model_final_stop:true}};
  const snapshotRef=await f.write('run-1-budget-snapshot.json',wrong);
  const ref=await f.write('run-1-budget-review-index.json',{...f.record,snapshot:snapshotRef});
  await assert.rejects(readBudgetReviewReference(ref,f.evidence,f.task,f.outcome));
});

test('opt-in with missing dedicated callbacks fails before spawn or operation ledger',async(t)=>{
  const f=await fixture(t);const operations=path.join(f.root,'operations');
  const start={kind:'start_task',version:1,task:f.task,run:{run_id:'run-1',mode:'execute',tools:[]},budget_stop_review:'review_closed_tool_round'};
  await assert.rejects(startTask({binary:process.execPath,start,modelToken:'never-started-grant',operationDirectory:operations,
    callbacks:{tool:async()=>{},verify:async()=>{},prepare:async()=>{}}}));
  await assert.rejects(fs.stat(operations));
});
