# REPL allocation repair and remaining cancellation gate — 2026-10-08

**Narrow repair accepted; complete REPL lifecycle and EDIT remain open.**
Fixed OMP `596f2da7101178214aa27a753529d15e6b7ad91d`; ARA base `dev`
`20aa0e2ef9872f633ba2671376cdf778477e7115`, preserving dirty work and index.
Inventory remains 111 surfaces: 0 complete / 1 tested / 27 implementing / 83 open.

## Delivered scope

Only `crates/ara-cli/src/main.rs` changes for this repair. `run_repl_loop`
returns a boxed future wrapping the original body, renamed `run_repl_loop_owned`.
Removing the wrapper and restoring the original name makes the whole source
identical to its before snapshot. Session, cancellation, tools, quota settlement
and callers are unchanged. PE stack reserve remains 1,048,576 bytes.

- Before main SHA256: `4e2f78501e88559c17bf30b8bb5bde25753afed773c8bf7fdd3f2f228c1c68d0`.
- Final main SHA256: `1c3a4b51565003c7b39b4fd3370ab8f8630e9bd2b004b32ad46b9129e25cde69`.
- Final debug binary SHA256: `444ab9874f015ac8184f68dab9720e18da898a8468a5fc861d1d79ddb6e80883`.

The [initial CAS failure](edit-host-joined-20261008.md) remains retained.
Controlled 15ms SSE pauses reproduce the original REPL overflow; buffered REPL
and spaced print pass. A diagnostic larger-stack copy passes, but is not the
delivered binary. The boxed body passes at the original reserve. This proves
the observed repair without identifying a unique compiled stack frame.

## Executed evidence

Packet root below is `C:/Temp/ara-model-fallback-batch`.

| Applicable gate | Result | Command time |
| --- | --- | ---: |
| Scoped format | PASS | 0.169s |
| Cached Agent/Tools/CLI original-target compilation, same features/lock | PASS | 100.456s |
| CLI binary original groups | 81/81 PASS | 9.853s |
| Original real-process RPC Host groups | 61/61 PASS | 116.910s |
| Original thinking print/REPL group | 1/1 PASS | 4.304s |
| Controlled spaced print / spaced REPL / buffered REPL | PASS, two successful Reads each | 2.267 / 2.249 / 1.484s |
| Actual CAS REPL edit/readback | PASS, four model calls / four tools | 6.016s |
| Actual new-process original-Session recall | PASS, one model call / zero tools | 1.954s |
| Original REPL EOF journal group | 1/1 PASS | 0.822s |
| Original REPL Bash-interrupt journal group | FAIL, exit 101 | 0.639s |

The first combined gate totals 232.190s. The e2e addition takes 25.267s
compilation, 27.169s including both tests. Sources and binary remain unchanged
during execution. Reuse unchanged joined results; the earlier 507-pass/3-failure
command is not relabeled a single 510-pass batch.

Actual route: Manager-configured CAS, `deepseek-v4.1-flash`,
`openai-completions`; local Manager OMP management was unavailable. The model
reads sales, reads and edits summary, then reads the result back:
`{"pen":16.5,"book":34.5,"cup":17.0,"grand_total":68.0}`. A separate process
answers original CSV facts without answers in its prompt or tool replay.
Both use Session `01a11859-02d1-767c-a801-635a745732d1` and the same journal.
Five completed calls report 19,353 total tokens; price remains unknown.

Receipts: `joined-repl-allocation-repair-execution-v1/receipt.json`,
`joined-repl-allocation-repair-controlled-v1/20261007T215030Z/receipt.json`,
`joined-repl-allocation-repair-cas-receipts/20261007T215045Z/receipt.json`,
and `joined-repl-allocation-repair-e2e-v1/receipt.json`.

## Remaining cancellation discrepancy

The unchanged e2e reaches live Bash update, CTRL_BREAK, Agent terminal, a
surviving REPL, successor answer, EOF exit 0, two requests and one journal.
It fails at `e2e.rs:2486`: roles are
`model_change, thinking_level_change, user, assistant, toolResult, user, assistant`.
It expects another aborted assistant after toolResult. Its later stopReason
assertion was not reached. The preserved original binary fails identically;
the final binary was restored and hash checked. Comparison receipt:
`joined-repl-interrupt-before-control-v1/receipt.json`.

