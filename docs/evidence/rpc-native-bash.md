# Native RPC user Bash checkpoint — 2026-09-30

Status: **tested WIP**, two bounded command implementations (`bash`,
`abort_bash`). RPC now has **23/42 bounded variants**; full CA-RPC, fixed OMP
parity and P1–P6 remain open. Fixed upstream marker is unchanged.

## Source and scope

OMP source is `596f2da7101178214aa27a753529d15e6b7ad91d`:

- `rpc-mode.ts:399–418,440–447,1404–1413`: Bash bypasses ordinary serial
  admission, returns a correlated typed result; abort cancels user-shell jobs.
- `session/bash-runner.ts:69–133,162–171,183–265,275–343`: captured Session
  target, independent tokens, deferred streaming receipts and transition
  destinations. Completed jobs cannot change their starting owner.
- `session/messages.ts:931–943,1024–1040,1195–1206`: native `bashExecution`
  receipt, User text projection and optional context exclusion.
- `session/session-manager.ts:2299–2320`: non-active branch append preserves
  the live leaf; index rebuild (`265–272,1166–1173`) selects the last raw entry
  on reopen. No new persisted leaf marker was added.
- `exec/bash-executor.ts:58–73,428–444,651–700` and
  `session/streaming-output.ts:1307–1368`: result fields, cancelled/timeout
  notices and body-only line/byte counters.

Six code/test files implement this boundary:

| File | Direct responsibility |
| --- | --- |
| `ara-tools/src/bash.rs` | Extract the existing process runner into a typed result; retain model BashTool formatting and process containment |
| `ara-session/src/lib.rs` | Native receipt, User projection, branch append, compaction/source/recovery recognition |
| `ara-session/tests/bash_execution.rs` | Native import/reopen/fork/rollback, exclusions and unknown-effect recovery |
| `ara-agent/src/agent.rs` | Atomic generic idle transcript updates preserving queues and recovery guards, plus tests |
| `ara-cli/src/rpc_host.rs` | Independent stdin Bash dispatch, tracked process/supervisor tasks, serial receipt owner, destinations and safe flush, plus fault/dispatch tests |
| `ara-cli/tests/rpc_bash.rs` | Seven actual RPC child/provider/process/journal scenarios |

The reference Host retains one Session writer while an outstanding Bash target
owns that file. Same-file reopen (including a normalized path alias) replaces
the journal under its shared lock, avoiding a stale snapshot rewrite. Different
Session/branch adoption keeps original jobs detached. Failure does not retarget
them. Current receipts defer until the owned Run joins, before another prompt
or queued continuation. Valid excluded Bash is transparent to interrupted-call
recovery; included or malformed Bash remains a context barrier.

Normal EOF drains accepted Bash and owed replies. Connection failure cancels
and joins jobs. A receipt write failure stops the connection and next prompt;
already performed process/file effects remain visible. The live model append
and disk append are not one transaction; this failure boundary is exercised.

## Executed Windows evidence

Receipt root: `C:\Temp\ara-bash-batch`. Six final source byte hashes are pinned
in `full-receipt.json`, `receipts-audit.json` and the actual-task summary.
They are equal before/after final verification and the task.

| Check | Result | Time |
| --- | --- | --- |
| Typed runner / idle APIs / native Session / Host dispatch and disk fault | 21/0/0 across 4 focused suites | 26.731 s, including compilation |
| RPC actual child cases | 7/0/0 | 1.64 s test execution |
| `python -X utf8 scripts/verify_backend.py` | 1,258/0/1 across 95 suites; fmt, all-feature Clippy, target and doc tests pass | 106.704 s |
| `cargo deny check` | Advisories, licenses, bans and sources pass | 2.446 s |
| `cargo build -p ara-cli --all-features` | Pass | 0.397 s |

Final full gate covers the delivered source after the identity-closure Clippy
correction; earlier focused evidence predates that semantics-preserving expression
change. Final executable SHA-256:
`8fc4d496377a97349aefd2f18a40d06329f23eb331bd2c4c3c627c537e4cc19a`.

