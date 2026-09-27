# AI-01e: draft-07 tool schema on Responses wire (WIP)

## Source and boundary

Pinned OMP commit `596f2da7101178214aa27a753529d15e6b7ad91d`:
`packages/ai/src/utils/schema/draft.ts::upgradeJsonSchemaTo202012` is called
by `utils/schema/wire.ts::toolWireSchema` before
`providers/openai-responses.ts` sanitizes and emits a tool. ARA now runs the
JSON-value upgrade in `ara-ai::schema_draft` before its existing Responses
sanitizer. The same upgrader now feeds tool-argument validation; see the
[validation receipt](ai-01f-draft-validation.md). Agent, Session, CLI and other
provider routes were not changed.

The conversion recognizes draft-07 shapes without a `$schema` declaration.
It upgrades the four recognized draft-07 `$schema` URIs, local
`#/definitions/` refs, `definitions`, tuple `items`/`additionalItems`,
array and schema `dependencies`, and OpenAPI `nullable`. It merges preexisting
`prefixItems`, `dependentRequired` and `dependentSchemas` by the pinned OMP
rules. `enum`, `const`, `default`, examples and other literal values are
preserved. Inputs are acyclic `serde_json::Value` objects; a depth limit
quarantines an excessively nested tool individually.

If both `definitions` and `$defs` contain the same name, ARA deterministically
uses `$defs`. OMP's JavaScript object insertion order decides the winner in
that collision, but ARA's JSON map does not preserve that order. This is an
intentional difference for ambiguous inputs.

The subsequent validation slice uses this upgrade before tool execution and
enforces selected upgraded constraints. ARA still lacks the remaining raw
`toolWireSchema` postprocessing, strict-mode and vendor dialect policy. The
wire and validation slices together do not prove full draft-07 tool behavior
or AI-01e acceptance.

## Executable evidence

- Unit tests in `schema_draft.rs` cover nested legacy keywords
  without a URI, local/external refs, literal-data preservation, tuple and
  dependency collisions, nullable forms, omitted legacy keywords, source
  immutability and the depth cap.
- `openai_responses_http.rs` captures a real HTTP request against a controlled
  fake upstream and checks the upgraded tool schema and forced tool choice.
- Independent Codex source and preimplementation reviews checked the fixed
  OMP chain and the scope. Final diff review and verification results are
  recorded in the [current handoff](../handoffs/2026-09-27.md).
