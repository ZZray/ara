# AI-01e follow-up: malformed tool calls in provider replay (WIP)

Date: 2026-09-27. Local code commit: `b17c690`. Fixed OMP source commit:
`596f2da7101178214aa27a753529d15e6b7ad91d`, particularly
`packages/ai/src/providers/transform-messages.ts:255-347` and
`packages/ai/test/transform-messages-malformed-tool-calls.test.ts:64-265`.

## Behavior and boundary

The provider-bound `ara-ai::transform::transform_messages` now removes
assistant tool calls whose ID or name is blank or whitespace-only before
duplicate-ID repair. It removes the matching tool-result occurrence, keeps
other assistant content and valid results, and drops an assistant message
only when no content blocks remain. Per-ID FIFO result queues reset at each
non-result message, so a missing rejection result cannot consume a later
valid call's result with the same ID. Input history is not mutated.

This follows fixed OMP's sanitization order. The Agent loop, first-run tool
handling, session journal, provider route, retry policy, budgets, usage and
Windows process lifecycle were not changed. OpenAI Chat may still observe a
malformed call during the first Run; this slice repairs subsequent history
replay, where otherwise the invalid call can make every next request fail.

## Executable evidence

The first new transform regression failed before implementation: the
malformed call's `Tool not found` result remained in replay. On `b17c690`:

| Check | Actual result |
| --- | --- |
| `cargo test -p ara-ai --all-targets --all-features --quiet` | Exit 0: 49 unit tests and 47 fake HTTP tests passed. New cases cover blank IDs/names, assistant text and valid calls, same-ID call occurrences in either order, absent rejection result across a boundary, input preservation and repeat transform. |
| `malformed_tool_call_and_result_do_not_reach_openai_wire` | Fake HTTP/SSE test passed: one POST carried only the valid `read` call and its real file content, preserved assistant prose and the continuation user message; fake upstream replied `continued`. No real model or tool effect was involved. |
| `cargo fmt -p ara-ai -- --check`; `cargo clippy -p ara-ai --all-targets --all-features -- -D warnings`; `git diff --check` | Exit 0 on the code snapshot. |
| `python scripts/verify_backend.py` | Exit 101. Owned formatting and workspace strict Clippy passed; the workspace test stage failed the existing Windows CLI `deadline_during_a_tool_and_zero_budget_exit_nonzero` timing assertion after 18/19 CLI end-to-end tests passed. Remaining workspace tests were not reached. Local log: `%TEMP%\ara-ai-malformed-wip-gate.log`. |
| `cargo deny check`; `python scripts/omp_inventory.py check`; `cargo test --workspace --doc --all-features --quiet` | Exit 0 each. Deny reported its existing duplicate-dependency warnings; no doc tests are defined. |

Independent Codex plan and diff reviewers checked the fixed OMP sanitizer,
the two changed files, and duplicate-ID result pairing. The diff review
covered 2/2 files with no confirmed introduced defect. It noted that a
delayed old result with an identical reused ID cannot be distinguished from
a later valid call's result by ID and position alone; fixed OMP has the same
limit. No Claude review was used.

## Delivery decision

The full backend gate is red and no bounded real-model task or complete
Agent host receipt ran on this commit. AI-01e and the overall gate remain
open; the local code commit is explicitly WIP. Accepted counts stay at
AI 6/7 and P0-P6 1/7. Cross-protocol Responses composite-ID pairing,
target-specific ID normalization and other provider replay differences
remain separate open work.
