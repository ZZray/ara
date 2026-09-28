# CA-MCP stdio first slice — WIP, 2026-09-28

Target: fixed OMP v18.1.8 commit `596f2da7101178214aa27a753529d15e6b7ad91d`, especially `packages/coding-agent/src/mcp/{client,tool-bridge,transports/stdio}.ts`. The latest tested ARA code is local WIP `4b71aef`. This is a bounded host path, not CA-MCP acceptance. The exact OMP checkout was verified at that SHA. The upstream marker remains unchanged.

## Scope and observable path

- Preserved: existing Agent loop, Session journal/recovery and Bash process lifecycle.
- Added: `ara-mcp` host-side JSON-RPC stdio adapter; CLI `--mcp-config` for one local server and exact `--mcp-allow server:tool` grants; direct executable argv; explicit environment mapping; bounded frame/catalog/result sizes; no automatic replay after an uncertain tool call. The fake MCP binary and integration test require the `ara-cli/test-fixture` feature, so default production builds exclude them.
- Deferred: HTTP/SSE, OAuth, resource/prompt catalogs, dynamic discovery, reconnect, `.cmd/.bat` wrappers and race-free Windows process-tree containment. On Windows, a per-server Job Object now terminates the assigned direct MCP child when the CLI force-exits; the spawn-to-assignment interval means a server could start a descendant before joining the Job.

The follow-up keeps one connection actor reading stdout while no tool call is active. It answers server `ping` requests with an empty result both while idle and during calls, as fixed OMP does. Calls wait on a single permit; an oversized outbound frame or cancellation before queueing is recorded as `executed: false` without stopping the server. A queued or written call interrupted later stays `executed: unknown`. If the caller drops the response receiver after dispatch, the actor closes that connection instead of waiting indefinitely or reusing it. Server JSON-RPC error messages are capped at 4 KiB before becoming tool/Session text.

The host launches an MCP child, negotiates protocol `2025-11-25`, sends `notifications/initialized`, lists tools, registers only granted tools as `AgentTool`s, then the ordinary Agent sink journals calls and results. A tool request that exits, times out or is cancelled after dispatch returns an error with `executed: unknown`; the model-visible text also says not to replay it automatically. The Session is resumed normally, without a new MCP call for the old effect.

The host config shape is `{"name":"local","command":"C:\\absolute\\path\\server.exe","args":[],"env_from_host":{"CHILD_VAR":"PARENT_VAR"}}`. Launch is opt-in with `--mcp-config <file> --mcp-allow local:tool_name`. `env_from_host` copies only named variables; keep credentials in the parent environment, not in the JSON file. The server's tool descriptions and returned content are untrusted tool data, not system instructions.

## Executed evidence

Commands ran on Windows PowerShell against the current worktree; no provider credentials were recorded in test output or files.

