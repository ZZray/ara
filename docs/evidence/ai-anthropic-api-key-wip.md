# AI-ANTHROPICa: API-key Messages route (WIP, 2026-09-27)

## Scope and source

Delivered WIP code: local commits d904ed7 and da86da5. Fixed OMP source:
packages/ai/src/providers/anthropic.ts at
596f2da7101178214aa27a753529d15e6b7ad91d, especially buildParams
(3886–4140), convertAnthropicMessages (4245–4550), stream envelope
(2499–2905), and mapStopReason (5073–5095). The ARA path is
ara-cli → ara-agent → ara-ai Anthropic Messages → real tools → ara-session.
It is an additive API-key path. Existing Agent loop, Session format, OpenAI
adapters and ARA Manager were not modified.

This slice implements the Messages request shape, normal text/thinking/tool
SSE lifecycle, signed same-model thinking replay, tool result pairing,
reported usage, HTTP errors, cancellation, idle/first-event limits and
live CLI events. A completed tool call is added to the assistant message
only after a valid object input and content_block_stop. The CLI uses
ANTHROPIC_API_KEY automatically only for HTTPS api.anthropic.com;
custom endpoints use ARA_API_KEY/ARA_TEST_API_KEY or explicit
--api-key-env. The fake upstream redacts x-api-key.

## Executed evidence on delivered code

Windows PowerShell, fixed OMP checkout unchanged, Git Bash available at
C:\Program Files\Git\bin\bash.exe for Bash-dependent workspace tests.
All keys in fixtures are dummy values.

| Check | Result and observed artifact |
| --- | --- |
| cargo test -p ara-ai --test anthropic_http --quiet | Exit 0, 7/7. Captured POST /v1/messages with system, tools, max_tokens 4096 and redacted x-api-key. Text + tool_use has id toolu_1, arguments for a.txt and usage input 11/output 7/cache read 2/total 20. Follow-up carries paired tool_result. Signed thinking is retained only on same model; absent usage remains unknown. |
| cargo test -p ara-cli --test e2e anthropic_ --quiet | Exit 0, 4/4. Real ara process wrote anthropic.txt with exact bytes "from anthropic\n", journaled assistant tool call and toolResult, restarted from Session and sent the paired result. A second JSON process exposed "first " while the Run was still active, then finished "first second". A truncated write call produced exit 1 and no should-not-exist.txt or toolResult. Official key was not sent to a custom endpoint. |
| cargo test -p ara-cli anthropic -- --nocapture | Exit 0 before the additional incomplete-tool e2e was added: route host unit test 1/1 and Anthropic e2e 3/3. The added fourth e2e passed separately above. |
| cargo clippy --workspace --all-targets --all-features -- -D warnings | Exit 0 on the final code. |
| cargo test --workspace --doc --quiet | Exit 0. |
| cargo deny check | Exit 0: advisories, bans, licenses and sources OK; existing duplicate/license-field warnings remain. |
| python scripts/omp_inventory.py check | Exit 0: fixed inventory consistent. |
| git diff --cached --check and rustfmt --check on seven changed Rust files | Exit 0 before code commit. Full cargo fmt --all -- --check remains red in unchanged vendored pi-* formatting. |
| cargo test --workspace --all-targets --quiet, with Git Bash in PATH | Exit 1 on d904ed7: ara-cli e2e 31/32, only deadline_during_a_tool_and_zero_budget_exit_nonzero fails elapsed <4s. This is the previously reproduced Windows Bash descendant/process-pipe issue; see windows-bash-process-tree.md. The full gate is not passed. |

## Non-strict tool schema follow-up (da86da5)

Fixed OMP source: packages/ai/src/providers/anthropic.ts
normalizeAnthropicToolSchemaNode (4561–4777) and
buildAnthropicBaseToolInputSchema (4995–5005), plus
packages/ai/src/utils/schema/wire.ts::toolWireSchema (601–608).
ARA's Anthropic tool encoder now upgrades legacy draft JSON Schema, forces
the Messages root object shape and string-only required names, then keeps
Anthropic's supported keywords. Unsupported constraints are retained in
the relevant description. Nested properties, array items, combinators and
definitions are visited only where they contain schemas; defaults, enum
values and const data are preserved. Object nodes close by default while
explicit open maps stay open. Strict tool mode remains outside this slice.

| Check on da86da5 | Result and observed artifact |
| --- | --- |
| cargo test -p ara-ai --test anthropic_http --quiet | Exit 0, 10/10. The fake HTTP POST body contains upgraded draft-07 refs, the filtered required list, supported format, spilled pattern/range/array constraints, closed and open maps, and no root allOf/oneOf. Input parameters remain unchanged. Empty and excessively deep schemas have explicit request-construction outcomes. |
| cargo test -p ara-ai --quiet | Exit 0 on da86da5: 108 unit, 10 Anthropic HTTP, 48 Chat HTTP, 13 Responses HTTP. |
| cargo test -p ara-cli --test e2e anthropic_ --quiet | Exit 0, 4/4 real CLI process cases. |
| cargo clippy --workspace --all-targets --all-features -- -D warnings | Exit 0 after the final deep-schema test. |
| cargo test --workspace --doc --quiet; cargo deny check; python scripts/omp_inventory.py check | Each exits 0. Deny retains existing duplicate/license-field warnings. |
| rustfmt --check on the two changed Rust files; git diff --cached --check | Exit 0. Full cargo fmt --all -- --check exits 1 in unchanged vendored pi-* formatting. |
| cargo test --workspace --all-targets --quiet with Git Bash in PATH | Exit 1: the known Windows Bash deadline case fails elapsed <4s (CLI e2e 31/32); full gate remains red. |

An independent Codex plan reviewer scoped this follow-up before the edit.
An independent Codex diff reviewer covered both changed code/test files,
compared the pinned OMP source and found no high-confidence reachable defect;
the reviewer did not run its own tests. The 128-level normalization limit is
an ARA fail-closed difference for acyclic JSON Values. Remaining generic
toolWireSchema postprocessing differences include nullable anyOf rewriting,
bare enum type inference and const-union collapse; these need separate
source-backed fixtures before parity is claimed. No real-model task ran.

The focused HTTP negatives cover an incomplete tool JSON/EOF with no
completed call, 401 with errorStatus, caller cancellation, and a missing
message_stop after a valid stop_reason (best-effort Done). Forced tool
choice is omitted if its target is unavailable; a per-call token request
is capped at the model ceiling.

## Review and open gates

Independent Codex pre-implementation and post-diff reviewers covered
the seven changed code/test files, Agent/CLI contracts and fixed OMP
source. Review found tool-use replay order, adjacent assistant turns,
error tool-result images, EOF classification, key routing, output ceiling,
forced choice and initial thinking delta issues. Each was corrected and
rechecked against the added fixtures. Final read-only diff review found
no further deterministic WIP blocker; the reviewer did not independently
execute tests.

Still open: Anthropic strict tools and remaining generic schema postprocessing,
model catalogue output limits (unknown models use a conservative 4096
default rather than OMP's catalogue-derived ceiling), OAuth and beta
features, cache controls, vendor dialects, server tools/fallback, complete
signature/prefix rules and the remaining fixed OMP behavior inventory.
No bounded real-model task or full delivery audit passed. This row remains
implementing (WIP); neither AI-ANTHROPIC nor P2 is accepted.

Estimate: 4–12 working days to reach the next Anthropic API-key protocol
checkpoint and bounded real-model evidence, low confidence. Full fixed
Anthropic parity is a larger part of the multi-month P0–P6 plan.
