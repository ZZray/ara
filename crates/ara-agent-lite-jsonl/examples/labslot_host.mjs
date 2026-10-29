// Candidate composition root. No Nest bootstrap, database startup or copied
// business tools. A trusted product adapter supplies the real service closures.
import { pathToFileURL } from 'node:url';
import path from 'node:path';
import { runTask } from '../clients/node/client.mjs';
import { createLabslotTodoBinding } from '../clients/node/labslot-todos.mjs';

export async function runLabslotTask({ binary, start, operationDirectory, bindings, modelToken, onEvent }) {
  const binding = await createLabslotTodoBinding(bindings);
  const original = { ...start, run: { ...start.run, tools: binding.tools } };
  return runTask({ binary, start: original, operationDirectory, modelToken,
    callbacks: binding.callbacks, onEvent });
}

// The module exports createAuthorizedLabslotBindings():
// {binary,start,operationDirectory,bindings}. `bindings` contains actual
// createTodoTools instances, original-schema conversion, captured authorization,
// real DB readback and durable product receipts. It must not start the full app.
if (process.argv[1] && import.meta.url === pathToFileURL(path.resolve(process.argv[1])).href) {
  try {
    if (process.argv.length !== 3 || !path.isAbsolute(process.argv[2])) throw new Error('usage: node labslot_host.mjs ABSOLUTE_TRUSTED_BINDING_MODULE.mjs');
    const product = await import(pathToFileURL(process.argv[2]).href);
    if (typeof product.createAuthorizedLabslotBindings !== 'function') throw new Error('trusted product binding export missing');
    const configured = await product.createAuthorizedLabslotBindings();
    const result = await runLabslotTask({ ...configured, modelToken: process.env.ARA_AGENT_MODEL_TOKEN });
    console.log(JSON.stringify({ delivery: result.delivery, stop: result.outcome?.stop ?? null,
      reconciliation_required: result.reconciliation_required, task_accepted: result.outcome?.task_accepted ?? false,
      product_acceptance: 'requires original product DB/tool evidence; IPC success alone is insufficient' }));
    if (result.delivery !== 'validated' || result.outcome?.stop !== 'AwaitingHostAcceptance') process.exitCode = 1;
  } catch {
    // Never print an injected service exception; it may include credentials.
    console.error('labslot_candidate_host_failed; inspect original Host receipts; do not replay automatically');
    process.exitCode = 1;
  }
}
