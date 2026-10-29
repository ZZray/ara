## Current composed checkpoint and retained-store repair - 2026-10-08

Fixed OMP remains `596f2da7101178214aa27a753529d15e6b7ad91d`;
ARA dirty `dev` base `20aa0e2ef9872f633ba2671376cdf778477e7115`.
Full OMP remains **0/111 complete**. This closes selected failures, not the
complete EDIT/AST/TUI/module or product acceptance.

### Executed composition v5

Packet: `D:/workspace/env/cache/ara-port-candidates/edit-composed-20261008-v5`.
The reviewed four-path changes restore intrinsic Map distinct-newTarget
construction, replace obsolete Map observations, correct ConPTY observation
boundaries and add original retained-fixture failure diagnostics. Their complete
source snapshot contains 63 paths, including 48 Rust files. Build succeeded in
748.798s with 418 compiled artifacts; switching the registry source prefix from
C to D invalidated the old cache. Subsequent builds keep the D prefix stable.

The exact delivered CLI lib artifact's original four actual-tools tests passed
in 1.501s. Their unchanged executable/source receipt was reused. New focused
execution passed six tests and failed the original retained REPL test; this is
**ten distinct passed tests and one failed**, not a single new all-green run.
The diagnostic proved `/clear` passed while `/new` produced the actual edit
receipt `isError=true`, `File not found: math.py`, leaving the file unchanged.
Original failed output is retained. The first Python runner failed to decode
UTF-8 stderr under Windows' default GBK encoding. A separate UTF-8 runner/log
root preserves that failure and completes all focused observations in 22.989s.

Fresh real Windows ConPTY passed all eleven checks: actual Bash child `started`,
visible aborted receipt, same-process next prompt and answer, two model calls,
original Session roles and aborted assistant, and idle exit 130. Observed
cancellation latency is 0.76s, idle exit 0.02s; observer command took 6.352s.
These are controlled-model real-terminal observations, not a real-model trial.
The earlier v4 failures remain historical evidence.

Source manifest SHA256:
`b7ef345d57c8ae6f8b3a9352d847991eab598db8c2655917e0f26eaa79da3b7d`.
Receipts: `early-actual-execution/result.json`,
`focused-execution-utf8-v1/result.json`, and
`focused-execution-utf8-v1/fresh-conpty-v5/results.json` in that packet.
Independent historical v5 execution report:
`D:/workspace/env/cache/ara-port-candidates/edit-composed-execution-post-20261008-v5`.

### One-hunk retained EditStore repair

Packet: `D:/workspace/env/cache/ara-port-candidates/retained-edit-repair-20261008`.
`main.rs::rebind_owned_tools` previously used `new_tool_context` on `/new`,
losing the previous read snapshots. Fixed `edit/store.ts` lazily stores one
EditStore on its owner; fixed AgentSession.newSession keeps that AgentSession
and its EditStore. Source was read at the fixed revision; the downloaded
store's URL and SHA are retained in `source-receipt.json`. Independent Codex
`/root/edit_vcs_review` approved the one-hunk repair: rebind only the previous
EditStore to the newly constructed ToolContext. Fresh journal, URI and pending
proposal owners follow the existing reset logic. The shared Rust Store clone
aliases its original Arc; `/clear` behavior and test expectations are unchanged.

The affected e2e target build passed in **162.915s**, **413 artifacts reused,
one compiled**. The existing original tests all passed in **2.505s**:

- `repl_clear_and_new_keep_retained_edit_snapshots` (1.804s): both boundaries,
  real nested-file effect, successful persisted edit receipt and four requests.
- `repl_new_starts_a_fresh_session_file` (0.233s).
- `repl_clear_compact_and_reopen_keep_only_the_active_suffix` (0.462s).

The test executable is unchanged (`bcc81337123f53920e858aa0ec5d3461ed306111cd0ce142afd60f571b2676d7`);
it invokes the repaired real CLI at the existing binary path. CLI SHA256:
`35ebd3bca548443b499941e2d91ecb356350e3515f080cf57ac1d3072b51ef52`.
Source manifest SHA256:
`70ac33e5297262681af880bddf33cc5103fe323bc9584cff6b75fef6375587b2`.
The other 62 source paths match v5. Git index bytes remain
`89d34e26be97c90a14680d44ace937e6b1ae5a86e8ee7b46dd6b90718a922475`.
Scoped rustfmt check passed. Build and raw stdout/stderr are in `build-result.json`
and `execution/result.json` plus the three named log pairs. No optional whole
workspace run or new model trial was performed for this narrow repair.
Independent Codex POST **APPROVE scoped repair**:
`D:/workspace/env/cache/ara-port-candidates/retained-edit-repair-post-review-20261008-v1/REPORT.md`.
Root read the actual report and raw outputs; this approves the repaired workflow,
without adding full module or surface completion.

Current next batch is the complete required dynamic renderer wiring, including
Session callback context, source-backed cumulative display coalescing and
ownership/lifecycle. Source-only plans are not execution evidence. All prior
failures and source approvals remain retained; no formal acceptance marker,
commit, push, deployment, installation or durable memory promotion is changed.

