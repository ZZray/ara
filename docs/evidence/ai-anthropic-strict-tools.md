# AI-ANTHROPICa: official-route strict tool planning (WIP)

Date: 2026-09-28. Local code commits `44c24a4` and follow-up `394061b`
on `dev` (not pushed).
Fixed OMP source is `596f2da7101178214aa27a753529d15e6b7ad91d`:
`packages/ai/src/providers/anthropic.ts` lines 4599–4602 and 4780–5070
select strict candidates, normalize their schemas and apply shared budgets;
lines 2925–2953 retry a classified strict rejection before first output.
`packages/ai/src/error/flags.ts` lines 224–263 classify the rejection.
This is an unfinished part of AI-ANTHROPICa, not a delivered point.

## Observable behavior and boundary

On the canonical HTTPS `api.anthropic.com` API-key route, ARA considers only
`bash`, `python`, `edit` and `find` for `strict: true`. A candidate must have a
closed, representable input schema and fit OMP's 20-tool, 24-optional-property
and 16-union budgets. An open map or incompatible raw schema stays non-strict
and remains available. The request keeps its original named `tool_choice`.
On a classified HTTP 400 before Start/SSE, the provider starts one bounded
fallback sequence with the complete non-strict tool schemas. Other 400s,
5xx, cancellation and later stream errors do not trigger that fallback.

Production changes are confined to `crates/ara-ai/src/providers/anthropic.rs`.
The canonical-route guard accepts only HTTPS `api.anthropic.com` at the
default/443 port, path `/` or `/v1` (with an optional trailing slash), and
no query or fragment. Custom hosts and paths retain their previous non-strict
body by default. The provider-local `StreamOptions.strict_tools` option is
`None` by default; an explicit `Some(true)` lets a controlled custom endpoint
exercise the same public `stream` fallback, and `Some(false)` suppresses strict
planning on an official route. No Agent, Session, CLI, shared Tool/Model API
or other provider production code changed.

This is not full fixed-OMP strict parity. ARA's shared `Tool` has no
per-tool `strict: false`; its `Model` has no compatibility metadata for
custom endpoints. OMP remembers a strict rejection per endpoint/model in
session state, while ARA may send one rejected strict request again on a
later turn. A live official model task, CLI/Session journal proof for strict
requests and broader Anthropic parity remain open.

## Executed evidence on code snapshot `44c24a4`

| Check | Observed result |
| --- | --- |
| New canonical-route test before provider edit | Exit 1: `tools[0].strict` was absent instead of `true`. It passed after the edit. |
| `cargo test -p ara-ai --all-targets --all-features --quiet` | Exit 0 on final code: 120 unit, 29 Anthropic HTTP, 48 Chat HTTP, 29 Responses HTTP. Tests cover eligible `edit`, open-map `bash`, raw incompatible `find`, custom route/path exclusion, tool-choice preservation, 24 optional and 16 union budgets, no source-schema mutation, classified 400 fallback and ordinary 400 non-replay. Controlled HTTP sees two requests on classified rejection: first strict, second complete non-strict schema; ordinary 400 sends one request. |
| `cargo test -p ara-cli --test e2e anthropic_ --quiet` | Exit 0: 9/9 actual CLI processes against a controlled custom endpoint. This checks that the existing custom-route tool/journal path remains working; it cannot prove official-route strict execution. |
| `python scripts/verify_backend.py` | Exit 101 on final code. ARA-owned format and strict workspace Clippy passed. Workspace all-target tests reached CLI e2e, where 38/39 passed; existing Windows Bash `deadline_during_a_tool_and_zero_budget_exit_nonzero` exceeded its four-second assertion at `crates/ara-cli/tests/e2e.rs:1340`. Log: `%TEMP%\ara-anthropic-strict-gate-final.log`. |
| `cargo fmt --all -- --check` | Exit 1 from 335 unchanged vendored `crates/vendor/pi-*` formatting diffs. Log: `%TEMP%\ara-anthropic-strict-fmt.log`. |
| `cargo test --workspace --doc --all-features --quiet` | Exit 0; no doc tests are defined. |
| `cargo deny check --hide-inclusion-graph` | Exit 0: advisories, bans, licenses and sources OK; existing duplicate-version warnings. |
| `python scripts/omp_inventory.py check`; `git diff --check` | Exit 0 each. |
| `python scripts/verify_bootstrap.py` | Exit 0: required files, fixed OMP marker, key placeholders, local links and Skill frontmatter. This is documentation integrity, not Agent behavior. |

