# Simplification proposals

Status: proposals 1, 3 and 10 are **implemented (round 2, 2026-09-29)**; see `docs/port-map.md` section 3 items
15-17. The others are proposals only: each would change behavior or the wire format and needs an explicit decision first.

Context: the state machine (task states, `version` + `generation` fencing, 120-second lease, `unconfirmed`,
`confirm-stop`, cursor-based ack, 24-hour retention) is correct but asks a lot of an AI caller. Most of the friction
below shows up as extra tool calls, extra tokens, or mistakes an AI makes because it cannot keep a timer or a cursor
in its head. Items are ordered by expected benefit over risk. "Risk" is the risk of adopting the change.

Source references: Go = old ARA `tools/ara-lite` (`core.go`, `message_store.go`, `main.go`); Rust = `src/store/mod.rs`
(`Draft`/`execute`), `src/store/messages.rs`.

## 1. Let the worker's own writes renew the lease

- **What:** `progress` by the owner also extends `lease_until`, like `heartbeat`. Today only claim, heartbeat and
  review(changes_requested) set it (`execute` arms `"claim"`, `"heartbeat"`, `"review"` in `store/mod.rs`; the
  `"progress"` arm never touches `lease_until`).
- **Why:** A worker that reports progress every minute still expires after 120 seconds, and a model cannot run a
  separate 30-second timer. Progress is an authenticated action by the owner, so it is proof of life. The prompts
  currently need a rule "heartbeat before and after slow steps" only because of this split.
- **Risk:** Low. A stuck worker that keeps calling `progress` would hold the task; the master can still see the progress
  text and use `confirm-stop`. Changes when `unconfirmed` appears, so tests that pin expiry timing need an update.
- **Status: implemented (round 2).** Tests: `store::tests::owner_progress_renews_the_lease`,
  `tests/e2e.rs::cli_lifecycle_against_a_real_service`.

## 2. Let the owner's action restore an `unconfirmed` task

- **What:** `progress` or `submit` by the task owner on an `unconfirmed` task acts as heartbeat and returns it to `running`.
  Today `progress` fails with "task is not running" and `submit` with "task is not running; renew an unconfirmed claim
  before submitting".
- **Why:** After a service restart every running task is `unconfirmed`. The worker must discover this from an error, call
  `heartbeat`, re-read versions, and only then continue. That is three calls, and `progress` gives no hint what to do.
- **Risk:** Medium. `unconfirmed` exists so the master can tell "maybe the worker is gone". Auto-restoring on any owner
  call is safe only if the caller's generation still matches; keep the generation check and the master's ability to
  `confirm-stop` at any time.

## 3. Do not require `version` + `generation` for `claim`

- **What:** `claim` needs only `task_id`. The state check `task is not queued` already prevents a double claim, and the
  response returns the new `version`/`generation`.
- **Why:** Today `claim` runs `get_task`, which rejects a wrong `version` or `generation`, so a worker must `list_tasks`
  first just to copy two numbers; `task version conflict` on a claim carries no information the state check lacks.
- **Risk:** Low. Fencing still applies to every later write, which is where the numbers matter. A claim racing a
  `confirm-stop` requeue could win a task the master meant to re-review; the master can see this in the `claimed` event.
- **Status: implemented (round 2).** Values that are sent are still checked; every other operation still needs both.
  Tests: `store::tests::claim_may_omit_version_and_generation`,
  `tests/e2e.rs::claim_without_version_or_generation_over_cli_and_mcp`.

## 4. One opaque `etag` instead of `version` and `generation`