| Command | Observed result |
| --- | --- |
| `cargo check -p ara-mcp -p ara-cli` | exit 0 |
| `cargo test -p ara-cli --test mcp_stdio --features test-fixture` on the no-debug C: target after the Windows Job change and fault fixtures | 11/11 passed, including a real helper parent calling `process::exit(130)` with an MCP server still alive at exit |
| `cargo clippy -p ara-mcp -p ara-cli --all-targets --all-features -- -D warnings` after the Windows Job change | exit 0 |
| `python scripts/verify_backend.py` with the no-debug C: target after the Windows Job change | owned formatting and workspace Clippy passed; all-target tests reached CLI e2e and failed the existing Windows Bash `deadline_during_a_tool_and_zero_budget_exit_nonzero` elapsed-time assertion (50/51); later suites and doc tests were not reached by this script |
| `cargo test --workspace --doc --all-features` after the Windows Job change | exit 0; each crate reports zero doc tests |
| `cargo deny check`, `python scripts/omp_inventory.py check`, `python scripts/verify_bootstrap.py` after the Windows Job change | exit 0; dependency audit retained existing duplicate/no-license-field warnings |
| `cargo clippy -p ara-mcp -p ara-cli --all-targets -- -D warnings` | exit 0 before final test additions; workspace Clippy below covers the later snapshot |
| `cargo fmt -p ara-mcp -p ara-cli` | exit 0 |
| `$env:CARGO_TARGET_DIR='C:\Temp\ara-verify-target'; $env:CARGO_PROFILE_TEST_DEBUG='0'; $env:CARGO_PROFILE_DEV_DEBUG='0'; $env:CARGO_INCREMENTAL='0'; cargo test -p ara-cli --test mcp_stdio --features test-fixture` | 8/8 passed after final MCP code and test-race fix |
| `python scripts/verify_backend.py` on the ordinary D: target | owned formatting and workspace Clippy passed; all-target link failed with MSVC `LNK1140` PDB limit |
| Same verifier with `CARGO_TARGET_DIR=C:\Temp\ara-verify-target`, `CARGO_PROFILE_TEST_DEBUG=0`, `CARGO_PROFILE_DEV_DEBUG=0`, `CARGO_INCREMENTAL=0`, `CARGO_BUILD_JOBS=2` | owned formatting and workspace Clippy passed; CLI e2e 50/51, only `deadline_during_a_tool_and_zero_budget_exit_nonzero` failed its 4-second Windows Bash deadline; script stopped before later suites/doc tests |
| `cargo test --workspace --doc --all-features` with the no-debug C: target | exit 0 (all doc-test targets report 0 tests) |
| `cargo deny check` | exit 0: advisories, bans, licenses and sources okay; existing duplicate/no-license-field warnings were emitted |
| `python scripts/omp_inventory.py check` | exit 0, pinned inventory consistent |
| `python scripts/verify_bootstrap.py` | exit 0, required files, marker and local links valid |
| `cargo test -p ara-cli --features test-fixture --test mcp_stdio large_server_error_does_not_fill_tool_result` before the message cap | failed as expected: the fake child sent a 900-KB JSON-RPC error and its text entered the tool result |
| `cargo test -p ara-cli --features test-fixture --test mcp_stdio` after actor, preflight, cap and test fixes | 16/16 passed: idle/in-call ping, queued cancellation, outbound preflight, bounded diagnostic, last-tool drop and force-exit direct-child cleanup included |
| `cargo test -p ara-cli --features test-fixture --test mcp_stdio dropped_dispatched_call_closes_server_before_another_call` before receiver-close handling | failed as expected: the next call waited on the abandoned hung request beyond two seconds |
| `cargo test -p ara-cli --features test-fixture --test mcp_stdio` after receiver-close handling | 17/17 passed; the dropped-call regression waits for child exit and then confirms the next call was not dispatched |
| `cargo clippy -p ara-mcp -p ara-cli --all-targets --all-features -- -D warnings` on this code snapshot | exit 0 |
| `python scripts/verify_backend.py` with the no-debug C: target before the final queued-cancellation test adjustment | owned formatting and workspace Clippy passed; CLI e2e 50/51, same existing Windows Bash deadline assertion failed; subsequent all-target/doc groups were not reached |
| `python scripts/verify_backend.py` with the no-debug C: target on the final MCP code and test snapshot | owned formatting and workspace Clippy passed; CLI e2e 50/51, same Windows Bash deadline elapsed-time assertion failed; script stopped before later suites and doc tests |
| `cargo test --workspace --doc --all-features` on the final code snapshot | exit 0; all crate doc-test targets reported zero tests |
| `cargo deny check` on the final code snapshot | exit 0; advisories, bans, licenses and sources okay, with existing duplicate/no-license-field warnings |
| `python scripts/omp_inventory.py check`, `python scripts/verify_bootstrap.py` on the final code/document snapshot | exit 0; fixed OMP inventory and project links valid |

The test binary uses a real stdio child and controlled fake model upstream. It checks exact grant/name mapping, no inherited provider key, no declared roots capability and a method-not-found response to a server roots request, normal CLI → tool → Session and model feedback, resource and structured result delivery, normalized-name collision rejection, large-result rejection, cancellation after dispatch, child EOF after an effect, and same-Session resume without replay. An initial test raced its fake child's evidence-file write; reading after the completed tool response removed that test race, and the 8/8 focused run passed. `cargo clean -p ara-cli` removed only regenerated CLI build artifacts after a low-disk/PDB link failure. No local Ubuntu was used.

The Windows force-exit test launches a separate parent test process, connects a fixture MCP server that keeps running after `tools/list`, records the server PID, and opens its process handle while it is alive. The parent then calls `std::process::exit(130)` without dropping its tools. The server handle becomes signaled within three seconds. The fixture stays alive after `tools/list` so closing stdin alone cannot make the test pass. This proves cleanup of an MCP process successfully assigned to the Job, not race-free containment of descendants. The CLI signal handler was not changed.

The additional fault fixture sends a malformed JSON line, a truncated frame followed by EOF, a repeated catalog cursor and a 65-tool catalog. Each startup fails before exposing any `AgentTool`. These cases exercise server-controlled framing and catalog limits through a real child process.

