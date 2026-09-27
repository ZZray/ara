# AI-01e: stateless Responses request encoder (WIP)

## Source and boundary

Pinned OMP commit `596f2da7101178214aa27a753529d15e6b7ad91d`:
`packages/ai/src/providers/openai-responses.ts::buildParams` and
`packages/ai/src/providers/openai-shared.ts::buildResponsesInput`,
`convertResponsesAssistantMessage`, and
`appendResponsesToolResultMessages`. ARA implementation:
`crates/ara-ai/src/providers/openai_responses.rs::build_request`.

This increment only prepares a `store:false`, `stream:true` `/responses`
body. It serializes system instructions, user/developer content, assistant
text, JSON function calls, paired function outputs, tool definitions, tool
choice and output limit. Existing `transform_messages` repairs history before
encoding. Responses composite IDs use their call component; foreign opaque
IDs are normalized and collisions remapped with paired outputs. Image blocks
in user input and tool results use native content with `detail:auto` only when
the host confirms image capability; otherwise they become the same non-vision
placeholder used by fixed OMP. Tool choice is omitted when no matching tool
is available.

Stored developer turns use the pinned OMP generic Responses `user` fallback.
The current ID normalizer keeps some foreign and long call-ID spellings that
fixed OMP hashes; all outputs are mapped to the emitted ID. This is an
intentional wire difference pending real endpoint compatibility checks.
Fixed OMP credential redaction is opt-in and disabled by default. This WIP
route has no host setting for enabling it.
Fixed OMP also normalizes Responses tool schemas, adapts strict mode and
quarantines incompatible tools before emitting them. This encoder currently
copies the supplied JSON Schema unchanged; that compatibility surface is
unverified and still open.

## Verification on this worktree

- `cargo check -p ara-ai --lib`: passed.
- `cargo test -p ara-ai providers::openai_responses::tests --lib`: 6 passed
  after independent review corrections; the last candidate added tool-result
  image assertions to the existing image test.
- `cargo test -p ara-ai`: 61 unit, 48 fake Chat HTTP tests and 0 doc tests
  passed on the final candidate.
- `cargo clippy -p ara-ai --all-targets -- -D warnings`: passed.
- Targeted `rustfmt` completed. Workspace-wide format check was not used as
  proof because unrelated vendored sources currently differ from local
  rustfmt output.

The new tests check the prepared JSON value, not an actual HTTP request.
No Responses HTTP/SSE fixture, CLI process chain or real-model run has run on
this code. This is WIP and changes no acceptance count.

Two independent Codex reviews were completed for this increment. The first
found three request-shape issues (developer history role, image detail and
capability handling, no-tool `tool_choice`), which were corrected. The second
found no new high-confidence P0/P1 issue. The first review's concern about
system-prompt credential redaction was withdrawn after confirming that fixed
OMP disables its opt-in redaction by default. The remaining tool-schema
compatibility and ID spelling differences are recorded above.

## Remaining delivery work

1. Implement a strict bounded Responses SSE decoder: partial UTF-8,
   interleaved `output_index` and `item_id` tool calls, final arguments,
   text, response ID, unknown usage, terminal/error/abort conditions.
2. Bind the provider to `ModelProvider` and an explicit CLI protocol choice;
   preserve the Chat default and handle Chat-only request-text diagnostics
   explicitly.
3. Verify real HTTP POST and streamed fake-upstream faults, then run a real
   `ara` process through tool effect, journal and resumed history.
4. Review the complete diff independently, run the full backend gate on the
   delivered code and a bounded real-model trial before acceptance.

Native reasoning output item replay and `previous_response_id` state are
still absent from ARA message storage and are separate parity gaps.
