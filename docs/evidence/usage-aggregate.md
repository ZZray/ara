# Usage aggregate and broker store bridge — tested WIP

## Target and decision

- ARA base: `c9046a8d99a77d529e25d5e22a90e54c8cbdaf47`, branch `dev`.
- Fixed OMP: `596f2da7101178214aa27a753529d15e6b7ad91d`; full parity marker remains null.
- Scope: native usage request enumeration, nullable aggregate ownership, store
  delegation, HTTP broker usage/cache/pool/overlay behavior and the shared REPL
  consumer. This is not complete AuthStorage, RemoteAuthCredentialStore or TUI.
- Result: five module groups204/0/0; Windows backend1750/0/20; independent Codex
  PRE/POST and Root source/receipt audit. Dependency gate remains FAIL for existing
  `fontdue -> ttf-parser`, `RUSTSEC-2026-0192`; no exception was added.
- Backend runner exit124 is retained. The owned all-target Cargo continued after
  the runner deadline and its retained OS handle directly confirmed exit0.
  Missing checks were supplemented on identical final source; runner failure
  is not rewritten as PASS.
- No point, complete surface or P1-P6 gate is promoted. The60 point and111 surface
  counts are unchanged. Delivery here is scoped tested WIP.

## Source -> Rust -> executable evidence

All source locations below refer to the fixed OMP commit above. Root and the
independent reviewer read original bodies through `git show`, rather than using a
moving branch. New modules retain upstream license/copyright notices.

| Fixed source | Rust implementation | Executed family |
| --- | --- | --- |
| `packages/ai/src/auth-storage.ts:1590-1658,3714-3796` | `auth_storage.rs` stored/runtime provider lifecycle; `auth_storage_usage::collect_usage_requests` | collector group: stored precedence, references, nullish keys, supports, provider URLs, insertion/replacement/removal/reload order and builtin restoration |
| `auth-storage.ts:3738-3765` | dedicated Host `Environment::variable` and OAuth request projection | collector group: stored usable/unusable OAuth, dedicated trimmed token and paid-only key exclusion |
| `auth-storage.ts:3798-3942` | ordered identity dedupe/merge in `auth_storage_usage` | identity group: provider/org isolation, richness/newness, missing vs null metadata, duplicate limits and non-transitive bridge grouping |
| `auth-storage.ts:3981-4013,4033-4055` | authoritative store OAuth lookup; public reporting-model helper | existing/request-store groups: None does not fall back; API keys stay local; quantitative model IDs exclude ambiguous labels |
| `auth-storage.ts:4221-4345` | nullable override > store > local, aggregate flights and provider tail owners | override/cancel and local-force groups: None vs empty, independent source waiters, native uncancelled local wait, same-provider serialization, parallel providers, siblings after error and settlement |
| `auth-storage.ts:3631-3712,6271-6302` | header store delegation, force-marker invalidation and best-effort notification | store group: override refusal, successful-ingest-only throttle, no local marker behind source/store, stale notification failure |
| `packages/ai/src/auth-broker/client.ts:275-286,400-507` | `auth_broker_usage` real reqwest GET `/v1/usage` and POST `/v1/usage/stale` | six broker loopback groups: raw account deadline multiplier, transport retry1, HTTP/body/JSON/schema no retry, stale envelope and caller cancellation |
| `auth-broker/wire-schemas.ts:198-264`; `packages/omptype/src/interp.ts:141-179` | broker-private Value/object parser and raw-field projection | existing wire group: duplicate-key last-value behavior, array-envelope rejection, arbitrary resetLabel and native metadata/credit arrays, exact roundtrip, invalid siblings and overlay spread |
| `auth-broker/remote-store.ts:1012-1026,1108-1172,1205-1265,1360-1510` | 15s positive/null cache, epoch/flight fence, Host snapshot/pool input and overlays | six broker groups: shared aggregate/account GET, no last-good after failure, stale waiters following current flight, raw-ID timeout counts, org/member pool matching and overlay TTL/merge |
| `packages/coding-agent/src/slash-commands/helpers/usage-report.ts:16-24` | actual reference REPL `/usage [refresh]` in `main.rs` | existing `device_login_tool_resume_compaction_and_logout_close_the_cli_account_workflow`: same logical Session/account/header owner, 11% then12%, warm GET reuse and explicit refresh |

Native families reused include `auth-storage-xai-oauth-usage.test.ts`,
`auth-storage-org-scoped-identity.test.ts`, `auth-storage-usage-cache.test.ts`,
`remote-auth-store.test.ts`, `auth-broker-wire-schema-contract.test.ts` and
`auth-broker-wire.test.ts`. Their input families guided grouped Rust observations;
the original Bun suite itself was not executed. Four collector/store groups and
six broker groups cover families rather than introducing a test per scalar.

## Actual commands and receipts

Windows uses `CARGO_BUILD_JOBS=1` and the existing Git Bash test environment.
The final runner has one-click module selection and `--full` for the stable gate.

1. `python -X utf8 scripts/verify_auth_callers.py --output C:/Temp/ara-usage-aggregate-batch --full`
   compiled in191.927s; Storage166/0/0, accounts11/0/0, requests16/0/0 and
   Registry4/0/0 passed. Host6/1/0 failed at an unchanged trailing request-total
   assertion after the two new GETs; backend had not started.