- **What:** Return and accept a single string (for example `"12.3"`) as the fencing token. Keep the two counters inside.
- **Why:** Two numbers, both mandatory on almost every write, are the most common source of caller mistakes ("reused an
  older one"). One token is copied whole.
- **Risk:** High for compatibility: every client, prompt and stored example changes. Worth doing only together with a
  wire-format version bump. Can be added as an alias first (accept either form).

## 5. Make `ack` monotonic and drop `expected_cursor`

- **What:** `ack` takes only `ack_seq` and moves the cursor forward if `ack_seq` is higher; lower or equal is a no-op.
  `poll` keeps returning `ack_cursor`.
- **Why:** There are five cursor terms today (`after`, `next_cursor`, `ack_cursor`, `expected_cursor`, `ack_seq`). The
  `expected_cursor` check protects two consumers sharing one profile, which the prompts already forbid.
- **Risk:** Medium. A second consumer of the same profile could acknowledge messages it never saw. Keep the check as an
  optional argument if shared profiles must stay safe.

## 6. Stop retaining task events that `list_tasks` already states

- **What:** Do not store `progress` events as messages; keep them only in task state (`progress`, `progress_note`), and keep
  `claimed`, `submitted` and the review results as messages.
- **Why:** A busy worker floods the master's inbox with `progress` events; the master must poll, read and ack them, spending
  tokens on data available from `list_tasks` at any time. It also makes `retention_gap` more likely to matter.
- **Risk:** Medium. The master loses the per-step progress history and the "something moved" wake-up on long-poll. A
  cheaper variant: coalesce to the latest `progress` event per task.

## 7. Accept worker names wherever `worker_id` is accepted

- **What:** `create` and `send` take `worker` as an ID or a name. Names are already unique per worker (`join` refuses a
  duplicate worker name).
- **Why:** The master has to call `list_clients` and copy a 24-character hex ID. Names are what the user told it.
- **Risk:** Low. Rename is not supported, so a name is stable. Reject IDs that equal another worker's name to avoid ambiguity.

## 8. Make `--snapshot` optional in the CLI `review`

- **What:** The CLI captures the live snapshot when `--snapshot` is omitted. The server still requires the review's
  snapshot to equal the one recorded at `submit` (`submission snapshot mismatch`, `"review"` arm). The Rust MCP
  `review_task` already works this way; the CLI (and the Go CLI) still requires `--snapshot`.
- **Why:** The two-step "snapshot, then review" exists to prove the master looked at exactly the delivered state, but a model
  easily skips or mis-times it. If the workspace changed after `submit`, the live snapshot differs and the review fails anyway.
- **Risk:** Low to medium. It weakens the explicit "I inspected this snapshot" evidence: a change between inspection and
  review is not caught by the client-side re-check. Keep `--snapshot` for callers who want the proof.

## 9. Clearer names for the two kinds of `heartbeat`

- **What:** Split `heartbeat` without `task_id` (client presence) from `heartbeat` with `task_id` (task lease) into two
  operations, or drop the presence form.
- **Why:** One op name, two behaviors and two result shapes (client vs task). Any authenticated operation already updates
  `seen` (end of `execute`), so the presence form adds nothing a `poll` or `status` call does not.
- **Risk:** Low. The presence form only touches the client's `seen` time.

## 10. Lower the default poll wait for AI callers

- **What:** MCP `poll_messages` default 25 s becomes 5 s (max stays 30). The HTTP API default without `timeout` is 30 s
  in Go (`main.go` 177) and stays.
- **Why:** Many hosts time tool calls out at 30-60 seconds and a model gains nothing from waiting longer than one step. The
  worker prompt already asks for 1..5 seconds.
- **Risk:** Very low. It changes a default, not the protocol.
- **Status: implemented (round 2).** Tests: `tests/mcp.rs::poll_without_a_timeout_waits_five_seconds`,
  `tests/e2e.rs::mcp_lifecycle_against_a_real_service`.

## 11. Fold `clear-tasks` into per-task `cancel`

- **What:** A `cancel` op for one queued/awaiting task, keeping `clear-tasks` only if bulk reset is really needed.
- **Why:** `clear-tasks` needs `workspace`, `expected_count` and a reason, and it refuses while anything is running.
  Its guard (`expected_count`) protects a bulk delete; a single-task cancel needs no count.
- **Risk:** Medium. New op, new state, new tests. Do not add it unless bulk clear proves too heavy in practice.

## Not proposed

- Push notifications or waking an AI: impossible from the service and outside the design ("nothing wakes an AI").
- Weakening the `confirm-stop` evidence rule or the loopback-only, no-`Origin`, bearer-token rules.
- Replacing SQLite: the current durability (`synchronous=FULL`, one transaction per operation) is the point.

## Suggested order

1, 3 and 10 first (small, low risk, remove the most AI mistakes; done in round 2), then 2, 7, 8, then decide on 4, 5, 6 with a wire
version bump. 9 and 11 depend on how the tool is actually used.
