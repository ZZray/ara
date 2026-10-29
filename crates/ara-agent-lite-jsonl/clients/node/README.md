# Node Host client

This is a dependency-free Node ESM client for the shared Rust JSONL v1 sidecar.
It starts one hidden child, services its original callbacks, and checks the
original Task outcome reference. It contains no Agent loop or retry policy.

```js
import { startTask } from './client.mjs';

const handle = await startTask({
  binary: absoluteRustBinary,
  start: startTaskFrame,
  modelToken: process.env.ARA_AGENT_MODEL_TOKEN,
  operationDirectory: absoluteFreshHostReceiptDirectory,
  callbacks: {
    async tool(request, { signal }) {
      // Commit a real operation receipt and settle the original effect.
      return { value: actualResult, is_error: false,
        settlement: 'Settled', operation_receipt: committedReceiptReference };
    },
    async verify(candidate, { signal, frame }) {
      // Read actual artifacts and commit evidence before returning.
      return { basis: candidate.basis, committed: true, settlement: 'Settled',
        verdict: 'Verified', evidence: [actualEvidenceReference] };
    },
    async prepare(continuation, { signal, frame }) {
      return { run_id_next: freshRunId, objective: actualVerifierFeedback };
    },
  },
});

// An explicit cancellation requests settlement; it does not claim rollback.
handle.cancel();
const result = await handle.wait();
```

`runTask(options)` is the same operation with the wait included. The result
contains `delivery`, `outcome`, process status, callback completion and the Host
receipt directory. `outcome.task_accepted` remains false. A validated IPC result
does not accept a business Task or establish external-effect settlement.

The child gets the credential only as `ARA_AGENT_MODEL_TOKEN`. The client does
not write the token or callback exception messages to its receipts. Host callbacks
own their original business receipts; the client records correlation and hashes.
Paths, credentials and tool grants must come from trusted Host configuration.
`binaryArgs` is an optional, bounded Host-selected argv list, used by ordinary
Node protocol fixtures; it is never model input.

Each reply echoes the exact original RPC/task/Run correlation and, for a tool,
its native call ID, tool name and supplied argument hash. The hash is echoed as
the Rust correlation field; it is not a caller authorization claim. Callback
payload references and final outcome references must be below the original Task
evidence directory and match their actual SHA256 bytes. The reference checks do
not create a hostile-filesystem sandbox.

## Closed call-budget snapshots

The default remains `budget_stop_review: 'reject'`, including when the field is
omitted. An Execute Host can opt in with the top-level
`start.budget_stop_review: 'review_closed_tool_round'`. Both dedicated callbacks
must be callable before the client starts a child:

```js
startTaskFrame.budget_stop_review = 'review_closed_tool_round';

const callbacks = {
  tool, verify, prepare,
  async verify_budget_snapshot(snapshot, { signal, frame }) {
    // Product code reads actual artifacts and commits its review evidence.
    return reviewBudgetArtifacts(snapshot, { signal, frame });
  },
  async prepare_budget_stop_continuation(continuation, { signal, frame }) {
    // Return only a fresh run_id_next and the actual correction objective.
    return prepareBudgetRepair(continuation, { signal, frame });
  },
};
```

`reviewBudgetArtifacts` and `prepareBudgetRepair` above are Host-provided business
functions. The first returns the usual `ReviewReceipt` bound to the complete
`snapshot.basis`. The second receives `previous` as a budget snapshot, its
committed rejection receipt, `next_run_number`, `remaining_calls` and the original
`deadline_unix_ms`. It returns only `{ run_id_next, objective }`. Neither callback
falls back to the ordinary `verify` or `prepare` callback.

The Core Run remains failed with `code=-1`. Its optional
`agent_end.termination` describes a saved, paired native tool round that stopped
at the call limit: version 1, cause `call_budget_exhausted`,
`native_round_closed: true`, `model_final_stop: false`. A snapshot contains
`basis`, `diagnostic_output`, `goal` and `termination`; its diagnostic text is not
a model final answer. The Host must check the actual artifact and effect
receipts. Unknown effects, cancellation, timeout, context errors and persistence
failures still prevent admission or release.

The client handles separate `verify_budget_request` and `prepare_budget_request`
RPCs and checks their fixed original files before and after the callbacks. A
validated terminal may carry `budget_review_ref` for
`run-N-budget-review-index.json`, whose kind is `closed_budget_snapshot_review`.
The client validates its snapshot hash and exact current receipt, then writes a
separate `budget-review` ledger entry. `result.outcome` remains unchanged; a
budget review has `last_candidate: null` and its receipt in `last_receipt`.
`readBudgetReviewReference(reference, evidenceDirectory, task, outcome)` can
recheck that separate reference. An earlier Run's index cannot replace the
current receipt.

`Verified` reaches only `AwaitingHostAcceptance` with `task_accepted: false`.
`NeedsChanges` may use only the original remaining Task calls, Run count and
deadline through the dedicated continuation. Exhausted budgets stop without an
extra model call. Old Hosts and default frames keep their existing behavior;
an old sidecar rejects the new opt-in field. Existing public Rust struct fields
and ordinary callbacks are preserved. See the
[snapshot contract and evidence](../../docs/evidence/closed-budget-snapshot-20261025.html)
for this bounded path; broader capability and product acceptance remain open.

## Cancellation and recovery

`callback_cancel` aborts the corresponding callback's `AbortSignal`. The original
callback still has to settle and return its original reply. Normal cancellation
waits for both callbacks and the Rust process. A callback that ignores its signal
can keep `wait()` pending. `forceKill()` terminates the child, marks delivery
unknown, and never claims descendant cleanup or product rollback. Callback
settlement is still awaited. A disconnect retains original operation receipts;
the client does not restart a child, change Session or replay an operation.

## Native probe and product binding

`native-probe.mjs BINARY CONFIG.json ROOT` is a small real Rust consumer. Root
supplies the controlled HTTP provider separately. The probe writes a real answer
file, checks it by readback, commits exact-basis review receipts and applies
bounded feedback through the sidecar. It saves the full original Task outcome
to `ROOT/node-outcome.json` and separate client/process/counter records.

The ordinary tests below use an explicit Node protocol fixture and injected
stand-ins. They do not prove Rust behavior, model capability, product integration
or real TodoService/PostgreSQL acceptance.

```sh
node --test clients/node/client.test.mjs clients/node/labslot-todos.test.mjs
```

`labslot-todos.mjs` and `../../examples/labslot_host.mjs` are a candidate product
binding. The product must inject its actual `createTodoTools` instances, convert
their original schemas, close authorization over the current captured identity,
read back real database rows, classify effect settlement, and commit original
operation/review receipts. The binding restricts updates to the selected todo's
priority. It never copies TodoService, starts Nest, registers devices, or changes
the default LangGraph/provider path. Protected readback rows should use stable
ordering so a serialization difference is not mistaken for a business change.

The captured actor, conversation, real user-message ordinal and confirmation
identity stay fixed across automatic feedback Runs. An internal Run is not a new
human confirmation. The current LabSlot tools often return Chinese text on
errors; a product settlement callback must use actual receipts/readback rather
than infer success or rollback from that text. Real product and database
acceptance remains open until a separately authorized fixture executes it.
