# AI-ANTHROPICa: cross-provider tool ID replay (WIP)

Date: 2026-09-28. Code commit `f6ec81a` on local `dev`, not pushed. This is
provider request conversion and host evidence, not an accepted
AI-ANTHROPICa point. Fixed OMP commit:
`596f2da7101178214aa27a753529d15e6b7ad91d`.

## Source and observed gap

Fixed OMP `packages/ai/src/providers/anthropic.ts:4064` calls
`transformMessages` with `normalizeToolCallId`. Its
`packages/ai/src/providers/transform-messages.ts:351-370,889-924` enforces
Anthropic's `[A-Za-z0-9_-]` tool-use ID characters and 64-character limit,
then carries Responses composite call/result IDs through one mapping. The
normalizer is in `packages/ai/src/utils.ts:32-35`. The original message
history remains the source; only the target request is rewritten.

ARA previously ran shared `transform_messages`, then copied the remaining
tool-call and result IDs directly into Anthropic `tool_use.id` and
`tool_result.tool_use_id`. A Responses call `call_X|fc_A` with result
`call_X|fc_B` became two different IDs containing `|`. A long or otherwise
invalid ID also reached the Anthropic request unchanged. The controlled
request test failed on the old implementation because at least one emitted
ID did not meet the target grammar.

## Change and boundary

`ara-ai::providers::anthropic::convert_messages` now plans one target ID per
surviving tool call after the shared history repair. It replaces unsupported
characters, caps IDs at 64 ASCII characters, adds a bounded suffix when two
different source IDs would collide, and uses `ToolCallOriginScope` to map a
Responses result's call component to the same target ID. The rewritten ID is
used only in the outbound Anthropic request. No persisted history, other
provider encoder, Agent execution, Session writer or CLI lifecycle changed.

ARA deliberately retains its existing shared transform order and applies the
target normalization at the Anthropic encoder boundary. Rust Unicode scalar
replacement can yield a different underscore count from JavaScript's UTF-16
regular expression for a non-BMP source ID, but both produce legal paired
IDs. This slice does not complete other target-specific ID transformations.

## Executed evidence

| Check | Result |
| --- | --- |
| New actual HTTP request test on old code | Failed: emitted tool ID violated the Anthropic character or length condition. |
| New actual HTTP request test on changed code | Passed: Responses composite call/result, a normalized-ID collision, overlong ID and plain call with composite result produced four distinct legal IDs with corresponding result text. Original Context was unchanged. |
| New Agent → Anthropic provider → fake HTTP test | Passed: one resumed Agent request paired `call_1|fc_A` and `call_1|fc_B` on the wire, retained the real result text `file contents`, and completed with `continued`. |
| `cargo test -p ara-ai --all-targets --all-features --quiet` | Exit 0: 120 unit, 34 Anthropic HTTP, 48 Chat HTTP and 29 Responses HTTP tests. |
| `cargo test -p ara-agent --all-targets --all-features --quiet` | Exit 0: six groups totalling 75 tests. The final added result-text assertion also passed in a focused rerun and in the later full verifier. |
| `cargo test -p ara-cli --test e2e anthropic_ --quiet` | Exit 0: 11 real CLI processes against controlled upstreams. |
| `cargo clippy -p ara-ai -p ara-agent --all-targets --all-features -- -D warnings` | Both crate checks passed when run separately. |
| `python scripts/verify_backend.py` | Exit 101: owned format and strict workspace Clippy passed; all-target workspace tests reached CLI e2e with 40/41 passing. The sole failure remained Windows Bash deadline assertion `crates/ara-cli/tests/e2e.rs:1465`. |
| `cargo deny check --hide-inclusion-graph`; `python scripts/omp_inventory.py check`; `python scripts/verify_bootstrap.py`; `git diff --check` | Exit 0. Existing duplicate-dependency and `tree-sitter-graphql` manifest warnings remain. |

Independent Codex pre-plan reviewer `/root/anthropic_next_plan_review`
identified the source-backed ID mismatch and the risk of collisions after
normalization. Independent diff reviewer `/root/anthropic_id_diff_review`
reviewed the provider, HTTP and Agent test changes and found no
high-confidence production issue. The reviewer suggested checking real
result text in the Agent test; that assertion was added and passed. The
reviewer ran `git diff --check` but not Cargo.

Actual Anthropic acceptance of these IDs, a bounded real-model cross-provider
task, other Anthropic parity work and the full Windows backend gate remain
open. Counts stay AI 6/8, registered points 27/37 and P0–P6 gates 1/7.

## Real CLI restart and persisted history follow-up

The `responses_tool_history_replays_to_anthropic_after_cli_restart` test runs
two separate `ara` processes against controlled upstreams. The first takes a
Responses `write` call and writes `cross-provider.txt`; its Session journal
retains `call_write|fc_write` on the call and result. The test then changes the
file to a different sentinel value. The second process resumes that same
Session on `--api anthropic-messages`. Its outbound request carries the real
write result under one legal, matching `tool_use`/`tool_result` ID. The
sentinel survives, so a replay of the old write did not occur. The original
journal entries remain an exact prefix of the appended journal when parsed
as JSON entries, and its tool-result count remains one. The second process
exits 0 with the scripted answer.

| Check on this follow-up | Result |
| --- | --- |
| `cargo test -p ara-cli --test e2e responses_tool_history_replays_to_anthropic_after_cli_restart -- --nocapture` | Exit 0, 1/1 actual CLI restart test after the independent review's sentinel and journal-prefix corrections. |
| `python scripts/verify_backend.py` | Exit 101: owned formatting and workspace strict Clippy passed; CLI e2e reached 41/42 passing. Only the existing Windows Bash deadline assertion failed, now at `crates/ara-cli/tests/e2e.rs:1563`. |

Independent Codex reviewer `/root/anthropic_id_diff_review` first found that
checking unchanged file bytes would not detect an identical overwrite, and
that checking only the presence of the old ID did not prove raw journal
preservation. After adding the sentinel and exact journal-prefix assertions,
the reviewer rechecked the test and found both evidence gaps resolved. The
reviewer ran `git diff --check`; Cargo was run separately by the author.
This strengthens host evidence but does not prove a real Anthropic model
accepted the request or clear the full backend gate. Formal counts stay
unchanged.
