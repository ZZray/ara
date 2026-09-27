# AI-01e explicit Responses full-history snapshot consumer (WIP)

## Requirement and fixed source

Fixed OMP `596f2da7101178214aa27a753529d15e6b7ad91d`,
`packages/ai/src/providers/openai-shared.ts:2008-2053` and
`packages/ai/src/utils.ts:475-499`, treats a full native-history snapshot as
replacement input rather than another assistant turn. Its
`openai-responses-history-payload.test.ts` covers that request shape.

ARA now consumes an **explicit assistant `dt:false`** Responses payload in
`ara-ai::providers::openai_responses::build_request`, after the provider has
warmed in this process. The snapshot replaces preceding input; later messages
remain, while system instructions stay in the separate `instructions` field.
Supported native messages, reasoning, function calls/results and compaction
items are validated and sanitized before replay. Call/result pairing is
rebuilt from the snapshot. A call with no later output receives an
`interrupted` result in the outbound request; no tool is executed by this
repair. The input item count and serialized byte size are bounded.

The change is confined to the Responses provider and its tests. Existing ARA
payloads **without `dt` retain their incremental meaning**; they are not
reinterpreted as full snapshots. Unsupported or malformed snapshots fall back
to visible history. User-message full snapshots, a producer of `dt:false`,
remote compaction, server-side chaining and complete fixed-OMP native item
coverage remain open. This consumer therefore cannot establish full-history
parity or point acceptance by itself.

## Executed evidence on committed WIP `3bffdba` (2026-09-27)

- Provider unit cases verify replacement and following messages, latest full
  snapshot precedence, subsequent incremental replay, call/result remapping,
  orphan repair, cold-state behavior and fallback for malformed JSON, role,
  phase or endpoint. Legacy incremental replay remains covered.
- `cargo test -p ara-ai --quiet` exited 0: 111 unit, 17 Anthropic HTTP, 48 Chat
  HTTP and 15 Responses HTTP tests passed. A controlled local HTTP server
  accepted two actual `POST /v1/responses` calls. After a warm seed, the
  second request used the injected canonical user and answer, inserted an
  interrupted output for the unmatched `call_branch`, kept `follow up`, and
  contained no `old user`. The server's completed answer was `done`. A second
  HTTP case proved an image-bearing tool output falls back for a non-vision
  request and replays for a vision-capable request.
- With Git Bash in PATH,
  `cargo test -p ara-cli --test e2e responses_ --quiet` exited 0 (9/9). These
  existing process cases exercise the Responses host route, but do not
  manufacture a `dt:false` producer.
- `cargo clippy --workspace --all-targets --all-features -- -D warnings`,
  `cargo test --workspace --doc --quiet`, `cargo deny check`,
  `python scripts/omp_inventory.py check`, and scoped
  `rustfmt --edition 2024 --check` on both changed Rust files exited 0.
  `cargo deny` retained non-fatal existing warnings.
- With Git Bash in PATH,
  `cargo test --workspace --all-targets --quiet` exited 101 at the existing
  Windows CLI deadline timing assertion: 33/34 CLI e2e cases passed and
  `deadline_during_a_tool_and_zero_budget_exit_nonzero` exceeded its four
  second limit. A focused rerun of that test also exited 101 at the same
  assertion. Other suites reached before that failure passed. The complete
  backend gate is red.
- `cargo fmt --all -- --check` exited 1 on unchanged vendored `pi-*` source;
  scoped formatting of the two changed files passed. Local diagnostic logs
  are `%TEMP%\ara-responses-full-snapshot-gate.log`,
  `%TEMP%\ara-responses-full-snapshot-fmt.log`, and
  `%TEMP%\ara-responses-full-snapshot-deny.log`.

Independent Codex plan reviewer `/root/responses_full_snapshot_plan_review`
bounded the work to assistant snapshot consumption. Independent Codex diff
reviewer `/root/responses_full_snapshot_diff_review` found and helped resolve
image-capability bypass, invalid role/content and phase acceptance, and
unmatched call handling. A subsequent re-review reported no remaining
concrete issue. The review did not replace executable verification.

## Remaining acceptance work

Implement and test the remaining full-history producer and user-message or
compaction paths, finish other AI-01e parity gaps, rerun the unfiltered backend
gate, execute a bounded real-model task on the delivered code where applicable,
then perform the independent point audit. No AI-01e, AI module or P0-P6
acceptance count changes with `3bffdba`.
