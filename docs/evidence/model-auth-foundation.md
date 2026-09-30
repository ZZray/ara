# Native model/authentication foundation — tested WIP

Fixed source: OMP `596f2da7101178214aa27a753529d15e6b7ad91d`.
Base: ARA `c6dc5928c952e944aabe0a6b6fa6d27abca8188e`; initial worktree clean.
Code checkpoint: `7dd813941183f1b23976ed79c75878ef1934090e` (WIP, pushed to dev).
This checkpoint does not accept R3/R4, AI-AUTH, CA-MODEL-REGISTRY, CA-RPC or
fixed OMP parity. RPC command coverage remains **27/42 bounded implementations**.

## Source and implementation

| Contract | Fixed source | Rust owner and executed boundary |
| --- | --- | --- |
| Bundled catalogue | `packages/catalog/src/{models.json,models.ts,types.ts,build.ts}` | `ara-cli::model_catalog` preserves every materialized row and literal provider/id identity. Source and MIT notice copied byte for byte. 67 providers / 4,776 rows; 12,244,267 bytes; SHA256 `4f609bf5d4f786c3164c77333ac9962dd2c8b0382291a82815667e558ebc04d0`. Six tests read the entire catalogue and exercise null/extended metadata, duplicate keys, literal suffixes and explicit unsupported projection. |
| Fallback-chain helpers | `packages/coding-agent/src/session/retry-fallback-chains.ts`, `config/model-resolver.ts`, `thinking.ts`, fixed six `retry-fallback.test.ts` fixtures | `ara-cli::retry_fallback` ports specificity, role/default, wildcard provider and id prefix, routed/plain identities, literal max/auto guards, thinking abbreviations, raw deduplication, missing primary/wrap, validation and pending-discovery warnings. Nineteen tests include the empty-role truthiness repair. Backoff remains in the existing retry owner. |
| Native credential storage | `packages/ai/src/auth/sqlite-credential-store.ts` | `ara-cli::credential_store` uses actual SQLite schema 7 with migrations, native/legacy tables and revision triggers, credential identity/CRUD, exact serialized-data CAS, refresh lease owner/expiry fences, block scopes, usage/cache/client records. Fifteen tests use isolated databases, two connections and a child process, reopen and schema 0/3/6 migration. No user installation is opened. |
| Complete request route | `config/model-registry.ts::getApiKey`, `auth-storage.ts::getApiKey`, fixed Agent per-call context sync and retry ownership | `ara-cli::model_route::PreparedRoute` binds model, API-specific options, lazy request-auth resolver and generation. Each logical call acquires an owned credential identity; both provider wire retry layers retain it. Protocol state is shared within a binding and reset for a fresh binding. The existing print/REPL/RPC startup now uses this wrapper with an explicit fixed host override. It does not implement native auth resolution precedence. |
| Busy next-call adoption | Fixed `packages/agent` Agent/loop context sync and `turn-recovery.ts` adoption ordering | Core `ExecutionSnapshot` optionally owns the model/provider/per-call options together with tools/prompt. The in-flight response retains its original snapshot; the next call observes the new one. Run deadline/call budget stay Run-owned. RPC publishes the complete snapshot. No new `set_model` command or adoption saga is claimed. |

## Executed checks

Commands on the checkpoint:

```text
cargo test -p ara-cli --lib model_catalog       # 6 passed
cargo test -p ara-cli --lib credential_store    # 15 passed after repairs
cargo test -p ara-cli --lib retry_fallback      # 19 passed after repairs
cargo test -p ara-cli --test model_route        # 10 passed
python -X utf8 scripts/verify_backend.py
cargo deny check
cargo build -p ara-cli --bin ara --all-features
```

The ten route tests make actual HTTP requests to controlled local upstreams.
They cover Chat, Responses, Anthropic proxy and API-key routes, both wire retry
layers, delayed old-request attribution, auth cancellation/consumer drop,
attribution and settlement persistence failure, stale model/protocol rejection
and static credential-header rejection. A real Agent remains busy while its
next-call snapshot changes from Chat to Responses, retains the first call's
tool adapter, writes the expected temporary file once, carries its tool result
and drains the previously queued follow-up on the new route. Settlement failure
keeps actual content, known usage and native response ID and blocks replay.

Two initial route fixtures had incorrect expectations: custom Anthropic proxy
routes use Bearer, and normal empty Chat stop is a valid result rather than an
opted-in empty retry. They were corrected after reading the fixed/current
adapter source. The wire-retry fixture instead exhausts the actual HTTP budget
and observes the outer replay retaining the same credential lease.

Final gate receipt is recorded below after execution. Raw logs, source pins,
binary hash, initial lint failure and superseded pre-review-fix PASS are retained
under `C:\Temp\ara-r3r4-batch`. Test counts are bounded software evidence,
never an OMP surface completion percentage.

## Independent review and repairs

Independent Codex reviewers used the ARA Git/Core procedures with fixed source:

