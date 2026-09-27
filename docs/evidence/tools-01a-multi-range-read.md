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
boundary context. ARA currently emits only selected lines and an ellipsis;
that upstream display behavior remains open. This slice is not full
CA-TOOL-READ parity. ARA also caps ordinary multi-range content while fixed
OMP's in-memory multi-range builder does not impose that aggregate cap.

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

## Acceptance and next steps

TOOLS-01a's previously accepted single-range subset is unchanged; this
extension remains WIP. The complete Windows Bash gate, a bounded real-model
multi-range task with tool result and artifact inspection, and a later
block-context parity decision remain open. No module or formal gate count
advances. Windows Bash work is a separate stable lifecycle change awaiting
the explicit scope decision recorded in
[`windows-bash-process-tree.md`](windows-bash-process-tree.md).