The later tests also send a server ping after tool listing and during a tool call, observe the exact JSON-RPC empty response, and then complete a normal call. The oversized outbound request is rejected before write, leaves no server call receipt and allows a following small call. A second call is polled while the first holds the permit, then cancelled with no server marker. A 900-KB server error is truncated below 5 KB of model-visible text. On Windows, dropping the final tool lets the actor release the Job and reap an otherwise lingering server. The first version of that drop test blocked its single-thread Tokio executor while waiting; asynchronous polling corrected the test, and 16/16 passed.

A subsequent drop-future fault test starts a hanging call, waits for its effect marker, aborts only the caller task, and leaves the cancellation token untouched. Before the fix, a second call timed out waiting on the abandoned request. The actor now observes the closed response receiver, drops the connection/Job, and the test waits for the child process handle to signal before confirming a new call is classified as not dispatched. The final focused suite is 17/17.

Two independent Codex pre-implementation reviewers checked the Windows-only ownership boundary and cross-reviewed the Job design. A separate post-diff reviewer found no direct-child cleanup blocker. It retained the spawn-to-assignment descendant race as an explicit boundary and identified early test assertions that could leave the helper briefly alive; a test-only drop guard now reaps the helper on panic. The focused 11/11 run and strict Clippy were repeated after the fault fixtures were added.

For the follow-up, independent Codex reviewers checked pinned OMP idle ping behavior and the actor ownership approach before implementation. Post-diff reviewer `/root/mcp_actor_post_review` checked all three changed code/test files with `ara-git-review` and `ara-rust-core-review` criteria. It found two test-coverage issues (blocking the single-thread runtime on process wait, and cancelling a second call before proving it had entered the permit wait); both were fixed. It also reviewed the dropped-Future cleanup fix and its process-exit synchronization. Its final pass found no confirmed production blocker and ran `git diff --check`. The root agent ran the 17/17 focused suite; strict Clippy and the broad gate are repeated on the final code below.

To inspect tests beyond the verifier's first stop, the root agent reran `cargo test --workspace --all-targets --all-features` with the no-debug C: target, Git Bash prepended to `PATH`, and selective `--skip` filters. With only the CLI deadline excluded, two existing Windows Bash process tests failed: `bash_keeps_stream_order_and_reaps_background_children` and `bash_timeout_kills_process_group`. Excluding those three exposed `pi-edit` `hashline_streaming_preview_cases_preserve_partial_and_final_contracts`, which also failed in an isolated rerun. Excluding four exposed `pi-edit` `patcher_apply_cases`. These exploratory runs did not complete the entire all-target matrix and do not turn skipped tests into passes. Neither `ara-tools` Bash nor vendored `pi-edit` was modified in this MCP slice.

## Independent review

Independent Codex subagent `mcp_postreview` read the original unstaged diff and fixed OMP source. It found that the first adapter dropped legal MCP resource and structured results, that a full-file read preceded the config limit, and that second Ctrl-C bypassed child Drop. Resource/structured conversion and bounded reading were fixed and re-reviewed; the follow-up review closed those two findings. The reviewer also found that the fake child would enter default production packaging; a feature gate now limits it to tests. At that stage force-exit cleanup was open; the later Windows Job regression above closes the assigned direct-child case. The reviewer could not confirm its own parallel test run because MSVC linking hit the PDB limit; the root agent's later serial pre-Job 8/8 focused run passed.

## Remaining gates

1. Resolve the Windows Bash gate separately from this MCP slice and rerun unfiltered backend verification. Investigate the two vendored hashline parity failures as their own scope; no pass is claimed for the all-target matrix.
2. Continue the bounded JSON-RPC fault audit before considering this stdio point accepted. Malformed/truncated frames, a repeated cursor, catalog overflow, idle/in-call ping, large outbound frame/error and direct-child cleanup are covered. Race-free process-tree containment remains a separate scope.
3. Keep dependency/license and exact upstream inventory checks green on the final delivery snapshot.
4. Run a bounded real-model task through an available verified route and inspect the tool effect, Session receipts, usage and final artifact. No provider key is configured in the current shell; no live trial is claimed.
5. Keep HTTP/SSE, OAuth and the remaining CA-MCP surface as separate work; this review covers only the stdio WIP slice.

The accepted count and P0–P6 gate are unchanged. Registering this new WIP point changes the registered denominator from 27/37 to **27/38 (71.1%)**. MCP is **0/1 accepted**. This point is `implementing`, not delivered or accepted.