- `ctx_oracle_diff_review`: complete route/startup/snapshot and route fixtures.
  Static authentication headers could override the resolved account while
  retaining another account's attribution; headers now belong to the same
  owned lease and static protocol credentials are rejected. Startup explicitly
  moves authorized header overrides into its Runtime lease. Settlement failure
  previously erased the provider terminal; it now preserves the message and
  vetoes replay. Direct saturated-channel cancellation and stateful Responses
  rebinding fixtures remain a stated coverage gap unless recorded separately.
- `rpc_owned_core`: all credential-store production APIs/SQL/migrations/tests
  and the four catalogue files. BOM identity normalization now uses the existing
  JS trim implementation; same-account reuse and U+0085 preservation are tested.
  Catalogue data/license bytes and all model identities were independently
  compared to the fixed Git blobs. No remaining confirmed catalogue/store defect.
- `host_bridges_independent_review`: all fallback helpers/tests and the six
  original fixed fixtures. Empty role hints and empty matched roles now follow
  JS truthiness; the added fixture passes and the narrow final review resolves
  the finding. Also reviewed the architecture/performance/security plan.

The first shared gate stopped on four Clippy style errors before tests; the
small let-chain/unwrap repairs are preserved in the diff. The dependency audit
found existing `yoke-derive 0.8.3` newly yanked, so only that locked package was
updated to 0.8.4. Dependency license/source policies were not relaxed.

## Differences and mandatory remaining work

- SQLite failures are returned as `Result` instead of OMP's best-effort error
  swallowing; this does not authorize success after an error. Host-injected DB
  location/busy timeout and private raw serialized data/revision support request
  attribution. Revision is the captured global DB revision, not a row counter.
- Fallback default expansion is an owned Rust snapshot; configuration changes
  must rebuild it. Malformed selected chains return explicit typed errors rather
  than incidental JavaScript iterable/type errors. Valid selection semantics
  retain the fixed behavior.
- Every bundled row still needs input/context/compat and other execution
  metadata integration. `try_execution_model` explicitly reports these missing
  contracts. A readable catalogue is not an available/executable model route.
- The full models.yml overlays/validation, models.db discovery cache, providers
  and pending-discovery lifecycle, native AuthStorage precedence, command keys,
  OAuth refresh/provider matrix, session credential release/rotation, quota/
  usage/preflight/reserve/cooldown, native model/role/thinking persistence and
  fallback adoption/served/revert remain mandatory next work. No empty-success
  implementation replaces them.
- Native store schema 1/2/4/5 initial-state fixtures, corruption/busy-opening
  faults and Linux execution remain unverified here. Real account DB/OAuth and
  cross-route model switching from actual RPC model commands are not exercised.
  No real-model task is claimed for this foundation checkpoint; selected actual
  task acceptance belongs to the connected registry/auth/fallback batch.
- R2 overflow/ThinkingLoop/interrupted recovery and every other required fixed
  inventory item remain open. Complete reproduction precedes customization.

## Final shared gate

Final Windows snapshot gate: **PASS, 1,366 passed / 0 failed / 1 ignored,
102 suites**, including owned-package fmt, all-feature/all-target Clippy and
tests, and workspace doc tests. Dependency advisory/license/ban/source checks
and `ara` binary build also pass. Total pipeline time **86.152 s**: backend
83.440 s, dependency check 2.349 s, binary build 0.351 s. Source pins are unchanged
through the gate. Binary SHA256:
`ddaa32439e3088830ff199ae104dabd1dfa24f7957d11327cc09cd6d89d4524e`.

Raw receipt: `C:\Temp\ara-r3r4-batch\full-receipt.json`, with three raw logs
and 18 source/data pins. The initial Clippy failure and the pre-empty-role-fix
PASS (1,365/0/1, 113.393 s) remain in separate directories and do not prove the
repaired source. Narrow final fallback audit resolves the empty-role finding
on SHA256 `7f77df37cb9a5e8497f75371cf662e4334593dbf1b52bdefe573772ceb500cce`.
Narrow final route/Core audit resolves both P2 findings and verifies the retained
Run deadline/call-budget/hook ownership. Saturated-channel cleanup, native
Responses rebinding and a different-target-deadline/budget direct fixture remain
explicit gaps. This point remains **implementing/tested WIP**.

Commit pin audit: all 18 tested source/data files match the committed content;
16 are byte-exact and the two Cargo manifests differ only by CRLF-to-LF Git
normalization. Catalogue and license remain byte-exact with explicit `-text`
attributes. Nine final independent route/Core/fallback/catalogue reviewer pins
match the raw gate receipt. Store's final narrow review predates only the
semantically equivalent Clippy let-chain/unwrap repair; all its tests run in the
final shared gate. `verify_bootstrap.py` and `omp_inventory.py check` pass after
the evidence/plan updates. Linux CI was not yet listed on the first exact-SHA
query after push; no Linux pass is claimed for this checkpoint.