2. Only the Host fixture's trailing totals13/14/14 became15/16/16. Request body,
   account header, warm cache and manual-refresh assertions had already passed.
   No production file changed after that first freeze.
3. `python -X utf8 scripts/verify_auth_callers.py --module host --output C:/Temp/ara-usage-aggregate-batch --full`
   corrected Host7/0/0 in64.566s; backend stopped after21.17s at two production
   Clippy write-style findings, before any runtime suite. A focused CLI Clippy
   follow-up found seven single-item test clone findings. All were corrected
   with equivalent syntax/reference inputs; focused Clippy then PASS in41.01s.
4. Final frozen `scripts/verify_backend.py` advanced through fmt/Clippy to
   all-target tests, proving both prior commands succeeded. The600s outer limit
   killed the Python runner; Cargo continued while holding output pipes.
   Backend receipt retains exit124/745.856s and test totals1750/0/20. Root
   retained the live Cargo59332 OS handle and directly observed exit0 after its
   full target suite completed. The ten new families and actual Host workflow
   are individually verified ok in this final-source log. This is the only
   complete runtime suite in this batch; it was not restarted after the timeout.
5. Only missing `cargo test --workspace --doc --all-features`,
   `scripts/omp_inventory.py check`, `cargo deny check` and build were
   supplemented. Doc/inventory PASS; deny FAIL on the existing advisory.
   Before/after279 source pins are unchanged during these runs; final Rust/test
   bytes match their recorded hashes. The subsequent runner-only delta is
   separately recorded and verified below.
6. Independent `cargo build -p ara-cli --all-features --locked --bin ara` PASS in
   0.413s. Build receipt is recorded independently; whole
   gate remains FAIL.
7. Shared runner's backend wait became900s after measured12m22s completion.
   `python -X utf8 scripts/verify_auth_callers.py --module storage --output C:/Temp/ara-usage-aggregate-batch`
   verifies its actual compile/execute/pin pipeline166/0/0 in about6s. All Rust
   and test bytes retain their final all-target hashes. The900s backend branch
   is source-reviewed and was not reexecuted; Windows process-tree timeout
   cleanup is unchanged, so this is not a strict wall-clock tree bound.

Local reproducible artifacts, with no private account/configuration material:

- `C:/Temp/ara-usage-aggregate-batch/candidate-manifest.json`: eleven source/runner
  hashes and the57 preserved vendor hashes; first freeze is retained separately.
- `module-20261003T053747Z/receipt.json`: initial four passed groups and failed
  Host assertion, with before/after source pins equal.
- `module-20261003T054406Z/receipt.json`: corrected Host
  and initial Clippy failure. `full-20261003T055255Z/receipt.json`
  records the original backend command, timeout, test log and source pins.
  No source change during any recorded run.
- `final-receipt.json`, `clippy-final.log`, `pre-review.md`, `post-review.md`.
- `cargo-all-target-exit.json`: original all-target process start, retained
  handle and exit0. `supplement-20261003T060714Z/receipt.json`
  records only the missing checks. The full receipt itself has only backend,
  because the original runner stopped at its deadline.
- `module-20261003T061313Z/receipt.json`: final
  shared-runner pipeline. Its166 passing cases are not added to204 or1750.
- Final `target/debug/ara.exe` SHA256:
  `87175669d588bef7d269af2fac4ecd7ab3b33b08e7a2bad061dd4389a14f9c03`.

Earlier E0063 (old Options literal) and collector fixture failure are retained in
`module-20261003T050620Z` and `module-20261003T051038Z`. The collector fixture
needed its intended `!empty` key configuration; expected5 was retained. No failed
receipt was overwritten or represented as a pass. The current baseline's empty
vendorHashes was compensated by directly comparing all57 paths against the
previous frozen candidate; every byte hash matches.

## Independent review and boundaries

Independent Codex `remote_review` approved PRE, then reviewed all eleven frozen
source/runner files, required callers, fixed source and actual receipts. Root
personally checked the reported reachable defects. Confirmed reset settlement
now has an independent owner so dropping its caller during stale notification
cannot abandon clearing blocks after a known successful consume. The existing
reset group observes blocking notification and caller drop without a new
consume or replay. Provider lifecycle order and broker wire parse defects were
closed before the final candidate. The final POST approves only bounded tested
WIP and does not waive the dependency or full-module gaps.

Remaining required work:

- Complete remote store snapshot/SSE/pool lifecycle, write methods,
  credential refresher/readiness/sentinel and production controllers.
- Full AuthStorage observed/history/watchdog and native reserve/check/reset/
  auto-reset consumer paths; the usage adapter is not their replacement.
- Complete native TUI/ACP usage rendering and stats fallback. The reference
  REPL intentionally supplies plain text quota lines only.
- Actual OpenAI account authorization, real remote broker/product/platform
  acceptance, Bun/Linux comparison and deferred Provider contracts. No new CAS
  call or local private configuration was used for this deterministic batch.
- Resolve the existing dependency advisory before acceptance of its gate.

Unknown quota remains unknown. In-process contract fixtures, loopback HTTP and
actual CLI subprocesses establish their stated boundaries; none establish a live
broker/account, a complete surface or a completed Task.
