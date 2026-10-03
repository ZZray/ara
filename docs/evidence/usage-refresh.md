# Builtin Codex usage refresh and native cache invalidation — tested WIP

## Requirement, revision and scope

Fixed OMP is `596f2da7101178214aa27a753529d15e6b7ad91d`; ARA base is
`164834cd21be2a9261ce4519393618e6c7194efc` on `dev`. This continues the existing
MODEL-COMPOSITION-01 / AI-AUTH row. It accepts no complete AuthStorage/Registry
surface, new bounded point or P1–P6 gate, and leaves the full-parity marker null.
Six source/test/runner hashes and 57 preserved vendor/EOL hashes are frozen in
`C:/Temp/ara-usage-refresh-batch/candidate-manifest.json`.

| Fixed source | Rust owner and observable behavior |
| --- | --- |
| `packages/ai/src/auth-storage.ts:3245-3268,3359-3468` | `AuthStorage::fetch_usage_owner`: original access/refresh/expiry eligibility, generic-hook precedence and builtin exact-account preparation; refreshed identity and one total preparation/main/detail deadline. |
| `:2497-2715,5309-5387` | `OpenAiCodexAuth`: shared observed-grant flight, exact durable row, lease reread/CAS, detached consumer settlement, own committed token valid after now versus peer valid strictly after now+60s. |
| `:2638-2668,5358-5387,3402-3413,3490-3518` | Definitive inner refresh CAS-disables the row. Usage wraps successful removal as a nondefinitive missing-row result; that flight may retain last-good, while the next public poll excludes the disabled row. Direct generic definitive failure on an originally expired access remains a cache purge. |
| `packages/ai/src/usage/openai-codex.ts:410-414,429-439` | Actual builtin Host adapter returns null on expired access or failed main GET, including 401/403. Custom typed Unauthorized/Forbidden remains distinct; ancillary detail failure preserves a valid main report. Native `Date.now()` is integer milliseconds. |
| `auth-storage.ts:6228-6240,6248-6252,6271-6286,6304-6307` | Automatic OAuth block invalidation retains each current account/default-URL value with stale expiry. Provider implementation replacement deletes its report prefix. Manual refresh deletes and marks force. Broker callers require the `:oauth` suffix. |

The ownership remains in the reference Host. Shared Core contains no account,
credential database path or product state. A successful refresh keeps durable
ID and stored authorizedAt/organization/routes/unknown fields, and does not
adopt an interactive sibling. Actual fetched identity owns history/healing;
the original request key owns the existing usage flight/cache result.

## Unknown grant and concurrency boundary

The fixed native advisory path preserves a row on a nondefinitive refresh
failure; the existing ARA authorizing path fences unknown external effects.
These requirements share a narrow dispatch record in the existing SQLite
cache table. Its key includes provider, exact ID and an opaque SHA-256 refresh
fingerprint; its value is only an owner UUID. No token/body/account data is
stored in the record, and no short TTL silently permits replay after restart.

- Pending is acquired only for an active exact OAuth row with matching raw
  data and a live lease, before dispatch. Existing Pending means zero new POST.
- Successful row CAS and owner-matched record removal commit in one Immediate
  transaction. The private nonpurging CAS and `stored_rows` avoid nested read
  transactions. Purge is inside that transaction: a purge failure rolls back
  the row and leaves Pending instead of misreporting a committed grant.
- Proven pre-dispatch cancellation and confirmed non-success HTTP rejection
  clear only the matching owner's record. Postdispatch loss, malformed success
  and unresolved fence/commit loss retain Pending. Metadata changes do not
  change the refresh fingerprint.
- Advisory-only unknown keeps the active row and may probe the original valid
  access; expired access sends no GET. Authorizing participation retains its
  exact-row unknown disable contract. Fresh peer adoption requires persistent
  changed access/refresh bytes, not a metadata-only raw JSON difference.
- Registration, release and sole-request cancellation share one activity lock.
  A usage consumer exits waiting without canceling the owner. A sole request
  retains its existing cancel/drop behavior; joined peers remain protected.
  Final receipt publication closes admission under that same lock and settles
  any late authorizing Unknown against actual dispatch raw data.
- Flight keys include observed grant identity; workers stay on the observed
  durable ID. A changed grant before dispatch re-enters the current flight,
  rather than POSTing a new grant under an older key. UUID-fenced cleanup cannot
  remove a newer activity. Settlement awaits both usage and OAuth workers.

The persistent Unknown record is an explicit ARA no-replay adaptation, not a
claim that native OMP has the same durable record. Row lifecycle and cache
behavior still follow the mapped advisory/authorizing contracts.

## Executed module evidence and retained failures

Repeatable entry: `python -X utf8 scripts/verify_usage_refresh.py --module all`.
The three groups reuse existing SQLite, FakeUpstream, Host/process, Session and
native usage/cache inputs. `--full` also invokes the existing workspace gate;
it does not run paid model/account trials.

