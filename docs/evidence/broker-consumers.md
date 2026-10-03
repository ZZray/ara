# CLI auth and RPC observed consumers — bounded WIP

Base `7c9de888694793e185cec8bbc39b7514705d3ec6`; fixed OMP
`596f2da7101178214aa27a753529d15e6b7ad91d`. This continues AI-AUTH and
MODEL-COMPOSITION-01, without adding a point or claiming a complete surface.

## Requirement and source mapping

| Fixed source | Rust consumer | Observable contract |
| --- | --- | --- |
| `packages/ai/src/auth-storage.ts:2736,3017-3099` | `main::run_auth_command`, `auth_storage_broker` | Issue an OpenAI device credential, then publish through the configured Local/Remote owner; logout uses that same owner |
| `packages/ai/src/registry/oauth/openai-codex.ts` | `openai_codex_device_login`, original `OpenAiCodexAuth::login_device` wrapper | Reuse device polling, token/profile/expiry validation and interactive authorization timestamp; preserve original Local SQLite Arc and refresh/CAS/leases |
| `packages/ai/src/auth-broker/remote-store.ts:842-858,1056`, `wire-schemas.ts:60,78` | Existing remote write owner; controlled server projection | Broker owns the real refresh grant, replies with the remote sentinel; OAuth upsert cleans legacy keys while preserving other accounts; logout is native best effort for known disable errors |
| `packages/ai/src/auth-storage.ts:3576-3614`, `packages/coding-agent/src/session/agent-session.ts:3049` | `AuthStorage::record_assistant_usage`, print/REPL HostSink and RPC RunSink | Report one settled assistant's provider/model/timestamp and completely known numeric usage through the same owner, outside journal locks |

Auth commands discover credentials independently of model configuration or
Registry readiness. Their config-key resolver uses the selected project cwd.
A configured Broker failure stops the command before another grant is issued;
it does not fall back to Local. Remote credentials are not mirrored into SQLite.
The owner is retained before mutation, and the existing normal-exit barrier
drains its already-dispatched writes and cache work before returning the result.

The issuer has no database or persistence owner. A token exchange cancelled
before dispatch is Cancelled. Dispatched cancellation, transport failure,
invalid successful token response or HTTP5xx is OutcomeUnknown, without replay;
HTTP4xx remains an explicit rejection. This conservative unknown-effect
classification is an ARA adaptation, not a full native retry-parity claim.
Dropping the issuer future can lose an issued grant; it is not evidence of
rejection. Hosts cancel and await the future for its classified result.

Each RPC Run captures the original ProviderFactory's shared account Arc. The
observer is synchronous, cannot accept a Task or authorize model replay, and
does not introduce a stop-reason filter. Any missing token bucket/cost, nonfinite
cost or token count outside i64 omits the entire numeric report. The original
unknown values stay in the Session journal; unknown is not zero.

## Executed modules

`python -X utf8 scripts/verify_broker_consumers.py --output C:/Temp/ara-broker-consumers-batch --full`
is the complete module and final-gate entry. During the repair, only affected
groups were rerun; unchanged groups were reused with their source pins.

Composite modules **235 passed / 0 failed / 0 ignored**:

- `module-20261003T091321Z`: compile283.268s; Storage194/0/0 in5.312s,
  accounts14/0/0 in2.285s, requests16/0/0 in1.953s, Host8/0/0 in51.235s.
  The Host includes the actual Local CLI login/tool/resume/logout workflow,
  independently rechecked before avoiding a duplicate fixture.
- `module-20261003T092232Z`: repaired auth-command2/0/0 in0.543s,
  compile5.657s. The actual Remote CLI registers and replaces the same account,
  retains another OAuth account and another provider, cleans a legacy key,
  and disables the provider through a separate temporary Broker SQLite authority.
  Post-commit upload/disable500 errors are unsuccessful, have observable
  authority effects, and are not replayed. A cold401 stops before device issuance.
  Child Local auth.db is absent and private token/refresh values are not printed.
