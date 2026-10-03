# Provider response and local usage-header ingestion — tested WIP

## Requirement and scope

Fixed OMP is `596f2da7101178214aa27a753529d15e6b7ad91d`; ARA base is
`e594e0f3be6430b4ee0dfd8d877fd95b4e934d9a` on `dev`. This continues the existing
MODEL-COMPOSITION-01 / AI-AUTH work. It does not create another accepted point,
accept a complete surface, or advance the full-parity marker. Candidate raw
hashes and 57 preserved vendor/EOL hashes are recorded in
`C:/Temp/ara-header-batch/candidate-manifest.json`.

| Fixed source | Rust owner and bounded behavior |
| --- | --- |
| `packages/ai/src/auth-storage.ts:3631-3711`, cache adapter `1211-1241` | `ara-cli::auth_storage`: optional provider parser, current active OAuth identity, synchronous local cache merge, successful 60s throttle and exhausted bypass. |
| `packages/ai/src/utils/provider-response.ts`, `types.ts` | `ara-ai::provider_response`, `CallOptions`: awaited transient callback, lowercase headers and optional/null request ID, separate from model events and journal serialization. |
| `packages/coding-agent/src/session/{agent-session.ts:1383-1398,session-stats.ts:387-395}` | `session_usage_headers`, `model_route`, `main` and RPC promotion: shared storage independent of credential-source selection; internal ingestion before the selected configured/per-call callback. Logical Session rebinds; side transport identity retains logical attribution. |
| `packages/coding-agent/src/config/model-registry.ts:2374-2375` | `ModelRegistry::get_provider_base_url`: first truthy provider URL in the current Registry generation, looked up when notification occurs. |
| `packages/ai/src/providers/openai-completions.ts:794-811`, `openai-responses.ts:565-585`, `utils/idle-iterator.ts:167-175` | Ordinary OpenAI final-success POST notification before body iteration, local callback-error replay fences and fresh body first-item budgets. |

Shared Core contains a callback port and response facts; the Host owns storage,
Registry and Session attribution. A request credential lease is not captured
as header ownership. A missing callback model or empty provider is a no-op.
The custom ModelProvider wrapper uses the same Host consumer. Response facts
are not added to assistant messages, request receipts or journals.

## Protocol and failure boundary

| Path | Exercised behavior |
| --- | --- |
| Ordinary Chat Completions / Responses | One awaited notification for the final successful POST; failed HTTP attempts do not notify. Host ingestion precedes the selected external callback; explicit per-call callback retains precedence over the protocol default. |
| Callback failure | A local Config failure after successful dispatch replaces prior failed-attempt evidence and blocks credential refresh and model/empty/thinking replay. Callback text cannot authorize another POST. |
| Ordinary Responses slow callback | Headers watchdog is disarmed before callback; callback may settle after the POST deadline, then body first-item budget opens. |
| Ordinary Completions slow successful callback | Callback work settles; caller cancellation takes priority, otherwise elapsed POST deadline returns fenced Timeout. Body first-item budget opens only if the request remains usable. |
| Ordinary Codex SSE | Quota-bearing successful loopback response still produces no generic callback or header cache write, matching the fixed source's absent producer. Retaining an option is not claiming notification support. |
| Controlled custom ModelProvider | Actual shared SQLite storage and logical Session: exhausted A header observation affects later A-to-B selection. This is controlled Host callback evidence, not actual OAuth/account authorization. |

Local ingest performs no full usage fetch, reconciliation, block creation,
healing, usage-history write or epoch invalidation. Prior limit IDs not present
in headers, raw and extra fields survive. Defined null metadata stays defined;
prior source absence removes header source. Existing logical expiry is not
extended; cold snapshots remain stale, and failed-probe cooldown remains.
Strict cache read/write failures propagate without consuming the successful
ingestion timestamp or overwriting the prior cache on a read failure.

## Executed modules and retained failures

Repeatable entry: `python -X utf8 scripts/verify_usage_headers.py`, with module
choices storage/requests/registry/chat/responses/codex. `--full` adds the existing backend
gate. The inputs reuse fixed Codex sticky/header/merge families, ordinary
provider-response inputs and existing HTTP/SSE fixtures.

- `modules-initial.log`: initial compile FAIL (missing async await and RPC
  Registry argument). `modules-corrected.log`: FAIL because an integration
  fixture accessed a private account store handle. The fixture now seeds and
  observes the same real SQLite file through its public independent connection;
  the production account interface was not widened.
- `module-20261003T011941Z`: storage **143 PASS / 2 FAIL**. Provider registration
  and a synthetic block write invalidated caches seeded for the assertions.
  The fixtures now register before seeding and separate the no-healing block
  case; the original expiry, metadata, rotation and block assertions remain.
