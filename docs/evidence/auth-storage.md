# Shared AuthStorage and request selection — tested WIP

## Requirement, source and boundary

Fixed OMP remains `596f2da7101178214aa27a753529d15e6b7ad91d`; base is
`681d552306769b55bde337a03938f23e0444f3b3` on `dev`. This extends MODEL-COMPOSITION-01 and starts
the previously stale AI-AUTH inventory row. It does not accept either full
surface, a phase, or the null full-parity marker. The 19 code/test/runner files
are frozen in `C:/Temp/ara-authstorage-batch/candidate-manifest.json`. All 57 unrelated
vendor/EOL hashes are preserved; `.codebase-memory/` remains excluded.

| Pinned OMP source | Rust owner and exercised scope |
| --- | --- |
| `packages/ai/src/auth-storage.ts` | `auth_storage`, `auth_storage_policy`, `auth_storage_state`: bounded account snapshots/selectors, ranking/usage cache and history, persisted/in-memory blocks, Session pin/sticky/RR and exact bearer/row feedback. |
| `packages/ai/src/auth-retry.ts`, `error/auth-classify.ts` | `request_auth_retry`, `model_route`: supported pre-output refresh/rotation, private failed lease, bearer-cycle guard and fresh request attribution. Partial output and unknown dispatch do not grant replay. |
| `usage/openai-codex{,-base-url,-reset}.ts` | `codex_usage`: usage/header parsers, canonical URL/identity and GET credit detail enrichment; consume/redeem POST is unbound. |
| `registry/engine/{common,refresh}.ts`, `error/flags.ts` | `openai_codex_auth`, shared Codex adapter: typed definitive/transient rejection, private first-500-UTF-16 raw-text classification and exact-row refresh/fencing/owned settlement. |
| Coding-agent Registry/config/main callers | `auth_storage_registry`, `config_request_auth`, `daily_model_config`, reference `main`: one Registry/request owner and shared command cache, explicit-auth precedence, ordinary login then request environment/catalog environment then static fallback, original Session rebinding. |

Explicit CLI/config/keyless/credential-header ownership stays ahead of storage.
The per-request default environment key is a private context lease, never a
global Registry override. Session cloning retains it. No product identity or
state enters shared Core. License/copyright notices remain with ported source.

## Executed checks, including failed attempts

Use `python -X utf8 scripts/verify_auth_storage.py --module all` for grouped
development; `--full` adds the shared workspace/inventory/dependency/build gate.
The runner exposes storage/accounts/requests/selection/host/registry/helpers.
Inputs reuse fixed native config-override, account-select, Codex-selection,
force-refresh/rotation and block-persistence families. Native Bun execution
and an exhaustive AuthStorage oracle are not claimed.

- `module-20261002T232631Z` fails linking (PDB limit); `233032Z` fails with
  disk error 112. They are failed invocations, not passes. Only checked,
  reproducible symbol caches are removed; source/profile/credentials stay intact.
- `module-20261002T233628Z`: storage142/accounts9/requests13/selection8 PASS;
  Host6 PASS/1 FAIL. The complete invocation stays failed. Subsequent fixture
  repairs keep catalog GET and model POST counts separate, use a legal native
  provider override and copy a real fresh native SQLite cache for no-config
  requests. No synthetic bearer is intentionally sent to public discovery.
- `ordinary-host-corrected.log` retains the failed scalar-only user-history
  assertion. Fixed Completions supports text blocks, and CLI hooks prepend a
  date/cwd reminder. The repaired assertion checks exact original text with
  only those source-supported representations; assistant and Session ID
  remain exact. `ordinary-host-history-final.log` passes 1/0/0 in 46.91s.
- `gate-20261003T000201Z` stops at Clippy, before tests. Mechanical let-chain,
  slice/array repairs preserve short-circuit/await/effect order; seven native
  signatures have documented function-only parameter-count allowances.
  `clippy-repaired.log` passes in 20.96s, without disabling the gate.

Final frozen gate: `C:/Temp/ara-authstorage-batch/gate-20261003T000835Z/receipt.json`.

| Step | Result | Seconds |
| --- | --- | ---: |
| `backend` | PASS (exit 0) | 426.707 |
| `inventory` | PASS (exit 0) | 0.441 |
| `deny` | FAIL (exit 1) | 2.438 |
| `build` | PASS (exit 0) | 0.409 |

