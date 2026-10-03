# Remote credential owner and reference Host — WIP

## Target and decision

- ARA base: `ca59d598d8e275239f8e75b38e5311f073d63915`, branch `dev`.
- Fixed OMP: `596f2da7101178214aa27a753529d15e6b7ad91d`.
- Scope: the remote credential owner, its HTTP/SSE client, short store facade,
  shared AuthStorage/Registry/request consumers, Host discovery and encrypted
  snapshot cache. It does not accept the Broker server, complete AuthStorage,
  all CLI/RPC consumers, actual accounts, another platform or another Provider.
- Module composite233/0/0 and final Windows backend1779/0/20 passed. The one
  backend gate took672.079s. Format/Clippy/doc/inventory and the supplementary
  independent build passed. The dependency gate remains FAIL for the existing
  unmaintained `ttf-parser` advisory; no exception was added. This is tested WIP,
  not an accepted point, complete surface or phase. Independent Codex final POST
  approves this bounded tested WIP only; the marker and parent statuses stay open.

## Fixed source -> Rust -> behavior families

The native files were exported from the exact fixed Git blobs, not a moving
branch. MIT copyright notices are retained. Original Bun tests are input
evidence; the Bun suite itself was not executed.

| Fixed source | Rust owner | Grouped executable observations |
| --- | --- | --- |
| `packages/ai/src/auth-broker/types.ts`, `wire-schemas.ts` | `auth_broker_wire` | 31 named schemas, last duplicate JSON key, required/null fields, OAuth extension fields and sentinel, strict envelopes, JS numeric projection and invalid siblings |
| `auth-broker/client.ts` | `auth_broker_client` | health/snapshot/ETag/long-poll, all native client endpoints, request/body/schema failures, GET retry, dispatched mutation uncertainty, unsupported observed reporting and streaming |
| `packages/utils/src/stream.ts:388` and Broker client stream parsing | independent Broker SSE decoder | split UTF8/chunks, CR/LF, BOM, comments, multiline data, first snapshot and malformed events; no generic model decoder substitution |
| `auth-broker/remote-store.ts` | `auth_broker_store` | live SSE snapshot/entry/removal, 404 long-poll fallback, idle/resume/backoff, pool filtering, raw-ID counts, generations, projection revisions, blocks and ephemeral cache |
| `remote-store.ts` refresh/readiness and mutations | same remote owner | refresh sentinel, suspect/readiness, out-of-pool refresh, partial replace/logout, detached callers, tombstones, callbacks outside locks and finite settlement |
| `remote-store.ts` observed/history/close | same remote owner and one `AuthBrokerUsageStore` | observed merge/timer/default identity/final flush, known retryable failure, unsupported latch, uncertain batch isolation, usage/history client APIs and permanent close fencing |
| `packages/ai/src/auth-storage.ts:383-568,5395-5414,5603-5620` | `credential_store_port`, `auth_storage`, `auth_storage_broker` | original Local SQLite Arc/Codex ownership, same Remote Arc, exact-ID owned refresh, explicit callback priority and Local/Remote publication, cancellation of a waiter without cancelling its owner |
| `auth-broker/discover.ts`, `snapshot-cache.ts`; `packages/coding-agent/src/session/auth-broker-config.ts`; `packages/utils/src/dirs.ts` | `auth_broker_discover`, `auth_broker_snapshot_cache`, actual `main` | env/config/token-file precedence, shared discovery, encrypted cache/startup/revalidation, install identity and normal exit drain |
| native shared request account ownership | actual CLI `openai_daily_cli` Broker family | remote API key -> Registry/request -> write tool -> file/journal -> original Session restart; fresh cache while Broker is offline; cold failure/missing bearer reject rather than substitute a Local key; no SQLite mirror or synthetic credential leak |

The family runner covers the entire CLI library plus `openai_codex_auth`,
`model_route`, `model_registry` and `openai_daily_cli`. Selection is by module;
`--full` continues with one workspace gate only after selected groups succeed.

## Review corrections

Three PRE perspectives reviewed implementation/pragmatism, architecture and
performance/safety. Independent Codex `/root/broker_integration_review` then
reviewed integration twice. Root read the critical Rust and fixed native hunks.

- Remote default refresh must use its own owner; an explicit callback has
  priority, while a registered provider still supplies its lease projection.
  Generic callback leases must not suppress builtin Codex account headers.
- Local explicit callback publication writes and reloads the same enabled OAuth
  ID through the original short SQLite lock. The callback need not write the
  database. One group covers Local/Remote normal and cancelled waits, retained
  fresh token and a second resolve without another refresh or Remote upload.
- Broker bearer401/403,429, configuration/schema errors and malformed identity
  are not evidence of a revoked provider grant. The actual AuthStorage fault
  group checks six responses, one refresh each, zero disable and original token
  retention. Non-OAuth/wrong ID/provider returns Configuration; a pool rejection
  returns Unavailable.
- Host generation follows a separate monotonic projection revision/watch;
  it is not `local counter + wire generation`. The watcher has a Weak owner and
  explicit cancellation/join.
- Permanent usage close fences reservations, old-flight cache publication,
  snapshot replacement and overlay ingestion. Already-dispatched finite owners
  settle; normal Host exit also drains cache writers.
- Native OAuth set dedupe retains the last matching identity. Install-ID
  fallback is stable in process per Host configuration root.

These corrections are source-reviewed; both new AuthStorage fault/callback
families and the actual Broker CLI family also pass in the final full suite.

## Required differences and unverified acceptance

These items are required work, not waived or accepted parity:

