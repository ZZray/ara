# Durable history, authorized Goal continuation and managed Jobs

Date: 2026-10-16. Scope: the independent HandWave Rust Host and its maintained
Go Hub/Ray integration. This does not advance fixed OMP/Core parity. Runtime
evidence is complete within the platform/version boundaries below; final point
audit and production activation are tracked separately.

## Requirements and source boundary

The user requested small standalone Rust execution with cloud messages, stronger
context, explicit Goal mode and long-running work. The [seven pinned upstream
sources](../knowledge/long-task-research.md) support separating original history,
model context, persistent Goal authority and process Jobs. They do not justify
unbounded retries, automatic permission extension or accepting the model's own
success claim.

Rust base: `0ddc39ef80607f6c404dc079f4a63fd3c56c0a80`; Go base:
`de27acc723da81c85aa74457729d2612a4653605`. Only `apps/handwave-node` belongs to
this Rust change. Other root OMP/Core WIP is excluded. The Go release excludes
pre-existing auth/browser/web WIP, including the original auth fields in hub.go.
The maintained build exports a fixed Git tree and verifies the checked-in eight
stable public web resources before embedding them. It never overlays dirty web
files or runtime credentials. The final executable source pins accompany the
private build receipts; no API keys, device identity or private state are included.

## Implemented contracts

- Original messages move to append segments with synced hash-bound head and
  per-Run indexes. The hot checkpoint remains bounded. A verified summary or
  revocation watermark is required before removing a closed prefix from hot RAM.
  `history_read`/`history_search` retrieve bounded original receipts; withdrawn
  ranges cannot re-enter model context. Pending/torn writes fence execution.
- Only the Node schedules a Goal. Hub authority binds controller/device/native
  Session/Goal, absolute expiry, root call/slice limits and optional Job permission.
  Each slice receives a fresh Run/grant; the first model/protocol/binding remains
  pinned. Summary calls share the root accounting. Unknown usage is not zero.
- Closing a page permits continuation only for an explicitly leased Goal.
  Ordinary Run cleanup is preserved. Stable command receipts distinguish pause,
  stop, expiry, Unknown, budget exhaustion and awaiting review. Acceptance checks
  the current revision and last settled Run. Cancellation fences survive restart.
- Managed Jobs use a hidden worker in the same EXE, stable operation IDs,
  persistent launch intent, bounded output and absolute lease. Waiting uses Host
  receipt observations, without asking the model repeatedly. A fresh Job directory
  exposes receipt identities only; log reads are new observations, never restored
  revoked cloud context.

See [runtime ownership and limits](../knowledge/long-task-runtime.md) for the
exact state and migration contract. Existing OS account authority still applies;
tool allowlists and cwd are not an OS sandbox.

## Executed evidence and version bridges

Private evidence root: `D:/WorkSpace/tmp/handwave-long-task-implementation-20261015`.
The following are actual receipts, not a claim that every platform or production
workflow passed:

| Check | Actual result and boundary |
|---|---|
| Windows units | 102 passed on release-v2-era source; `windows-tests-final.log` |
| Final Windows v4 | fmt/clippy passed;102 units passed,7.67s; EXE4,899,328bytes, SHA256 `a38da90708ca929780438d9e0c00c935a2018e71aa0ea1a99d254200968c7db1`; source manifest remained unchanged throughout verification |
| Dependency policy |`cargo deny --frozen check` passed advisories/bans/licenses/sources against the checked-in Node policy and cached advisory database; no database freshness/network-update claim; lockfile/dependencies unchanged |
| Linux full suite | 100 passed as `ara-verify`; source hashes match; 248.353s compile+tests, actual tests155.82s; `linux-v2/linux-result.json` |
| Goal fault processes | release-v2, 7/7 passed,104.770s: root limits/restart, close/return, active pause/tree cleanup/grant end/command replay, unrelated ordinary Run cleanup, cloud withdrawal/new Job log observation, abort restart fence and bye cleanup |
| Strengthened cancellation | release-v2,2/2 passed,31.896s; observe6.3s beyond the worker's delayed unsafe write |
| Final Goal faults | release-v4,7/7 passed,111.938s; includes the increasing canceled revision and matching Hub mirror before restart |
| Context processes | release-v2,15 groups passed,142.504s; actual raw originals, summary provenance, corrections/revocation, store faults, invalid summaries, cancellation and budgets |
| Experience processes | release-v2,5 groups passed,47.675s; current-Run receipts, explicit adoption, recall and revocation; no automatic Skill promotion |
| Windows managed worker | diagnostic-v3,9 process cases passed; output, parent exit, tree cancel, timeout, lease, disk quota and exit7 |
| Linux managed worker | final v3 normal binary `d18c0439…8155`,9 cases passed,19.014s as ara-verify; evidence copied before WSL shutdown |
| Final Linux v4 build |54.255s, exact source hashes match; normal binary `dcd7ac16ec7b39c04e87863622dc2de3e2004fae67567ed80b3a64e7fa4ac2b8`; only the platform-neutral idle mirror hunk differs from v3, not a fresh100-case execution |
| Linux v4 release P0 | Original180-round fixture PASS508.432s within600s;360bulk records/19,840,532bytes,20segments, peak hot checkpoint221,407bytes/4messages; complete restart/read/search/revocation/torn-tail chain |
| Windows v4 P0 combination | Two90+90 segments across native process restart PASS; same360bulk records/19,840,532bytes; all original recovery/retrieval/revocation/torn-tail assertions. First execution186.994s/200s, second156.724s/240s; not one continuous180-round/600s trial |
| Final logical Auto processes |4groups PASS on v4/maintained Hub: actual3slices/6calls, exact review/original receipt, Job2slices/3calls with no polling, preserved logical alias and zero route resolve; no physical random-distribution claim |
| Goal multi-slice | diagnostic-v3,3 groups passed;3slices/6calls, exclusive artifacts, original command-result replay and no model polling during Job wait |
| Earlier maintained Go tree | `0a56ddbd3dc50b0f5a8adc4e5a8b1c7a8aeed686`; HandWave/memorystore/Ray tests24.940s and vet1.634s passed; Linux Hub/Ray builds passed |
| Publication Go tree | `06787c89c8da95689f6f190e03a6da0045650d8d`; clean HandWave/memorystore/Ray tests19.358s, vet1.449s, Hub build0.930s/Ray build0.920s PASS; includes Auto repair and final embedded packages |

release-v2 EXE:4,897,280bytes, SHA256
`16229c37f41ec5c13e0cfb091d6f84ab2cb8b03109dacaf0d11fd6a1223f2b41`.
release-v3 EXE:4,897,280bytes, SHA256
`27b34a14a037749aaf5a68967119b84bd07430473d6e42de605172efdffb2f13`.
Final v4 ZIP3,605,260bytes, SHA256
`a4c4c9b518467daa12892de2bb58001f88adc990d81922ea893d3ee4fec6c8d9`;
source418,319bytes, SHA256
`68329b9ce15f2c42d4652504e918714d2c61ab7d7391ff74c1570734277cc34d`.
The608-file source manifest and ZIP allowlist exclude real node.conf, identity,
history and tokens. Installer script remains `fe4b10bee6a73659…` and old published
packages retain their original URLs/bytes. Artifacts are staged, activation below
must be verified before claiming installed/public delivery.
The linux-v2/v3 source hash manifests and tar comparison show only the
`job_start` tool description changed: scripts are stored privately and data paths
are relative to cwd, not `__file__`/`PSScriptRoot`. Runtime fault/context evidence
can bridge this explanatory change; real model behavior needs its own trial.
The idle-abort mirror fix passed the final v4 seven-group fault run (111.938s), including idle canceled revision and strict Hub mirror.

## Failures retained and open gates