Backend includes owned fmt, all-feature Clippy, all-target and doc tests:
**1,721/0/20**. Total final gate 429.995s. The 20
ignored tests are unexecuted. All 19 before/after source hashes match the
candidate. Dependency audit retains `RUSTSEC-2026-0192` in the existing
`fontdue -> ttf-parser` path; no policy exception is added. **Whole gate FAIL.**
Binary SHA256: `e429aeeece30436f15c969a00785bf051f57ea891f7b964216e128a1577d699e`.

## Actual bounded CAS task and independent audit

`live-20261003T001618Z` remains FAIL: 4.333s, exit2, zero model calls and no
active request. Its temporary config used model metadata overrides without a
catalog row, which the supported daily selector explicitly rejects. Root reads
the selector's allowed fields and removes only those unrelated fixture fields;
headers/login checks and the CLI/wire 2,048-token cap remain unchanged. The next
attempt uses a fresh Session; no unknown request or failed write is replayed.

`C:/Temp/ara-authstorage-batch/live-20261003T001731Z/receipt.json`: **PASS**, 14.869s,
5 model calls, current verified `deepseek-v4.1-flash` over
`openai-completions` using CAS from Manager configuration. Current Manager configuration and
live catalog are queried without modifying or copying private values to Git.
Bounds: two processes, five calls, 90s/process, 2,048 output tokens/call.

The temporary native auth store contains a login command expression; real key
material stays in the environment. A synthetic ordinary default must yield to
that stored login. Every incoming bearer/key/header proof matches. Exactly
read(invoice.csv), write(totals.json), read(totals.json) succeed. Root checks
the actual JSON/hash, three tool receipts and original/current Session entries.
The answer-free, tool-disabled original-Session reopen returns the exact JSON
and leaves the artifact unchanged. ID/raw login payload remain unchanged;
helpers execute once per fresh process in effective cwd. No request remains
active at process end; source/binary hashes match the final gate. Observed SSE
usage is retained in the receipt; monetary cost is unknown. This is not an
actual OpenAI OAuth authorization or subscription-model trial.

Root's separate artifact audit passes: actual JSON SHA256
`6207a4252420e94ced297eb1cbe07a8b3f645c4703f49a38935529c3ae7f8deb`,
three successful tool receipts, all 11 original Session rows retained exactly
as a prefix of 13 current rows, and the same original Session ID after reopen.
Actual SSE reports 16,089 input and 615 output tokens; cost remains unknown.

Independent Codex `/root/remote_review` final POST is **approve, limited to this
tested WIP batch**, with no remaining reachable source blocker. The reviewer
checked pinned source, all 30 delivery paths (19 code/test/runner and 11 metadata),
57 preserved vendor hashes, mechanical repairs and final code/gate/live receipts.
Report: `C:/Temp/ara-authstorage-batch/final-post-review.md`; reviewed delivery
manifest SHA256: `758d6ab2aabb72914ebba14b14e160375505b53e06a1c2d836d30c8d03d69d81`.
The complete dependency gate remains FAIL; no broad acceptance is inferred.

Root compared staged raw bytes with the reviewed worktree: 12 paths differ only
by CRLF/LF normalization, including `crates/ara-cli/Cargo.toml` and all 11 metadata
paths. The other 18 paths are byte-identical. All 30 canonical contents match,
and all 57 vendor hashes remain unchanged. Individual worktree/staged SHA256
values are retained in `C:/Temp/ara-authstorage-batch/staged-identity-preclosure.json`.
Recording this final POST does not change production code or require a repeated
backend gate or paid model trial.

## Mandatory remaining work and progress

- Built-in Codex usage-only advisory refresh is not connected to the primitive;
  the generic `prepare_usage_credential` hook is insufficient proof.
- Native `AuthStorage.ingestUsageHeaders` cache merge and the positive
  `session/session-stats.ts:391` production caller still require binding.
  Fixed ordinary Codex SSE does not forward the generic `onResponse` callback;
  adding automatic Codex HTTP feedback is not inferred as a parity requirement.
- Reserve/health/reset consume/redeem, broker/remote credential stores and full
  native caller/selector contracts remain required.
- Synchronous offline peek does not start a cold command; Session-aware sticky
  peek is a Host extension. Registry discovery retains its bounded one-401 path.
- Actual OpenAI device authorization/account task, browser callback, advanced
  Codex/RPC transports, other Providers and Bun/Linux remain open.

111 surfaces: 27 implementing/one tested/83 open/zero complete acceptances;
started 28/111 (25.2%). Existing bounded points stay 35 limited acceptances,
23 implementing/two tested (60, 58.3% is not total completion). P0/V1 accepted,
P1–P6 open, RPC27/42 and null parity marker remain. Full-project ETA is unknown.
