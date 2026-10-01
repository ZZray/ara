# OpenAI daily configuration and account Host slice

## Requirement and snapshot

User priority, 2026-10-01: custom OpenAI-compatible configuration first, OpenAI
account login next; retain other Provider differences as later work. Verify by
module and one shared final gate. This batch implements the existing native
configuration → private request auth → provider → tool → Session/restart path.

Parent: `9688c584dc7d2f39f150b9d3dc9fc1f768ae7606`; delivered code is identified by
the module runner's complete before/after SHA256 pins. No upstream marker is
advanced. Fixed OMP source: `596f2da7101178214aa27a753529d15e6b7ad91d`.

| Fixed source | Rust / observable scope |
| --- | --- |
| `packages/coding-agent/src/session/auth-storage.ts`, native models configuration | `daily_model_config`, existing `ModelsConfigFile`/native credential store; custom selection and explicit overrides. The daily auth subset differs from full AuthStorage ordering. |
| `packages/ai/src/registry/oauth/openai-codex.ts` | `openai_codex_auth`: official device flow, private SQLite, refresh, exact-data CAS, settlement and logout. |
| `packages/ai/src/providers/openai-codex-responses.ts`, `openai-codex/{request-transformer,response-handler}.ts` | Dedicated `openai_codex_responses` SSE API; private account/residency lease, native encrypted reasoning history, Host Session attribution and fixed headers. |
| Existing Chat/Responses adapters and original source fixtures | Existing encoder/parser/retry reuse; strict public Responses guards retained. |

Exact source exports/hashes:
`C:\Temp\ara-openai-daily-batch\source\source-receipt.json`.
The fixed inventory contains 27,404 original test cases; that is an inventory
count, not a claim that all OMP tests or all parity surfaces were executed here.

## Execution

```powershell
python -X utf8 scripts/verify_openai_daily.py --module all --output C:/Temp/ara-openai-daily-batch --full
```

Four module suites: config 5, auth 7, SSE 6, CLI 4. The CLI suite launches real
child processes and sockets with synthetic OAuth/upstream data; it checks
stream output while active, tool file effects, same-Session restart, summary
adoption/rejection, logout, unknown-refresh settlement/restart, new Session wire
identity, Windows home fallback, legacy API coexistence and early route rejection.
These fixtures prove the Host software boundary, not a real account login.

Final receipt:
`C:\Temp\ara-openai-daily-batch\module-20261001T060131Z\receipt.json`.
Exit 0; all 138 before/after/current source pins match. Modules **22/0/0**;
backend **1,491/0/20**, covering fmt, all-feature Clippy, target and doc tests;
inventory, cargo-deny and build PASS. Total **134.536 seconds**: compile 2.678,
config 0.067, auth 0.622, SSE 0.632, CLI 1.178, backend 125.629,
inventory 0.460, deny 2.608 and build 0.443. The existing ignored cases are
reported as ignored, not passes. Development uses module selection; final
backend checks are shared across the stable batch.

Superseded receipts `045643`, `050133`, `051805`, `054946`, `055248` remain in
the same local directory. Earlier failures were one selected-policy fixture,
pure Clippy writes, and the new legacy Anthropic fixture's missing synthetic key
and named SSE framing. The latter reuses the existing e2e framing in the final
code; no production provider relaxation or retry expansion was used. Final
expected task outcomes remain unchanged.

### Real configured CAS task

Receipt/raw events/artifacts:
`C:\Temp\ara-openai-daily-batch\live\20261001T063130Z`.
Read the user-specified local Manager configuration privately, without editing
it or printing/committing credentials. Live catalogue confirms
`deepseek-v4.1-flash`; actual route is Manager → local OMP management → CAS,
protocol `openai-completions`. Configuration is a temporary native models.yml
with a private environment key; no CLI API/base URL/key overrides are supplied.

