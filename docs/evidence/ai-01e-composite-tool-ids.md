# AI-01e follow-up: Responses composite tool IDs in Chat replay (WIP)

Date: 2026-09-27. Local code commit: `3271bb7`. Fixed OMP source commit:
`596f2da7101178214aa27a753529d15e6b7ad91d`, especially
`packages/ai/src/providers/transform-messages.ts:33-245,1004-1204`,
`packages/ai/src/providers/openai-completions.ts:1938-1963`, and
`packages/ai/test/duplicate-tool-results.test.ts:633-1204`.

## Requirement and boundary

Responses-family history can hold a plain assistant call ID `call_A` beside a
tool result ID `call_A|fc_R`, or two composite IDs with the same `call_A`
half and different item halves. Pairing by the complete string discards a
real result and inserts `No result provided`. ARA now collects call origin
from the assistant `api` and uses the first component as a pairing key only
for Responses-family IDs. A concrete Chat Completions ID containing `|`
remains an opaque token, even if an earlier Responses call used the same
prefix. Empty call halves (`|fc_X`) also retain their full key.

The generic transform preserves each result's original ID except for the
existing duplicate-occurrence rewrite. It suffixes every segment of a
duplicate composite call so its wire call component stays distinct. The
current OpenAI Chat adapter maps each transformed result to its assistant
call's actual emitted ID. For a Responses-origin composite call, that ID is
the sanitized first component, capped at 40 characters; collisions receive
a distinct `_dupN` ID. Chat-origin opaque IDs remain byte-exact unless a
wire collision requires a distinct ID.

This is useful when imported or switched Responses history is replayed to
the current OpenAI Chat adapter. There is no native Responses adapter in ARA
yet, so this does not claim a working Responses model route. Fixed OMP also
normalizes pipe-bearing IDs from other cross-model origins; this ARA slice
normalizes only source-identified Responses IDs. The Agent loop, Session,
CLI, provider routing, tool effects and Windows process lifecycle were not
changed.

## Executable evidence

The first new plain-call/composite-result test failed on the old transform:
it emitted `result:call_A:No result provided` instead of the real output.
The new shared-call-component test then exposed an implementation mistake:
the first attempt appended `_dup1` to the pairing key and lost the item
half. It now appends the suffix to the complete call ID and passes.

| Check on `3271bb7` | Actual result |
| --- | --- |
| `cargo test -p ara-ai --all-targets --all-features --quiet` | Exit 0: 55 unit and 48 fake HTTP tests. New cases cover plain/composite crossing, parallel calls, same call half, reuse across turns, aborted real result, empty call half, Chat opaque IDs and a mixed-prefix collision. |
| `responses_composite_results_and_opaque_chat_ids_match_calls_on_the_wire` | Fake HTTP/SSE test passed: one actual POST carried the Responses result under the emitted Chat call ID `call_A`, retained both opaque Chat IDs and their own results, and received `ack`. No real model or tool effect was involved. |
| `normalized_responses_call_id_collision_keeps_distinct_chat_pairs` | Serializer test passed: two distinct Responses call IDs that share their first 40 characters received distinct Chat wire IDs; both tool results matched their emitted calls. |
| `cargo fmt -p ara-ai -- --check`; `cargo clippy -p ara-ai --all-targets --all-features -- -D warnings`; `git diff --cached --check` | Exit 0 on the code snapshot. |
| `python scripts/verify_backend.py` | Exit 101. Owned formatting and workspace strict Clippy passed; workspace tests stopped at the existing Windows CLI `deadline_during_a_tool_and_zero_budget_exit_nonzero` timing assertion after 18/19 CLI end-to-end tests passed. Local log: `%TEMP%\ara-ai-composite-final-gate.log`. Later workspace tests were not reached. |
| `cargo deny check`; `python scripts/omp_inventory.py check`; `cargo test --workspace --doc --all-features --quiet` | Exit 0 each. Deny retained existing duplicate-dependency warnings; no doc tests are defined. |

Independent Codex plan and diff reviewers compared the fixed OMP source and
all three changed files. The plan reviewer required Chat wire remapping to
make the fix useful in the current adapter. The final diff review covered
3/3 files and found no confirmed introduced defect; it did not rerun Rust
tests. No Claude review was used.

## Open limits and delivery decision

An exact cross-protocol collision remains ambiguous: a Responses result
`call_A|fc_R` can be byte-identical to a later Chat opaque call ID. The
global concrete-ID origin scope in both fixed OMP and ARA then favors the
opaque interpretation. The result message carries no source protocol, so
this slice does not invent a new pairing rule for that case. Native Responses
wire encoding and other target-specific ID rules remain open.

The full backend gate is red, and no complete Agent host receipt or bounded
real-model task ran on this commit. AI-01e remains WIP; accepted counts stay
at AI 6/7 and P0-P6 1/7.
