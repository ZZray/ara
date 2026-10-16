# HandWave Node 0.2.0 — 2026-10-16

Scope: compact Rust product Host only, independent of root ARA Core/OMP parity.
Base Rust56fd2ab, Go326b1ba. No root Core WIP or dirty legacy browser/auth code
is part of this release. Final publication/independent review is recorded below.

## Candidate and execution

Windows x64 standalone EXE:4,922,880bytes, version0.2.0, SHA256
`cb676ce827a0a496a23decd05246d0f78d5e24e0f6b5c8f569dc1e7eb5e304b1`.
The earlier4,809,216byte5f47d78b candidate was dynamically linked and is NOT the
public artifact. Reading an old static-CRT Cargo fingerprint did not identify
that candidate's actual imports. `scripts/build.ps1` rebuilt with explicit
static CRT; final PE no longer references vcruntime140/API-MS-WIN-CRT.
All affected process and real-model suites are rerun against cb below. Independent
workspace fmt/Clippy117 unit tests and frozen cargo-deny passed (advisory database
is the locally cached snapshot; duplicate dependencies are warnings).

Evidence root: `D:/WorkSpace/tmp/handwave-version-20261016/`. The isolated maintained
Windows Hub is SHA`83e922282c577feba768ff88146611d61238ef5f6380cd81b58d9c9d1bbe6cb0`.
Private model/process transcripts are not committed. Results below refer to
the initial5f candidate EXE, not to old installed-image trials; final cb receipts
are listed in the release closeout.

| Requested group | Executed evidence and boundary |
|---|---|
| Environment installation/deployment | `tools-release-0.2.0-audit.json`: fresh Python venv, self-authored local wheel/pip no-index install, real import/path, HTTP body, graceful shutdown/port closed;6 assertions. Third-party network install and Linux source installation are separate. |
| Tools | same report: problem_tools3 scenarios, Host22 assertions, Jobs9 assertions; search/read, Chinese long lines, Python repair, download, actual rustc compiler, stdout/stderr/exit7, cancellation/process tree, restart, Job lease/time/log limits. No desktop Computer Use API claim. |
| Long context and memory | `context-release/result.json`:15 groups in124.55s, including original sources, summary/restart/incremental coverage, pair closure and invalid/cancelled/exhausted summaries. |
| Reference memory | original reference ID/hash/version, explicit adoption and revocation tested. Actual model trial below proves post-compaction file recall and denial after revocation. Cloud scopes and session links have separate process regressions. |
| Task list | new Execute-only `task_update`, at most12/32KiB; done requires evidence.8 unit tests plus real tool-call fixture. It is a Run checklist, not a global project task database. |
| Goals | `modes-release/result.json`:5 groups; `long_goals-release/result.json`:4 groups, fresh granted slices, closed browser, root budgets, review revision, Jobs/logs and logical model identity. Goal steps remain the authoritative durable list. |
| Research/token efficiency | [Primary research and pinned implementations](../research-efficiency-20261016.md). `efficiency-release/result.json`:12 requests/12.517s, DeepSeek/OpenAI usage, unknown zeros, real checklist, stable wire system, dynamic carrier/raw separation. No measured savings percentage. |

The tools batch took48.447s plus13.746s for a fresh Jobs supplement. Durations are
observations, not a benchmark against another agent.

## Actual DeepSeek context/memory task

`context-live-release-v2/result.json` passed in44.549s,5 model Runs/12 upstream
reservations within15/240s. Verified actual current directory model
`deepseek-v4.1-flash`, protocol`openai-completions`, through read-only SSH to the
existing Gateway. All identity, approvals, model context and effects belong to
fresh local Hub/Node directories. Temporary approval was revoked afterwards.

The isolated directory narrows this model's declared context from1,000,000 to
262144 only for this fixture: two110KB reads then trigger real model summary.
This is not a full1M capacity test and does not change production declarations.
Real project rule/output marker, persisted summary, immutable source hash,
sourced candidate not autoaccepted, trusted adoption, exact recalled file and
reference denial after revocation all passed. Usage remains adapter-sourced,
not billing; no real cache-hit or savings claim is made.

