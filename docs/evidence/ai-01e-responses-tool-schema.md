# AI-01e: generic Responses tool schema compatibility (WIP)

## Source and boundary

Pinned OMP commit `596f2da7101178214aa27a753529d15e6b7ad91d`:
`packages/ai/src/providers/openai-responses.ts` builds the emitted tool list and
selects `tool_choice` from it. `packages/ai/src/utils/schema/normalize.ts`
normalizes JSON Schema for the Responses route, and
`packages/ai/src/utils/schema/strict-tool-validation.ts` rejects a tool whose
`enum` or `const` contradicts its declared type.

ARA implements the generic JSON Schema slice in
`crates/ara-ai/src/providers/openai_responses.rs::build_request`. It visits
schema-valued positions, maps `oneOf` to `anyOf`, turns an empty schema into
`true`, adds absent object `properties`, and removes regex lookaround patterns.
If multiple `patternProperties` keys collapse to `.*`, their schemas are
combined with `anyOf`. Literal data inside `enum`, `const`, `default` and
examples is left intact. A schema nesting limit of 128 prevents unbounded
recursion. A tool with an incompatible `enum` or `const`, or excessive nesting,
is omitted individually with a bounded, JSON-escaped stderr diagnostic. The
outbound `tool_choice` is computed from the tools that survived filtering.

This does not implement OMP's full draft-07 upgrade, vendor schema dialects,
ArkType conversion or strict-mode policy. The CLI default still sends
`store:false`; a later [explicit opt-in WIP](ai-01e-responses-chaining.md)
can use server-side `previous_response_id`. The subsequent
[wire postprocessing WIP](ai-01e-responses-wire-postprocess.md) adds three
schema transformations. These slices do not change the accepted point count.

## Executable evidence

- Unit tests in `openai_responses.rs` cover schema-only traversal, empty and
  pattern-only nodes, escaped regex text, collapsed patterns, conflicting
  `enum`/`const`, integer-valued JSON numbers, and survivor-only tool choice.
- `openai_responses_http.rs` checks the actual captured HTTP request against a
  controlled fake upstream: a conflicting tool is absent, the safe tool's
  schema is normalized, and no forced choice refers to the omitted tool.
- Independent Codex source, plan and final diff reviewers checked the fixed
  OMP boundary, this implementation and its tests. The diff reviewer found an
  empty-node normalization error and an unescaped diagnostic reason; both were
  corrected. The final review found no remaining blocking defect. The exact
  diagnostic text has no automated assertion.

Verification results for the delivered WIP commit are recorded in the
[2026-09-27 handoff](../handoffs/2026-09-27.md). The full delivery gate and a
bounded real-model run remain open.
