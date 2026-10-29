// Actual shared sidecar plus Host-owned file effects. Controlled HTTP is supplied
// by the Python driver; this is not a product/database/model acceptance probe.
import { promises as fs } from 'node:fs';
import path from 'node:path';
import { createHash } from 'node:crypto';
import { startTask } from './client.mjs';

const [binary, configPath, root, mode = 'normal'] = process.argv.slice(2);
const start = JSON.parse(await fs.readFile(configPath, 'utf8'));
const hash = (bytes) => createHash('sha256').update(bytes).digest('hex');
const save = async (name, value) => {
  const handle = await fs.open(path.join(root, name), 'wx');
  try { await handle.writeFile(`${JSON.stringify(value, null, 2)}\n`); await handle.sync(); }
  finally { await handle.close(); }
};
let writes = 0; let reviews = 0; let prepares = 0;
const callbacks = {
  async tool(frame, { signal }) {
    if (signal.aborted) throw Error('cancelled before tool');
    if (frame.tool_name !== 'write_answer') throw Error('tool not registered by probe');
    const receipt = `node-write-${++writes}.json`;
    await fs.writeFile(path.join(root, 'answer.txt'), frame.arguments.content);
    await save(receipt, { frame, actual_sha256: hash(await fs.readFile(path.join(root, 'answer.txt'))) });
    return { value: { written: true }, is_error: false, settlement: 'Settled', operation_receipt: path.join(root, receipt) };
  },
  async verify() { throw Error('ordinary verify must not receive a budget snapshot'); },
  async prepare() { throw Error('ordinary prepare must not receive a budget continuation'); },
  async verify_budget_snapshot(snapshot, { signal }) {
    if (signal.aborted) throw Error('cancelled review');
    const actual = await fs.readFile(path.join(root, 'answer.txt'), 'utf8');
    const receipt = `node-budget-review-${++reviews}.json`;
    await save(receipt, { snapshot, actual, correct: actual === 'beta' });
    if (mode === 'tamper') {
      const changed = { ...snapshot, diagnostic_output: 'changed during callback' };
      await fs.writeFile(path.join(start.task.evidence_dir, `run-${snapshot.basis.run_number}-budget-snapshot.json`), JSON.stringify(changed));
    }
    return { basis: snapshot.basis, committed: true, settlement: 'Settled',
      verdict: actual === 'beta' ? 'Verified' : { NeedsChanges: { feedback: 'Write beta to the actual file.' } },
      evidence: [path.join(root, receipt)] };
  },
  async prepare_budget_stop_continuation(continuation, { signal }) {
    if (signal.aborted || continuation.deadline_unix_ms !== start.task.deadline_unix_ms) throw Error('invalid continuation');
    prepares += 1;
    await save(`node-budget-prepare-${prepares}.json`, continuation);
    return { run_id_next: `run-${continuation.next_run_number}`, objective: 'Write beta to the actual file. ' + continuation.receipt.verdict.NeedsChanges.feedback };
  },
};
const handle = await startTask({ binary, start, callbacks,
  modelToken: process.env.ARA_AGENT_MODEL_TOKEN, operationDirectory: path.join(root, 'node-operations') });
await save('node-sidecar-started.json', { pid: handle.status().pid, binary });
const result = await handle.wait();
await save('node-result.json', result);
await save('node-host-ledger.json', { writes, reviews, prepares });
process.stdout.write(JSON.stringify({ delivery: result.delivery, stop: result.outcome?.stop, writes, reviews, prepares }) + '\n');
