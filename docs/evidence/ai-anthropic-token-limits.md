# AI-ANTHROPICa: explicit output-token limit (WIP)

Date: 2026-09-27. Local code commit: `45aa7a6` on `dev` (not pushed).
Fixed OMP source: `596f2da7101178214aa27a753529d15e6b7ad91d`,
`packages/ai/src/providers/anthropic.ts::buildParams` around lines 4058–4074,
with `CLAUDE_CODE_MAX_OUTPUT_TOKENS = 64000` in
`packages/ai/src/providers/claude-code-fingerprint.ts`. This is part of the
open AI-ANTHROPICa provider surface, not point acceptance.

## Requirement and scope

An explicit CLI `--max-tokens 8192` must reach an Anthropic Messages request
as `max_tokens: 8192` when the model has no catalogue limit. Before this
change, `DEFAULT_MAX_TOKENS = 4096` was used as both request default and
unknown-model ceiling, so that explicit request silently became 4096. Fixed
OMP caps an explicit request at the model limit or 64000 when the model limit
is unknown. ARA retains its existing conservative **default request** of
4096 without a catalogue limit; this is an intentional difference from OMP's
64000 default. Explicit requests now use the OMP fallback ceiling of 64000.
A known model ceiling remains authoritative, including when above 64000.
Zero remains a configuration error before HTTP.

Production diff is limited to `crates/ara-ai/src/providers/anthropic.rs`.
Regression tests are in `crates/ara-ai/tests/anthropic_http.rs` and
`crates/ara-cli/tests/e2e.rs`. No Agent, Session, CLI production code, other
provider, model catalogue, or Windows Bash lifecycle was changed. The host
path exercised is CLI `ara --api anthropic-messages --max-tokens 8192` →
Agent → Anthropic provider → Messages HTTP body → assistant output and
Session journal.

## Executed checks on the code snapshot committed as `45aa7a6`

| Check | Actual result |
| --- | --- |
| Focused token-limit test on the old provider | Exit 1: explicit 8192 was observed as 4096. The same test passed after the provider change. |
| `cargo test -p ara-ai --all-targets --all-features --quiet` | Exit 0: 116 unit, 27 Anthropic HTTP, 48 Chat HTTP, 29 Responses HTTP. Fake Messages HTTP checks the exact outgoing body for absent limit/default 4096, explicit 8192, explicit 128000 capped to 64000, and known ceiling 128000 with explicit 100000. Zero produces a terminal error and sends no request. |
| `cargo test -p ara-cli --test e2e anthropic_ --quiet` | Exit 0: 9/9 real `ara` process cases. New case observes one upstream request with `max_tokens: 8192`, stdout `Limit received.`, exit 0, and a final journaled assistant stop. |
| `python scripts/verify_backend.py` | Exit 101 on two complete attempts. ARA-owned formatting and strict workspace Clippy passed; workspace all-target tests reached CLI e2e and passed 38/39. The existing Windows `deadline_during_a_tool_and_zero_budget_exit_nonzero` assertion at `crates/ara-cli/tests/e2e.rs:1340` required elapsed time under four seconds and failed. Logs: `%TEMP%\ara-anthropic-token-limit-gate.log` and `%TEMP%\ara-anthropic-token-limit-gate-rerun.log`. |
| `cargo test -p ara-cli --test e2e deadline_during_a_tool_and_zero_budget_exit_nonzero -- --exact --nocapture` | Exit 0 in isolation: 1/1 passed in 1.77 seconds. This does not make the full gate green. |
| `cargo fmt --all -- --check` | Exit 1: 335 unchanged vendored `crates/vendor/pi-*` diffs; ARA-owned formatting passed in the verifier. Log: `%TEMP%\ara-anthropic-token-limit-fmt.log`. |
| `cargo test --workspace --doc --all-features --quiet` | Exit 0; no documentation tests are defined. |
| `cargo deny check --hide-inclusion-graph` | Exit 0: advisories, bans, licenses and sources OK; existing no-license-field and duplicate-version warnings. |
| `python scripts/omp_inventory.py check`; `git diff --check` | Exit 0 each. |

Independent Codex plan reviewer `/root/anthropic_token_limit_plan` compared
the fixed OMP formula with ARA's model/CLI path and checked scope and test
strategy before implementation. Independent Codex diff reviewer
`/root/anthropic_token_limit_diff_review` reviewed all three changed files,
the fixed OMP source and the CLI/Agent route after implementation; it found
no reachable defect and ran `git diff --check`, but did not run Cargo tests.
The author's review found no changed ownership, authorization, cancellation,
credential handling, or tool effect. No bounded real-model task ran. The
full delivery gate, model catalogue and broader Anthropic parity remain
open. Accepted counts remain AI 6/8, registered points 27/37, P0–P6 1/7.
