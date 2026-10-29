// Injected stand-ins test only this binding contract. No actual LabSlot
// StructuredTool, TodoService, Nest application or PostgreSQL is executed.
import test from 'node:test';
import assert from 'node:assert/strict';
import { createLabslotTodoBinding } from './labslot-todos.mjs';

async function fixture() {
  const state = { target:{id:'own-todo',userId:'owner',title:'Task',status:'pending',priority:'medium'},
    protected_rows:[{id:'foreign-todo',userId:'other',title:'Private',priority:'low'}] };
  const counts = { reads:0,updates:0,authorization:0,settlements:0,reviews:0 };
  const controls = { authorized:true,unknown:false,persistenceFailure:false };
  const turns = []; const records=[];
  const turn = { actor_id:'owner',conversation_id:'conversation-original',user_message_ordinal:7,
    confirmation_id:'human-confirmation-original',confirmation_version:'version-original' };
  const tools = [
    {name:'list_user_todos',description:'Fixture original list schema',schema:{type:'object',properties:{status:{type:'string'},priority:{type:'string'},search:{type:'string'}}},
      invoke:async()=>{counts.reads+=1;return JSON.stringify([state.target]);}},
    {name:'update_todo',description:'Fixture original update schema',schema:{type:'object',properties:{todoId:{type:'string'},priority:{type:'string'},title:{type:'string'},status:{type:'string'},pinned:{type:'boolean'},tags:{type:'array'}},required:['todoId']},
      invoke:async(args)=>{counts.updates+=1;state.target.priority=args.priority;return 'fixture product tool result';}},
    {name:'device_invoke',schema:{},invoke:async()=>{throw new Error('device tool must not be registered');}},
  ];
  const binding = await createLabslotTodoBinding({tools,schemaToJson:async(schema)=>schema,
    assertAuthorization:async()=>{counts.authorization+=1;if(!controls.authorized)throw new Error('revoked');},
    readback:async()=>structuredClone(state),turn,criteria:{todo_id:'own-todo',expected_priority:'high'},
    operations:{started:async(meta)=>{records.push(meta);turns.push(meta.turn);return `fixture-start-${records.length}`;},
      finished:async(start,meta)=>{if(controls.persistenceFailure)throw new Error('fixture receipt failure');records.push({start,meta});return `fixture-finish-${records.length}`;},
      unknown:async(start,meta)=>{records.push({start,meta});return `fixture-unknown-${records.length}`;}},
    settleTool:async()=>{counts.settlements+=1;return {is_error:false,settlement:controls.unknown?'Unknown':'Settled'};},
    commitReview:async(proof)=>{counts.reviews+=1;records.push(proof);return `fixture-db-review-${counts.reviews}`;},
  });
  let sequence=0;
  const frame=(tool_name,args)=>({kind:'tool_call',version:1,task_id:'task-original',run_id:'run-original',rpc_id:`fixture-${++sequence}`,
    native_call_id:`native-${sequence}`,args_sha256:'0'.repeat(64),tool_name,arguments:args});
  const candidate={basis:{task_id:'task-original',run_id:'run-original'},output:'not proof',goal:null};
  const context={signal:new AbortController().signal,frame:{task_id:'task-original',run_id:'run-original',rpc_id:'verify'}};
  return {binding,counts,controls,state,turn,turns,records,frame,candidate,context};
}

test('binding invokes only injected todo tools and verifies readback across feedback with fixed human turn', async()=>{
  const f=await fixture();
  assert.deepEqual(f.binding.tools.map((tool)=>tool.name),['list_user_todos','update_todo']);
  assert.deepEqual(Object.keys(f.binding.tools[1].parameters.properties),['todoId','priority']);
  f.turn.user_message_ordinal=99; f.turn.confirmation_id='forged-later-value';
  await f.binding.callbacks.tool(f.frame('list_user_todos',{}),f.context);
  await f.binding.callbacks.tool(f.frame('update_todo',{todoId:'own-todo',priority:'medium'}),f.context);
  const first=await f.binding.callbacks.verify(f.candidate,f.context);
  assert.ok(first.verdict.NeedsChanges.feedback); assert.equal(first.committed,true);
  const prepared=await f.binding.callbacks.prepare({receipt:first,next_run_number:2},f.context);
  assert.deepEqual(Object.keys(prepared).sort(),['objective','run_id_next']);
  await f.binding.callbacks.tool(f.frame('update_todo',{todoId:'own-todo',priority:'high'}),f.context);
  const final=await f.binding.callbacks.verify(f.candidate,f.context);
  assert.equal(final.verdict,'Verified');assert.equal(f.counts.updates,2);assert.equal(f.counts.reads,1);
  assert.equal(f.state.protected_rows[0].priority,'low');
  assert.ok(f.turns.every((turn)=>turn.user_message_ordinal===7&&turn.confirmation_id==='human-confirmation-original'));
  assert.equal(f.binding.captured_turn.user_message_ordinal,7);
  assert.ok(f.counts.authorization>=10);
});

test('foreign IDs, unexpected mutations and revoked authority do not invoke the injected domain operation', async()=>{
  const f=await fixture();
  for(const args of [{todoId:'foreign-todo',priority:'high'},{todoId:'own-todo',priority:'high',status:'completed'}]){
    const result=await f.binding.callbacks.tool(f.frame('update_todo',args),f.context);
    assert.equal(result.is_error,true);assert.equal(result.settlement,'Settled');
  }
  f.controls.authorized=false;
  const result=await f.binding.callbacks.tool(f.frame('update_todo',{todoId:'own-todo',priority:'high'}),f.context);
  assert.equal(result.is_error,true);assert.equal(f.counts.updates,0);assert.equal(f.counts.settlements,0);
  await assert.rejects(f.binding.callbacks.verify(f.candidate,f.context));
});

test('unknown settlement and lost product receipts cannot be promoted by a matching target state', async()=>{
  for(const failure of ['unknown','persistenceFailure']){
    const f=await fixture();f.controls[failure]=true;
    const result=await f.binding.callbacks.tool(f.frame('update_todo',{todoId:'own-todo',priority:'high'}),f.context);
    assert.equal(result.settlement,'Unknown');
    const review=await f.binding.callbacks.verify(f.candidate,f.context);
    assert.equal(review.verdict,'Unknown');assert.equal(review.settlement,'Unknown');
    await assert.rejects(f.binding.callbacks.prepare({receipt:review,next_run_number:2},f.context));
    const before=f.counts.updates;
    await f.binding.callbacks.tool(f.frame('update_todo',{todoId:'own-todo',priority:'high'}),f.context);
    assert.equal(f.counts.updates,before);
  }
});
