# AI-ANTHROPICa shared tool wire postprocessing (WIP, 2026-09-27)

## Source, scope and observed behavior

Fixed OMP `596f2da7101178214aa27a753529d15e6b7ad91d` calls
`packages/ai/src/utils/schema/wire.ts::toolWireSchema` in
`packages/ai/src/providers/anthropic.ts::buildAnthropicBaseToolInputSchema`
before forcing the Messages root object shape and running
`normalizeAnthropicToolSchema`. ARA WIP `024d5fc` extracts the three already
tested Responses wire transformations into crate-private `schema_wire.rs` and
applies them to Anthropic after draft-07 upgrade and before Anthropic root
shaping and normalization:

- scalar nullable `anyOf` becomes a type array including `null`;
- a homogeneous bare `enum` receives its scalar type;
- a homogeneous all-`const` union with compatible descriptions becomes a
  typed `enum`.

The path is host/Agent `Tool.parameters` → draft upgrade → shared raw schema
postprocessing → Anthropic `input_schema` normalization → Messages HTTP
request. Agent argument validation still reads the original tool definition.
Responses uses the extracted helper with unchanged behavior. No Agent or
Session production lifecycle, CLI routing, or Bash behavior changed.

This is a partial port. The existing [Responses wire receipt](ai-01e-responses-wire-postprocess.md)
records intentional schema-position-only traversal, preserving an existing
`type`, and the deferred Ark-style property-key rewrite. Other OMP
postprocessing, strict tools, catalogue limits, native stream/history features
and live-model evidence remain open.

## Verification on committed WIP `024d5fc`

Windows PowerShell with Git Bash in `PATH` for CLI workspace tests. The HTTP
fixture uses a dummy credential; the fake upstream redacts headers.

| Check | Result and observed evidence |
| --- | --- |
| `cargo test -p ara-ai -p ara-agent --quiet` | Exit 0. AI: 113 unit, 18 Anthropic HTTP, 48 Chat HTTP and 28 Responses HTTP; Agent: 36 loop cases and all other groups passed. |
| Anthropic fake HTTP | New case observed POST `/v1/messages` with nullable `skip`, typed bare `bare`, collapsed `mode`, unchanged literal default and unchanged original `Tool.parameters`; final text `Schema received.`. |
| Agent → Anthropic fake HTTP → tool | New case returned an error for `skip:"bad"` without a tool effect, then executed `skip:null` once with `{skip:null, mode:"b", flag:true}`. The second request carried both matching `tool_result` IDs; final text `checked`. |
| `cargo test -p ara-cli --test e2e anthropic_ --quiet` and `responses_ --quiet` | Exit 0, 6/6 Anthropic and 10/10 Responses real-process cases. |
| `cargo clippy --workspace --all-targets --all-features -- -D warnings`; `cargo test --workspace --doc --quiet`; `cargo deny check`; `python scripts/omp_inventory.py check`; changed-file `rustfmt --check` | Each exited 0. Deny reports advisories, bans, licenses and sources OK, with existing warnings. |
| `cargo test --workspace --all-targets` | Exit 101 on the committed code: CLI E2E 34/35; the existing Windows Bash `deadline_during_a_tool_and_zero_budget_exit_nonzero` elapsed-time assertion failed. Its focused rerun failed too. Log: `%TEMP%\ara-anthropic-wire-024d5fc-gate.log`. |
| `cargo fmt --all -- --check` | Exit 1 only in unchanged vendored `crates/vendor/pi-*` files. All six changed Rust files pass scoped formatting. Log: `%TEMP%\ara-anthropic-wire-024d5fc-fmt.log`. |

Independent Codex plan reviewer `/root/anthropic_wire_postprocess_plan`
checked the fixed source and insertion point. Independent diff reviewer
`/root/anthropic_wire_postprocess_diff_review` found one changed deep-schema
error text; the code now preserves Anthropic's previous error text and the
full AI/Agent suite passed afterward. The reviewer found no second concrete
defect. Neither review nor the controlled upstream is real-model evidence.

No model quota was used. The full delivery gate and point audit remain open;
accepted counts do not change.
