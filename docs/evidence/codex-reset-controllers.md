# Saved-reset controllers — bounded tested WIP

**Later continuation, 2026-10-03:**
[HTTP hints and same-Session quota recovery](session-quota-recovery.md) records
the subsequent body-hint and post-emitted actual CLI/RPC work. That continuation
updates those previously required items; the results and differences below
remain the receipts for this earlier candidate.

## Requirement and source

Base `87ed35abf69c23c79aba108b685633caa0f99ab3`, branch `dev`; fixed OMP
`596f2da7101178214aa27a753529d15e6b7ad91d`. Connect saved-reset planning,
explicit Host consent, operation ownership and supported pre-output quota
recovery to the existing shared AuthStorage owner. A confirmed response alone
must not authorize replay after failed receipt persistence or a stale Host.

Original source, read from the fixed commit:

- `packages/coding-agent/src/session/codex-auto-reset.ts`: complete pure
  planner, live-credit overlay, attempt/defer/cooldown and process coordinator.
- `session/agent-session.ts:9822-9844,9894-10216`: usage heartbeat, consent,
  blocked pass and successful-fetch salvage sweep.
- `session/turn-recovery.ts:2140-2240`: sibling-first recovery, existing
  attempt budget and immediate retry after a reset.
- `packages/ai/src/usage/openai-codex-reset.ts:160-206`: credit selection,
  UUID wire identity and consume classification.
- `slash-commands/builtin-session.ts:22-69`,
  `slash-commands/helpers/reset-usage.ts`,
  `config/settings-schema.ts:5812-5864` and `config/settings.ts:2231-2241`:
  manual selectors, defaults and nested boolean migration.
- Native `codex-auto-reset.test.ts` and integration fixtures inform the coarse
  Rust families; they are not attributed as executed Bun tests.

## Implementation and ownership

| Files | Bounded behavior |
| --- | --- |
| `codex_auto_reset.rs`, `tests/codex_auto_reset.rs` | Native planning, matching, exhaustion, freshness, synthetic 429, reserve/expiry rules, sorting and overlay in eight coarse families |
| `rpc_host_settings.rs` | Nested tri-state mode, native numeric coercion, lazy scoped persistence and defaults: unset/60 minutes/keep 0/salvage 12 hours |
| `codex_reset_receipts.rs` | Host SQLite journal; durable Pending before POST, exact UUID/account/row/credit/endpoint identity and immutable terminal transitions |
| `auth_storage_resets.rs`, `auth_storage.rs` | Observed facade owns consume and settlement even after its awaiting caller disappears; original facade remains available |
| `credential_store.rs`, `auth_broker_client.rs`, `auth_broker_store.rs` | Private canonical credential authority used to scope coordination and receipts; no credential database mirror for Remote ownership |
| `model_route.rs`, `request_auth_retry.rs`, `tests/model_route.rs` | Siblings first; one receipt-correlated same-bearer reset exception within MAX64; callback/partial/unknown/cancel vetoes remain |
| `codex_reset_controller.rs`, controller/support tests | Shared scoped coordination, manual/list/blocked/sweep, consent, notices, owned shutdown and successful-fetch refresh without blocking usage output |
| `main.rs`, `rpc_host.rs` | Actual print/REPL/RPC account owner, `/usage reset [active\|email\|account]`, render/state/completed freshness and real Host Session/provider/model/epoch admission |
| `lib.rs`, `verify_reset_controllers.py`, `verify_openai_daily.py` | Public module exposure and one grouped runner; `@bin:ara` includes actual settings tests without bypassing a failed gate |

Pending/Unknown fences use the actual credential authority, Provider and canonical
consume endpoint. Logical account matching survives a row replacement. Within
that same scope, exact-row matching also survives accountId metadata enrichment;
an email alias survives replacement only when at least one identity lacks an
accountId. Two identified accounts sharing an email do not alias. The check and
Pending insert occur atomically in an SQLite Immediate transaction across
independent connections. A fresh balance GET cannot clear an unresolved fence.
Receipts contain no bearer or raw response body. They are Host metadata, not a
remote credential mirror.

