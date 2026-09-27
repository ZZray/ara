# AI-ANTHROPICa: API-key request header routing (WIP)

Date: 2026-09-27. Local code commit: `ea749db` on `dev` (not pushed).
Fixed OMP source: `596f2da7101178214aa27a753529d15e6b7ad91d`,
`packages/ai/src/providers/anthropic.ts::buildAnthropicHeaders` (258–377)
and `buildAnthropicClientOptions` (3343–3375), plus
`packages/catalog/src/compat/anthropic.ts::isOfficialAnthropicApiUrl`.
This is a partial AI-ANTHROPIC port, not a point acceptance.

## Behavior and scope

`ara-ai` now sends the configured API key as `X-Api-Key` to the official
HTTPS `api.anthropic.com` endpoint, and as `Authorization: Bearer` to an
ordinary custom Anthropic Messages endpoint. The fixed OMP exceptions
`opencode-go`, `opencode-zen` and `umans` use `X-Api-Key` on their proxies.
Caller-supplied `Authorization` or `X-Api-Key` values are selected without
regard to header-name case; the last occurrence wins and no automatic
duplicate credential header is added. An explicit `X-Api-Key` may coexist
with Bearer on a generic proxy, and an explicit `Authorization` may coexist
with `X-Api-Key` on the official endpoint, as in fixed OMP.

The existing CLI selects `ANTHROPIC_API_KEY` for any HTTPS URL with the
official hostname, including a nonstandard port. To avoid sending that key
to a custom port without changing the stable CLI route in this slice, the
provider rejects such a URL before making an HTTP request. Explicit `:443`
is accepted as official. This is an intentional ARA difference from OMP's
string-based origin/path predicate. A lookalike host and HTTP on the same
hostname remain custom endpoints and use Bearer.

The code change is confined to `crates/ara-ai/src/providers/anthropic.rs`;
its controlled HTTP tests and existing `ara` process assertions were
updated. Agent loop, Session, CLI production routing, OAuth, Cloudflare and
other gateway-specific protocols are outside this slice.

## Executed checks on `ea749db`

| Check | Actual result |
| --- | --- |
| New custom-endpoint Bearer HTTP test on old code | Exit 1: `authorization` was absent; old code sent only `x-api-key`. |
| `cargo test -p ara-ai --all-targets --all-features --quiet` | Exit 0: 116 unit, 22 Anthropic HTTP, 48 Chat HTTP and 29 Responses HTTP tests. The new HTTP cases inspect default Bearer and case-insensitive explicit overrides without logging credentials. Unit cases check official URL, `:443`, lookalike/HTTP host, `:8443` rejection, three known proxy exceptions and credential-header cardinality. |
| `cargo test -p ara-cli --test e2e anthropic_ --quiet` | Exit 0, 6/6 real `ara` process cases. The controlled custom endpoint receives redacted Bearer, not `x-api-key`; official key material is absent. Existing tool-effect, restart, live JSON and stalled-stream cases remain green. |
| `python scripts/verify_backend.py` after the Clippy-only test initializer correction | Exit 101. Owned formatting and strict workspace Clippy passed. Workspace all-target tests reached CLI e2e: 35/36 passed, and the existing Windows Bash `deadline_during_a_tool_and_zero_budget_exit_nonzero` elapsed-time assertion failed. Log: `%TEMP%\ara-anthropic-auth-final-gate.log`. The verifier stopped before doc tests. |
| `cargo deny check --hide-inclusion-graph`; `cargo test --workspace --doc --all-features --quiet`; `python scripts/omp_inventory.py check`; `git diff --check` | Exit 0 each. Deny retains existing duplicate/no-license-field warnings; no doc tests are defined. |

The first verifier attempt stopped at a Clippy warning in the newly added
test initializer. That was corrected, then the full verifier was rerun as
recorded above. An independent Codex plan reviewer compared the pinned OMP
header paths and identified the CLI nonstandard-port key-selection risk. A
separate independent Codex diff reviewer covered all three modified files,
the CLI key source, shared HTTP request/retry code and fixed OMP branches;
it reported no remaining reachable defect. The reviewer did not rerun
Cargo tests.

No bounded real-model task ran on this snapshot. Full backend verification,
remaining Anthropic protocol behavior and point audit are open. AI stays
6/8 accepted registered points; the P0–P6 gate count remains 1/7.
