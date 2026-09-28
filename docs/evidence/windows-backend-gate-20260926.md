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
| `cargo test --doc --workspace --all-features` on `cd124f7` | exit 0; every crate reports 0 documentation tests |
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
`pi-edit --test hashline_parity`: 8/10 passed. One failing assertion used a
Unix `/workspace` absolute path in a pure parser test. The other, a streaming
preview of a nested file through a bare hashline header, returned zero files
instead of one on Windows. The latter is an unresolved edit path-recovery
behavior and is not waived as a fixture mismatch.

The failing parity cases are
`pure_format_input_and_streaming_contracts_cover_uncaptured_cases` and
`hashline_streaming_preview_cases_preserve_partial_and_final_contracts`.
The diagnostic command was:

```powershell
$env:PATH = 'C:\Program Files\Git\usr\bin;' + $env:PATH
cargo test --workspace --all-targets --all-features --quiet -- --skip deadline_during_a_tool_and_zero_budget_exit_nonzero --skip bash_timeout_kills_process_group --skip bash_keeps_stream_order_and_reaps_background_children
```

On the follow-up test-only snapshot, `hashline_parity` is 9/10 after the
pure parser test uses a native absolute path. `hashline_parse` is 41/41 after
the same fixture correction. `hashline_patcher` is 5/6: the case
`patcher_apply_cases` fails to redirect a bare `a.txt` header to the tagged
`nested/a.txt`, leaving the file unchanged. This and the streaming preview
failure share the Windows verbatim-prefix mismatch in the worktree containment
check; the CLI's explicit `--cwd` can reach that path. No editor runtime code
was changed. A diagnostic **five-skip** workspace command (the three Bash
cases plus `hashline_streaming_preview_cases_preserve_partial_and_final_contracts`
and `patcher_apply_cases`) exited 0 across the remaining workspace tests.
This is not an unfiltered gate pass. The backend script's full gate and bounded
real-model tasks have not passed on the follow-up snapshot.

```powershell
$env:PATH = 'C:\Program Files\Git\usr\bin;' + $env:PATH
cargo test --workspace --all-targets --all-features --quiet -- --skip deadline_during_a_tool_and_zero_budget_exit_nonzero --skip bash_timeout_kills_process_group --skip bash_keeps_stream_order_and_reaps_background_children --skip hashline_streaming_preview_cases_preserve_partial_and_final_contracts --skip patcher_apply_cases
```

The Windows CLI SIGINT and crash-resume process cases remain Unix-gated, so
their Windows behavior has no equivalent executable evidence here.

Follow-up test hardening uses relative marker names in the Bash process tests;
Git Bash otherwise treats native backslashes in an unquoted absolute path as
escapes and can produce a false negative marker check. The hardened
`ara-tools --test tools` run remains 8/10, with the same two elapsed-time
failures. This test-only follow-up does not alter process lifecycle behavior.

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

## Current-code verifier follow-up (2026-09-26, WIP)

At `5acddec` plus the test-runner change, `scripts/verify_backend.py` now
finds the installed Git Bash on Windows and prepends its `usr/bin` directory
only to the all-target test subprocess. It does not change the machine PATH,
product commands, or test exclusions. If Git Bash cannot be found, the script
reports the missing test prerequisite. The final script was run from a shell
whose PATH did not contain `bash`; it selected
`C:\Program Files\Git\usr\bin\bash.exe`. Its owned-package format check and
strict workspace Clippy passed. The unfiltered workspace test then ran
`ara-cli --test e2e`: **17/18 passed**; only
`deadline_during_a_tool_and_zero_budget_exit_nonzero` failed the under-four-
second assertion (the suite took 5.75 seconds). The script exited 101, so
doc tests were not reached in that run. This is a failed delivery gate, not a
pass. The local receipt is `%TEMP%\ara-autobash-verifier-final.log`.

On the same product code, a diagnostic run skipping only that CLI deadline
case reached `ara-tools --test tools`: **8/10 passed**, with the two Bash
process-tree timing cases failing. Skipping those three Bash cases reached
`pi-edit --test hashline_parity`: **9/10 passed**, with nested-file streaming
preview failing. The focused `patcher_apply_cases` also fails on the same
nested-path recovery. These skips locate failures; they do not waive them.
`cargo deny check`, the fixed OMP inventory check, Python syntax parsing of
the verifier, and `git diff --check` passed on this follow-up. The only new
file change here is the verification script; the Rust Core and module
architecture are unchanged.

Two read-only Codex reviewers independently examined narrow repair plans.
The Windows Bash plan uses a Windows Job Object to reap descendants on timeout,
cancel, and drop while leaving Unix behavior alone. The editor plan compares
Windows drive paths consistently when one side has the verbatim `\\?\` prefix;
it leaves URL admission and sandbox policy intact. Both touch established
shared behavior, so implementation awaits the explicit scope decision required
by `AGENTS.md`. No point is accepted: the roadmap remains 1/7 gates (14.3%)
and the registered ledger remains 27/36 accepted. Conditional estimate for
both fixes, regressions, unfiltered gate, and evidence review: 2–4 working
days, low confidence; it excludes real-model availability and later product
phases.

## 2026-09-28 Windows gate continuation (WIP)

The authorized Windows Bash Job repair is local WIP commit `8940f3c`.
`python scripts/verify_backend.py` on that code passed owned formatting,
workspace Clippy, CLI e2e 51/51, proxy discovery 6/6 and `ara-tools` tool
integration 16/16. It then failed in vendored `pi-edit` hashline streaming
preview parity (9/10 in that test binary). A separate focused run of
`cargo test -p pi-edit --test hashline_patcher patcher_apply_cases -- --nocapture`
failed one nested-file recovery fixture: `a.txt` was reported missing and
`nested/a.txt` remained unchanged. An exploratory all-target run that skipped
only those two named failing tests exited 0; it is not a delivery-gate pass.

Read-only diagnosis traced both failures to `path_policy.rs`:
`Workspace::new` uses a Windows canonical cwd with a `\\?\` prefix, while
`canonical_key` removes that prefix from the snapshot path. The
`allow_tag_path_recovery` containment comparison therefore rejects a valid
child file. The streaming preview hides the last section's resulting error,
yielding zero preview files; apply reports the missing authored basename.
An independent Codex reviewer reproduced both failures and agreed on this
cause. The reviewer cautioned that the existing generic prefix stripper would
turn verbatim UNC paths into relative paths, so a fix must normalize drive
forms narrowly and keep out-of-root recovery denied. The shared editor's
write-target boundary is a stable flow under `AGENTS.md`; implementation is
pending the separate explicit scope request. No point acceptance changed.
