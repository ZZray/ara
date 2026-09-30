# Production RPC dependency: owned Run configuration

**2026-09-30, tested WIP; no production RPC acceptance.** Source baseline:
OMP `596f2da7101178214aa27a753529d15e6b7ad91d`. ARA base: `e1933249`.

## Requirement and delivered scope

A persistent Agent must accept a fresh owned configuration for each Run while
keeping its transcript, queues, queue modes, cancellation and recovery guard.
Original APIs keep their stored defaults. This is a direct dependency of the
[production Host plan](../rpc-host-plan.md), not a replacement for that Host.

| Path | Requirement mapping |
| --- | --- |
| `crates/ara-agent/src/agent.rs` | Add `prompt_with_config` / `continue_run_with_config`; wrap supplied hooks with canonical queues; original APIs delegate original default clone |
| `crates/ara-agent/tests/agent_loop.rs` | Five executable API/ownership regressions using existing provider/tool fixtures and controlled Notify gates |
| `scripts/rpc_dispatch_oracle.py` | Reproduce unchanged fixed-source dispatcher/shutdown tests before Host scheduling implementation |
| Plan, architecture, ledger and handoff | Record complete command denominator, real interface dependencies and bounded WIP status |

Fixed `packages/agent/src/agent.ts` creates a loop configuration in `#runLoop`.
ARA accepts a Run snapshot explicitly; this **does not** implement OMP live
system prompt/tools/model/reasoning updates between model calls (`1439–1446`,
`1483–1486`). The free loop, Session format, provider, print/REPL and HostSink
implementation files were unchanged by this patch.

## Windows execution

Code snapshots reviewed and tested before commit:

- `agent.rs` SHA256 `92964b1d96c702a43ab51204b50c670d7ba7c330a5bb8dc88e8f7a895efe8b67`
- `agent_loop.rs` SHA256 `d754a4a2d967c7f5467a192eef76f5168d3a032d061afe7ed5ed4f59dec26b57`

| Check | Command | Actual result |
| --- | --- | --- |
| Focused | `cargo test -p ara-agent --test agent_loop run_config -- --nocapture` | 5 passed / 0 failed; final compile 4.29 s, test 0.02 s |
| Complete backend | `python scripts/verify_backend.py` | Format, workspace/all-target/all-feature Clippy and tests, doc tests passed; 1,098 passed / 0 failed / 1 ignored across 85 suites; CLI e2e 87/87; 108.741 s |
| Dependencies | `cargo deny check` | Advisories/bans/licenses/sources passed; existing warnings retained; 9.440 s |
| Scoped diff | `git diff --check` | Passed; existing Windows LF/CRLF conversion warnings only |

Local complete receipts:
`C:\Temp\ara-rpc-host-run-config-validation\backend-windows.log`,
`backend-windows-result.json`, `cargo-deny.log`, `cargo-deny-result.json`.
First focused compile took 25.64 s. The full continuation's starting time was
not recorded; these durations describe executed commands only.

The five tests exercise:

1. Supplied provider/model/system prompt/tools/options, supplied context and
   queue hooks, permission behavior, tool effect and model-call budget; subsequent
   original `prompt` restores default provider/tools/hooks.
2. Expired default configuration followed by two fresh-config prompts and a
   queued continuation, preserving the same transcript and `QueueMode::All`.
   Original `continue_run` still observes its expired default and leaves queues.
3. Empty, user-ended and paired-tool-result-ended constructed transcripts
   continue with supplied hooks/deadline; an already recorded tool is not replayed.
4. Notify-controlled cancellation after supplied hook dequeue restores FIFO
   inputs; an expired continuation leaves them queued; both new APIs refuse an
   unpaired tool tail with no provider/tool effect.
5. Busy refusal leaves queued input; dropping the waiting caller at a controlled
   tool boundary leaves the owned Run alive, produces exactly one tool effect
   and records the final tool-result message in `RecordingSink`.

