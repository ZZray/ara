# Handoff 2026-10-03 — builtin usage refresh tested WIP

Goal remains active: fixed complete OMP parity, independent Rust Core, ARA
capabilities, product integration, then P0-P6 acceptance. Provider priority
stays custom OpenAI-compatible routes and OpenAI account login. Deferred
Providers remain mandatory recorded parity work.

## Candidate, evidence and acceptance

- Base `164834cd21be2a9261ce4519393618e6c7194efc`, branch `dev`; fixed OMP
  `596f2da7101178214aa27a753529d15e6b7ad91d`.
- [Usage refresh evidence](../evidence/usage-refresh.md) maps native builtin
  preparation, original eligibility, shared exact-row flights, own/peer freshness,
  one total deadline, last-good and stale/forced/replacement invalidation.
  Persistent unknown-grant markers are an explicit ARA no-replay adaptation.
- Six source/test/runner paths are frozen in
  `C:/Temp/ara-usage-refresh-batch/candidate-manifest.json`; preserve the 57
  unrelated vendor/EOL hashes and exclude `.codebase-memory/` from Git.
- Module receipt `module-20261003T024403Z`: storage145/0/0, accounts11/0/0,
  actual Host7/0/0; 134.355s including 87.175s compile.
- Final receipt `gate-20261003T025320Z`: Windows backend **1,729/0/20**,
  fmt/Clippy/inventory/build PASS, 439.353s. Whole gate remains **FAIL** on
  existing RUSTSEC-2026-0192; bans/licenses/sources pass. No waiver.
- Stable final binary SHA-256:
  `d9ef131aee62be55c69de94ddfcbf0d487e43b3484b5067e7530c507638935ab`.
  Final before/after source hashes match the candidate. The third module
  preceded the source-reviewed lint-only redundant exact-row argument removal;
  the final all-feature gate exercises that corrected source.
- Independent Codex `/root/remote_review` PRE/POST found and closed A/B
  account selection and late authorizing Unknown cancellation/drop settlement.
  Reports are in the batch directory. Root independently checked the critical
  changes and receipts. This is bounded tested WIP, not complete acceptance.
- Retain both module failures and the initial Clippy-stop receipt. The floating
  epoch timestamp failure was fixed by native integer milliseconds, preserving
  equality assertions. No new fine-grained tests or repeated API trials.
- No new paid CAS, actual OpenAI authorization/account usage, Bun oracle or Linux
  receipt. Exact 60s wall-clock and late Unknown publication windows have source
  review; executed peer/cancellation/Unknown cases are separately identified.
- 431 generated PDB files (12.81GiB) have a recoverable backup under
  `C:/Temp/ara-usage-refresh-batch/pdb-backup/`; use its checked manifest before
  any restoration. No source/executable/vendor WIP was removed. Root owns
  Cargo/network/private configuration/Git writes.

## Progress and next batch

111 surfaces: 27 implementing, one tested, 83 open, zero complete acceptances;
started coverage **28/111 (25.2%)**. Sixty bounded points: 35 limited acceptances,
23 implementing, two tested; **58.3% is not overall replication completion**.
P0/V1 accepted, P1-P6 open, RPC 27/42, full marker null. Full-project ETA is
unsupported by measured remaining throughput.

Continue reserve/health/reset/broker and complete AuthStorage/Registry/usage
callers using fixed source and grouped native inputs. Remaining header
override/delegate/aggregate storage hooks, Completions instant abort and
latched-timeout/late callback rejection, actual account/platform and deferred
Providers remain required. Reuse existing receipts/source extractions and one
stable final batch gate; do not replay completed PRE work or paid CAS trials.