Independent Codex plan reviewer `/root/anthropic_strict_plan` identified the
OMP allowlist/budgets and the missing ARA compatibility metadata. Independent
Codex diff reviewer `/root/anthropic_strict_diff_review` covered both changed
files twice and found one reachable classifier gap (`structured_outputs not
enabled`/`does not support`) in the first candidate. The classifier and
controlled HTTP test were repaired; its final read-only review found no
remaining high-confidence defect. The reviewer did not run Cargo or a model
call. The author's audit found no change to tool execution or credential
routing, and the fallback occurs before stream Start. Full delivery, real
model evidence and point acceptance remain open. Counts stay AI 6/8,
registered points 27/37 and P0–P6 gates 1/7.

## Public stream and Agent follow-up on code commit `394061b`

The fake upstream rejected the first strict `edit` request with HTTP 400 and
`The compiled grammar is too large`, accepted the second request without
`strict`, then returned a tool call. The public `anthropic::stream` test saw
one Start and one Done, no error or tool event from the rejected attempt, and
usage of 3 input, 2 output, 5 total tokens on the accepted response. Its
negative case sent only one request and ended with an error for an unrelated
400. An explicit `Some(false)` builder assertion also checked the official
route's opt-out branch.

The Agent fake HTTP chain then sent three requests: rejected strict request,
successful non-strict tool turn, and follow-up after one in-memory test `edit`
execution. The first two request histories matched. The Agent recorded one
tool execution start/end and one successful `toolu_edit` result; the third
request contained matching `tool_use` and `tool_result` IDs, and the Run ended
with assistant text `done`. `max_model_calls=2` bounded successful model turns.
This test uses an explicit custom-endpoint opt-in over plain HTTP. It does not
prove official HTTPS routing, a CLI process, Session journal persistence, or
real-model behavior.

| Check on `394061b` | Observed result |
| --- | --- |
| `cargo test -p ara-ai --all-targets --all-features --quiet` | Exit 0: 120 unit, 31 Anthropic HTTP, 48 Chat HTTP, 29 Responses HTTP tests. |
| `cargo test -p ara-agent --all-targets --all-features --quiet` | Exit 0: 1, 37, 12, 8, 11 and 4 tests in its six groups. |
| `cargo test -p ara-cli --test e2e anthropic_ --quiet` | Exit 0: 9/9 existing custom-route CLI cases. They do not exercise strict mode. |
| `cargo fmt -p ara-ai -- --check`; `cargo fmt -p ara-agent -- --check` | Exit 0 each. |
| `cargo fmt --all -- --check` | Exit 1: 335 unchanged vendored `crates/vendor/pi-*` diff blocks, zero non-vendor blocks. Log: `%TEMP%\ara-anthropic-strict-public-394061b-fmt.log`. |
| `cargo test --workspace --doc --all-features --quiet` | Exit 0; zero documentation tests defined. |
| `cargo deny check --hide-inclusion-graph` | Exit 0: advisories, bans, licenses, sources OK; existing duplicate-version and tree-sitter-graphql license-field warnings. |
| `python scripts/omp_inventory.py check`; `python scripts/verify_bootstrap.py`; `git diff --check` | Exit 0 each. |
| `python scripts/verify_backend.py` | Exit 101: owned format and strict Clippy passed; workspace tests reached the existing Windows Bash deadline assertion at `crates/ara-cli/tests/e2e.rs:1340` (38/39 CLI e2e). Log: `%TEMP%\ara-anthropic-strict-public-394061b-gate.log`. |

Independent Codex plan reviewer `/root/anthropic_strict_public_plan` recommended
the provider-local tri-state switch and scoped fake upstream route. Independent
Codex diff reviewer `/root/anthropic_strict_public_diff_review` read all three
changed files, including the opt-out assertion, and found no high-confidence
defect; it ran `git diff --check` but did not run Cargo or a model task. The
author reviewed the request and Agent event path and found no change to
production tool execution, credential selection, Session or CLI. The full
gate, official real-model task and point acceptance remain open. Counts stay
AI 6/8, registered points 27/37 and P0–P6 gates 1/7.
