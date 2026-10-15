# Durable history, bounded Goals and managed Jobs

Implementation follows the [pinned upstream research](long-task-research.md).
This page describes the Host contracts; executed acceptance and remaining gaps
belong to the delivery evidence. It does not advance root OMP/Core acceptance.

## 0.2.0 context and checklist additions

Execute has a Run-local `task_update` checklist (at most12 items/32KiB), with
evidence required for done. Its complete tool result is journaled; frontend
delivery failure is explicit. Plan remains read-only, and Goal retains its own
durable steps rather than mirroring them into a second task store.

Static Goal instructions remain system rules. Fresh Goal/Job observations are
labelled Host data in the projected working view, excluded from original raw
messages and charged against the projection budget before compaction. Model
call observations record system hash/UTF-8 bytes, not a token or cache guarantee.
Positive normalized adapter usage has explicit provenance; missing/zero values
cannot establish free consumption or cache misses. The completed compaction
receipt is emitted only after saving, using global raw indexes even after hot
history eviction. See the [version evidence](../evidence/version-0.2.0-20261016.md).

## Native Session history

`session_journal.rs` separates original messages from the model working view.
Append segments are normally at most1MiB, with an explicit4MiB single-record
ceiling and8192-segment capacity. The synced, checksummed head commits the
sequence, hash chain, active range and Host metadata. Pending intent, a torn
tail or an unconfirmed Run index fences execution instead of truncating or
replaying an operation. Completed Run IDs remain in immutable per-ID indexes
after eviction from the recent64-entry list.

The JSON Session file is a replaceable hot projection. Maintenance starts at
128 messages/2MiB; the512-message/8MiB hard bound accommodates settlement of an
already issued tool batch. Only a closed prefix covered by a verified summary
or revocation watermark leaves hot memory. Summary ranges and watermarks use
global raw indexes. `history_search`/`history_read` return bounded, hash-bound
raw receipts in the current native scope; revoked ranges cannot re-enter the
model through these tools. A receipt already truncated by an old tool cannot
reconstruct output that was never retained.

Legacy migration preserves exact v1 JSON in a separate backup, validates its
transcript and old summary provenance, then imports originals in segments.
An interrupted import remains fenced. A legacy active tail over8MiB without
sufficient valid summary coverage is explicitly rejected for inspection; full
automatic migration of every old16MiB file is not claimed. Process interruption
tests do not establish power-loss durability on every filesystem.

## Goal authority and accounting

The Node is the only Goal scheduler. The Hub authorizes and mirrors; it does
not start a second orchestration loop. An explicit controller policy binds
controller/device/native Session/Goal, absolute expiry, maximum model-call
reservations, maximum slices and optional Job permission. Limits are at most
24hours/256calls/32slices; the UI requests1hour/48calls/8slices. The Hub clips
expiry to the device approval. A model cannot extend any of these values.

Each slice has a fresh Run ID and a fresh Hub model grant. The first selection
pins the Hub's logical model/protocol/binding; continuation cannot replace that
tuple. First selection uses the configured directory/default without a routing
model. A logical Auto alias retains the Gateway's random behavior and does not
prove a fixed physical downstream.
Explicit model selection is a Host decision and is not sent to the Gateway as
an invented upstream route decision. Existing incompatible native bindings fail
explicitly; starting a compatible new web Session is a user action.

Root reservations are synchronously committed before every agent or summary
HTTP request. Timeout and unknown usage do not refund them. Slice budgets do not
reset the root budget. A closed tool round can yield; the Host resumes an idle
Goal only while the contract and root allowance remain valid. Waiting for a Job
uses Host receipt checks, with no model polling loop. Completion is a proposal
in `awaiting_review`; `accept` checks the Goal revision and reviewed settled Run.

Only an explicitly leased Goal continues when the browser closes. Ordinary
Runs, uploads and unrelated queued work retain foreground cleanup semantics.
Returning with the same controller observes a live leased Run without replacing
it. Pause/stop uses stable command IDs and durable result receipts. A canceled
dispatch fence wins over late state observations and survives restart. One
unresolved leased Goal owns a native Session at a time.

Node restart preserves budgets and receipts, but does not manufacture a lost
Hub binding or erase a pending-effect fence. Reconnection may require the
controller to return and review authorization. No fresh Session, automatic
permission extension or unknown script replay substitutes for native recovery.

## Managed process contract

`job_start`, `job_status`, `job_read_log` and `job_cancel` use a detached worker
of the same EXE. The Host fixes owner, lease and quotas; tool arguments contain
only the intended script/interpreter/cwd/timeout and stable `op_id`. A durable
intent forbids a second spawn for the same operation, including ambiguous
startup. Reusing an operation ID with different input fails explicitly.

The default Host policy retains8 Jobs, permits one active Job, limits combined
stdout/stderr to8MiB and individual duration to3600seconds within the absolute
Goal expiry. The default requested timeout is600seconds. A fresh Host directory
exposes receipt identities/statuses without scripts or log bodies, so a later
slice can find its Job after cloud-context withdrawal without restoring revoked
cloud text. Log reads are new, bounded observations with byte cursors and hashes.

Windows starts the command suspended, assigns its Job Object, then resumes it;
the worker owns that process tree. Unix uses a separate process group. A dead
worker is `interrupted` with unknown effects, never an invitation to restart it
or kill an unrelated reused PID. Unix worker SIGKILL can leave descendants;
strict descendant termination after that fault is not claimed. This boundary
requires reconciliation before dispatching another owned Job.

Cancel records the dispatch fence, requests active Run cancellation, waits for
settlement, then scans Jobs again so a concurrent `job_start` also receives a
cleanup marker. A marker is a request; only the worker's terminal receipt
confirms its cleanup. Existing file/network effects are not rolled back.

Files and scripts retain the OS account's authority. Neither the working
directory nor a tool allowlist is an OS sandbox. This implementation adds no
computer desktop API, vector database, automatic Skill adoption or arbitrary
distributed workflow engine.
