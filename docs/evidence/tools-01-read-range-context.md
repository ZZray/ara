# TOOLS-01 / CTX-01d follow-up: read line-range context (WIP)

Date: 2026-09-27. Local code commit: `59cc41e` on `dev` (not pushed).
Fixed OMP source: `596f2da7101178214aa27a753529d15e6b7ad91d`,
`packages/coding-agent/src/tools/read-format.ts:242-272` and
`read.ts:1619-1667,1702-1741`. This follow-up does not add a new accepted
point or accept CTX-01d.

## Behavior and boundary

Non-raw `read path:start-end` now includes one line before the requested
start and three lines after a finite end. Open-ended and tail selectors
include the leading line when there is one. `:raw` still returns precisely
the selected lines. The same `read_window` is used by ordinary text files,
`skill://` files and `skill://` directory text resources, so all three
receive the context behavior. The requested start, rather than the leading
context line, drives beyond-EOF guidance. Hashline edit visibility includes
every line actually shown, and continuation names the next unshown line.

This changes `crates/ara-tools/src/read.rs` and the directly affected tool
tests. Bash, Agent, Session, CLI, multi-range/delimited reads, suffix
resolution and skill directory locale collation remain outside this slice.

ARA has an intentional difference from fixed OMP: if the leading context
line is too large for the 50 KiB output budget, or it would prevent the
requested line from fitting, ARA drops that context line and shows the
requested line. Fixed OMP expands the range first, then applies the byte
limit to the first shown line (`read.ts:1702-1741,2762-2777,2962-2985`).
That can leave the requested line unreachable through the same selector.
The ARA behavior preserves forward progress for the requested read.

## Executed evidence on `59cc41e`

| Check | Result |
| --- | --- |
| New ordinary `:4-4` and skill range fixtures against the old code | Failed as expected: only the selected lines appeared. |
| New `:2-2` fixture with an oversized preceding line against the first implementation | Failed as expected: only line 1 appeared and the continuation repeated `:2`. |
| `cargo test -p ara-tools --test tools read_explicit_range_reaches_requested_line_when_leading_context_exhausts_byte_limit -- --exact` | Exit 0 after the correction, 1/1. Covers an oversized and a nearly full preceding line. |
| `cargo test -p ara-tools --all-targets --all-features` with Git Bash on PATH | Exit 1: 73 tool tests passed; two existing Windows Bash process-tree timing tests failed (`bash_keeps_stream_order_and_reaps_background_children`, `bash_timeout_kills_process_group`). All read, skill URL and edit tests passed. |
| Same tools command with those two tests and `dropping_a_bash_call_kills_its_group` filtered | Exit 0: 72 passed, 3 filtered. Diagnostic only; not a full gate. |
| `python scripts/verify_backend.py` | Exit 101. Owned formatting and strict workspace Clippy passed; workspace tests stopped at CLI e2e `deadline_during_a_tool_and_zero_budget_exit_nonzero` (35/36 CLI e2e passed). Log: `%TEMP%\ara-read-range-verify-20260927.log`. Later workspace tests and doc tests were not reached by this command. |
| `cargo deny check`; `python scripts/omp_inventory.py check`; `cargo test --workspace --doc --all-features --quiet`; `git diff --check` | Exit 0. `cargo deny` reports its existing duplicate/no-license warnings; there are no doc tests. |

An independent Codex plan reviewer checked scope, raw selectors, EOF,
continuations and hashline visibility. An independent Codex diff reviewer
reviewed all four changed files against `d37b648`, found the initial
large-leading-line stall, then reviewed the corrected diff and found no
remaining reachable defect. The reviewer ran `git diff --check`; the Cargo
results above were run by the implementing agent.

No new real-model task ran on this commit. The previously recorded model
trials exercised older snapshots. Full unfiltered backend verification,
selected host/model evidence and CTX-01d review remain open. The feature
ledger and P0–P6 accepted counts stay unchanged.