The original Windows continuous180-round large-history trials did not complete
all assertions. One
180s run reached344messages/18.96MB; another reached157/180rounds and17.4MB.
Other runs failed Hub model-directory lookup with502 (37/180 in the300s v4 trial).
No product retry was added. Isolated actual-Node60/60 and catalog200/200 diagnostic
probes did not reproduce502 and are explicitly NOT_FUNCTIONAL. The next full
trial uses a core-equivalent diagnostic Hub to capture error types/errno without
tokens, URLs or bodies. A self-imposed180s fixture timeout is retained as a
performance observation, not silently relabeled a pass at a larger deadline.
The300s v5 run completed159/180rounds (17,525,786bytes) before its watchdog;
all160catalog/316model HTTP requests succeeded. Independent review approved one
600s functional budget (original script default900s, no user latency SLA), keeping
all180rounds/assertions and preserving the failures. That final trial instead
failed at round72 with502 after105.650s:71rounds completed, catalog and summary
succeeded, the next agent request did not reach its provider handler. Isolated
diagnostics found the transport wrapper loses its lower dial error chain through
`%v`; lower-level instrumentation is required before any product fix. No retry or
additional blind complete trial is justified. Isolated2000catalog and40mixed
GET/summary/agent cycles, then90actual-Node Runs, did not reproduce the lower
dial failure. Specific errno remains unknown; no production transport/retry
policy was changed.

P0 now has separate positive evidence. Linux release binary SHA256
`425de564a00ed0b7eb0ced99a158fed922d7c515a2e94c7fc29cc08c4de28d62`,
built from the exact v4 source, ran the unchanged fixture SHA256
`118dfbfe1298d5b7b7ced671569c650a6fc3b619490f7f071159963908b05ff7`
with maintained Linux Hub `9e6a4c7fad9cf069eedb40dda0277023d2d22496c76a9ec0f485e7e4cbe4bc8b`.
Its original180rounds and every tail phase passed; the bulk count is360,
with bounded hot projection. The first Linux debug attempt was explicitly
interrupted after25rounds when measured debug throughput could not fit the budget;
its exit130 and partial state remain, never a functional pass.

For Windows, independent review approved continuing the known-settled90-Run
diagnostic Session rather than blindly restarting180rounds. The entire original
trial was archived (119files/77.13MB) and every hash rechecked; the original
NOT_FUNCTIONAL result SHA256 remains
`e19692a0daad657761dda26860e03698c95ebe43245954da28e4680f5826cb17`.
Continuation retains the exact original state-path string/work/owner and normal
device approval, because context scope is bound to the Session path. A relocated
copy is not the same native identity. Before appending90new unique markers, the
new process proves the original metadata, raw hash chain and90Run receipts.
All original tail assertions then execute unchanged. Final offline audit proves
all372committed records and180unique bulk receipts, exact marker order, unchanged
first90byte prefixes and only the deliberately added20-byte unconfirmed suffix.
`D:/tmp/handwave-journal-combined-90plus90-20261016/delivery-review.json` accepts
only TWO_SEGMENT_WINDOWS_P0_SEMANTIC_COMBINATION; it does not rewrite any earlier
failure or establish a continuous-process performance/peak-memory benchmark.

Actual DeepSeek trials use `deepseek-v4.1-flash/openai-completions`, at most
12root calls/6slices/180seconds,2calls/60seconds per slice and210s watchdog:

1. v1 failed: script located input beside private `__file__`, producing real
   FileNotFoundError/exit1; scope detection also rejected `os`. Tool guidance was
   clarified without moving scripts or weakening the file/import scope.
2. v2 failed: after reading input the model listed cwd; the fixture had omitted
   that normal read-only operation from its allowed scope. Independent review
   allowed only canonical cwd listing, with seven offline scope checks. Original
   failure remains.
3. v3 ran3slices/6calls, generated sum42/count3, actually read stdout and report,
   and reached awaiting_review. Final assertion failed because Windows extended
   canonical path and ordinary path compared unequal. Independent offline audit
   independently reconstructed the complete native hash chain and successful
   tool/worker artifacts. It proves at least1.273s of model-free Job waiting and
   correlates6reservations/6distinct actual responses/6Hub HTTP attempts across
   three persisted Run IDs. Original private grant stdout is unrecoverable;
   Codex context_modes_audit accepted this bounded semantic equivalence and its
   v4 bridge after exact Rust source, LF-normalized Go gate source and actual
   executable SHA checks. The coordinator repeated those pins in
   `real-model-coordinator-provenance.json`. Original grant log bytes were not
   reconstructed. The original harness result remains FAIL; no model rerun.

