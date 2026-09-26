# CTX-01d follow-up: skill read resource types (WIP)

Date: 2026-09-27. Local code commit: `bc1f5db`. Fixed OMP source commit:
`596f2da7101178214aa27a753529d15e6b7ad91d`. CTX-01d remains
changes requested; this receipt does not add an accepted ledger point.

## Requirement and boundary

`read skill://<name>/<path>` uses the loaded skill's backing resource. In the
fixed OMP, `internal-urls/skill-protocol.ts:93-126` reads **every regular file**
with `Bun.file(...).text()` and marks the handler immutable; it makes a text
directory resource through `internal-urls/filesystem-resource.ts:10-39`.
`tools/read.ts:2299-2374` renders that text with selectors and no skill result
limit. Only `local://` has the separate image fast path. Therefore a skill
image or invalid-UTF-8 file is returned as replacement-decoded text, not an
image block or a binary refusal. Directory subpaths list directories first
and are not capped at ARA's ordinary 500-entry directory limit.

ARA implements this in `crates/ara-tools/src/read.rs`: an internal skill file
becomes UTF-8 replacement-decoded text, while a skill directory becomes a
text listing before selectors run. It retains immutable, untruncated output
and returns the backing `resolvedPath`. Normal file/directory paths, the
skill URL resolver, Bash, `grep`, `glob`, `write`, the Agent loop and the
Session journal were outside this change.

## Executable checks on `bc1f5db`

Git Bash was prepended to PATH for the tool integration tests on Windows;
no local Ubuntu service was used. Test fixtures use a temporary skill tree.

| Check | Actual result |
| --- | --- |
| New `read_skill_resource_types` test on the old code | Exit 1: `image.png` produced two blocks instead of one text resource. |
| `cargo test -p ara-tools --test skill_urls` | Exit 0, 14/14. The new fixture checks an image, invalid UTF-8/NUL, CRLF, empty and raw trailing-line behavior, 502 directory entries, directory-first order and `:raw:1-1`; existing unknown-skill and traversal cases also pass. |
| `cargo test -p ara-tools --test tools read_directories_binaries_images` | Exit 0, 1/1. Ordinary file, image, binary and directory behavior remains covered. |
| `cargo fmt -p ara-tools -- --check`; `cargo clippy -p ara-tools --all-targets --all-features -- -D warnings`; `git diff --check` | Exit 0 for each on the code snapshot before commit. |
| `cargo test -p ara-tools --all-targets --all-features -- --skip bash_timeout_kills_process_group --skip bash_keeps_stream_order_and_reaps_background_children --skip dropping_a_bash_call_kills_its_group` | Exit 0, 69 passed, 3 filtered. Diagnostic only; this is not an unfiltered gate pass. |
| Unfiltered tools run, then standalone `dropping_a_bash_call_kills_its_group` with Git Bash available | Exit 1. The additional Windows Bash process-group case fails on this machine; no Bash lifecycle code was changed here. |
| `python scripts/verify_backend.py` on committed `bc1f5db` with Git Bash available | Exit 101. Owned formatting and workspace strict Clippy passed; the workspace test run stopped at `ara-cli` `deadline_during_a_tool_and_zero_budget_exit_nonzero` after 18/19 CLI e2e passed. Log: `%TEMP%\ara-ctx01d-bc1f5db-gate.log`. Later workspace tests and doc tests were not reached. |
| `cargo deny check`; `python scripts/omp_inventory.py check`; `cargo test --workspace --doc --all-features --quiet` | Exit 0 for each. `cargo deny` retained existing duplicate-dependency warnings; no doc tests are defined. |

## Review and remaining differences

An independent Codex plan reviewer verified the fixed OMP text-resource path
and proposed the narrow branch plus image, binary, directory and selector
regressions. A separate independent Codex diff reviewer covered both changed
files. It found one P2 parity difference: OMP sorts directory names with
`localeCompare`, while this Rust branch uses case-folded ordinal ordering.
For example, OMP lists `a_1.txt` before `a-1.txt`, while ARA reverses them.
That can alter line selector results. This remains an explicit open difference
and is not counted as OMP directory-order acceptance.

OMP also expands context around non-raw line ranges. ARA's shared read window
does not yet do that for ordinary files or skill resources; the pre-existing
TOOLS-01/CTX-01d evidence records this common open item. The 256 MiB
post-window scan budget and platform locale collation may also differ from
OMP's in-memory rendering. The reviewer treated range context as a documented
residual, not a regression in this resource-type slice.

A bounded real-model task and a fully green backend gate on this commit are
still missing. Earlier Agnes/OpenRouter runs exercised older commits. The
point remains WIP, and no Product/Task acceptance is implied by a successful
tool Run.