---

# EDIT failure repair batch — 2026-10-08

Target: dirty `dev`, HEAD `20aa0e2ef9872f633ba2671376cdf778477e7115`;
fixed OMP `596f2da7101178214aa27a753529d15e6b7ad91d`.
This repairs the five failures from the separate 625/5 execution. It does not
replace that failed receipt or accept complete EDIT/OMP.

## Scope and independent source review

| File | Required correction |
| --- | --- |
| `ara-cli/src/main.rs` | Borrowed side factory excludes the main request-text observer sender |
| `ara-cli/src/model_route.rs` | Clone only, clearing the Completions observer; preserve auth, usage, callbacks, policy and Session owners |
| `ara-cli/src/actual_tools/async_effect.rs` | Read the saved same-Session failure without consulting revoked admission or consuming the receipt |
| `ara-cli/src/native_run.rs` | On Promise drain failure, retain original builtin failure; preserve original error when no saved failure exists and Compound on restoration failure |
| `ara-cli/src/actual_tools/ast_edit_native_tests.rs` | Add diagnostics to the unchanged grant-denial predicate |
| `ara-cli/src/native_observations/edit_post_apply.rs` | Two success observations use the original Graph flag's truthiness, matching upstream's omitted false flag |
| `ara-cli/tests/support/ast_edit_rpc.rs` | Create this fixture's cache parent before model selection |
| `ara-cli/tests/e2e.rs` | Correct only the Bash successor test's toolResult/abort-assistant indices; retain original content/stopReason assertions |

All paths above are under `crates/`. No new test entry, relaxed timeout,
permission reopening, replay, Session replacement or unrelated source change.
The observer test retains its original five-second budget. Original iterator
close, grant denial, late observation, no-write, repair/order/journal and
same-Session/restart requirements remain.

Independent Codex `/root/repl_metadata_v6_pre` used `ara-git-review` and
`ara-rust-core-review`: **SOURCE APPROVE, zero source blockers**.
Report: `C:/Temp/ara-model-fallback-batch/edit-failure-repairs-independent-source-pre-v1/REVIEW.md`,
SHA256 `7235d2c145305e78145662e0800fd3f2ca31e533a89b6e97932b12594cc13f81`.
The prior runtime POST remains REJECT. Source approval does not establish a
repaired runtime outcome.

## Delivered application

Application packet:
`C:/Temp/ara-model-fallback-batch/edit-failure-repairs-application-v1`.
Applied UTC `2026-10-07T23:47:11.264343+00:00`.
Eight reviewed paths applied incrementally, with original bytes backed up;
candidate comparison normalizes only Git's CRLF conversion. Scoped Rustfmt
was applied only to these Rust paths. The original 776-path boundary includes
the prior three selected vendor paths: exactly the eight allowlisted paths
changed, and the Git index stayed unchanged.

The reviewed draft plan only gained the actual review binding and approved
status. Final plan SHA256:
`2de69f698e70ade9499ab7fab182886428bd36fb2709af0132cab8e32a182f64`.

## Execution — terminal failure

Root invoked `execute-edit-failure-repairs-v1.py` with the application receipt
and `C:/Temp/ara-model-fallback-batch/edit-failure-repairs-execution-v1`.
Application-to-execution source and index equality are required before running.
Scoped format check passed in 0.280s. One cached compilation completed:

```text
cargo test -p ara-cli --all-features --locked --lib --bin ara
  --test e2e --test rpc_host --no-run --message-format=json
```

Cache: `D:/Temp/ara-build/ara-github-target-20261005`; jobs/test threads 2.
Executed the original CLI lib/bin and RPC module plus four original
REPL EOF/cancel/successor/deadline tests. Unchanged Core/Edit/Tools results retain
their original receipts; no fictional merged all-green result is reported.

| Entry | Passed | Failed | Seconds |
| --- | ---: | ---: | ---: |
| Cached compilation | — | — | 294.043 |
| CLI lib | 245 | 0 | 13.089 |
| CLI bin | 81 | 0 | 9.910 |
| RPC Host | 61 | 1 | 121.105 |
| Four original REPL/deadline cases | 4 | 0 | 2.843 |

Terminal Cargo/test exit is **101**, with **391 passed / 1 failed**.
Total wall time is **445.277s**. Source, index and harness hashes were preserved.
The earlier five failures now have four passing repairs; the original AST RPC
group gets past startup and preview but exposes a production reminder-admission
failure. Its original artifact assertion stays unchanged.

An unchanged-binary RPC diagnostic took **0.938s**, with one provider request.
AST preview reports two replacements and `applied=false`. A new
`resolve-reminder` enters context as RawJson; Actual request preparation then
rejects it because the Host has not admitted it into the current Session Graph.
No second provider request or `ast-accept` write ran; the file remains unchanged.
Raw frames, requests, journal and artifacts are retained in
`C:/Temp/ara-model-fallback-batch/edit-ast-rpc-unchanged-diagnostic-v1`.

