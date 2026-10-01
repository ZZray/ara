# Cross-turn tool-call loop guard — 2026-10-01

Status: bounded Windows checkpoint, tested/audited WIP. Parent ARA commit
`b3c84d5a1bafb50aa0bb194600be6df8631fff97`; fixed OMP
`596f2da7101178214aa27a753529d15e6b7ad91d`. This adds behavior to existing
RPC variants: RPC stays 27/42, P0/V1 accepted, P1–P6 open and
`ported_through_commit=null`. Complete AI-STREAM-GUARDS, CA-TURN-RECOVERY and
CA-RPC are not accepted by this checkpoint.

## Fixed source and implementation scope

`C:\Temp\ara-tool-loop-batch\source\source-receipt.json` retains 12 complete
exports and SHA-256 values. Root compared every export byte-for-byte with
`git show` from the fixed object database; no moving upstream reference is used.

| Fixed OMP source | Delivered Rust owner and behavior |
| --- | --- |
| `ai/src/utils/tool-call-loop-guard.ts` and original detector tests | `ara-ai/src/tool_call_loop_guard.rs`: batch identity, recursive `i`/`__intent` exclusion, JS key/number/UTF-16 semantics, mixed exempt calls, first non-exempt report, exact threshold hit, no-call/all-exempt reset and result/argument summaries. |
| `coding-agent/src/config/settings-schema.ts:1476–1509`, `session/stream-guards.ts:201–215` | `ara-cli/src/rpc_host_settings.rs` and `rpc_host_loop_guard.rs`: enabled by default, threshold 5, exempt `hub`; raw settings changes or disable rebuild/reset the long-lived Host detector. |
| `coding-agent/src/session/agent-session.ts`, `stream-guards.ts:176–184,219–240`, `agent/src/agent-loop.ts::emitTurnEnd` | `ara-agent/src/{agent_loop,agent}.rs` and `ara-cli/src/rpc_host.rs`: awaited completed-turn inputs enter Core memory and the durable input sink after TurnEnd and before terminal/steering decisions. Error, abort and deadline gating use Core cancellation. State survives prompt/new/switch in the same Host. |
| `coding-agent/src/session/tool-call-loop-redirect.ts`, system redirect template and `utils/src/prompt.ts::render/format` | `ara-session/src/loop_guard_notice.rs`: fixed native custom type, agent attribution, display false, exact details/template, checked Developer projection, restart and compaction source IDs. Tool/Gemini templates reuse `ara-prompt` post-formatting; ThinkingLoop keeps its raw template. |

Changed files also include `ara-ai`/`ara-session` manifests and `Cargo.lock`
for existing `ara-rpc` wire values and `ara-prompt` formatting, both crate exports,
the AI module corpus, existing RPC/Session whole-flow tests, new RPC support
module and thin `scripts/verify_stream_guards.py`. No vendored content changes.
Tools, permissions, route/authentication and retry budgets retain their owners.

### Required remaining behavior

The pure detector and error receipt preserve a summary ending in an unpaired
UTF-16 surrogate. Today's Provider/model text remains UTF-8. Host refuses that
projection, emits lossless native details and cancels both Core child and Host
Run tokens, preventing automatic retry. **Full lossless model-text projection
is mandatory remaining parity work**, not an accepted substitute for OMP.

Historical ARA `b3c84d5` Gemini entries used the exact unformatted template.
Restore accepts only that exact old form or the native formatted form; raw
entries/IDs stay intact and model/event projections use native formatting.
Live Host inputs require the native form. This restricted compatibility is
source reviewed; the new Session workflow executes current native notices,
without separately executing old-format Gemini restoration.

Advisor, non-RPC Host adoption, terminal-tool special hooks, complete native
interruption/recovery, other stream guards and the full parent inventory remain
open. No new notice journal-write failure injection or precise cancellation
interleaving execution is claimed.

## Module testing and one shared gate

```powershell
python -X utf8 scripts/verify_stream_guards.py --module all --output C:/Temp/ara-tool-loop-batch --full
```

Development selects `--module ai|host|session`; selected executables alone are
compiled. `--full` shares the existing backend, inventory, cargo-deny and CLI
build gate. The deterministic runner never reads live credentials.

Final receipt: `C:\Temp\ara-tool-loop-batch\module-20261001T091537Z\receipt.json`.
Its 152 before/after/current source hashes match. All steps exit 0:

| Step | Executed outcome | Seconds |
| --- | --- | --- |
| Selected compile | PASS | 0.411 |
| AI module | 1 passed, 0 failed, 0 ignored | 0.022 |
| Existing RPC module | 18 passed, 0 failed, 0 ignored | 4.102 |
| Existing Session module | 1 passed, 0 failed, 0 ignored | 0.053 |
| Shared backend | fmt/Clippy/all target/doc tests; 1,503 passed, 0 failed, 20 ignored | 186.723 |
| Inventory / cargo-deny / binary build | PASS | 0.456 / 2.567 / 0.401 |
| Entire one-command check | PASS | 194.995 |