Linux-v3 build and4managed-Job unit cases passed. Its first actual-worker command
failed because the exported snapshot lacked the test script; the corrected command
uses the existing reviewed test path and passed9actual-worker cases (persisted
receipt above). Linux clippy is UNAVAILABLE (component absent),
not a pass. Binary-only `cargo test --doc` exits101 because there is no library;
it is not a failed unit test and no doctest acceptance is claimed.

Independent Codex context_modes_audit: ACCEPT_BOUNDED_SOFTWARE_POINT. All 25 Rust paths, 17 Go paths, 23 runtime pins, 608 packaged files and final artifacts were checked. Publication tree: 06787c89c8da95689f6f190e03a6da0045650d8d. The scoped software point was subsequently published and activated below; authenticated browser task acceptance remains pending.

## Explicit limits

No power-loss proof on all filesystems,1M-token stress trial, precise token saving
benchmark, autonomous Skill/program mutation, desktop Computer Use, vector store,
ARM/musl/macOS execution acceptance or unlimited background operation is claimed.
Unix worker SIGKILL can leave descendants; such effects remain interrupted/unknown.
Legacy checkpoints with an unsummarized active tail over8MiB need inspection.
Journal v2 does not promise transparent downgrade to an old executable; preserve
the original backup and reconcile unknown effects before any rollback/replay.


## Publication and installed activation — 2026-10-16

Scoped commits were pushed: Rust `7d086c7d8a29c832ef6f8a38613279d8e0849151`
and Go `1f29b4ddb0693e7820e6d19ab548f9202cca14ac`. The Go commit differs
from tested prospective tree `06787c89c8da95689f6f190e03a6da0045650d8d`
only in the delivery evidence document. Existing Go authentication/browser WIP
remains unstaged; `hub.go` committed task-only blob remains
`787d30e43bcc3b688a3c55c36d7fcc7a9fdaff94`. Root Rust WIP is excluded.
The Git whitespace check explicitly accepts CRLF using `cr-at-eol`; no packaged
source files were rewritten to satisfy an LF-only whitespace default.

Binary-only cloud activation passed the deployment receipt: Hub SHA256
`9e6a4c7fad9cf069eedb40dda0277023d2d22496c76a9ec0f485e7e4cbe4bc8b`
and Ray SHA256
`353c675e49326b69d5445e7852a45471bf7bb2a5bfb04dad7243ed6cd49372dd`.
The actual running processes, Hub health/Bearer/unauthorized gates, eight stable
web assets, workspace-list read, normal Ray login and all six old/new package
hashes passed. Reference metadata/source assertions are conditional on existing
workspace/session rows; their receipt flags alone do not prove nonempty reference
integration. All30protected configuration/data files and the other three service
processes remained unchanged. Backups are private; no credentials are published.

The installed Windows Node was stopped only after confirming seven settled native
Sessions and no command children, then upgraded to SHA256
`a38da90708ca929780438d9e0c00c935a2018e71aa0ea1a99d254200968c7db1`
(4,899,328bytes). Actual running image, original startup arguments and unchanged
config/identity/start command passed. All17pre-existing native state files remain
byte-identical; the runtime-owned lock is excluded. No automatic downgrade of v2
Session data is attempted.

Two preflight failures are retained: empty Go device slice encodes as JSON null,
and the prior device approval had disappeared. Both occurred before binary
replacement. The private release helper now validates null as an empty device
list and preserves absent approval exactly; it does not create or extend access.
The successful deploy had zero online Nodes and zero Hub Gateway connections.

Actual browser reload reaches the normal Ray six-digit-code login gate. Current
approval storage is empty before and after deployment. User authentication and
fresh24hour device authorization were requested separately. Actual post-release
browser Goal/Job controls and task completion are **PENDING**, not accepted from
API/software fixtures. The page, screenshot and private receipts remain in
`D:/WorkSpace/tmp/handwave-long-task-implementation-20261015/deployment/`.

Independent Codex post-deployment audit: `ACCEPT_BOUNDED_DEPLOYMENT_AND_NODE_UPGRADE_EVIDENCE` (`post-deploy-audit.json`). This accepts deployment/installation evidence, while actual authenticated UI tasks remain pending.