Root inspected fixed OMP `agent-loop.ts:1196-1245,1438-1538` and its
`finishAbortedStream`: an installed pre-model gate can stop without an extra
assistant after a closed tool turn; the default path continues to an aborted
stream receipt. Fixed-repository registration search finds Agent APIs and test
uses, but no coding-agent registration of these gate methods. Current Rust
actual turn-end propagates external cancellation before another assistant.
Independent contract reconciliation confirms that the default-path assertion
is valid. Keep this original failure; the later v5 attempt below is separate.

## Subsequent cancellation v5 attempt — not accepted

Independent Codex `/root/repl_abort_v3_review` SOURCE PRE approved the frozen
three-path candidate: `agent_loop.rs`, `e2e.rs` and `rpc_host.rs`. The default
ordinary tool continuation defers cancellation to the existing admission owner,
and the new aborted metadata reads the live selected model. The original e2e
assertions remain; only its two erroneous journal offsets change. The original
RPC held-tool/model-switch group retains success and adds the abort branch.
Earlier v3/v4 type/order SOURCE rejections remain in their Temp packets.

Packet: `joined-repl-interrupt-parity-execution-v5/receipt.json` under the same
packet root. One all-feature locked cached no-run build and the original groups
execute on delivered candidate pins. Format passes; compilation takes 446.199s.
The whole command takes 476.024s: **95 passed, 2 failed**, exit 101.

| Original executed group | Result |
| --- | --- |
| Agent lib / agent_loop / core_hooks | 11 / 56 / 21 PASS |
| RPC held model switch + abort, live abort, backpressure, persistence fault, unknown-effect no-replay | 5 PASS |
| REPL EOF / tool deadline | 2 PASS |
| REPL Bash cancel then next prompt | FAIL: empty assistant instead of successor text |
| REPL Skill cancel then next prompt | FAIL: one provider request instead of two |

The fixtures wait for AgentEnd and the Host's cancelled-turn receipt before
writing the next prompt. Root and the independent reviewer directly traced
`finish_admission_failure -> commit_final`: the new synthetic assistant is a
typed `AgentMessage::from_model` record. CLI and RPC both call
`convert_current_actual`, whose canonical-history consumer requires Graph
records (`actual_message_projection.rs:176-187`). It refuses the next request
before HTTP, explaining the empty error assistant and request-count failure.
The successful RPC abort case ends that Run without a successor prompt and
does not establish same-Session continuation. Keep both REPL assertions.

The next candidate must create only this new synthetic metadata with the
existing SessionValues/owned Graph boundary and retain zero extra HTTP/tool
effects during abort. Do not relax the consumer, convert existing canonical
Graph history through serde, replace the Session, or replay tools. Include a
successor prompt in the existing RPC coarse group. No second Cargo command or
real-model trial has been run for this failed candidate.

Delivered binary SHA256:
`4fabe61828f4f8ea49962cd79974cd44b8dcd89f557c632ff1237dc411b5a139`.
All 759 recorded source pins remain unchanged during the gate and the Git index
is preserved. Four earlier persisted allocation-gate source maps independently
differ from application-v5 only at the three allowed paths; application backups
and candidate hashes match. The first application preflight used the wrong Temp
source directory before mutation; after the correct copy, a slash-normalization
assertion failed before saving its receipt. Recovery verified delivered/backup
bytes without recopying. These application attempts remain recorded separately.

The allocation repair's earlier CAS/stack evidence keeps its original coverage;
it is not a real-model execution of this new binary. v5 cancellation and full
REPL/EDIT remain unaccepted; inventory stays **0/111 complete**.

## Independent review and limits

Codex `/root/native_realm_pre` PRE/POST accepts the sole allocation repair and
exercised Edit/receipt/recall families. POST:
`joined-repl-allocation-repair-independent-post-v1/REVIEW.md`, SHA256
`fd47ca28f649225faee46d94bf5ddc3142f0c3adebda3287ef7d7e015463817d`.
Root checked source/binary/review hashes, receipts and final artifact directly.

Complete lifecycle, AST, blackbox repair, renderer, VCS join, production ACP/LSP,
the Unix engine row, OpenAI account task, strict workspace Clippy, release and
full product gates remain open. No commit, push, deployment, installation or
durable memory promotion is part of this repair.