| Receipt under `C:/Temp/ara-usage-refresh-batch/` | Actual result |
| --- | --- |
| `module-20261003T023334Z` | Compile PASS 111.790s; storage 145/0/0; accounts 10/1/0. The shared-flight fixture changed email and therefore cache identity before its next poll, exhausting the scripted cold GET. Only that fixture's email was stabilized; its single POST/cancellation/report assertions were retained. |
| `module-20261003T024014Z` | Compile PASS 63.888s; accounts 10/1/0. Warm cache/report equality exposed fractional epoch milliseconds changing under JSON roundtrip. Native uses integer Date.now; builtin Rust time now uses as_millis rather than relaxing the assertion. |
| `module-20261003T024403Z` | Compile PASS 87.175s; storage **145/0/0** in 4.802s, accounts **11/0/0** in 2.327s, actual Host **7/0/0** in 39.909s. Total 134.355s, source unchanged. |

New account groups inspect real temporary SQLite row ID/fields, actual
loopback POST/GET paths, account headers and redacted bearer lengths,
original cache key/value, disabled rows and Pending across invalidate/reopen.
They exercise owned 3600s/30s/zero grants, definitive expired/skew outcomes,
transient rejection, main/detail failures, persistent fresh/short peers,
advisory unknown, one refresh+GET/detail budget, both usage/request arrival
orders and a controlled A-to-B interactive login while exact A refresh is live.
Exact 60000/60001ms wall-clock interleaving is source-verified, not separately
claimed as a deterministic runtime fixture. The precise late Unknown
publication window is source-reviewed; ordinary shared cancel orders and
Unknown lifecycle are executed separately.

Independent Codex `/root/remote_review` PRE and POST reports are in this batch.
POST found the LatestInteractive A/B activity defect and the late
authorizing cancel/drop settlement window. Both are repaired; the stable
module and A/B HTTP-gated corpus pass. POST approves bounded tested WIP only.
Root read the fixed source and actual changed dependencies independently.

## Final gate and required remaining work

Final receipt: `C:/Temp/ara-usage-refresh-batch/gate-20261003T025320Z/receipt.json`.
The six before/after source hashes match the frozen candidate; the binary
SHA-256 is `d9ef131aee62be55c69de94ddfcbf0d487e43b3484b5067e7530c507638935ab`.

| Stable final step | Actual result |
| --- | --- |
| `python -X utf8 scripts/verify_backend.py` | Owned formatting, Clippy, all-feature target tests and doc tests PASS; **1,729/0/20**, 436.125s. |
| `python -X utf8 scripts/omp_inventory.py check` | PASS, 0.444s. |
| `cargo deny check` | FAIL, 2.413s: existing `fontdue -> ttf-parser` RUSTSEC-2026-0192; bans/licenses/sources pass. No waiver. |
| `cargo build -p ara-cli --all-features --locked --bin ara` | PASS, 0.370s. |

The complete invocation is **FAIL**, 439.353s, because the dependency gate is
unresolved. The earlier frozen `gate-20261003T025006Z` stopped at Clippy
`too_many_arguments` after 12.475s before tests. Removing the redundant exact-row
selection parameter fixed the lint without a suppression; all five other
module hashes stayed unchanged. The stable final gate exercises the corrected
six-file source. This remains tested WIP, not delivered parity.

Root independently recounted the original backend log and compared all six
current raw source hashes, the candidate manifest and final binary with the
receipt. All match; the 57 unrelated vendor/EOL hashes remain exact.
`python -X utf8 scripts/verify_bootstrap.py` passes (31 required files, OMP
marker, key placeholders, local links and Skills); scoped `git diff --check`
passes. These checks validate the documented snapshot, not actual account
authorization or complete OMP behavior.

The 431 generated PDB files were moved to
`C:/Temp/ara-usage-refresh-batch/pdb-backup/`, with checked absolute paths and a
recoverable manifest. No source, executable or vendor WIP was removed.

Native Bun execution, a new paid CAS task, actual OpenAI authorization/account
usage and Linux/platform acceptance are NOT RUN in this batch. Controlled
usage/account fixtures do not substitute for an actual OpenAI account. Prior
CAS evidence is retained at its original source/binary identity, not relabeled
as acceptance of these new OAuth behaviors.

Reserve/health/reset/broker, complete AuthStorage/Registry/usage callers and
other Providers remain required. Header override/delegate/aggregate hooks and
the recorded Completions watchdog lifecycle/late rejection precedence remain
open. Full OMP parity/P1–P6 and the existing RUSTSEC-2026-0192 dependency gate
are unaccepted. Counts remain 111 surfaces: 27 implementing/one tested/83 open,
zero complete acceptances; 60 points: 35 limited acceptances/23 implementing/
two tested. Started coverage 28/111 (25.2%) and limited acceptance 35/60 (58.3%)
are not full replication completion.
