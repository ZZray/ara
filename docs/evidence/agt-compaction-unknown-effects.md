# A3 compaction: uncertain tool effects and durable handoff (WIP)

Date: 2026-09-28. Code commit `a1b5d58` on local `dev` (not pushed).
This is an Agent/Session compaction guard, not an accepted A3 point.

## Requirement and observed gap

A soft summary may replace a completed turn only when it does not conceal an
uncertain tool effect. `ara-agent::agent_loop::run_tool` records an execution
panic as a tool error with `details.panicked = true`. Before this change,
`validate_completed_summary_span` and Session's
`safe_soft_summary_prefix` recognized timeout and interrupted-recovery markers
but accepted that panic receipt after a later final assistant turn. The Agent
summary serializer also reported `unknown_effect: false` for it. A raw journal
still retained the receipt, but a future model-visible summary could hide its
uncertainty.

Both validators now reject the existing `panicked: true` receipt. The Agent
summary serializer uses the same predicate, so its diagnostic field also says
`unknown_effect: true`. A completed Bash failure with an `exitCode` remains
eligible. Agent tool execution, receipt production, Session writing and CLI
resume were not changed.

## Executed checks

| Check | Observed result |
| --- | --- |
| New Agent and Session regressions on old code | Both failed: Agent returned `Ok(())` instead of `UnknownToolEffect`; Session returned a summary projection instead of `UnsafeSummaryBoundary`. |
| Same focused regressions on `a1b5d58` | Exit 0, 1/1 each. The Agent test also checks summary serialization and a completed `exitCode` control; the Session test checks reopened journal projection and the completed failure control. |
| `cargo test -p ara-agent --all-targets --all-features --quiet` | Exit 0: groups of 1, 37, 13, 8, 11 and 4 tests. |
| `cargo test -p ara-session --all-targets --all-features --quiet` | Exit 0: groups of 0, 7, 8 and 13 tests. |
| `cargo clippy -p ara-agent -p ara-session --all-targets --all-features -- -D warnings` | Exit 0. |
| `python scripts/verify_backend.py` | Exit 101: owned formatting and strict workspace Clippy passed; all-target workspace tests reached CLI e2e, where 40/41 passed. The sole failure was the existing Windows Git Bash deadline assertion at `crates/ara-cli/tests/e2e.rs:1465`. |

Independent Codex pragmatism, architecture and performance/security reviewers
traced the compaction and Session boundaries. The performance/security review
found the reachable panic gap and a separate cancelled-Bash receipt ambiguity.
Independent diff reviewer `/root/compaction_unknown_effect_diff_review`
identified that an initial rule rejecting every error with empty details would
also block known unexecuted host-denied tools. That rule and its assertions
were removed. Its final re-review of all four changed files found no
high-confidence issue; it ran `git diff --check` but not Cargo.

## Remaining uncertainty and proposed integration boundary

Cancelled Bash execution can return `ToolError` after side effects, and
`run_tool` currently records that error with empty details. The same empty
shape can come from a tool blocked before execution. The existing receipt
cannot distinguish those cases. This change does not claim to solve them.
The next receipt-contract change should mark proven pre-execution denials as
`executed: false` and post-start unknown errors as an explicit uncertain
outcome, then test cancellation after a file effect, completed nonzero exit,
and later compaction eligibility. That touches the stable Agent tool receipt
path and needs explicit scope confirmation under the supplied `AGENTS.md`.

Durable compaction also needs more than a summary helper. A proposed first
transaction is: serialize **all** Session append/rewrite writers for one
journal, re-read the durable session and leaf under that coordination, validate
the selected completed-turn cut and exact source entry IDs, then append one
typed soft-compaction entry while preserving every raw message and receipt.
A lock used only by the new compaction writer cannot prevent an ordinary
`append_raw` from racing it. Tests must cover two handles, stale leaf, malformed
source, unsafe cut, I/O failure, reopen and raw history. This changes stable
Session write behavior and likewise needs explicit scope confirmation.

The CLI currently resumes via tolerant `SessionJournal::build_context()` and
ignores compaction records. A later reviewed host/Core slice must consume the
strict projection, keep a summary typed with source attribution until model
conversion, and verify a real process restart against a controlled upstream.
It must not journal a derived summary as an ordinary user instruction. Actual
provider request size, incremental summary lineage, a bounded long-context
model task and the full backend gate remain open. AGT-COMPACTION, A3 and the
formal module/overall counts do not advance with this WIP.