The last two are API interleavings, **not durable Host journal evidence**.
Constructed transcript continuation is **not process restart or native
provider Session resume evidence**. No production RPC binary, live model task,
queue durability or live configuration claim is made.

## Executed fixed dispatch baseline

Unchanged `packages/coding-agent/test/rpc-input-frame.test.ts`: **15 passed,
0 failed, 62 assertions**, Bun **1.4.0**, first run 19 ms.

Original retained run:
`C:\Temp\ara-rpc-host-design-oracle\run-20260930T032900Z-f1e076a1`.
Source manifest SHA256:
`404a05355c77942b5ca10331477b8937945eb22bffca10b5bb17f520622617f5`.
Git blobs and exact extracted declaration ranges were hashed; helpers only
re-export source. Dispatcher/pending-request declarations are extracted from
`rpc-mode.ts`, with full host-tool/proxy/essential/Snowflake sources and exact
URI/schema guards. No substitute behavior was used.

This establishes source scheduling evidence: ordinary serial commands,
immediate control frames, tracked background bash, command error recovery,
pending/future host-tool and extension-UI rejection after EOF, and one-shot
shutdown coordination. It excludes full `runRpcMode` initialization/EOF,
native `AgentSession`, actual process pipes and product teardown.

The promoted reusable script was also executed on its final bytes:

```text
python scripts/rpc_dispatch_oracle.py --upstream C:\Temp\omp-596f2da --bun C:\Temp\ara-ctx-skill-sort-oracle\bun-windows\bun-windows-x64\bun.exe --output C:\Temp\ara-rpc-host-design-oracle
```

Result: 15/0 in 1.078 s total (test process 0.047 s), run
`run-20260930T034340Z-59391115`; manifest SHA256
`096552bb5d7c88665c57e6847519008a4d741181569651962011762d2b2c7a33`;
script SHA256 `ce9598f6d1352df33ae670f1051addd635d38c787549a60d69502436e0e4700b`.
The full source exports and helper bytes were rehashed after execution.

## Independent reviews and current decision

Plan: three independent Codex views and cross-review, all approve the narrow
prerequisite and full Host ownership direction. Diff: `rpc_host_pragmatism` and
`rpc_host_architecture` reviewed both complete changed code/test paths at the
hashes above; no paths skipped, no actionable defect. Architecture independently
reran the five focused tests: 5/0, 0.02 s. Performance reviewed the configuration
ownership and executed the fixed-source dispatch baseline.

Final scoped audit by `rpc_host_pragmatism`: **approve, 9/9 changed paths
reviewed, 0 skipped, no unresolved finding**. Eleven oracle source blobs were
compared byte-for-byte with fixed Git, and all four extraction ranges, five
helpers and two archived harness scripts matched their manifest hashes. The
42-command map matched the pinned command union exactly. Three documentation
source ranges were corrected and verified; Rust/script bytes did not change.
The final audit permits a scoped WIP checkpoint only.

**Decision: tested WIP.** Production Host entry, full 42 commands/side channels,
live settings, compaction replacement, Session changes, durable queued Skill
provenance, actual RPC task/cancellation/restart/shutdown and real-model task
remain open. Linux delivered-code CI is recorded separately when available.
CA-RPC, A5, P2 and the full OMP marker do not
advance.

## Delivered-code Linux gate, 2026-09-30

Exact `600cf762f33c39053f710e32b0d677862c819452` repository run
[36666427491](https://github.com/ZZray/ara/actions/runs/36666427491), job
`109731963935`, completed successfully. Its actual log records **1,101 passed,
0 failed, 1 ignored across 85 suites**, including CLI e2e **90/90**.
The backend step took 228 seconds (03:53:38–03:57:26 UTC). Downloaded log:
`C:\Temp\ara-rpc-host-run-config-validation\github\logs-36666427491\verify\7_Run python scripts_verify_backend.py.txt`.
This closes the predecessor's Linux compilation/regression check; production
Host acceptance remains separate.
