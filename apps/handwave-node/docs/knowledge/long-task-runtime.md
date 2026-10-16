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

### Native terminal and pipe input (0.2.4)

`terminal=true` implies interactive input and creates native ConPTY on Windows,
PTY on Unix, initially24rows/80columns (rows10..300, columns20..500). Terminal
stdout/stderr share a single byte artifact in stdout; stderr is empty. Tools
`job_terminal_resize` and `job_terminal_signal` share the original input lease,
owner and durable operation IDs. Ctrl+C is a terminal control key, not guaranteed
Job cancellation; EOF is Ctrl+D on Unix or Ctrl+Z+Enter on Windows and leaves
the terminal writer open. `job_cancel` remains the separate process-tree stop.
Terminal operations have8KiB per input,1MiB total text/2048 control receipts,
and a separate2MiB atomic journal. Pipe limits below stay unchanged.

The worker retains a bounded vt100 screen and answers live DSR5/6/private6,
primary/secondary DA and ESC Z requests through the same serialized writer.
It never reparses historical artifacts for input. Query floods or input/output
service faults interrupt the Job and clean up the owned terminal tree. Screen
state is bounded300x500 with no scrollback. OSC colors, DECRQM, DCS and all
xterm extensions are not claimed; browser xterm displays VT output and forwards
only actual user input. Its pinned6.0.0 CoreService user-input adapter fails
closed if that interface is unavailable; upgrades require the actual-vendor
keyboard/IME/paste/query tests. Windows requires OS ConPTY support.

Linux cleanup includes process groups in the original PTY session, including
shell job-control groups. A child deliberately creating a new session escapes
that boundary; it is not an OS sandbox. Non-Linux Unix session-wide group
cleanup has not been run in this delivery. Windows owns a Job Object assigned
before the child resumes. Closing ConPTY runs off the Tokio thread while the
output pump continues draining.

The browser starts observing and may attach only after snapshot replay and
xterm parsing finish. Keyboard is scoped to its focused terminal; input batches
are bounded,80ms and single-ACK serialized. Attached visible views refresh at
150ms without overlapping requests, with an immediate read after input/signal
ACK; observation retains2s polling. Disconnect, pause, owner/Job changes and
unknown ACK discard unsent bytes. Credentials can be echoed by the terminal,
so direct input is excluded from chat storage, not guaranteed absent from logs.

### Pipe compatibility


An explicitly leased `job_start` may set `interactive=true`; the default and
legacy noninteractive Spec serialization remain unchanged. The same detached
worker retains stdin across model Runs and controller reconnects. This is a
pipe; use terminal=true for the native terminal above. Neither provides desktop control. Interpreters remain supplied
by the host machine. Windows suspended -> Job Object -> resume is preserved.

`job_write_input` writes exact text (including any intended newline), and
`job_input_status` retrieves metadata-only receipts. A normal Execute follow-up
can access existing-process tools only after the Host checks the durable
Runner cancellation fence and native Goal against the identical original lease;
it cannot gain `job_start`. Plan remains read-only. Original Goal identity,
device/Session ownership, both Host and worker deadlines, and cancellation
markers gate new input. A new Goal cannot renew an old process's input authority.

Input uses authenticated bounded loopback IPC to the original worker. The
worker serializes input, commits a pending receipt before writing, then records
written or unknown. Stable input IDs prohibit duplicate writes or conflicting
replacement text. Partial writes, failed acknowledgement and pending outcomes
are reconciled by receipt; they do not authorize a new ID or automatic replay.
Limits are8KiB per input and64KiB/64 operations per Job. EOF closes stdin.
Writes are bounded independently of cancellation and process-tree cleanup.

The browser console uses a separately authorized Hub signal and does not call
the model for refresh, input, EOF or cancellation. It defaults to collapsed;
expanded views read bounded stdout/stderr chunks with independent cursors.
Selection generations reject stale log responses. Controller/device/Session/
Goal changes fence replies. Before input, sessionStorage retains only scoped
Job/input IDs; after a refresh, an unresolved input disables new submissions
until its receipt is checked. Input text is not stored there or in chat/native
history by this direct path. A child program can still echo its input into logs.
Model-mediated text input is ordinary tool history, so credentials belong in
the direct console rather than chat/tool arguments. Private worker nonces are
removed from model/browser receipts and runtime observations.

The exact Windows process tests, controlled browser checks, real-model
regression and unverified platform/publication boundaries are recorded in
[0.2.4 evidence](../evidence/interactive-0.2.4-20261016.md).

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
