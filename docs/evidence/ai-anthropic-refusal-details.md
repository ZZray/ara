# AI-ANTHROPICa: refusal and sensitive stop details (WIP)

Date: 2026-09-27. Local code commit: `11c81f3` on `dev` (not pushed).
Fixed OMP source: `596f2da7101178214aa27a753529d15e6b7ad91d`,
`packages/ai/src/providers/anthropic.ts` `message_delta` handling around
lines 2818–2840 and `packages/ai/test/anthropic-stream-envelope.test.ts`
around lines 1773–1811. This is part of the open AI-ANTHROPICa point.

## Requirement and boundary

An Anthropic `refusal` or `sensitive` stop terminates as an error and keeps
the source `stop_details` for review. A null or absent detail becomes
`{"type":"refusal"}` or `{"type":"sensitive"}`. Structured refusal
category and trimmed explanation form the error text; null details use the
fixed OMP fallback text. The Agent must not execute a completed tool call
when the provider ends that turn with a refusal.

Production changes are confined to `crates/ara-ai/src/providers/anthropic.rs`.
The other two changed files add controlled HTTP and real `ara` process tests.
Agent, Session, CLI production flow, other provider protocols, and Windows
Bash process lifetime remain unchanged. Full Anthropic parity and a point
acceptance remain open.

The exercised path is `ara` CLI → Agent turn → `ara-ai` Anthropic Messages
HTTP/SSE → Agent terminal error → Session journal and CLI exit. The real
process fixture offers a complete `write` tool call before refusal. Its
target file is absent; the journal records one synthetic, unexecuted error
tool result, preserving the refusal details on the assistant message.

## Executed checks on the code snapshot committed as `11c81f3`

| Check | Actual result |
| --- | --- |
| New controlled HTTP test against old provider code | Exit 1: null `stop_details` was retained as JSON null, not the fixed OMP fallback object. |
| `cargo test -p ara-ai --test anthropic_http refusal_details_and_sensitive_stops_keep_their_reason_in_terminal_errors -- --exact` | Exit 0. Five fake HTTP cases: null and absent refusal details, structured category and trimmed explanation, blank explanation, null sensitive details. All terminate as errors and preserve usage and stop details. |
| `cargo test -p ara-ai --all-targets --all-features --quiet` | Exit 0: 116 unit, 23 Anthropic HTTP, 48 Chat HTTP and 29 Responses HTTP tests. |
| `cargo test -p ara-cli --test e2e anthropic_ --quiet` | Exit 0: 7/7 real `ara` process cases. New case exits 1 with the refusal reason, has no written file, one synthetic `toolResult` with `executed=false`, preserved `stopDetails`, and one upstream request. |
| `python scripts/verify_backend.py` | Exit 101. ARA-owned formatting and strict workspace Clippy passed. Workspace all-target tests reached CLI e2e: 36/37 passed; the existing Windows `deadline_during_a_tool_and_zero_budget_exit_nonzero` elapsed-time assertion failed at `crates/ara-cli/tests/e2e.rs:1265`. Log: `%TEMP%\ara-anthropic-refusal-final-gate.log`. Verifier stopped before doc tests. |
| `cargo test --workspace --doc --all-features --quiet` | Exit 0; no documentation tests are defined. |
| `cargo deny check --hide-inclusion-graph` | Exit 0; existing duplicate and no-license-field warnings remain. |
| `python scripts/omp_inventory.py check`; `git diff --check` | Exit 0 each. |

An independent Codex plan reviewer checked the fixed OMP branches and scoped
the change to the provider. A separate independent Codex diff reviewer
examined the three-file diff against fixed OMP. It found a weak initial CLI
no-side-effect assertion, which was strengthened with a completed `write`
call and synthetic-result checks. Its second read-only review found no
remaining reachable defect; the reviewer did not run Cargo tests.

No bounded real-model task ran. The complete backend gate is red, and the
remaining Anthropic protocol surface and point audit are open. AI remains
6/8 accepted registered points, all registered points 27/37, P0–P6 gates
1/7. This commit is WIP, not delivered or accepted.
