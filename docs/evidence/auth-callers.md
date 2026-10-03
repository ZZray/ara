# AuthStorage health, diagnostics and saved resets — tested WIP

Fixed OMP: `596f2da7101178214aa27a753529d15e6b7ad91d`; ARA base:
`059c6d7d8b6796d65e2f98a1e06e0b0835b2ebc0`, branch `dev`.
This continues MODEL-COMPOSITION-01 / AI-AUTH. No new bounded point,
complete surface or phase is accepted. Source/runner hashes and 57 preserved
vendor/EOL hashes are in `C:/Temp/ara-auth-callers-batch/candidate-manifest.json`.

## Fixed source and Rust owners

| Fixed source | Rust implementation and observation |
| --- | --- |
| `packages/ai/src/auth-storage.ts:4067-4203,4211-4215` | `auth_storage_health`: managed OAuth/login-key origin, full-array sticky identity, scoped blocks, parallel usage and Codex healing, plan exclusion, current windows, MIN future exhausted reset, four-state precedence and narrow sticky release. |
| `:4373-4536` | `auth_storage_diagnostics`: sequential active stored snapshot, resolved key references, exact-expiry OAuth preparation, direct uncached usage and independent completion deadline. Initial-request support and builtin Codex null survive the bridge; reports drop raw. Private callback credentials have no Debug/serialization. |
| `:5995-6056,6069-6221` | `auth_storage_resets`: preserve original durable identity on failed account access; live dedicated GET, OR target match and business outcomes; confirmed reset stales current identity/base-URL keys and clears all target block scopes. Existing request accessor projections are unchanged. |
| `packages/ai/src/usage/openai-codex-reset.ts` | `codex_usage`: stable dated/undated/first-credit picker, native consume POST/body-code precedence/JSON fallback, optional lower-level UUID and a fresh default UUID per invocation. No automatic POST retry. |

## Ownership, differences and required remaining work

The same Host AuthStorage/SQLite/Registry owns accounts, usage and assignments.
Core imports no product state. The Host grants permission to consume a reset;
these public library operations do not establish a new production CLI/RPC
controller. Independent caller inspection finds only grouped-test consumers
for the new facades. Complete native usage/check/reserve/auto-reset consumers,
strict model filtering, aggregate/remote store hooks, lifecycle/watchdog and
Broker B1-B5 remain mandatory work. All 18 deferred usage Providers, actual
OpenAI authorization/account behavior and platform contracts remain required.

Diagnostic preparation does not apply the ordinary request's 60s skew or
request-readiness hook. Expired but unrefreshable input still reaches probes;
refresh failure skips both. Current typed error categories are safe receipts
rather than full native error-message parity. A remote sentinel requires its
remote hook and never dispatches a local refresh endpoint. Codex preparation
reuses its existing exact-row/grant advisory owner and durable unknown fence.

After-dispatch consume loss is an explicit `OutcomeUnknown` with a retained
lower-level UUID; the native AuthStorage facade does not return/supply a UUID.
The lower transport does not retry. Confirmed reset is retained even if local
settlement fails: both stale/clear operations are attempted independently of
caller cancellation and the receipt has an optional safe `settlementError`.
These are ARA Host outcome adaptations, not native broker POST-retry parity.
The precise confirmation/cancel and local-store-failure windows are source
reviewed, not separately executed by the current corpus. ISO/RFC date inputs
are supported; Bun's wider Date.parse behavior remains an explicit gap.

## Executed groups, failures and final gate

One-click entry: `python -X utf8 scripts/verify_auth_callers.py --module all`;
`--full` adds the existing backend/inventory/deny/build gates and stops on a
failed mandatory step. The final correction used `--module storage --full`
so other module executables were not separately repeated before the full gate.
Set `CARGO_BUILD_JOBS=1` on this Windows machine for the memory-heavy link.

Receipts are under
`C:/Users/loveu/AppData/Local/Temp/ara-auth-callers-batch/`.

| Receipt | Actual result |
| --- | --- |
| `module-20261003T035538Z` | Compile FAIL 38.346s: two new fixture argument types; both repaired without changing production behavior or assertions. |
| `module-20261003T035653Z` | Compile PASS 56.460s; storage156/0/0, accounts11/0/0, requests16/0/0, Registry4/0/0, actual existing Host7/0/0: **194/0/0**. Source unchanged during this invocation. |
| `module-20261003T040106Z` | After identity-cache repair, compile PASS20.163s, storage156/0/0. Fmt/Clippy PASS, then backend target compilation FAIL270.871s: E0786/mmap OS1455 pagefile pressure and LNK1180 under parallel links. Full tests, inventory/deny/build did not execute; source unchanged. |
| `module-20261003T041057Z` | One-build-job retry: Windows backend **1740/0/20**, fmt/Clippy/doc tests PASS in457.097s; inventory PASS, deny FAIL only on existing RUSTSEC-2026-0192. Total466.379s, source unchanged. The stopped runner skipped its build step; an independent same-source all-feature build then PASSed in0.29s (tool wall0.548s). |

The new eleven grouped test functions reuse native input families, actual
SQLite rows, existing FakeUpstream/loopback HTTP and current Host regressions.
Observations include exact account IDs, reference/refreshed bytes, independent
true/false/null fields, uncached fetch order and preserved cache, expiry versus
skew, sticky/RR/scope state, reset method/body/header/UUID and a single dispatch
on cancellation/timeout. The reset group exercises failed access identity,
OR lookup, explicit credit, current identity after refresh, base-URL stale
last-good retention and target/sibling durable+memory block separation.
Existing Host regression is not a new production reset/check/controller trial.

Independent Codex `/root/remote_review` PRE/POST and Root primary source audit
found and closed the current-identity cache defect: the refresh hook can CAS
account/email before HostState reload. The stale helper now reloads current
durable rows before constructing keys, with an input added to the original
settlement group. Independent final POST closure is recorded in the batch report; approval is bounded tested WIP, with the failed dependency gate and all complete parent requirements retained.

Forty-six generated PDBs (8,460,943,360 bytes) were moved to the batch's
`pdb-backup/` after source/destination containment and SHA256 checks. The
recoverable manifest preserves original locations and hashes. No source,
executable, credential or vendor WIP was removed. The final retry uses one
Cargo build job and reuses completed target compilation.

No new CAS request, actual OpenAI account/reset-credit consumption, Bun oracle
or Linux acceptance ran in this batch. The existing RUSTSEC-2026-0192 remains
unwaived. Counts stay 111 surfaces: 27 implementing/one tested/83 open/zero
complete acceptances; 60 bounded points: 35 limited acceptances/23 implementing/
two tested. Started coverage is 28/111 (25.2%); limited acceptance is 35/60
(58.3%), not full replication completion. P0/V1 accepted; P1-P6 open,
RPC27/42 and the null full-parity marker remain.

## Final receipt identity

Aggregate receipt: `C:/Temp/ara-auth-callers-batch/final-receipt.json`.
Final binary SHA256: `674904ce6ced51512746a9c42320de16452e9f91eebbd879a8e1008f62254643`.
All five Rust source hashes match the final backend before/after snapshot.
The runner alone subsequently gained the Windows default of one Cargo build
job, equal to the final backend invocation's explicit environment. Its delivered
bytes execute in `module-20261003T042013Z`: compile PASS0.390s, storage156/0/0
in5.050s, total5.629s, source unchanged. This script-only resource default
does not relabel prior backend source/binary evidence. The final manifest
contains the delivered runner hash and the unchanged five Rust hashes.