Reference Hosts capture their epoch in each context and bound resolver. New or
adopted Sessions and Codex rebindings advance it; EOF/disconnection/factory drop
close admission. Automatic settings and current Host identity are rechecked
after eligibility IO and before each POST. The quota callback checks current
identity before and after its asynchronous lease resolution. An already admitted
consume still settles; a stale or canceled caller cannot obtain model replay.
Core imports no Host setting, product identity or permanent timer.

## Deliberate differences and required work

1. Native ambiguous consumes can release an attempt after 30 minutes and create
   a fresh UUID. ARA retains Pending/Unknown durably and never authorizes that
   replay from a balance read. Safe pre-POST `no_account`, `account_unavailable`
   and `credit_list_failed` projections may defer only with `operation=None`
   and no settlement error; arbitrary errors or unresolved receipts do not.
2. Native accepts a success status without a parsed code as reset, and even a
   non-success response carrying `code=reset` is classified as reset. The new
   observed facade requires 2xx and the original parsed `code=reset`; recognized
   already-redeemed/no-credit/nothing-to-reset below 500 are known no-ops. 5xx,
   malformed/missing/unknown codes and contradictory reset responses are Unknown.
   The legacy facade keeps its original classification. Receipt-finish failure
   preserves positive upstream observation separately from durable Pending and
   cannot grant retry; owned local settlement still runs.
3. Native blocks a new sweep while a blocked pass is active. ARA also makes a
   blocked pass wait for an already active same-scope sweep and adopt its confirmed
   receipt, preventing a second concurrent spend in the opposite interleaving.
4. Reference headless Hosts preserve unset, eligibility IO and deduplicated notice;
   they cannot provide the Native selection UI. An actual Host callback can persist
   explicit Yes/No. The full Native Settings service, invalid enum behavior,
   overrides/overlay and broad JavaScript Date.parse grammar remain required.
   Quoted flat dotted disk keys remain unrelated preserved values, consistent
   with Native getByPath; they do not grant nested auto-redeem authority.
5. Post-visible-output Session recovery remains required. This batch wires only
   supported pre-output rejection recovery. TUI/ACP, remaining usage consumers,
   B reserve/fallback, C diagnostics/strict checks/Gateway and complete Broker
   server/management/watchdog remain required. No complete reset family, parent
   MODEL-COMPOSITION-01, surface, P1-P6 phase or full parity marker is accepted.
6. The actual CLI/RPC quota fixture uses a persistent 429 with a long
   `Retry-After: 7200`; only a successful consume clears its quota. This follows
   fixed Native fetch-retry's delay-ceiling admission path. Native
   `fetch-retry.ts:112,268` also parses a body-only `reset in 2 hours` hint,
   while Rust `post_with_retry_detailed` currently consults headers at that
   decision. A body-only first 429 can therefore enter ordinary backoff in Rust
   before the Host sees a terminal rejection; this specific parity difference
   remains required. The new header-based receipts do not prove it fixed.
   The pre-existing request family uses HTTP400 insufficient_quota. No unrelated
   provider retry change is made here.

No real account/subscription reset or paid model task is run for these
deterministic control-plane faults. Actual OpenAI authorization, live Broker,
Bun/Linux/platform and deferred Provider acceptance remain required.

## Execution receipts

The frozen candidate manifest and all failed/development/final receipts are in
`C:/Temp/ara-reset-controller-batch`. Source, Cargo and runner/test-support hashes
are pinned; 57 unrelated vendor/EOL hashes and `.codebase-memory/` are preserved.
Final executed counts, independent review and dependency boundary follow below.

### Retained failures and scoped corrections

- `compile-first.log`: the existing diagnostics test requires `test-fixture`;
  the unsupported all-target feature selection failed. Its fixture was unchanged.
  The first supported compile also caught a new test helper-name collision;
  the delivered helper variable is corrected. Supported candidate check PASS.
- `module-20261003T111350Z`: compile321.673s, storage200/0, planner8/0,
  settings58/0, requests17/0; controller11/2, exit101/sourceUnchanged=true.
  The original one-shot429 fixture allowed ordinary adapter backoff to succeed
  before the Host's reset hook. The corrected fixture retains quota until an
  actual consume and sends the supported long Retry-After hint. CLI still needs
  two model POSTs/one consume/confirmed receipt/same Session and bearer; RPC
  still checks reset notice, two same-Session calls and a new adopted Session.
  `module-20261003T112406Z`: controller13/0 in7.198s, compile12.908s,
  sourceUnchanged=true. The body-hint parity difference above stays required.
