# TOOLS-01a multi-range `read` (WIP)

## Requirement and boundary

Fixed OMP commit `596f2da7101178214aa27a753529d15e6b7ad91d`:
`packages/coding-agent/src/tools/path-utils.ts::parseLineRanges` sorts and
merges comma-separated inclusive spans; `read-selector.ts` accepts them with
optional `raw`; `read-format.ts::buildInMemoryMultiRangeResult` renders
disjoint spans, skipped out-of-bounds notices, and edit anchors. This slice is
on local code commit `8fec372` (WIP, not pushed).

The existing single-range `read` behavior, other tools, Agent, Session, and
CLI production code stay unchanged. `read.rs` now parses and coalesces spans
and uses a separate forward-scan path for disjoint reads. It preserves exact
selected lines, raw CRLF and terminal empty-line semantics, one hashline
header, and only actually displayed lines in edit provenance. Ordinary file
output retains ARA's 3000-line/50-KB content limit, with bounded diagnostic
tail text; immutable `skill://` reads remain exempt. The real CLI process
passes the two selected lines through a tool result to the next model request
and the Session journal.

The fixed OMP non-raw multi-range renderer also adds lexical/AST block
boundary context. A later local WIP follow-up now adds that context for
complete, buffered UTF-8 local files up to 4 MiB using the existing
`pi-edit::diff_string::find_block_context_lines` implementation. Requested
rows and fitting out-of-bounds notices take priority over optional context;
newly displayed boundary rows enter hashline provenance. Raw reads, single
ranges, and larger streamed files retain their prior output. Immutable
`skill://` resources still omit block context, although fixed OMP's
in-memory renderer includes it. This is not full CA-TOOL-READ parity. ARA
also caps ordinary multi-range content while fixed OMP's in-memory builder
does not impose that aggregate cap.

## Executed evidence on `8fec372`

| Check | Result |
| --- | --- |
| New `read_multiple_ranges_keep_exact_lines_and_edit_provenance` on old code | Failed: `File not found: lines.txt:9-9,3-4,4-5,20-22`. |
| `cargo test -p ara-tools --lib read::tests --quiet` | 3/3 passed: selector merge, aliases, literal path, overflow, and bounded late-span scan. |
| `cargo test -p ara-tools --test tools read_ --quiet` | 7/7 passed, including exact spans, raw/CRLF/EOF, out-of-bounds cap, oversized first/later lines, continuation, and hashline/raw seen lines. |
| `cargo test -p ara-tools --test skill_urls read_skill_urls --quiet` | 1/1 passed; multi-range immutable resource keeps numbering and no hashline tag. |
| `cargo test -p ara-cli --test e2e read_multiple_ranges_reaches_model_and_session_journal --quiet` | 1/1 passed. Real `ara` binary sent one `read` call, returned `3:line3` and `9:line9` without adjacent `line4`, forwarded the same receipt to fake upstream, and journaled it. |
| `python scripts/verify_backend.py` | Owned formatting and workspace all-target/all-feature Clippy passed. Workspace tests reached CLI e2e: 44/45 passed; the pre-existing Windows Bash `deadline_during_a_tool_and_zero_budget_exit_nonzero` still exceeded four seconds. Therefore the full gate failed (exit 101), and doc tests were not reached by this script. |
| `cargo test --workspace --doc --all-features --quiet` | Passed separately; no documentation test cases. |
| `cargo deny check --hide-inclusion-graph` | Exit 0: advisories, bans, licenses, sources OK; existing duplicate/no-license-field warnings remain. |
| `python scripts/omp_inventory.py check` | Passed: fixed baseline inventory consistent. |
| `git diff --check` before commit | Exit 0; only Windows line-ending notices. |

Independent Codex reviewer `/root/host_tools_next_gap` inspected all four
changed files against the fixed OMP source. The reviewer found three reachable
continuation/limit issues: a long first selected line could loop on itself;
many skipped-range notices could exceed the output cap without a bound; and a
completed finite selection could falsely request continuation when an EOF
scan hit its budget. The reviewer also found a late oversize line case. These
were fixed, covered by focused tests, and re-reviewed; no further
high-confidence blocker was reported. Reviewer did not run Cargo. A separate
Codex pre-plan review confirmed this scope and the remaining block-context
difference.

## Buffered local block-context follow-up (WIP)

Fixed OMP source: `packages/coding-agent/src/tools/read.ts` buffered local
multi-range branch and `read-format.ts::buildLineEntriesWithBlockContext`;
the vendored Rust helper maps AST and lexical block boundaries. This follow-up
modifies `crates/ara-tools/src/read.rs` plus focused Tools, hashline Edit and
real CLI fake-upstream tests. No Agent, Session, CLI production or other tool
code changes are needed.

| Check | Observed result |
| --- | --- |
| New TypeScript boundary test on prior code | Failed: only selected rows and ellipses were present. |
| New near-limit block context plus out-of-bounds notice before fix | Failed: optional boundary row displaced the specific `[Range 100-100 ... skipped]` notice. |
| `cargo clippy -p ara-tools --all-targets --all-features -- -D warnings` after parameter cleanup | Passed. |
| `cargo test -p ara-tools --test tools read_ --quiet` | 9/9 passed, including TypeScript/Python boundaries, selected-row budget, the notice regression, raw and >4 MiB streaming. |
| `cargo test -p ara-tools --lib read::tests --quiet` | 4/4 passed, including cancellation before optional context computation. |
| Focused Edit and real CLI fake-upstream tests | 1/1 each passed; visible boundary rows work as hashline anchors and reach the Session tool result and next model request. |
| `python scripts/verify_backend.py` | Formatting and workspace all-target/all-feature Clippy passed; workspace test stopped on the existing Windows Bash deadline assertion, with CLI e2e 45/46 passed, including the new context case. Full gate failed (exit 101). |
| `cargo test -p ara-tools --all-targets --all-features` with Git Bash on PATH | Read cases passed; two existing Bash timing tests failed (14/16 `tools.rs` passed). This narrower run also does not pass the full gate. |
| `cargo test --workspace --doc --all-features --quiet` | Passed; zero documentation test cases. |
| `cargo deny check --hide-inclusion-graph`, `python scripts/omp_inventory.py check`, `git diff --check` | All exited 0; Cargo deny retained existing duplicate/no-license-field warnings and Git showed only Windows line-ending notices. |

Independent Codex reviewer `/root/read_context_diff_review` caught the
notice-budget regression and a cancellation window around AST computation.
The implementation now reserves space for notices before adding context and
checks cancellation before and after boundary extraction. The reviewer also
identified the still-open `skill://` resource difference. The final review
reported no remaining high-confidence blocker. The reviewer ran 2/2 focused
Tools, 1/1 Edit, 1/1 CLI and 3/3 prior read unit tests; the extra cancellation
unit test was added afterward and passed locally (4/4 read unit tests).

## Acceptance and next steps

TOOLS-01a's previously accepted single-range subset is unchanged; this
extension remains WIP. The complete Windows Bash gate, a bounded real-model
multi-range task with tool result and artifact inspection, and resource
block-context parity remain open. No module or formal gate count
advances. Windows Bash work is a separate stable lifecycle change awaiting
the explicit scope decision recorded in
[`windows-bash-process-tree.md`](windows-bash-process-tree.md).
