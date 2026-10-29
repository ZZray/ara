// Ordinary Node consumer of the real Rust sidecar. Root supplies a separately
// observed controlled HTTP provider; this program contains no model/Agent loop.
// Usage: node native-probe.mjs BINARY CONFIG.json ROOT
import { promises as fs } from 'node:fs';
import path from 'node:path';
import { createHash } from 'node:crypto';
import { runTask } from './client.mjs';

const hash = (bytes) => createHash('sha256').update(bytes).digest('hex');
async function save(target, value) {
  const handle = await fs.open(target, 'wx');
  try { await handle.writeFile(`${JSON.stringify(value, null, 2)}\n`); await handle.sync(); }
  finally { await handle.close(); }
}

async function main() {
  if (process.argv.length !== 5) throw new Error('usage: native-probe.mjs BINARY CONFIG.json ROOT');
  const [binary, configPath, root] = process.argv.slice(2);
  if (![binary,configPath,root].every(path.isAbsolute)) throw new Error('absolute Host-selected paths required');
  const bytes = await fs.readFile(configPath);
  if (bytes.length > 262144) throw new Error('config byte limit');
  const start = JSON.parse(bytes.toString('utf8'));
  if (path.resolve(start.run.cwd) !== path.resolve(root)) throw new Error('probe cwd must be the explicit fixture ROOT');
  const operationRoot = path.join(root, 'node-native-operations'); await fs.mkdir(operationRoot);
  const counts = { writes:0, reviews:0, prepares:0 };
  const callbacks = {
    async tool(frame, {signal}) {
      const operation = path.join(operationRoot, `${hash(frame.rpc_id)}-started.json`);
      await save(operation, { request:frame, state:'started' });
      if (signal.aborted || frame.tool_name !== 'write_answer' || !frame.arguments
        || Object.keys(frame.arguments).length !== 1 || typeof frame.arguments.content !== 'string'
        || frame.arguments.content.length > 32768) {
        const rejected = `${operation}.rejected.json`; await save(rejected, { state:'known_not_executed' });
        return { value:{rejected:true},is_error:true,settlement:'Settled',operation_receipt:rejected };
      }
      try {
        const target=path.join(root,'answer.txt'); const handle=await fs.open(target,'w');
        try { await handle.writeFile(frame.arguments.content); await handle.sync(); }
        finally { await handle.close(); }
        counts.writes += 1;
        const actual=await fs.readFile(target);
        const complete=`${operation}.completed.json`;
        await save(complete,{state:'completed',answer_sha256:hash(actual),bytes:actual.length});
        return {value:{written:true},is_error:false,settlement:'Settled',operation_receipt:complete};
      } catch {
        const unknown=`${operation}.unknown.json`; await save(unknown,{state:'effects_unknown',replay:false});
        return {value:{effects_unknown:true},is_error:true,settlement:'Unknown',operation_receipt:unknown};
      }
    },
    async verify(candidate) {
      counts.reviews += 1;
      const answer=await fs.readFile(path.join(root,'answer.txt'));
      const correct=answer.equals(Buffer.from('correct'));
      const evidence=path.join(root,`node-review-${counts.reviews}.json`);
      await save(evidence,{basis:candidate.basis,actual_answer_sha256:hash(answer),correct,model_prose_used_as_proof:false});
      return {basis:candidate.basis,committed:true,settlement:'Settled',
        verdict:correct?'Verified':{NeedsChanges:{feedback:'Actual answer file differs from correct.'}},
        evidence:[`${evidence}#sha256=${hash(await fs.readFile(evidence))}`]};
    },
    async prepare(continuation) {
      if (!continuation.receipt.verdict?.NeedsChanges?.feedback) throw new Error('original Host feedback missing');
      counts.prepares += 1;
      return {run_id_next:`run-${continuation.next_run_number}`,objective:'Correct the actual file using original Host feedback. Actual answer file differs from correct.'};
    },
  };
  const result=await runTask({binary,start,modelToken:process.env.ARA_AGENT_MODEL_TOKEN,
    operationDirectory:path.join(root,'node-client-operations'),callbacks});
  await save(path.join(root,'node-client-result.json'),result);
  await save(path.join(root,'node-counts.json'),counts);
  if (result.outcome) await save(path.join(root,'node-outcome.json'),result.outcome);
  console.log(JSON.stringify({delivery:result.delivery,stop:result.outcome?.stop??null,...counts,
    task_accepted:result.outcome?.task_accepted??false}));
  if (result.delivery!=='validated'||result.outcome?.stop!=='AwaitingHostAcceptance') process.exitCode=1;
}
main().catch(()=>{console.error('native_node_probe_failed; inspect original operation and runtime evidence');process.exitCode=1;});
