# ara-workflow

`ara-workflow` is a public Rust library above the shared Full Core. It owns a
bounded sequence of runs on the caller's existing `Arc<ara_agent::Agent>`.
After a normal assistant stop, the Host checks actual artifacts and effects,
commits a review receipt, and can request a repair in the same Agent history.
`VerifiedArtifact` leaves Task acceptance with the Host.

The caller supplies `WorkflowRequest`, `WorkflowPolicy`, the existing
`AgentConfig` and event sink, and an implementation of `WorkflowHost::review`.
Start with `Workflow::start(...)`, retain the returned control handle, then
call `handle.wait().await?`. `WorkflowHandle` is cloneable and `wait(&self)`
returns the same `Arc<WorkflowOutcome>` on repeated calls. Dropping a waiter
does not cancel the owned worker. Call `handle.cancel()` to close admission;
the worker still awaits the original Core run and Host callback settlement.

For a complete local consumer using the actual `WriteTool`, persistent
`SessionJournal`, controlled HTTP provider and joined native file-reading
process, run:

```powershell
cargo run -p ara-workflow --example controlled -- NEW_EVIDENCE_DIRECTORY
```

The example writes an incorrect artifact, receives committed Host feedback,
repairs that artifact in the same Agent, and seals the final evidence. Its
scripted provider establishes deterministic orchestration behavior. Actual
coding and reverse-analysis gains require separate real-model task evidence.

The Host exclusively owns the supplied Agent and Session generation throughout
execution, review, cancellation and reconciliation. It must bind all tools,
event persistence and receipts to that generation, join external effects,
detect artifact changes, and commit its own review evidence before returning
`ReceiptCommit::Committed`. The library checks receipt identity and structure;
the Host remains the authority for artifact truth and settlement.

Only `Completed` with a reliably exposed final assistant `StopReason::Stop`
can reach review. Wrong-basis, uncommitted or unknown receipts cannot request
another run. With `Workflow::start`, errors, length stops and ambiguous terminal
records stop the workflow. Feedback contains its attempt, Session generation, criteria revision,
artifact revisions and committed receipt ID as Core input provenance.

`Workflow::start_with_length_recovery` explicitly enables a bounded recovery
path for a completed, tool-free Length response. Supply
`LengthRecoveryPolicy { max_continuations }` and implement the Host's
`settle_length_stop`. Its separate `LengthSettlementReceipt` has no success
verdict: it binds actual artifacts, effect settlement and persistence to the
same attempt. A committed, settled receipt with nonempty continuation feedback
permits a new input in the existing Agent and Session. `None` stops after
settlement. Unknown effects, invalid receipts and callback errors stop admission.

The canonical owner seals eligibility before exposing shared message aliases.
Graph bodies, non-completed Runs, final tool calls and provider errors cannot
use this path. Continuations retain the original cumulative request allowance,
attempt limit and absolute deadline, with an additional workflow-wide length
continuation limit. Cancellation still joins the Host callback. Only a later
normal Stop can reach ordinary artifact review and `VerifiedArtifact`.

The Core already handles certain Length responses containing tool calls, and
the reference CLI has its own recovery policy. This public entry does not copy
those policies, perform compaction, change reasoning settings, rewrite history
or replay a truncated tool call. A route may still reject or exhaust its next
request; bounded continuation does not guarantee task completion.

For a deterministic HTTP provider with real file writes, Journal provenance
and joined native children, run:

```powershell
cargo run -p ara-workflow --example length_recovery -- NEW_EVIDENCE_DIRECTORY
```

The example settles a partial file after Length, supplies sourced continuation,
and verifies the repaired file after a normal Stop. It proves the integration
mechanism. Real-model recovery and coding gains require separate task evidence.

`max_prepared_requests` is cumulative across attempts. It counts reservations
at an additive final-provider gate, preserving existing primary and additional
Host hooks. It measures logical prepared requests; HTTP retries, tokens and
costs need Host accounting. The Core's direct compaction/handoff provider calls
are outside this gate. This module does not initiate that side work. The
absolute deadline applies across runs and review; an uncooperative callback
can remain pending during cancellation because its effects are unresolved.

If settlement is unproven, `WorkflowOutcome::reconciliation` retains a closed
gate. Keep at least one control handle until obtaining the outcome and
reconciling its exact basis. After joining the real effects and committing a
current receipt, call `fence.release(&receipt)`. Release is synchronized and
idempotent for a valid receipt. Dropping all handles or the fence leaves
admission closed and loses in-process access to that fence; it does not settle
effects. Persistent receipts and cross-process recovery belong to the Host.

`WorkflowHost::drive_run` defaults to awaiting the original Core future. A Host
can wrap that sealed future in its native driver. The SDK retains a canonical
owner, rejects late first polling after the driver returns, and cancels and
joins an admitted run if the driver returns early. Driver failures have their
own typed result. The canonical owner seals terminal admission before exposing
shared message records to the driver; later record edits cannot turn an
original length stop into a reviewable stop. The present terminal check supports typed Core messages;
Graph bodies that cannot expose a checked model snapshot are rejected. This
seam does not establish CLI Native Graph review or continuation support.

