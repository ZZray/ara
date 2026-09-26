# Windows backend gate, 2026-09-26 (WIP)

## Scope and decision

This receipt covers the Windows backend verification follow-up on `dev` after
`69293de`. It does not accept CTX-01d, A3, or the full backend gate. The
roadmap completion remains 1/7 gates (14.3%). Local development uses Windows
and Git Bash; Ubuntu is not a local prerequisite. Manager configuration was
not changed. No credential or local provider configuration is recorded here.

The candidate adapts test-only Unix assumptions, repairs two Windows path
behaviors in `ara-tools`, and makes the HTTP reset tests deterministic. The
`read` selector now treats Windows invalid-name errors as a missing literal
path; glob/search probes do the same. `skill://` expansion supplies slash
paths that Git Bash can use in commands and environment variables while Rust
continues to resolve the working directory. The vendored `pi-edit` changes
only adjust test expectations for Windows path spelling and CRLF fixture
checkouts. Unix runtime paths and Bash process lifecycle are unchanged.

## Executed checks

Git Bash was temporarily prepended to the child process `PATH` for tests
that invoke `bash`; the workspace configuration was not changed.

| Check | Result |
| --- | --- |
| `cargo fmt -p ara-ai -p ara-cli -p ara-tools -- --check` | exit 0 |
| `cargo clippy --workspace --all-targets --all-features -- -D warnings` | exit 0 |
| `cargo test -p ara-ai --test openai_http` | 21/21 passed, including synchronized reset after a finish frame |
| `cargo test -p ara-tools --lib` | 19/19 passed |
| `cargo test -p ara-tools --test skill_urls` with Git Bash on `PATH` | 10/10 passed; command and environment-variable script execution both observed |
| `cargo test -p ara-tools --test search_upstream` | 7/7 passed |
| `cargo test -p pi-edit --lib` | 90/90 passed |
| `cargo deny check` | exit 0; advisory, ban, license and source policies pass, with existing warnings |
| `python scripts/omp_inventory.py check` | exit 0; fixed inventory consistent |
| `git diff --check` | exit 0 |

`cargo fmt --all -- --check` exits 1 with extensive pre-existing formatting
differences in the vendored `pi-ast`/`pi-edit` crates under stable rustfmt;
their config also requests nightly-only options. The scoped ARA format check
above passes. No vendored formatting sweep was made.

## Failed and incomplete checks

The unfiltered `cargo test --workspace --all-targets --all-features` with Git
Bash on `PATH` stops at `ara-cli --test e2e`: 15/16 passed, and
`deadline_during_a_tool_and_zero_budget_exit_nonzero` failed because the
Windows call waited approximately the full `sleep 5` duration instead of
exiting within four seconds after a one-second deadline. The independent
`ara-tools --test tools` run is 8/10: the timeout and background-child
reaping cases each waited for a still-open descendant pipe. These are real
Windows process-tree cleanup failures; test limits were not relaxed.

A diagnostic workspace run that explicitly skipped those three tests reached
`pi-edit --test hashline_parity`: 8/10 passed. One failing assertion uses a
Unix `/workspace` absolute path in a pure parser test. The other, a streaming
preview of a nested file through a bare hashline header, returned zero files
instead of one on Windows. The latter is an unresolved edit path-recovery
behavior and is not waived as a fixture mismatch. Tests after that target,
documentation tests, the backend script's full gate, and bounded real-model
tasks have not passed on this candidate.

The failing parity cases are
`pure_format_input_and_streaming_contracts_cover_uncaptured_cases` and
`hashline_streaming_preview_cases_preserve_partial_and_final_contracts`.
The diagnostic command was:

```powershell
$env:PATH = 'C:\Program Files\Git\usr\bin;' + $env:PATH
cargo test --workspace --all-targets --all-features --quiet -- --skip deadline_during_a_tool_and_zero_budget_exit_nonzero --skip bash_timeout_kills_process_group --skip bash_keeps_stream_order_and_reaps_background_children
```

The Windows CLI SIGINT and crash-resume process cases remain Unix-gated, so
their Windows behavior has no equivalent executable evidence here.

## Review and next work

Independent Codex reviewers `stream_reset_plan_review`,
`windows_path_plan_review`, and `windows_gate_diff_review` examined the
relevant plan and diff. The reset reviewer found an immediate-RST race in the
fake server; synchronized test coverage was added without provider behavior
changes. The path reviewer found and then verified the environment-variable
`skill://` use case. Diff review reported no remaining blocker in the scoped
HTTP, CLI, path, or test-only vendor changes; it did not certify the full
backend gate.

The Windows Bash process-tree change would alter an established subprocess
lifecycle, and the nested hashline recovery may require changing the shared
vendored editor. The repository's scope rule requires explicit confirmation
before either stable path is changed. After those scopes are settled, rerun
the unfiltered full gate on the final code, review the final diff, complete
the real-model and point-delivery evidence, then decide acceptance. A passing
diagnostic run with skipped tests cannot replace that gate.