## Retained failures and audit traps

- `environment-debug`: the first fixture terminated a Windows venv redirector
  while its child was still closing. The corrected fixture uses graceful server
  shutdown, verifies its own child exit and closed port; product runtime unchanged.
- `efficiency-debug`: exact dict comparison incorrectly rejected Host-added
  identity/timestamps. Core fields stay exact and Host identity has its own checks.
- First release Jobs batch: Python opening receipt.json raised PermissionError.
  Atomic publication is a plausible competing observation, not a proven OS root
  cause. Reader now retries only PermissionError/FileNotFound for≤1s, never corrupt
  JSON;9 assertions passed in a fresh directory. Four injected reader faults pass.
- `goal_faults-release`:6/7 passed; cloud/Job inventory case used the old system
  marker. The actual request already had the new labelled Host data and Job42
  artifact. Supplement and independent audit are recorded below.
- `cloud_snapshot-release`: the raw-history comparison is being checked against
  the segmented original journal. It is not relabelled as passed.
- `context-live-release`: real summary/candidate succeeded, but the model used an
  extra history search and exhausted3-call Run budget. New fixture makes the
  stop-after-candidate instruction explicit; the failed8 reservations stay recorded.
- `goal-live-release`/`goal-live-release-v2`: test AST inspection omitted safe
  builtins generated by the actual model. Original results and Job cancellation
  receipts are preserved; these attempts do not count as successful live Goals.

## Release gate

Candidate tests above do not prove production activation. Source package,
Windows ZIP, Linux source installer, scoped commits/pushes, independent final
review, installed Node replacement and Ray public download verification are
pending until their receipts are appended. Existing platform authentication,
device identity/state, Auto random behavior and unrelated services must remain
unchanged. Production authenticated-browser acceptance is separate from local
fixtures; no TOTP bypass or permission extension is allowed.

Deferred: desktop Computer Use, binary image transport, vector storage, arbitrary
third-party installer matrix, every Linux/ARM/musl/macOS target, automatic code
or Skill self-modification, offline sync, raw usage-presence provenance, billed
cache economics, and global cross-session Goal inventory. Memory/experience
promotion retains explicit sources and adoption/review.

## Final standalone closeout

Independent Codex `/root/context_modes_audit` accepts the bounded Windows
software point; [the pinned review](version-0.2.0-review.json) retains source
findings/resolutions,31 reviewed pins, artifact hashes and all acceptance gaps.
Independent tools review and cache/Go release review were performed by separate
Codex agents. Root inspected the material source hunks and actual result files.

Final cb676ce8 EXE fresh execution (none inferred from the dynamic5f candidate):

| Suite | Final result |
|---|---|
| Context |15 groups PASS,194.0s, including original-preserving restart and summary faults |
| Efficiency/checklist |5 groups/12 actual controlled SSE requests PASS,36.457s |
| Environment/tools/Host/Jobs/cloud/linked references |6 complete scripts PASS,68.882s wall time;6/3/22/9/5/4 respective assertions/scenarios |
| Plan/Goal modes |5 groups PASS |
| Leased long Goals |4 groups PASS, logical Auto and no extra routing model |
| Goal failure paths |7/7 PASS,135.037s |
| Actual DeepSeek context/memory |5 Runs/12 requests PASS,68.122s; actual summary, source, adoption, file recall and revoked-reference denial |
| Final EXE actual Goal |NOT PASSED: the test rejected a legitimate reference_read before Job start; no final-EXE real-Goal success is claimed |

The5f fixed-script real Goal completed4 calls/3 slices in29.644s, with verified
report/logs and awaiting_review state. That remains old-image evidence. No old
complete raw-source manifest was recovered, so source review plus the CRT-only
intent does not establish a full byte-for-byte image bridge. The final cb Goal
gap is explicit even though its controlled Goal/Job effects and separate actual
provider/context task pass. No further model retries were used to force a pass.