This library adds no product identity, permission system, storage framework,
model routing, Task acceptance, new Session, automatic effect replay, Lite Core
unification or cross-process resume. The caller continues to own those ports.

`examples/live_cpp.rs` is an opt-in consumer of the same public API. Set
`ARA_CPP_COMPILER` to an absolute C++ compiler path and supply the explicit
route through `ARA_MODEL_BASE_URL`, `ARA_MODEL_ID` and `ARA_MODEL_API_KEY`.
Keep the key in the caller's environment. Then use a new absolute directory:

```powershell
cargo run -p ara-workflow --example live_cpp -- --run-live NEW_EVIDENCE_DIRECTORY
```

It bounds the workflow to 16 logical prepared requests, three attempts and
300 seconds, using the provider's default reasoning behavior. It saves the
actual Session journal, input provenance, source revisions, process receipts
and Host reviews. Five public examples and 24 additional Host cases exercise a
small C++ decimal parser. This is an integration smoke task; it does not replace
the broader multi-file coding or reverse-analysis acceptance tasks. Running the
example without `--run-live` prints usage and sends no model request.

`examples/project.rs` applies the same library to an existing C++ project. The
Host supplies a JSON configuration with separate absolute `project_root` and
new `evidence_root`, explicit `readonly_paths` and `writable_paths`, fixed
`public_check_commands`, `public_case_receipts`, `review_command`,
`review_result_path`, and `expected_hidden_total`. Commands use an argument
array and a timeout; the only substitutions are `{source}`, `{build}` and
`{evidence}`. Writable originals must be listed `.cpp` or `.hpp` files.
Reads authorize the original base path and preserve finite line windows from
the existing ReadTool, including `:N-M`, `:N+K`, tail and finite multiple ranges.
Raw and open-ended reads are rejected. Writes require an exact original path.

The public command sequence configures, builds and runs tests. If an earlier
compile step fails, cases remain unrun. After the last command, the Host
checks every configured `{build}/.../case.json` receipt for an integer
`exit_code` and an `errors` string array. Missing receipts, timeouts or cleanup
errors cancel the shared workflow. A normal semantic mismatch can produce
repair feedback. The independent reviewer receives a sealed source copy and
pinned raw Full Core Session records, including original append IDs and input
provenance. Its result distinguishes executed failures from unrun cases and
unknown effects. The Host passes only public output and aggregate private
counts to the model.

```powershell
cargo run -p ara-workflow --example project -- --run-live CONFIG_JSON
```

Use the same three model environment variables as `live_cpp`. This example
sets cumulative bounds of 32 logical prepared requests, three attempts and
900 seconds, with at most 16384 output tokens per response. Optional
`generation_policy` defaults to `provider_default`. Explicit
`thinking_type_disabled` uses the existing provider policy API to send
`thinking: {"type":"disabled"}` on the selected Chat route. The caller must
verify that its route accepts and honors that wire contract; a catalog flag
does not establish its remote effect. The example asks the model to submit
after a public PASS so independent Host verification can drive further repair.
It exposes the real read/write tools and fixed public checks. The
project verifier is a Host adapter; its oracle and task data are separate from
the public library. Tool restrictions and source seals are not an OS sandbox.
Killing and reaping an original child does not prove descendant settlement;
unknown effects prevent further admission and require Host reconciliation.

## Two-process context and knowledge example

`examples/continuity.rs` uses the existing Agent and Session interfaces to
prepare two tool-free Runs, summarize their actual sourced messages, and
reopen the same persisted Session in a new process with a new Agent. It keeps
the original user correction, source revisions and pending action available.
The second process reads a project index and its linked note through the real
file tools, then submits a JSON artifact for Host verification.

Build the `continuity` example once, then invoke that ordinary executable in
two separate processes with the same new absolute evidence directory:

```powershell
continuity.exe prepare NEW_ABSOLUTE_DIRECTORY
continuity.exe resume NEW_ABSOLUTE_DIRECTORY
```

The default uses a controlled local HTTP upstream. For an authorized live
trial, set the three model environment variables described above and pass
`--run-live` to both invocations. The live example's metadata matches the tested
Chat route; verify a different route before adapting it. Its cumulative limit
is 16 logical calls, including summary calls, within the original 600-second
wall-clock deadline. Reopening does not replenish that allowance. HTTP retries
and billed tokens are separate measurements.

The Host publishes the prepare receipt after syncing the Journal and source
records. Resume checks their original hashes and budget ledger, then acquires
a synced exclusive marker before admitting calls. An existing marker or failed
resume requires inspection; do not delete the marker to retry. Protected files
and original provenance are checked again during final review. Acceptance
requires that actual tool results contain the index link and note's random
fact before the successful write, as well as an exact artifact match.

This is a text/plain-soft-compaction consumer example. It uses strict checked
observations of ordinary persisted sources and refuses live Graph owners,
archives, provider-native replay and explicit unknown effects. It does not
restore the original Agent instance or expose a general Session converter.
Its two granted knowledge files are a bounded retrieval fixture, not a broad
knowledge-search benchmark. The successful controlled and real-model trials
and remaining limits are recorded in
[the workflow evidence](../../docs/evidence/core-workflow-20261023.html#continuity).