Bounds: two Runs, at most 8 calls/180 seconds each, 8,192 output tokens per call.
Actual task: read `sales.csv`, compute totals, write and reread `summary.json`.
Expected/observed: `{"pen":7.5,"book":24,"cup":21,"grand_total":52.5}`.
Neither task nor recall prompt contains the expected numeric answers. Three
successful read/write/read tool results and four model calls are in raw events;
task exit 0, 5.699 seconds. A second process resumes the original Session
`01a0f629-338e-75c1-bd50-43cc703054fe`, has an empty tool list and one model call,
and returns `pen_quantity=3; cup_unit_price=5.25; grand_total=52.50`, matching
the original numeric data; exit 0, 1.333 seconds. Both process headers carry
that same Session ID. Total **7.032 seconds**. Observed total tokens: 13,531
then 2,972; output 391 then 44. No measured billing cost is asserted. Root
inspects raw user prompts, final events, tool results and actual artifact.

Earlier live receipts `060424Z` and `063051Z` used prompts containing expected
answers. They establish tool effects/restart transport but cannot prove recall;
the reviewer explicitly identified that boundary. Root's final answer-free,
tool-disabled recall above supersedes those semantic acceptance claims on the
same built binary. No code or deterministic gate changed for this correction.

Binary SHA256 matches the final built binary and trial receipt:
`d003cfbff2d27f52a74c9e0cfd7278e8a59df411d63bcd38a8fec313592b9566`.

### Independent review and bounded audit

Three independent Codex cross-reviews reuse the prior scope and original source:

- `/root/ctx_oracle_diff_review`: B auth/C SSE, final
  `daily-auth-sse-independent-review.json`; original unknown refresh grant
  replay and native reasoning-only history findings closed with code and final
  process/wire evidence. Final auth lint changes reviewed as equivalent.
- `/root/model_policy_port`: C/Root integration, final
  `reviews/daily-codex-root-independent-review.json`; normal-process settlement,
  reasoning-only history and `/new` Session attribution findings closed.
- `/root/rpc_owned_core`: A/Root/runner, final
  `daily-config-root-independent-review.json`; final source pins, all gates and
  raw CAS tool/artifact/restart evidence verified. Legacy API coexistence and
  early Codex RPC rejection included. Seven scoped files fully covered.

All receipt paths are under `C:\Temp\ara-openai-daily-batch`; independent raw
CAS review covers the earlier tool/process receipt and states its known-answer
limitation. Root separately accepts the final answer-free task/recall artifact.
Reviews are
bounded approvals, not acceptance of full Provider/Core parity. Root checks
critical fixes, final source pins, raw tools/artifact, Session identity and
failure boundaries; the configured custom CLI slice passes its bounded audit.
Device/Codex account code is tested and reviewed WIP: **no real OpenAI account
authorization or subscription-model task has run**, so its live acceptance
stays open. No existing local Codex/OMP account tokens were imported.

## Scope and acceptance limits

Preserved: fixed source SHA, existing native store/config ownership, legacy APIs,
original Core/Session/tool contracts, private auth data, other Provider WIP.

This batch: CLI custom route adoption; device login/logout; Codex SSE with
per-request auth/refresh; normal-exit settlement; original Session continuation,
new Session identity and locally checked summary adoption; module runner/docs.

Deferred: full provider registry/cache execution projection, complete OMP
AuthStorage precedence/credential commands/reserve/rotation, browser login,
multi-account ranking, Codex RPC/WS/Lite/native compaction, image/native item
contracts beyond the supported subset and remaining providers. These remain
required parity work, with explicit rejection at unsupported execution points.

Hard kill/power loss and completely unwritable storage cannot guarantee durable
settlement of an unknown refresh. Codex summary budgets govern local adoption,
not a server output-token or billing limit. No new Linux execution evidence is
claimed by the Windows gate. P0/V1 remain accepted; P1–P6, complete Provider
surfaces and R3/R4 remain open; RPC remains 27/42 and the parity marker null.