Independent Codex `/root/repl_metadata_v6_pre`: **FINAL POST REJECT**;
SOURCE/APPLICATION APPROVE is retained separately. Final report:
`C:/Temp/ara-model-fallback-batch/edit-failure-repairs-independent-post-v1/REVIEW.md`.
The selected CAS AST task is unrun while this deterministic gate fails.
### Two-file reminder admission repair — executed, still rejected

The candidate is limited to `resolve.rs` and `main.rs`, reusing the existing
Host message-creation adapter. Independent Codex `/root/repl_metadata_v6_pre`
gave SOURCE PRE APPROVE, zero blockers. Report:
`C:/Temp/ara-model-fallback-batch/ast-rpc-reminder-admission-independent-pre-v1/REVIEW.md`.
Both delivered paths match the approved candidate exactly after scoped format.
Only these two source paths changed; the original 776-path boundary and index
are preserved. Application receipt:
`C:/Temp/ara-model-fallback-batch/ast-rpc-reminder-admission-application-v1/receipt.json`.

One same-cache/all-features compilation builds only CLI bin and RPC targets.
The original CLI bin passes **81/81**, 9.971s. The original AST RPC family
fails **0/1**, 2.422s, now at `tests/support/ast_edit_rpc.rs:65`: no persisted
`resolve-reminder` journal entry. Original preview/no-write, apply, discard,
same-Session and four retained tool-result assertions before that line pass.
The six-response, request soft-choice and restart assertions after that line
remain unexecuted. No test expectation or timeout changed.

Compilation is **202.980s**, total **216.689s**, terminal exit **101**.
Source/index/harnesses remain unchanged. Binary SHA256:
`87ae84c6458c3e68839df9c4a43b2d5cb9cd7aa89dbb83d31f2bccef6bd7961c`.
Execution receipt and original logs:
`C:/Temp/ara-model-fallback-batch/ast-rpc-reminder-admission-execution-v1`.

The admission repair has executable evidence; complete AST RPC acceptance
still fails on persistence. Independent runtime POST is pending. The prior
391/1 gate remains a separate failed receipt; its unchanged lib/e2e and other
RPC results are reused without reporting a fictional merged all-green gate.
Selected AST/CAS remains unrun. Next: locate and repair the actual reminder
creation-to-journal path, preserving all original assertions and Session identity.
Complete OMP stays 0/111; no full marker, phase, product or release acceptance.
No commit, push, installation, deployment or durable memory update.

## Latest closure — original AST RPC and retained CAS task pass

The admission production repair above remains unchanged. Source inspection of
fixed `session-manager.ts:2476–2501`, current SessionJournal and
GraphJournalEncoder established the canonical flat custom-message shape. The
single existing fixture now checks that shape, retaining existence and adding
`display=false`, the original tool name and complete content. No flow, input,
timeout, Session, restart or continuation requirement was relaxed.

Final application:
`C:/Temp/ara-model-fallback-batch/ast-rpc-reminder-journal-observation-application-v2`.
Original grouped execution:
`C:/Temp/ara-model-fallback-batch/ast-rpc-reminder-journal-observation-execution-v1`.
Scoped format passed in 0.095s; cached RPC compilation passed in 55.409s.
The original
`ast_edit_rpc::rpc_ast_edit_proposal_apply_discard_soft_reminder_and_restart_group`
passes **1/1**, exit 0, with every original preview/apply/discard, four tool
results, six responses, soft-choice/context, restart/history and no-replay check
executed. Total gate 60.316s; original test footer 2.94s. Unchanged CLI and other
module receipts are reused, not reported as a new aggregate all-green run.

The retained actual CAS Run is
`C:/Temp/ara-model-fallback-batch/delivered-ast-cas-receipts/20261008T004253Z`.
Its original FAIL receipt stays unchanged. Read-only reassessment fixes the same
flat-journal observation and writes `receipt-journal-observation-corrected.json`:
**PASS**. No new model request, process run or tool effect occurred during
reassessment. The original CLI Run exited 0: deepseek-v4.1-flash,
openai-completions, five calls in 7.219s, read → ast_edit → write(xd://resolve)
→ read. Contemporaneous preview retained the original file, two replacements
and `applied=false`; apply changed exactly two call sites, and readback retained
the original `oldApi(99)` comment. Four tool call IDs each have one start, end
and persisted result in Session `01a118f6-98f0-72b4-aafb-e3b807a72e0d`.
Usage retains actual values; cost remains unknown. Real-model restart was not
repeated; restart/no-replay evidence is from the original controlled RPC group.

Independent Codex `/root/repl_metadata_v6_pre` final POST: **bounded APPROVE,
zero blockers**. Original report:
`C:/Temp/ara-model-fallback-batch/ast-rpc-reminder-journal-observation-independent-post-v1/REVIEW.md`.
Root rechecked the actual output, receipt and current hashes on 2026-10-08:
main.rs, resolve.rs and the fixture match the delivered gate; production
binary SHA256 remains
`87ae84c6458c3e68839df9c4a43b2d5cb9cd7aa89dbb83d31f2bccef6bd7961c`.
This closes the exercised ordinary AST workflow and observation correction.
Full dynamic AST source contracts, renderer41, VCS5 and production services
remain required; complete OMP is still **0/111**.