1. Fixed native POST/DELETE retries transport failures without a general server
   idempotency receipt. Rust retries GET, but retains dispatched uncertain
   mutations,2xx body/schema failures and possibly-partial5xx as OutcomeUnknown;
   it does not replay uncertain observed batches. No new server receipt is
   invented. Source parity remains open until the contract is reconciled.
2. Native ID/generation first has JS IEEE754 Number semantics. Rust applies that
   projection before i64 admission, then rejects values outside i64; that range
   boundary remains a recorded source difference.
3. Rust cache admission validates the complete current wire schema. Native uses
   a cheaper authenticated shape guard. The persisted `OMPS` v2 header,12-byte
   nonce, SHA256(token) AES256-GCM key and original-URL AAD are retained, but
   cross-runtime cache acceptance and platform behavior have not been exercised.
4. Snapshot TTL uses Rust decimal f64 parsing; native `Number(raw.trim())` also
   admits hexadecimal/binary/octal strings. For example native `0x0` disables
   caching while Rust uses its default. Full numeric/trim parity remains open.
5. The fixed numeric observed-report wire cannot express unknown token buckets
   or cost. The reference Host submits only fully known values, retains unknown
   journal values and does not fill missing fields with zero. The controlled
   model Host case proves omission, not numeric observed POST acceptance.
6. CLI auth login/logout still use Local Codex; the RPC sink does not yet report
   observed usage. Production reserve/check/reset/auto-reset controllers,
   watchdog tails, complete AuthStorage/Registry consumers and Broker server
   must still be connected and accepted. Custom provider lease families,
   actual Broker/account use, Linux/Bun and deferred Providers remain open.

No new paid-model call is needed to prove a Broker control-plane fixture. No
new real Broker, actual OpenAI authorization or subscription-model trial has
been performed in this batch; earlier real tasks are not attributed to this
changed source/binary.

## Commands and artifacts

- `cargo clippy -p ara-cli --all-targets --all-features -j1 -- -D warnings`:
  final CLI candidate PASS41.43s, after draft check/lint failures preserved in
  separate logs. A temporary test assertion using a nonexistent private-header
  accessor was removed; the production lease interface was not expanded.
- `python -X utf8 scripts/verify_broker_store.py --output C:/Temp/ara-broker-store-batch --full`:
  compile363.493s; Storage194/0/0, accounts11/0/0, requests16/0/0 and Registry4/0/0
  passed. Host7/1/0 failed before model dispatch at Validate(models): the new
  fixture removed authored authentication but still declared custom models,
  contrary to fixed `models-config.ts:78`. The workspace gate had not started.
- Only the Host fixture changed to native-valid `models:[]` plus explicit CLI
  model and provider transport overrides; no production validation or credential
  ownership changed. All other21 source/Cargo pins and57 vendor hashes match.
- `python -X utf8 scripts/verify_broker_store.py --module host --output C:/Temp/ara-broker-store-batch --full`:
  compile11.208s and Host8/0/0 in57.370s. Four unchanged groups are reused;
  composite module result233/0/0. Backend PASS672.079s,1779/0/20, including
  format, workspace Clippy and doc tests; inventory PASS0.431s. Unlike the prior
  batch, the900s outer backend deadline completed normally, with exit0.
- Dependency check FAIL2.529s: the only error is existing unmaintained
  `ttf-parser0.25.1 <- fontdue0.9.4 <- ara-snapcompact`, RUSTSEC-2026-0192.
  The runner retains exit1/sourceUnchanged=true and did not run its build step.
  This advisory is not reported as a confirmed vulnerability, and no waiver
  or unrelated dependency change was introduced.
- Only the missing build was supplemented:
  `cargo build -p ara-cli --all-features --locked --bin ara`, exit0, Cargo0.27s.
  `build-receipt.json` records outer time and exact binary SHA256. The successful
  workspace runtime suite was not restarted.
- `C:/Temp/ara-broker-store-batch/candidate-manifest.json`:20 changed/new scoped
  source/runner files plus Cargo pins; all57 vendor hashes match the baseline.
- `module-20261003T080729Z/receipt.json` retains the first Host failure and
  passing unchanged groups; `module-20261003T081831Z/receipt.json` retains the
  repaired Host/full suite/dependency failure. Both have sourceUnchanged=true.
- `base-manifest.json`, `first-candidate-manifest.json`, `implementation-contract.md`,
  three `pre-*.md` reports, `post-integration-draft.md` and
  `post-integration-round2.md` retain source and review history.
- `post-integration-final.md`: independent Codex final POST approves bounded
  tested WIP only, with no remaining confirmed finding. The reviewer independently
  re-summed145 backend summaries to1779/0/20, checked all22 source/Cargo pins
  against receipt before/after/current bytes, all57 vendor pins against baseline
  and current bytes, and the build binary hash. Root independently checked the
  critical source hunks and executed receipts. The whole gate remains FAIL.
- `final-receipt.json` links both module receipts, final backend/dependency
  result, supplementary build and final review. Final binary SHA256:
  `663954f85a61f8aa40d1c7db37fad4ab3879d46a2b611241f09d513064f826bf`.
- Documentation/bootstrap verification PASS:31 required files, OMP marker,
  secret placeholders, local links and Skills. Scoped diff check PASS.

Counts remain111 surfaces27 implementing/one tested/83 open/zero complete
acceptances;60 bounded points35 limited accepted/23 implementing/two tested.
Started28/111=25.2%; limited35/60=58.3%, not total completion. P0/V1 accepted;
P1-P6 open; RPC27/42; full marker null. This batch deepens the existing parent
module and does not manufacture another accepted point or denominator.
