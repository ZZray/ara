# AI-01e Responses incomplete native history (WIP)

## Requirement and source

Fixed OMP `596f2da7101178214aa27a753529d15e6b7ad91d` collects
`output_item.done` records in `packages/ai/src/providers/openai-responses.ts`
around lines 751–850. After a successful terminal event, including
`response.incomplete` with `max_output_tokens`, it saves a native history
payload. `packages/ai/src/providers/openai-shared.ts` around lines 3341–3390
maps truncation to `length` and promotes only provably complete tool calls to
`toolUse`. `packages/ai/src/utils.ts` around lines 337–373 and 414–421
allows warm replay only when sanitized items contain non-hidden output and
filters malformed function arguments.

ARA now captures native items for successful completed and incomplete
Responses terminals in `ara-ai::responses_stream`. Capture includes
reasoning-only turns, but these cannot warm or replay. Valid visible `length`
turns can warm and replay through `ara-ai::providers::openai_responses`;
restart creates a cold host state. Partial tool argument parse markers remain
raw in the saved item and cannot be replayed as valid native calls. Agent tool
execution and Session writing were not modified.

## Executed evidence on this WIP worktree (2026-09-27)

- `cargo test -p ara-ai --quiet`: 93 unit, 48 Chat HTTP and 11 Responses HTTP
  tests passed. The new fake HTTP cases assert visible truncated text and
  encrypted reasoning warm the same session, a new state starts cold, and a
  hidden-only truncated response is stored without warming.
- `cargo test -p ara-cli --test e2e responses_ --quiet`: 8/8 real-process
  Responses cases passed. The new CLI case runs two prompts in one process,
  checks the first `length` assistant and encrypted payload in its Session
  JSONL, observes native replay on the second request, then starts a new
  `--resume` process and checks visible-only cold history. Text-mode output
  contains only the final answer and no opaque marker.
- Unit tests verify an actual partial function terminal preserves invalid
  raw arguments in its payload and leaves the tool call at `length`. A
  separate request-encoder regression rejects `__parseError` calls as native
  history. Existing tests cover complete tool promotion, mixed complete and
  partial calls, `content_filter`, failed responses, and cancellation; the
  cancellation test now also asserts no payload.
- `cargo clippy --workspace --all-targets --all-features -- -D warnings`,
  `cargo test --workspace --doc --quiet`, `cargo deny check`,
  `python scripts/omp_inventory.py check`, scoped `rustfmt --check`, and
  `git diff --check` passed. `cargo deny` retains non-fatal existing warnings.
- With Git Bash in PATH, `cargo test --workspace --all-targets --quiet`
  reached CLI E2E and passed 26/27 there; the existing Windows tool deadline
  timing assertion failed. `cargo fmt --all -- --check` fails in unrelated
  vendored `pi-*` files; the local transcript is
  `%TEMP%\ara-incomplete-history-fmt.log`. The complete delivery gate is red.

Independent Codex agents `/root/incomplete_source` and
`/root/incomplete_risk` reviewed the fixed source, plan, and diff. The diff
review found no new high-confidence blocking defect in this slice.

## Remaining difference and acceptance

For a truncated response containing visible text plus a malformed tool call,
fixed OMP filters the invalid native call and can replay the remaining valid
items. ARA rejects that turn's entire native payload and uses visible
fallback history; `native_history` still requires one item per assistant
block. This also affects previously implemented completed turns. Incremental
filtering needs explicit repair of any orphaned tool result before it can be
called parity. Server-side `previous_response_id`, `dt:false` full snapshots,
a bounded actual-model task, the full backend gate, and point audit remain
open. No AI-01e or P0–P6 acceptance is claimed.
