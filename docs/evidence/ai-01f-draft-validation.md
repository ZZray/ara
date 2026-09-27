# AI-01f: upgraded tool schema validation before execution (WIP)

## Source and boundary

Pinned OMP commit `596f2da7101178214aa27a753529d15e6b7ad91d`:
`packages/ai/src/utils/validation.ts::getValidationContext` upgrades raw
tool schemas before validating arguments. Its
`normalizeOptionalNullsForSchema` traverses `allOf`, and its union validation
keeps branch issues available for numeric coercion. The source fixture at
`packages/ai/test/tool-argument-coercion.test.ts:712-759` exercises draft-07
definitions, nullable values and tuple items without a `$schema` URI.

ARA shares `ara-ai::schema_draft::upgrade_json_schema` between the Responses
request encoder and `ara-ai::validation`. Before a tool executes, validation
now checks local `$ref` (including escaped JSON Pointer tokens), boolean
schemas, `allOf`, `prefixItems` and its `items` tail, `dependentRequired` and
`dependentSchemas`. Reference cycles and schema depth are bounded. Optional
`null`/`"null"` placeholders are removed in direct properties and `allOf`
branches. A failed `anyOf`/`oneOf` branch retains its leaf type issue so the
existing lossless scalar coercion can run.

This is a JSON Schema subset, not full OMP validator parity. Other JSON Schema
keywords, OMP argument normalizers, strict-mode and vendor schema dialects
remain outside this slice. Agent ordering, Session persistence and the CLI
host were not changed. Accepted AI-01f and AI-01e counts do not change from
this WIP extension.

## Executable evidence

- `ara-ai::validation` unit tests use the pinned draft-07 fixture shape and
  cover bad tuple type/tail, dependencies, escaped and recursive refs, false
  schemas, nullable optional values, union numeric coercion and `allOf`
  placeholder removal.
- `ara-agent/tests/agent_loop.rs` runs both a scripted Agent and a real Agent
  with Responses fake HTTP. Invalid extra tuple items produce a tool error
  and no effect; the valid call executes exactly once, and the HTTP capture
  checks emitted schema and paired results on the next request.
- On the final code candidate, `cargo test -p ara-ai -p ara-agent --quiet`
  passed: AI 108 unit, 48 Chat HTTP and 13 Responses HTTP tests; Agent
  1/34/12/8/7/4 test groups passed. Existing real CLI Responses e2e passed
  9/9. Strict workspace Clippy, changed-file rustfmt, workspace doc tests,
  `cargo deny check` (with existing duplicate dependency warnings), and
  `python scripts/omp_inventory.py check` passed.
- Independent Codex preimplementation source and plan reviews scoped this
  change. Independent diff review found two valid-call rejection cases in
  union coercion and `allOf` normalization; both were fixed and the final
  re-review found no remaining high-confidence regression in the changed
  files. Validation tests passed 11/11, and scripted and HTTP Agent effect
  tests passed 1/1 each.

The unfiltered `cargo test --workspace --all-targets --quiet` passed the
changed AI and Agent groups, then failed the existing Windows CLI
`deadline_during_a_tool_and_zero_budget_exit_nonzero` timing assertion
(27/28 CLI e2e). `cargo fmt --all -- --check` remains red in unchanged vendored
`pi-*` files; changed Rust files pass `rustfmt --check`. No bounded real-model
task ran on this code candidate. This WIP slice does not make AI-01e, the
full Agent surface or P0–P6 accepted.
