# CTX-01d follow-up: skill URL routes in grep, glob and write (WIP)

Date: 2026-09-27. Product code: `41cb9a7` plus prompt follow-up
`ca2a076` on local `dev`. Fixed upstream:
`596f2da7101178214aa27a753529d15e6b7ad91d`.

## Scope and source map

The observable requirement is that `grep` and `glob` can search a loaded
`skill://` resource, while `write` rejects immutable skill URLs before any
filesystem effect. A bare URL names the skill directory for search; `read`
continues to use its `SKILL.md`. This closes the URL routing difference
recorded in the CTX-01d review. Ordinary paths, Bash expansion and the
existing skill read flow remain as before. Plugin `containRoot` handling and
other internal URL schemes remain open.

| Fixed OMP source | ARA implementation | Executable evidence |
| --- | --- | --- |
| `internal-urls/skill-protocol.ts:45-113`, `tools/path-utils.ts:454-525` | `ara-tools::internal_urls` path-only resolution, stat and URL selector peeling; `ToolContext` exposes the path-only route | `skill_urls::{grep_skill_urls,glob_skill_urls,write_skill_urls_are_read_only}` |
| `tools/grep.ts:161-200,772-865,1475-1485` | `grep` searches the backing file/directory, filters absolute line ranges, accepts `raw`/`conflicts`, rejects malformed selectors and URL globs, and omits editable hashline tags on skill results | `skill_urls::grep_skill_urls`; mixed ordinary-file tag check |
| `tools/glob.ts:206-255` | `glob` resolves a non-pattern skill URL to its backing file/directory and rejects URL glob patterns | `skill_urls::glob_skill_urls` |
| `tools/write.ts:1123-1216`, `internal-urls/router.ts:145-150` | `write` rejects immutable URLs before creating parents or writing bytes; line-range selectors are rejected separately | `skill_urls::write_skill_urls_are_read_only`; CLI asset hash/content check |
| ARA host guidance for the new route | `ara-context/prompts/system-prompt.md` names `grep`, `glob` and `write` behavior only when the tool is available | `ara-context::internal_urls_follow_the_host`; CLI system-prompt assertion |

On Windows, the URL search path drops the canonical `\\?\` prefix before
the search parser sees its `?` as a glob. Calls containing skill URLs use a
cloned `ToolContext` with an equivalent unprefixed cwd. This keeps workspace
results relative and leaves the original context and edit store shared.

## Checks on final product commit `ca2a076`

| Command or action | Result |
| --- | --- |
| `cargo fmt -p ara-tools -p ara-cli -p ara-context -- --check` (also run by the backend verifier) | Exit 0. |
| `cargo clippy -p ara-tools --all-targets --all-features -- -D warnings` | Exit 0. Final backend verifier's workspace strict Clippy also exited 0. |
| `cargo test -p ara-tools --test skill_urls` with Git Bash supplied only to the subprocess | Exit 0, 13/13. Normal, malformed selector, missing/mixed path, uppercase scheme, traversal, hashline and no-write-effects cases passed. |
| `cargo test -p ara-tools --all-targets --all-features -- --skip bash_timeout_kills_process_group --skip bash_keeps_stream_order_and_reaps_background_children` with the same Git Bash environment | Exit 0, 69 passed, two pre-existing Windows process-tree cases filtered. This is diagnostic, not an unfiltered pass. |
| `cargo test -p ara-cli --test e2e skill_url_search_and_read_only_write_reach_cli` with Git Bash environment | Exit 0, 1/1. The real CLI and fake upstream exchanged glob, grep and rejected write receipts; a same-text ordinary file stayed outside the skill results, and the skill asset stayed unchanged. |
| `cargo test -p ara-context --all-targets --all-features --quiet` | Exit 0, 21/21. Read guidance is gated by the available tool list, including a host with `grep` but no `read`. |
| `python scripts/verify_backend.py` on the final code and host test | Exit 101. Owned format and workspace strict Clippy passed; the unfiltered workspace suite stopped at `ara-cli` deadline during tool, 18/19 e2e passed. Log: `%TEMP%\ara-ctx01d-ca2a076-gate.log`. Doc tests in this verifier were not reached. |
| `cargo test --workspace --doc --all-features --quiet` | Exit 0; no doc tests are defined. |
| `cargo deny check`; `python scripts/omp_inventory.py check`; `git diff --cached --check` | Exit 0 for each; deny retained existing duplicate dependency warnings. |

The Windows `ara-tools` unfiltered run separately failed two Bash process-tree
timing cases (8/10 in `tests/tools.rs`). Those and the CLI deadline failure
predate this URL routing. No process lifecycle or vendored editor code was
changed. The full backend gate is still red.

An independent Codex plan reviewer compared the route and selectors with the
fixed OMP checkout before implementation. An independent read-only Codex diff
reviewer found two issues in the first diff: a missing skill resource was
silently skipped in mixed search, and an uppercase `SKILL://` could evade the
write guard. Both were fixed and tested. Its second review found a Windows
relative-path display issue, which was fixed and tested. Its final code review
found no remaining high-confidence regression. It then found that the first
CLI test could pass after an unintended cwd-wide search or without proving
that receipts reached the model; the final test adds an ordinary-file control
and checks the subsequent fake-upstream requests. The reviewer did not rerun
the final test itself; the command above did.

The same reviewer checked the prompt follow-up and found that a host with
`urls.skill=true` but no `read` would still be told to call `read`. The prompt
now gates that line; a `grep`-only render test covers it. The CLI receipt test
checks that the corrected guidance reaches the model request. The reviewer
did not rerun the corrected tests; the commands above did.

## Limits and decision

The fixed OMP `glob` also treats a percent-encoded literal `[`/`]` in a skill
filename as glob syntax after path resolution; this slice preserves that
behavior. Other scheme routes, plugin symlink containment, and skill image,
binary and directory read parity remain open. The prior Agnes and OpenRouter
real-model trials exercised an earlier commit, not `ca2a076`; a bounded live
task on this snapshot is still needed for CTX-01d acceptance.

**Decision: changes requested / WIP.** The new route has focused and host
evidence, but full unfiltered verification and final point audit have not
passed. No CTX or overall roadmap acceptance count changes.
