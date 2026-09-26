# AI-01e follow-up: delayed real tool results and repeated IDs (WIP)

Date: 2026-09-27. Local code commit: `6ef8132`. Fixed OMP source:
`596f2da7101178214aa27a753529d15e6b7ad91d`, especially
`packages/ai/src/providers/transform-messages.ts:157-268,1007-1117` and
`packages/ai/test/duplicate-tool-results.test.ts:145-234,476-525,1428-1498`.

## Requirement and boundary

For provider-bound history, every surviving assistant tool call needs one
adjacent result. When its real result is later in the recorded message list,
the fixed OMP moves the earliest unconsumed result **after that call** into
the call's result window, retaining its content, error flag and timestamp.
An earlier orphan with the same ID must never be taken. Fixed OMP first
rewrites repeated tool-call IDs and their results by occurrence; without that
step, a delayed result for a later reused ID could be stolen by the first
call.

ARA now applies the same two-stage rule to its current opaque-ID OpenAI Chat
history in `ara-ai::transform`: repeated IDs get a distinct `_dupN` suffix
with a 64-character segment ceiling, then delayed real results are indexed
by ID and original position and pulled into normal or aborted call windows.
Consumed results are skipped at their old positions. The original history is
cloned and is not mutated. `ara-ai::providers::openai_completions` consumes the
transformed list before building the outbound request.

This slice leaves the Agent loop, Session, CLI, tool effects, provider route,
retry policy, budget and usage accounting unchanged. Cross-protocol Responses
composite-ID pairing, target-specific ID normalization, thinking-block
conversion and malformed-call sanitization remain open.

## Executable evidence

The new transform tests failed on the old code: delayed results became
`No result provided` or `aborted`, and a repeated ID's later result was
dropped. On `6ef8132`:

| Check | Actual result |
| --- | --- |
| `cargo test -p ara-ai --all-targets --all-features --quiet` | Exit 0: 45 unit tests and 46 HTTP integration tests passed. New cases cover normal and aborted windows, gaps across guidance/user messages, earlier same-ID orphan, multiple pending calls, duplicate results, repeated IDs across turns and inside a turn, suffix collision/length, input immutability, and moved result metadata. |
| `delayed_tool_result_is_adjacent_on_the_openai_wire` | Unit serializer test passed: outbound order is assistant call → real tool result → later guidance. |
| `delayed_real_tool_result_reaches_the_wire_before_guidance` | Fake HTTP/SSE test passed, 1/1: one actual POST carried the real result adjacent to its call and before the later user message; the fake upstream returned `ack`. No real model or tool effect was involved. |
| `cargo fmt -p ara-ai -- --check`; `cargo clippy -p ara-ai --all-targets --all-features -- -D warnings`; `git diff --check` | Exit 0 for each on the code snapshot. |
| `python scripts/verify_backend.py` on committed `6ef8132` with Git Bash on PATH | Exit 101. Owned formatting and workspace strict Clippy passed; the workspace test stage stopped at Windows CLI `deadline_during_a_tool_and_zero_budget_exit_nonzero` after 18/19 CLI e2e passed. Later workspace and doc tests were not reached. Log: `%TEMP%\ara-ai-transform-6ef8132-gate.log`. |
| `cargo deny check`; `python scripts/omp_inventory.py check`; `cargo test --workspace --doc --all-features --quiet` | Exit 0 for each. Deny retained existing duplicate-dependency warnings; no doc tests are defined. |

An independent Codex plan reviewer found the reused-ID stealing risk before
implementation. A separate independent Codex diff reviewer covered the
transform and serializer changes and later covered the added fake HTTP test.
It reported no confirmed introduced defect. It verified the focused tests
and noted that composite Responses IDs and target-specific normalization are
outside this OpenAI Chat slice.

## Delivery decision

This is source-backed provider evidence, not Agent/CLI host acceptance. The
full backend gate and a bounded real-model task on this commit remain open.
The AI module and overall P0–P6 accepted counts do not change. The code is
committed as WIP because those delivery conditions are unmet.
