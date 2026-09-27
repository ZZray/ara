# AI-ANTHROPICa: tool-call stream snapshots (WIP)

Date: 2026-09-27. Local code commit: `4a7ebe1` on `dev` (not pushed).
Fixed OMP source: `596f2da7101178214aa27a753529d15e6b7ad91d`,
`packages/ai/src/providers/anthropic.ts` tool block start/delta/finalization
around lines 2300–2353 and 2671–2800, and
`packages/utils/src/json-parse.ts::parseStreamingJsonThrottled`.
This is part of the open AI-ANTHROPICa surface, not point acceptance.

## Behavior and scope

Anthropic `tool_use` now occupies an assistant content index at
`content_block_start`; `toolcall_start` carries its ID, name and bounded
initial argument preview. `input_json_delta` keeps that index and updates the partial
argument preview after geometric growth. At `content_block_stop`, a strict
parse writes the complete arguments at that index before `toolcall_end`.
Interleaved text blocks do not shift the call's index. The raw deltas remain
available in the event stream; the Agent executes a tool only after a
completed call and successful terminal turn.

ARA's `AssistantMessageEvent` owns a cloned snapshot per delta, whereas OMP
mutates a shared JavaScript message. ARA limits the preview to 4 KiB and
clears it above that size so small deltas cannot repeatedly copy an
ever-growing argument value. The streamed raw JSON is still bounded at
1 MiB and strictly parsed at block stop. The 4 KiB preview limit is an
intentional ARA difference. An Error/Aborted provider snapshot can contain
an in-flight call, as in OMP; it has no `toolcall_end`, and the Agent removes
unfinished calls before committing the turn. ARA still rejects malformed
final JSON without a `toolcall_end` instead of OMP's best-effort repair.
That preexisting final-parse difference remains open.

Production changes are confined to `crates/ara-ai/src/providers/anthropic.rs`.
The HTTP and real-process tests are in `ara-ai` and `ara-cli`; Agent, Session,
CLI production paths, other providers, strict tools, model limits and Windows
Bash lifecycle are unchanged. The path exercised is CLI → Agent → Anthropic
SSE provider → live JSON event → completed tool → file and Session journal.

## Executed checks on the code snapshot committed as `4a7ebe1`

| Check | Actual result |
| --- | --- |
| New tool snapshot HTTP test on old provider code | Exit 1: `toolcall_start.contentIndex` was 1, but its `partial.content` length was 1, so no call existed at that index. |
| `cargo test -p ara-ai --all-targets --all-features --quiet` | Exit 0: 116 unit, 26 Anthropic HTTP, 48 Chat HTTP and 29 Responses HTTP tests. Fake SSE tests check start/delta/end snapshots, initial input, interleaved text, throttled preview, full final arguments, >4 KiB input sent in 128-byte deltas, malformed final input, timeout and cancellation. Error snapshots have no `toolcall_end`. |
| `cargo test -p ara-cli --test e2e anthropic_ --quiet` | Exit 0: 8/8 real `ara` process cases. The new JSON-mode case observes `toolcall_delta` while the process is active and `live.txt` is absent; `toolcall_end` precedes tool execution, which writes exactly `one`, journals one tool result and uses two upstream requests. The existing incomplete-tool case still exits nonzero without a file or tool result. |
| `python scripts/verify_backend.py` | Exit 101. ARA-owned formatting and strict workspace Clippy passed. Workspace all-target tests reached CLI e2e: 37/38 passed; the existing Windows `deadline_during_a_tool_and_zero_budget_exit_nonzero` elapsed-time assertion failed at `crates/ara-cli/tests/e2e.rs:1315`. Log: `%TEMP%\ara-anthropic-tool-stream-gate.log`. |
| `cargo fmt --all -- --check` | Exit 1 only in 335 unchanged vendored `crates/vendor/pi-*` diffs; no ARA-owned diff. Log: `%TEMP%\ara-anthropic-tool-stream-fmt.log`. |
| `cargo test --workspace --doc --all-features --quiet` | Exit 0; no documentation tests are defined. |
| `cargo deny check --hide-inclusion-graph` | Exit 0; existing duplicate and no-license-field warnings remain. |
| `python scripts/omp_inventory.py check`; `git diff --check` | Exit 0 each. |

Independent Codex plan reviewer `/root/anthropic_tool_stream_plan` checked
the fixed OMP behavior, event indices, Agent cleanup and test scope.
Independent Codex diff reviewer `/root/anthropic_tool_stream_diff_review`
found a real unbounded argument-copy regression in the initial candidate.
The 4 KiB preview limit and a large-input fake HTTP case resolve it; its
final read-only review found no further reachable correctness defect. It
did not run Cargo tests. No long-stream benchmark or bounded real-model task
ran. The complete delivery gate, broader Anthropic parity and point audit
remain open; accepted counts stay AI 6/8, registered 27/37, P0–P6 1/7.