- `module-20261003T013526Z`: storage **145/0/0**, requests **15/1/0**. The ordinary
  Codex fixture lacked the required account header and failed before HTTP;
  the complete invocation remains FAIL.
- `module-20261003T013854Z`: repaired requests **16/0/0**, compile 5.350s and
  execution 2.455s; source unchanged. The legal synthetic account lease reuses
  the existing Codex side-request fixture. No production expectation was relaxed.
- `gate-20261003T013928Z`: owned fmt and Clippy PASS; all-target compilation
  fails before tests with Windows LINK LNK1318/PDB LIMIT in
  `model_registry_discovery`, after 201.421s. The complete gate remains FAIL.
  E: had only 0.30GiB available. Only generated debug/deps PDB files are moved
  to a recoverable temporary backup with checked absolute source/destination
  paths; source, executables, credentials and preserved vendor WIP stay intact.

Independent POST found a real provider-attribution defect before acceptance:
the native warmed full-snapshot provider lookup may return all providers, so
`get_provider_base_url` must retain fixed source's explicit provider filter.
The accessor is repaired and the existing production Registry family now
checks warm p/q/absent-provider URL attribution. These changed dependencies
require a new frozen candidate and executed Registry/related route results;
earlier green modules do not prove the repaired accessor.

Those successful receipts identify the pre-Registry-correction sources; they
are not relabeled as delivered checks of the repaired accessor. The final
workspace gate owns the remaining protocol families and all related
Host/process regressions on the new frozen candidate.
Native Bun oracle and a new paid model/account trial are not run in this batch.
The previous real CAS task is not relabeled as acceptance of these new headers.

## Required remaining work

- Native usage override, synchronous store delegate and aggregate-store hooks
  require real interfaces; the concrete local SQLite branch does not cover them.
- Built-in usage-only refresh, reserve/health/reset/broker and full callers,
  actual OpenAI authorization, other Provider parsers/transports and platform
  acceptance remain open.
- Completions settlement/result parity is exercised, but cancellation of the
  connected HTTP body at the exact native watchdog instant is not implemented
  or observed. A post-callback deadline check does not prove that timing.
  If that callback rejects after the deadline, the current local Config error
  also precedes the elapsed check; fixed `error/finalize.ts:52-53` prefers its
  already-latched local timeout reason. This watchdog lifecycle/precedence
  remains required work, without weakening the exercised no-replay fence.
- Existing `expire_usage_cache` deletes stored report values; fixed
  `auth-storage.ts:6228-6240` retains each prior value with stale logical expiry.
  This existing invalidation difference is recorded for the next usage/cache
  work; fixture preparation must not make it disappear from the parity backlog.
- Full AuthStorage, Registry, OMP surfaces and P1–P6 remain unaccepted. Existing
  dependency advisory RUSTSEC-2026-0192 remains a mandatory unresolved gate.

## Final frozen gate and review

Final frozen gate: `C:/Temp/ara-header-batch/gate-20261003T014859Z/receipt.json`.
The 19 candidate source hashes are unchanged before/after and match the final
worktree; all 57 unrelated vendor/EOL hashes remain exact.

| Step | Result | Seconds |
| --- | --- | ---: |
| Backend | PASS: owned fmt, Clippy, **1,727/0/20** all-target/doc tests | 468.128 |
| Fixed inventory | PASS | 0.452 |
| Dependency audit | FAIL: existing `fontdue -> ttf-parser` RUSTSEC-2026-0192 | 2.525 |
| Actual CLI build | PASS | 0.355 |

The complete gate remains FAIL; licenses/bans/sources pass and no advisory is
waived. Total command time is 471.461s. The source-pinned final CLI SHA256 is
`eebc8377989c7cec94ef543ec607cbc623bab7aa4421c44e1a53a37d1457c466`.
All repaired storage/route/Registry families and existing actual CLI/RPC,
process, Session, failure/cancellation and protocol families execute on this
candidate. This is Windows evidence, not Linux or actual account acceptance.
The repaired Registry module independently passes **4/0/0** in 8.833s
(`module-20261003T014738Z`), with 30.196s compilation and unchanged source.

Independent Codex `/root/remote_review` covered the 18 original paths and the
19-path repaired candidate, including source, module/gate receipts and
documentation. Its initial changes-requested finding and the exact-provider
repair are retained in `C:/Temp/ara-header-batch/final-post-review.md`.
Final independent POST **approves only bounded tested WIP**. Root independently
checked the source, binary and preserved vendor hashes and inspected the gate
outputs. No complete surface or point acceptance follows from these results.