The AI corpus reuses the ten native detector input families as one module,
including canonical keys/numbers/UTF-16 checks. Three added RPC whole flows
exercise actual Rust children, sockets, tools and journals: repeated Bash
effects → native notice before queued steering → different artifact → original
Session restart; disabled/exempt/budget and UTF-16 refusal with retry enabled;
and detector lifetime across a new Session. The expanded existing Session flow
checks native roundtrip, Developer projection, compaction IDs and forged
content rejection. Existing retry/effect veto cases remain in the same module.

Earlier checks are retained as superseded receipts:

- `090058Z`: new test code referenced wrong existing harness helper names;
  corrected without changing product expectations.
- `090252Z`: module 20/0/0 before two review repairs, not the delivered gate.
- `091337Z`: new lossless test adapter called nonexistent `WireValue::remove`;
  corrected using existing `insert` without extending the production codec.
- `091456Z`: final module 20/0/0, source unchanged, 7.764 seconds before full.

## OMP native test reuse

`native-oracle/receipt.json` pins Bun 1.4.0, the exact command, import rewrites
and source/log hashes. Original test bodies stay unchanged: **10 passed,
0 failed, 25 expectations**, approximately 11 ms. Only imports are adapted.
The complete OMP suite and native primary/advisor Session suites were not run
by this extracted detector oracle. Rust execution is recorded separately above.

## Bounded real CAS task

Artifacts: `C:\Temp\ara-tool-loop-batch\live\20261001T091903Z`.
The existing private relay/process/restart harness reads the current Manager
configuration, checks the enabled route and live catalogue, and uses
**Manager → OMP management → CAS**, `deepseek-v4.1-flash`, OpenAI Chat
Completions. Credentials stay private; the Rust child receives a dummy relay
credential. No local account/configuration is modified or committed.

Bounds: two Runs, eight calls and 120 seconds per Run, 4,096 output tokens per
call, ten relay requests total. Observed six requests: first two deliberately
request identical `read(invoice.csv)` calls; four subsequent requests reach
actual CAS. This tests controlled triggering followed by real completion,
without claiming a spontaneous live runaway.

Root independently computes the expected totals from the original CSV and
checks `invoice-summary.json`: marker 15, folder 25.5, lamp 34.5, total 75.
Four tool results complete once each (`read`, `read`, `write`, `read`). Native
notice `8da40ca1`, parent `4b344cff`, has count 2, original arguments/result,
display false and agent attribution. The first actual CAS request and restored
request both contain the notice. No automatic retry occurs.

Restart preserves Session `01a0f6c2-99d7-7150-a110-d4c62ee15747`; with tools
disabled, recall returns original marker quantity 4, folder price 8.5 and total
75. Task takes 7.767 seconds, restart 1.244: **9.011 seconds across Runs**;
the receipt includes relay cleanup and totals 9.550 seconds. Root's `audit.json`
pins raw requests/events, journal and artifact. Missing controlled usage remains
unknown; four real usage records are retained and cost is unreported, not zero.
Runtime success is followed by explicit Root artifact acceptance.

Final binary SHA-256:
`5ea9448e1bd113bc634aa383af65ecdf55c8cbec912a0e50f4b3fdbc7646a66a`.
It matches the tested/live/current binary; no duplicate paid task is needed.

## Independent review and Root audit

Independent Codex `ctx_oracle_diff_review` reviewed the PRE plan and POST diff
under `ara-git-review` and `ara-rust-core-review`. PRE rejected an ordinary
steering-queue injection because in-memory/persisted order could diverge;
the dedicated awaited Core hook passed the revised review. POST found child-only
cancellation could permit Host retry and that raw tool templates bypassed native
formatting. Both were repaired and covered by the existing whole flows.

`C:\Temp\ara-tool-loop-batch\post-review.json` retains findings, original/final
source pins and final gate/live binding. Root separately verifies source exports,
152 delivered source hashes, raw task/journal/tool outputs, numeric recall and
final binary in `root-point-audit.json`. The bounded checkpoint is audited WIP;
UTF-16 projection and stated execution gaps prevent full parent acceptance.

Gate receipt SHA-256:
`2ca0fc9233c21a2ffbc0631c55f9ae6a6dacf76efb34068e7ce3f406c2d7a4d0`.
Root live audit SHA-256:
`7e9d81a6dfe250443ccea35ba37e30380cc9c55a6662f0711f9810f3866ddeeb`.
Independent final review SHA-256:
`fa3d830780fee570bfb83efb7039cb0991fc04792f09934d457c1ac4df801349`.
Documentation bootstrap and `git diff --check` pass after the plan/ledger update.

## Time and next batch

Source export began around 08:49 UTC. Integration and source/review fixes
dominate elapsed work; the three module suites take 4.177 seconds. The shared
backend includes 69 seconds rebuilding all target tests after dependency/owner
changes. Keep one Cargo owner, scoped development compilation, reused native
inputs and one final stable batch gate. Do not add a micro-test for each mapping,
repeat unchanged checks after compaction, or rerun an unchanged paid artifact.

Next: required overflow/interrupted/native special-stream-cap recovery and
remaining RPC/registry, with lossless model text/advisor/Host gaps retained in
the parity backlog. Actual OpenAI account authorization remains a separate live
gate. Full OMP completion time is not inferred from fixture counts.