The actual child cases cover UTF-8 output and nonzero exit, native/public/model
views and restart; parallel user jobs and child-tree cancellation while model
Bash continues; new/switch/fork owner retention; same-file alias reopen and
failed switch; held streaming response with deferred receipt; and EOF drain.
Additional real-child Host tests hold serial Run settlement while Bash starts,
and inject actual journal failure after a file effect. Native tests include
create/import/reopen and included/malformed exclusion companions.

Initial failures are preserved. `focused-rpc-first-failure.log` exposed the
disabled completion-channel branch (background-only input had no serial wakeup).
The Host now always polls that channel. The subsequent 6/7 run caught a fixture
reading a new file before its assistant settled. Fixtures now honor the fixed
lazy-file rule and assert absence before Run completion. Cancellation-output
and excluded-recovery findings were corrected with source-backed tests.
`full-first-clippy-failure.*` preserves the lint failure. No failing Run was
promoted to acceptance.

## Bounded actual-model task and artifact audit

Final task: `real-bash-q6xsxhp8/summary.json`, using a freshly queried B.AI
`deepseek-v4.1-flash` catalogue and OpenAI Chat Completions. Authorization stays
in the environment. Bounds: two Runs, three calls/Run, 512 output tokens/call,
35 s/Run and 100 s total. Executed driver `real_bash_artifact_trial.py` is retained.

The user Bash process obtains an unpredictable environment nonce, writes a
temporary source file and returns the nonce in its native output. The model
writes `result.json` from that receipt. After shutdown, source deletion and
native restart, the model writes `recall.json` with the identical value.

Result: **PASS**, **11.564 s**, **2 Runs / 4 calls / 1 native Bash / 2 write
receipts / 12,375 reported tokens**. Cost is unknown. Provider request bodies
were not captured; effects are confined to controlled trial files.

Parent `audit_receipts.py` checks six source pins, the executable/driver pins,
205 raw frames, six inputs, nine native messages, the native/public Bash object,
both complete native/live tool-result objects, Run terminals, usage and actual
JSON files. `receipts-audit.json` records **PASS** and final artifact hashes.

An earlier task in `real-bash-rv3bm8_f` is **FAIL**, retained at 25.647 s: artifact
and nonce recovery were correct, but its exact text-format assertion rejected
an extra space. Its executed driver is unchanged. The final task uses a new
independent nonce and verifies requested JSON artifacts; it does not relabel
the earlier failure or replay an uncertain operation.

## Independent audit and remaining gates

Independent Codex `host_bridges_independent_review` applied scoped Git and Rust
Core review before and after implementation. `pre-review.txt` records required
single-writer, atomic-idle and fixed-dispatch conditions. `post-review.txt` and
`post-review-final.json` record both initial P2 findings resolved and bounded
code/focused-evidence approval. `final-runtime-doc-review.json` supersedes that
earlier focused snapshot: final Windows/full, actual task, six committed blob
bytes and scoped documentation receive bounded approval. Two documentation
wording findings were corrected in this record.

Delivered code is `30bd6d81e8bbe7c7fbc8b54f5a0a36f7f54d8b69`.
`commitblob-audit.json` verifies all six tested source files equal the committed
Git blob bytes. Exact-code Linux [repository run 36725776546](https://github.com/ZZray/ara/actions/runs/36725776546)
passes **1,261/0/1 across 95 suites**, including fmt, all-feature Clippy, target
and doc tests, in **319.341 s** including fresh dependency compilation.
`linux-final.log`, `linux-final-status.json` and `linux-audit.json` retain the
raw log, exact SHA, commands and independently recountable results. The status
API returned stale pending step fields after its terminal success; raw commands
and final PASS supply the execution evidence. The same-code fixed Skill workflow
36725776096 also succeeds; it does not substitute for this backend gate.

Remaining behavior gaps: persistent brush shell state, PTY/user-shell settings,
artifact spill, column caps/middle elision, extension/interceptor integration
and the other RPC commands. Connection-failure/panic and autonomous queue plus
Bash combinations do not have a new dedicated runtime oracle in this batch;
EOF, cancellation, deferred streaming and disk-failure paths were exercised.
This checkpoint accepts no entire OMP surface or phase.
