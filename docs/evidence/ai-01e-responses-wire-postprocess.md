# AI-01e Responses tool wire postprocessing (WIP)

## Source, scope and observable behavior

The fixed OMP commit `596f2da7101178214aa27a753529d15e6b7ad91d`
applies `packages/ai/src/utils/schema/wire.ts::toolWireSchema` before the
Responses-specific tool sanitizer. Its `postProcessJsonSchema` rewrites a
nullable scalar `anyOf`, infers a homogeneous bare enum's scalar type, and
collapses a homogeneous all-`const` `anyOf` when its branch descriptions can
be preserved. The fixed source tests include the nullable raw JSON Schema
case in `packages/ai/test/schema-wire.test.ts`.

ARA WIP `99a0a4b` runs those three transformations after draft-07 upgrade and
before the existing Responses sanitizer, only for outbound Responses tool
parameters. Integers in a bare enum infer JSON Schema `number`, matching
OMP's JavaScript `typeof` rule without converting the numeric values. The
input `Tool.parameters` remains unchanged for execution-time validation.
Other providers, Agent lifecycle, Session, CLI flags and default storage
behavior were not modified by the production diff.

The following differences are intentional at this stage:

- OMP recursively visits every object key, including instance values in
  `default`, `examples`, `enum` and `const`. ARA visits only schema-valued
  positions so these literal values stay unchanged.
- OMP rewrites a nullable scalar `anyOf` even when its parent already has a
  `type`; ARA preserves the existing `type` to avoid changing the declared
  constraint and diverging from execution-time validation.
- OMP also interprets Ark-style comments embedded in property names for raw
  JSON Schema. ARA does not rename those keys on the wire: the Agent validates
  tool arguments against the original schema, and a wire-only rename would
  make model-supplied arguments fail. That behavior needs a separate shared
  validation/schema ownership decision and collision policy.

This is a partial `toolWireSchema` port. ArkType conversion, strict mode,
vendor dialects and the remaining AI-01e behavior stay open.

## Executed evidence on committed WIP `99a0a4b` (2026-09-27)

- `cargo test -p ara-ai -p ara-agent --quiet` exited 0: Agent test groups
  including 35 `agent_loop` cases passed; AI passed 113 unit, 17 Anthropic
  HTTP, 48 Chat HTTP and 28 Responses HTTP cases. Two new provider unit tests
  cover positive and guarded shapes, nested schema positions, literal data,
  numeric inference, source immutability and property/required order.
- A new Responses fake HTTP case observed the final request body: nullable
  `skip`, collapsed `mode`, typed bare `flag`, unchanged literal default,
  the one surviving tool and its forced choice. The incompatible tool was
  omitted without changing the source `Tool.parameters`. The completed model
  text was `Schema received.`
- A real Agent → Responses HTTP fake upstream → tool test observed one invalid
  call (`skip:"bad"`) and one valid call (`skip:null`). The first generated
  an error tool result with no effect; the second executed exactly once with
  `{skip:null, mode:"b", flag:true}`. The next request contained both paired
  tool outputs and the final assistant text was `checked`.
- `cargo test -p ara-cli --test e2e responses_ --quiet` passed 10/10 with Git
  Bash in `PATH`. Strict workspace Clippy, workspace doc tests, `cargo deny
  check`, fixed OMP inventory and scoped Rust formatting exited 0.
- On committed `99a0a4b`, `cargo test --workspace --all-targets --quiet`
  exited 101 at the existing Windows Bash deadline assertion after 34/35 CLI
  E2E cases passed. Log:
  `%TEMP%\ara-responses-wire-postprocess-99a0a4b-gate.log`. Workspace
  `cargo fmt --all -- --check` still reports only unchanged vendored `pi-*`
  sources; log: `%TEMP%\ara-responses-wire-postprocess-fmt.log`.
- Independent Codex plan reviewer `/root/responses_wire_postprocess_plan`
  checked the fixed source and recommended the wire-only boundary and
  safeguards. Independent diff reviewer
  `/root/responses_wire_postprocess_diff_review` covered all three changed
  files, independently reran the new Agent case, and found no confirmed
  defect. The reviewer noted that the full-schema shape is asserted in the
  provider HTTP case while the Agent case focuses on validation/effects.

No real model was called on this commit. The full backend gate, remaining
wire behavior and AI-01e point audit remain open; no accepted count changes.
