# CA-MCP stdio first slice — WIP, 2026-09-28

Target: fixed OMP v18.1.8 commit `596f2da7101178214aa27a753529d15e6b7ad91d`, especially `packages/coding-agent/src/mcp/{client,tool-bridge,transports/stdio}.ts`. This is a bounded host path, not CA-MCP acceptance. The exact OMP checkout was verified at that SHA. The upstream marker remains unchanged.

## Scope and observable path

- Preserved: existing Agent loop, Session journal/recovery and Bash process lifecycle.
- Added: `ara-mcp` host-side JSON-RPC stdio adapter; CLI `--mcp-config` for one local server and exact `--mcp-allow server:tool` grants; direct executable argv; explicit environment mapping; bounded frame/catalog/result sizes; no automatic replay after an uncertain tool call. The fake MCP binary and integration test require the `ara-cli/test-fixture` feature, so default production builds exclude them.
- Deferred: HTTP/SSE, OAuth, resource/prompt catalogs, dynamic discovery, reconnect, `.cmd/.bat` wrappers and Windows process-tree containment. The second Ctrl-C force-exit path can bypass the direct child's Drop cleanup. This is an open issue before acceptance.

The host launches an MCP child, negotiates protocol `2025-11-25`, sends `notifications/initialized`, lists tools, registers only granted tools as `AgentTool`s, then the ordinary Agent sink journals calls and results. A tool request that exits, times out or is cancelled after dispatch returns an error with `executed: unknown`; the model-visible text also says not to replay it automatically. The Session is resumed normally, without a new MCP call for the old effect.

The host config shape is `{"name":"local","command":"C:\\absolute\\path\\server.exe","args":[],"env_from_host":{"CHILD_VAR":"PARENT_VAR"}}`. Launch is opt-in with `--mcp-config <file> --mcp-allow local:tool_name`. `env_from_host` copies only named variables; keep credentials in the parent environment, not in the JSON file. The server's tool descriptions and returned content are untrusted tool data, not system instructions.

## Executed evidence

Commands ran on Windows PowerShell against the current worktree; no provider credentials were recorded in test output or files.

| Command | Observed result |
| --- | --- |
| `cargo check -p ara-mcp -p ara-cli` | exit 0 |
| `cargo clippy -p ara-mcp -p ara-cli --all-targets -- -D warnings` | exit 0 before final test additions; workspace Clippy below covers the later snapshot |
| `cargo fmt -p ara-mcp -p ara-cli` | exit 0 |
| `$env:CARGO_TARGET_DIR='C:\Temp\ara-verify-target'; $env:CARGO_PROFILE_TEST_DEBUG='0'; $env:CARGO_PROFILE_DEV_DEBUG='0'; $env:CARGO_INCREMENTAL='0'; cargo test -p ara-cli --test mcp_stdio --features test-fixture` | 8/8 passed after final MCP code and test-race fix |
| `python scripts/verify_backend.py` on the ordinary D: target | owned formatting and workspace Clippy passed; all-target link failed with MSVC `LNK1140` PDB limit |
| Same verifier with `CARGO_TARGET_DIR=C:\Temp\ara-verify-target`, `CARGO_PROFILE_TEST_DEBUG=0`, `CARGO_PROFILE_DEV_DEBUG=0`, `CARGO_INCREMENTAL=0`, `CARGO_BUILD_JOBS=2` | owned formatting and workspace Clippy passed; CLI e2e 50/51, only `deadline_during_a_tool_and_zero_budget_exit_nonzero` failed its 4-second Windows Bash deadline; script stopped before later suites/doc tests |
| `cargo test --workspace --doc --all-features` with the no-debug C: target | exit 0 (all doc-test targets report 0 tests) |
| `cargo deny check` | exit 0: advisories, bans, licenses and sources okay; existing duplicate/no-license-field warnings were emitted |
| `python scripts/omp_inventory.py check` | exit 0, pinned inventory consistent |
| `python scripts/verify_bootstrap.py` | exit 0, required files, marker and local links valid |

The test binary uses a real stdio child and controlled fake model upstream. It checks exact grant/name mapping, no inherited provider key, no declared roots capability and a method-not-found response to a server roots request, normal CLI → tool → Session and model feedback, resource and structured result delivery, normalized-name collision rejection, large-result rejection, cancellation after dispatch, child EOF after an effect, and same-Session resume without replay. An initial test raced its fake child's evidence-file write; reading after the completed tool response removed that test race, and the 8/8 focused run passed. `cargo clean -p ara-cli` removed only regenerated CLI build artifacts after a low-disk/PDB link failure. No local Ubuntu was used.

To inspect tests beyond the verifier's first stop, the root agent reran `cargo test --workspace --all-targets --all-features` with the no-debug C: target, Git Bash prepended to `PATH`, and selective `--skip` filters. With only the CLI deadline excluded, two existing Windows Bash process tests failed: `bash_keeps_stream_order_and_reaps_background_children` and `bash_timeout_kills_process_group`. Excluding those three exposed `pi-edit` `hashline_streaming_preview_cases_preserve_partial_and_final_contracts`, which also failed in an isolated rerun. Excluding four exposed `pi-edit` `patcher_apply_cases`. These exploratory runs did not complete the entire all-target matrix and do not turn skipped tests into passes. Neither `ara-tools` Bash nor vendored `pi-edit` was modified in this MCP slice.

## Independent review

Independent Codex subagent `mcp_postreview` read the unstaged diff and fixed OMP source. It found that the first adapter dropped legal MCP resource and structured results, that a full-file read preceded the config limit, and that second Ctrl-C bypasses child Drop. Resource/structured conversion and bounded reading were fixed and re-reviewed; the follow-up review closed those two findings. The reviewer also found that the fake child would enter default production packaging; a feature gate now limits it to tests. The force-exit process cleanup issue remains open. The reviewer could not confirm its own parallel test run because MSVC linking hit the PDB limit; the root agent's later serial 8/8 focused run passed.

## Remaining gates

1. Resolve the Windows Bash gate separately from this MCP slice and rerun unfiltered backend verification. Investigate the two vendored hashline parity failures as their own scope; no pass is claimed for the all-target matrix.
2. Resolve and test force-exit child cleanup, malformed/fractured JSON-RPC frames, and the bounded catalog/fault matrix before considering this stdio point accepted.
3. Keep dependency/license and exact upstream inventory checks green on the final delivery snapshot.
4. Run a bounded real-model task through an available verified route and inspect the tool effect, Session receipts, usage and final artifact. No provider key is configured in the current shell; no live trial is claimed.
5. Complete the final independent diff review after the packaging fix. HTTP/SSE, OAuth and the remaining CA-MCP surface are separate work.

The accepted count and P0–P6 gate are unchanged. Registering this new WIP point changes the registered denominator from 27/37 to **27/38 (71.1%)**. MCP is **0/1 accepted**. This point is `implementing`, not delivered or accepted.
