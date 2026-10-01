# Native terminal recovery and configured promotion checkpoint

Base `2e6d8f33c2585fb5489978bc59193b45b6ba962d`; fixed OMP
`596f2da7101178214aa27a753529d15e6b7ad91d`. Bounded WIP only:
CA-TURN-RECOVERY, AGT-COMPACTION, complete registry and P1–P6 stay open.
RPC remains 27/42 and the full port marker remains null.

## Observable scope and fixed sources

- `agent/compaction/compaction.ts:1006–1016`: a nonempty visible Length summary
  may succeed. Core preserves actual terminal reasons, requires matching event
  and message reasons, and retains cancel/deadline, empty, tool and image vetoes.
  Responses/Codex Length summary terminals require positive content-only wire
  evidence; prior Stop summary acceptance is unchanged. New Host Responses/Codex
  empty-Stop and Length recovery both require positive content-only evidence.
  A later fold failure/cancellation returns no accepted partial summary.
- `coding-agent/session/turn-recovery.ts:795–879`: tool-free empty Stop and
  classified provider-empty errors are durably dropped before continuation.
  Three recovery attempts precede a terminal cap. The fixed runtime developer
  reminder permits the next required tool call; it is not a new user source.
- `session-maintenance.ts:1840–2045,2154–2220`: current-model fresh Length and
  overflow try configured context promotion before available soft compaction.
  Actionable output resets incomplete attempts; unsigned thinking alone does not.
  Old/unknown Responses terminals and unsafe native output cannot recover.
- `role-models.ts:37–69`, `model-resolver.ts:209–234`, `thinking.ts:65–89`:
  exact configured target resolution, native suffix handling, distinct identity
  and finite strictly larger context. Promotion defaults disabled.
- The Host prepares the selected target's own protocol, endpoint, credential,
  model prompt and provider before publishing on the original Agent/Session.
  Missing target contracts/auth and invalid promotion settings fall back to
  configured compaction. Journal discard plus model metadata use one atomic
  rewrite. Stateful provider history is newly bound. Existing queues remain.
- `turn-recovery.ts:1064–1093`: a narrow Session API preserves the native
  `accepted-terminal-empty-stop` marker and only prunes a direct custom prompt
  parent. Ordinary user sources remain. Strict projection recognizes only this
  exact marker. The corresponding full Host acceptance caller is still open.

Exact source blobs and independent PRE receipts are in
`C:\Temp\ara-terminal-recovery-batch\source` and `pre-review.json`.

## Verification entry and evidence boundaries

`python -X utf8 scripts/verify_terminal_recovery.py --full` runs Provider,
summary, Session, daily configuration and actual RPC process modules, followed
by one stable backend/inventory/dependency/build gate. Only Root runs Cargo.

The module contains successful Length summary/continuation, successful empty
Stop continuation, unknown native successful terminals, target protocol switch
with an actual inflight queued follow-up, missing auth/disabled/invalid settings
fallback, durable empty-output cap/new prompt reset, reload and storage rollback.
Owned Host tests cover the fourth incomplete admission with prior attempts=3,
meaningful-output reset and failed-maintenance rollback, old None and foreign
Length veto. This is not a continuous three-round Length process trial: the
current whole-turn soft cut can reach no-cut rollback before that cap.

## Executed final snapshot

Artifact root: `C:\Temp\ara-terminal-recovery-batch`. Final gate
`module-20261001T135938Z/receipt.json` exits 0 with identical before/after
source hashes, in **137.539s**. Only ARA-owned packages are formatted.

| Module | Passed / failed / ignored | Execution seconds |
| --- | --- | --- |
| Responses wire | 33 / 0 / 0 | 0.668 |
| Summary | 11 / 0 / 0 | 0.196 |
| Session recovery | 4 / 0 / 0 | 0.073 |
| Daily configuration | 7 / 0 / 0 | 0.698 |
| Actual RPC processes | 13 / 0 / 0 | 7.746 |
| Full target/doc backend, 121 suites | 1,519 / 0 / 20 | 123.702 |

Module execution totals **9.381s**; compile is 0.642s. fmt, Clippy
(`--workspace --all-targets --all-features -D warnings`), all target/doc tests,
fixed inventory, cargo-deny and final CLI build pass. cargo-deny retains existing
policy warnings; exit 0 is not a claim of warning-free dependencies. This exact
snapshot has no Linux gate. The 20 ignored tests retain their existing rules.

### Default Windows stack regression and repair