Linux x86_64 GNU: the exact26e8d678 source archive was fetched over a temporary
trusted loopback HTTPS fixture and installed into a fresh `/tmp` prefix using
the unchanged real installer and `cargo build --release --locked`. Real compile
and installation passed in136s; installed `--version` is0.2.0, binary SHA256
`9cc2bd445241996b237760452560bf5fa595b7c5f76ac83b84f769fad4e9a768`.
No system dependencies, service, device approval or Node process were installed
automatically. This is one Linux target, not all distributions. Original early
WSL environment/PATH failures are retained separately from this successful run.

Publication artifacts:

- EXE:4,922,880bytes,cb676ce8 full hash above.
- ZIP:3,617,213bytes,`d82735bb1cae580db5600ed5a61a8a88b86b20aff18ab34e9294ada87e83685a`,
  [download](https://118.25.103.226/ray/releases/handwave-node-windows-cb676ce8.zip).
- Source:438,694bytes,`26e8d6780498e121042973cd1fc12c668c3ee4999e23c8c7d62ac548fcd53de1`,
  [download](https://118.25.103.226/ray/releases/handwave-node-source-26e8d6780498e121.tar.gz).
- Installer remains immutable fe4b10bee6a73659; stable `/ray/install.sh` now pins
  this new source. Archived runtime files match all22 current sources exactly.
  Runtime bundles intentionally omit full evidence/research/build documentation;
  use this Git directory for those links and complete verification scripts.

Nine JavaScript suites, Go Ray tests and vet passed on a clean326b1ba export
with the reviewed allowlist. Ray binary
`0d2a8a3468e3871e1d09d797d17017bb1ba01c5bdd2934e2624aae4ce2a28223`
is running as PID256370. Binary-only deployment preserved all30 protected files,
all10 old packages and four other services' PIDs/hashes;12 download URLs,
login-page HTTP200 and unauthenticated application302/303 checks pass. This
does not prove a successful TOTP submission. Hub was not restarted. The preparation
receipt predates gofmt: final source_install.go SHA is
`b31b72f309038d5f250e8f852169be99e817c21e3ae4286635c141c6bccb8456`.

Installed Windows Node is PID90172, final cb image;7 native Sessions/17 durable
files, configuration, identity and startup command are unchanged. The exclusive
node.lock was excluded from content hashing. Initial preflight lock-open and a
short post-exit EXE-lock failure were preserved; after confirming no process,
old image hash and exact backup/state equality, replacement/start succeeded.
No automatic downgrade after possible journal migration was attempted.

Production status confirms the same device connected but **pending approval**;
there is no live device grant in the observed status. Existing approval data
was preserved, and no grant was created or extended by publication. This limits
production task usability until an authorized workspace controller approves it.
This is not a successful production chat acceptance. Browser automation timed
out, so production post-login visual interaction remains NOT RUN; no TOTP bypass.

Root verify_bootstrap was attempted and failed on generated ignored dist README
links (documentation omitted from runtime bundles) and unrelated root WIP. It
is not reported as PASS. Changed source documentation links and scoped diff
checks are validated separately; no shared Core ledger or unrelated WIP repaired.

Commit preparation normalized only the copied review JSON and the newly added
verify_version_live.py from CRLF to LF. The published immutable source archive
retains that fixture's original CRLF bytes; its LF-normalized bytes are identical
to the committed test, with no code/assertion changes. The22 packaged runtime
sources still match current Git source bytes exactly. Scoped staged whitespace
checks passed after this normalization. First delivery commits: Rust`edd10cf`
and Go`7490e37`; normal push/tag receipts are verified separately.

## User-authorized follow-up — 2026-10-16 08:45 +08:00

The user subsequently renewed this device for24h and completed the normal
browser login. A new [production Goal/Job trial](postlogin-production-20261016.md)
on the final cb standalone EXE passed4actual DeepSeek calls/2slices/one Job,
with report/log readback, original journal and UI criteria/evidence confirmed.
Goal remains awaiting_review. This closes the pending-grant, final-image actual
Goal and authenticated-browser workflow gaps for that bounded task; the earlier
failed fixtures and publication-time pending status remain historical evidence.