- `module-20261003T112721Z`: Host6/2, no backend execution. One old oracle
  expected the removed Codex RPC ban; it now checks actual ready/EOF shutdown,
  empty accounts and zero upstream requests. The device workflow's sequential
  HTTP script mixed usage GETs with model/summary responses after the new render
  refresh. Two existing fake servers now isolate usage while the unexpired grant
  is reused. Original artifact, compaction, Session/rebinding and unknown-refresh
  assertions remain; two usage GETs and11%/12% output are still required.
- `module-20261003T113459Z`: Host8/0 in56.512s, compile6.853s. The backend
  preflight then failed Clippy's collapsible-if on one planner hunk after24.509s;
  no full runtime suite ran. Equivalent if-let/boolean syntax corrects it without
  a lint suppression. Only the planner group is rerun on that final source before
  the stable runtime gate; earlier unchanged groups are reused.
- `module-20261003T113708Z`: planner8/0 after39.194s compile; backend preflight
  failed after36.872s on default-field reassignment in its new test. A keep-going
  Clippy pass then identified only a needless borrow in the render call. Both
  received equivalent syntax fixes without suppressing lints. Independent review
  reverses each of the three lint hunks in memory and reproduces the exact old
  reviewed whole-file hashes. `module-20261003T114159Z` owns the final planner
  and stable gate; the old preflight attempts contain zero runtime test totals.

Development checks are separately retained: storage5/0, planner8/0 and requests
17/0. Their build/link times do not represent frozen acceptance. Failed receipts
are not counted as passes, and a requested `--full` stopped in a module or
preflight is not described as an executed full runtime suite.

### Final frozen execution and boundary

`module-20261003T114159Z/receipt.json` owns the stable final gate. Final planner
8/0 runs in0.062s after20.385s compile. The seven module groups total
**305/0/0**: storage200, planner8, settings58, requests17, controllers13,
Host8 and RPC-observed1. Unchanged groups are reused from the retained receipts;
syntax-equivalence review and the final all-target run cover the delivered code.
`composite-receipt.json` binds all seven paths and current23 source/Cargo pins.

The single stable Windows backend gate PASS: **1815/0/20** in803.858s,
including owned-package fmt, workspace/all-target/all-feature Clippy, actual
all-target tests and doc tests. A retained native Windows Cargo handle records
the all-target process exit separately in `native-all-target-exit.json`.
Inventory PASS in0.45s. The runner's final exit is
1/sourceUnchanged=true because dependency check FAIL
in2.471s on the existing unmaintained
`ttf-parser0.25.1 <- fontdue0.9.4 <- ara-snapcompact`, RUSTSEC-2026-0192.
No waiver or dependency change is made. Only its skipped build is supplemented:
PASS in0.371s, frozen source, binary SHA256
`d9b5c5670d3b84e55b295fe3b0d4c04bcf9471e36f99fd151000fd502f7e1530`. A passing runtime gate is not complete dependency acceptance.

Independent Codex `/root/broker_integration_review` performed PRE, SOURCE rounds,
the alias-fence and current-Host corrections, fixture-repair review and final
lint-equivalence/source-pin audit. Final execution/document POST in
`post-integration-final.md` is **APPROVE TESTED BOUNDED WIP**: raw module/backend
totals, delivered pins, binary identity, retained failures and recorded gaps
match, with no confirmed blocking source/integration finding in this scope.
Reports and failed receipts are retained under the same Temp evidence root.
The complete dependency gate remains FAIL; no parent point, complete surface,
P1-P6 or full-parity marker is promoted.

Counts remain111 surfaces27 implementing/1 tested/83 open/0 complete;60 points
35 limited accepted/23 implementing/2 tested. Started28/111=25.2%; limited
35/60=58.3%, not total completion. P0/V1 accepted; P1-P6 open; RPC27/42;
full marker null. Baseline-to-final-build/document collection is
6255.719s, excluding the preceding approximately15-minute
inventory investigation and final independent review/Git closure. The independent
POST is complete; scoped unfinished Git identity is recorded separately in
`git-closure.json` under the same evidence root.
Executed compile/test times overlap development and are not extra wall time.
No reliable full-project ETA is established.