The earlier full attempt `module-20261001T134524Z/backend.log` failed the existing
`host_bridges::side_channels_after_session_join_and_abort_are_consumed_without_cross_session_completion`:
child `main` overflowed its default stack after the old Run terminal and before
new-Session ACK. New recovery async state enlarged the owned completion call
chain. The narrow repair boxes the two production `completed` awaits in abort
and the main loop, retaining serial settlement, cancellation and join-before-adopt.
No stack setting or test expectation changed. All six existing bridge tests pass
in `stack-fix-host-bridges.log` (6.709s including compile), and the final complete
backend also passes. No backtrace was collected; future-layout growth is the
source-supported explanation with successful before/after runtime intervention.
Other failed attempts retain their receipts. In particular, old summary tests
that demanded rejection of nonempty Length now use a genuine Error negative case;
the REPL Length scenario requires its actual summary and existing V1 second-cut veto.

### Preferred real-model task

`live/20261001T140522Z/receipt.json`: **PASS, 11.573s**, verified current Manager
→ OMP management → CAS `deepseek-v4.1-flash`, `openai-completions`. Private
`用户.ry_switch` supplies route/auth; no credentials are copied into receipts.
Bounds: two Rust processes, 120s each, six model calls per Run, 2,048 output
tokens per call, maximum 12 relay requests. Observed: six requests, comprising
one controlled initial empty Stop and five actual CAS requests; no route fallback
or relay failure.

The first process reads `stock.csv`, computes folder `5 × 8.50 = 42.5` and lamp
`3 × 17.25 = 51.75`, writes and reads `stock-total.json`, and finishes with
`grand_total = 94.25`. CSV bytes remain unchanged. The second process disables
tools, reopens Session `01a0f7c8-bc2a-73b1-afec-1a1a25f1b1a5`, and returns
`folder_quantity=5; lamp_unit_price=17.25; grand_total=94.25` with no tool effects.
The journal has one exact discard marker and no discarded empty assistant or
developer reminder. Actual continuation requests contain the runtime reminder;
the reopened request does not. Both stderr files are empty.

Five actual assistant receipts report input 6,671, output 508, cache-read 9,472
and total 16,651 tokens. Cache-write and cost are unknown. The controlled empty
terminal has absent usage; it is not counted as zero-token CAS activity.
Binary SHA-256 remains
`8a4bba9d41d5dc4f42805d49f557145b7c1aba5bd2d0962c4f1f96226a1f3e1f`.
This trial proves controlled empty-terminal recovery followed by a real task,
not that CAS emitted the injected fault or that account/Responses live parity passed.

### Independent review and Root audit

Independent Codex `/root/ctx_oracle_diff_review` applies Git, Rust Core and
Provider review to the 17 tracked source/test files plus the module script,
against base `2e6d8f33c2585fb5489978bc59193b45b6ba962d`. PRE,
`continuation-pre-addendum.json` and `stack-pre-review.json` approved bounded
scope with explicit required conditions. `static-final-review.json` binds all
18 source hashes and records fixes for invalid-setting classification, target
auth fixtures, actionable-content detection and the Windows stack regression.
Root directly reads the raw source, gate, request/stream/journal and task artifacts;
`root-audit.json` records matching hashes and the exact acceptance limits.
Final independent execution closure is recorded in `post-review.json`.
This checkpoint is tested/audited bounded WIP; no new complete parent surface,
P1–P6 gate or full OMP marker is accepted.

## Required remaining work

- Full split-turn and method/reducer/rescue parity, manual-summary native
  one-shot retry defaults, orphan toolUse and unexpected-stop recovery.
- Full registry execution/auth projection, bundle-only promotion targets,
  pre-prompt/threshold promotion and the narrow already-promoted stale-overflow
  exception. Foreign Length stays blocked. Account promotion in RPC remains
  explicitly unavailable, matching its existing startup contract.
- Host terminal-answer acceptance flag/caller for the tested accepted-empty API.
- Whole-span 1 MB/256-source admission, approximate non-Claude token counts,
  lossless UTF-16 boundaries, complete REPL settings and durable exposure of
  every summary invocation. These are required work, not accepted differences.
- Native promotion ordinarily drops the selected failed branch; this checkpoint
  intentionally performs a stronger durable discard and atomic model receipt.
  Storage failure rolls back the original selection and stops publication.
- Linux evidence for this exact snapshot and actual OpenAI account/model trials
  remain unexecuted. Controlled fault plus real CAS continuation does not claim
  that CAS itself emitted Length, empty Stop or native overflow.