- `module-20261003T092620Z`: repaired RPC1/0/0 in24.003s, compile1.514s.
  The family runs both200/500 observation responses: real write artifact,
  numeric POST while the original Run is still streaming and before agent_end,
  explicit abort, same-Session continuation, known zero cost/cache, unknown
  omission in public events and persistent journal, and final EOF flush.
  Exactly two distinct known batches and four model requests occur; an uncertain
  observation is not replayed. Same install identity/provider/model attribution
  is checked. Testkit redacts authentication headers: capture proves their
  presence/length, not the complete bearer value.

Every module receipt records before/after source equality. Candidate manifests
retain12 source/Cargo pins and the57 preserved vendor pins. Only the two new
process fixture files differ from the first candidate; production source did not
change during these repairs.

## Failed observations and corrections

Failed receipts remain under `C:/Temp/ara-broker-consumers-batch`:

1. First auth-command0/2 used `login --provider`; the actual CLI contract is
   `login [PROVIDER]`. The executable rejected argument parsing before dispatch.
2. `module-20261003T092015Z` auth-command0/2 passed parsing but failed startup
   wire validation because the fixture exposed a raw refresh grant instead of
   the Broker's required sentinel. The error-path oracle also used Run exit1,
   while the CLI's established top-level Err exit is2. Both were corrected from
   source; production schema and error behavior were preserved.
3. `module-20261003T092306Z` RPC0/1 reached its final auth-header assertion,
   which incorrectly expected a raw value from the redacting testkit. The
   corrected oracle follows the existing capture contract. This is not evidence
   of a provider authentication failure.

The initial independent SOURCE approval missed fixture-contract defects and
is retained as insufficient evidence. Revised reports and executed failures
record the corrections. No failed module invocation reached the full gate.

## Full gate and acceptance

The single stable Windows backend gate PASS in725.964s: **1785/0/20**, including
format, workspace/all-target/all-feature Clippy, all-target tests and doc tests.
Inventory PASS0.450s. The final module runner retains exit1/sourceUnchanged=true:
dependency check FAIL2.408s, solely the existing unmaintained
`ttf-parser0.25.1 <- fontdue0.9.4 <- ara-snapcompact`, RUSTSEC-2026-0192.
This is not a confirmed vulnerability and no waiver/dependency change is made.
The skipped build alone is supplemented: PASS0.361s, unchanged source, binary
SHA256 `3a519de7337c99ace1dabbc7b5f7e97b1eb58325b28cd16360aed55b8af7dd4f`.
No full runtime suite is restarted.

Independent Codex `/root/broker_integration_review` performed PRE, scoped SOURCE
POST and corrections, recorded in `pre-review.md`, the retained first/round2
reports and `post-source-review-final.md`. Final receipt/document audit in
`C:/Temp/ara-broker-consumers-batch/post-integration-final.md` independently
recomputes235/0/0 modules and147 backend summaries=1785/0/20, verifies all295
final execution pins plus12 source/Cargo and57 vendor pins, and approves this
tested bounded WIP with no remaining confirmed scoped defect. It grants no
point, complete surface, phase, real-account/platform or full-parity acceptance.
The candidate remains tested bounded WIP; the complete dependency gate is FAIL.

No new live Broker, real OpenAI account authorization, subscription task, paid
model trial, Bun or Linux run is attributed to this candidate. The deterministic
control-plane tests do not need a paid model. Auth-specific process cancellation
while waiting for Broker publication and command-token resolution with explicit
`--cwd` are not new runtime observations; existing owner cancellation tests and
source review support those narrower components only.

Broker server/management, production reserve/check/reset/auto-reset/watchdog,
full runtime Provider login and other consumers, actual account/platform tasks,
and the previously recorded retry/numeric/cache/unknown-usage differences remain
required. A successful native best-effort logout does not prove all remote grants
were disabled when the server rejected individual requests.

Counts remain111 surfaces27 implementing/one tested/83 open/zero complete
acceptances;60 points35 limited accepted/23 implementing/two tested. Started
28/111=25.2%; limited35/60=58.3%, not total completion. P0/V1 accepted;
P1-P6 open; RPC27/42; full marker null.

Measured batch time from baseline to the candidate receipt is3096.875s
(51min37s); final independent review and documentation/Git closure follow.
Initial module compilation is283.268s, successful module execution85.331s,
failed fixture execution12.419s and the single stable backend gate725.964s.
These executed checks overlap the development interval and are not additional
wall-clock time. No reliable full-project completion ETA is established.
