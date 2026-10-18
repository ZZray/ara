# Independent Codex POST review — environment libraries 0.2.7

Reviewer: `/root/environment_evolution_review`; procedures: ara-git-review,
ara-rust-core-review, point-delivery-audit. Date: 2026-10-18.

## Scope and current judgment

FINAL: SOURCE / CONTROLLED-HOST APPROVE. No remaining blocking source defect
found in the bounded environment-library implementation. The full autonomous
two-Session real-model point is NOT ACCEPTED: all four bounded attempts retained
FAIL, and the final attempt never admitted the second Session. No further model
requests were made by the reviewer or requested by this audit.

Repository `E:/repos/ara-github`, HEAD
`be0a3aa56a21ad50f2d4be13fb52bef163f33659`; target is the uncommitted independent
`apps/handwave-node` 0.2.7 v2 snapshot, not the root OMP worktree.
The reviewer recomputed all 17 entries in `source-pins-v2.json`: all matched.
The release image was independently hashed:

* `handwave-node-0.2.7-v2.exe`, 5,349,888 bytes
* SHA256 `81ec56dac455c8e20d7e2907a6f6767332bedb7748f73502a82732723c11eaf3`

Reviewed: environment_library.rs and its tests; skill_library.rs and its tests;
environment_tools.rs, environment_prompt.md, both embedded SKILL.md files;
agent.rs/config.rs/execution.rs/experience.rs/main.rs/transport.rs integration;
Cargo version-only changes; node.conf.example; environment knowledge/evidence
documentation; relevant actual Host fixture assertions.
local_tools.rs changes were inspected and identified as the preceding already
reviewed Web/checksum work, with no new environment execution primitive.
Web internals and its prior acceptance remain scoped to the separate review.
Root shared ARA Core, its unrelated dirty files, production services, credentials,
and deployment were excluded and unmodified by this reviewer.

## Resolved review findings

1. Project Skill retirement now uses `base/.retired` on the same filesystem;
   it no longer renames the project directory into a possibly different state
   volume. Discovery excludes the hidden retired directory and preserves files.
2. Skill discovery has one shared cross-scope 4 MiB/256-entry/1-second cooperative
   budget and explicit partial coverage. The returned directory contains metadata,
   not script bodies. Selected bundle reading remains independently bounded.
3. Knowledge reuse checks the current managed entry, indexed body hash and
   revocation state. Revoked, superseded and unpublished managed revisions are
   rejected for reusable read/search. Revoked IDs cannot be reactivated by a
   stale writer supplying the former content hash. Human pages remain unverified.
4. Compressing telemetry and final projection now insert all dynamic carriers
   in the same order, avoiding omission of environment data during Goal runs.
5. Skill installation requires an absolute source_dir. It cannot silently
   resolve a task-relative source in the agent process working directory.
6. Receipt association was honestly renamed `execution_evidence_attached`.
   Successful native exec/script receipts associated with an exact bundle hash
   do not automatically establish that the command exercised that bundle. The
   tools, prompt, embedded guides and durable documentation state this limit.

## Semantic checks

The directory roots come explicitly from the resolved runtime state and selected
working directory, not inferred Session filenames or a developer-only path.
Project selection follows the nearest Git marker or cwd. Explicit knowledge_dir
changes only the runtime-global knowledge location. Initialization preserves
existing INDEX files and does not run in Plan or identity/config resolution.
Initialization failures are disclosed without suppressing ordinary tasks.

Knowledge mutations lock the selected shared directory, preserve human index
bytes outside their managed segment, publish immutable note revisions and replace
the index only after validation. Cancellation/failure can retain an unpublished
revision; the old index is preserved and this boundary is explicit. Read paths,
file/output sizes, symlinks/reparse points/hardlinks, malformed manifests, stale
hashes, UTF-8 offsets and partial search coverage have executable cases.

Skill create/install do not run code. Bundles are staged, bounded, checked and
published without overwriting an existing name, preserving licenses/resources.
Windows handles are closed before directory publication. Changed resources
invalidate the receipt/hash association. Removal requires owned metadata and
the current content hash. The runtime remains an ordinary OS account, not an OS
sandbox; hostile same-account mutation and blocked OS filesystem calls are not
prevented by cooperative locks or deadlines.

Dynamic catalogs are provider-only user-data carriers, refreshed before calls;
they are not fixed system instructions, raw user history or summary input.
Their serialized overhead is reserved before compaction. Plan tool schemas and
the Host admission hook permit only read tools. Current Run evidence is resolved
from completed native call/result pairs, with no copied private output bodies.
The original failure/recovery experience validation and explicit private-memory
adoption contract remain unchanged. No Task/Goal acceptance or permission/grant
renewal is introduced by the new tools.

## Evidence independently inspected / rerun

Coordinator's final code checks were read directly:

* rust-tests-v2.log: 201 passed, 0 failed, 1 explicitly ignored public-network test.
* clippy-v2.log: successful warnings-as-errors check.
* deny-v2.log: advisories/bans/licenses/sources OK; warnings remain in the log.
* fmt-v2.log: empty; coordinator reports exit 0 (empty output alone does not prove
  an exit status).
* release-v2.log and the actual final-image SHA above.

Reviewer directly reran the delivered debug test image without Cargo rebuild:

`D:/WorkSpace/target/handwave-node/debug/deps/handwave_node-128c1fcf2b94c8e6.exe`

* environment_library::tests:: — 15 passed, 0 failed, 0 ignored; 0.08 s.
* skill_library::tests:: — 12 passed, 0 failed, 0 ignored; 0.34 s.
* experience::tests::library_observations — 1 passed, 0 failed; 0.00 s.

This independently exercises current note reuse/revocation/integrity, index
failure preservation, locks, actual Windows hardlink/junction rejection,
canonical paths, bundle creation/install/license retention, no implicit resource
execution, hash invalidation, retirement, limits and the preserved private
experience contract.

Actual release-image integration results inspected:

* host-release-v3/result.json: five scenarios PASS, 55.107 s, 32 controlled model
  requests, no fixture errors; image SHA matches. Fixture assertions inspect
  actual native calls, selected resource path, script marker artifact, both
  knowledge scopes, restart/new-Session reuse, retirement/revocation and Plan.
* host-regression-v2/result.json: 27/27 PASS, 77.879 s, 23 controlled requests,
  no fixture errors; same image SHA. Includes real writes, compilation, restart,
  replay rejection, process cancellation and proxy behavior.

These are controlled-provider Host effects, not a claim of real-model autonomy.
The v1 canonical-path product failure and v2 LF/CRLF fixture failure remain
documented with their original artifacts; successful v3 did not change the
product image or weaken byte-consistency expectations.

## Pending / excluded acceptance

Await bounded real-model two-Session trial, actual requested output inspection,
and recheck of final source pins. Linux/macOS, arbitrary system package installation,
unattended self-modification, personal-memory promotion, production deployment,
device grants, commit and push are not accepted by this local review.

### First real-model trial inspected, not accepted

`live-two-sessions-v1/result.json` and both trial terminal/event records were
inspected independently. They retain FAIL: create stopped at the sixth forwarded
call with `assistant stopped with length` and 1500 adapter output tokens; reuse
was refused on its eighth attempt by the fixture's 300000 conservative
UTF-8/output per-trial reservation (seven HTTP requests forwarded). Overall:
13 forwarded requests, 62.311 s, conservative reservations 530418. These are
explicit output/budget failures, not successful autonomous two-Session acceptance.
No model call or retry was made by the reviewer. Source and controlled-Host
approval remain distinct from this open real-model gate.

`live-two-sessions-v2` was also inspected: create exhausted its 16-call allowance
before a final response (71.19 s); reuse is explicitly NOT_RUN. Overall 16
forwarded calls, 76.478 s, conservative reservations 831961. The actual event
sequence contains skill creation, script execution, evidence attachment and both
knowledge writes, but effects alone do not make a failed Run a completed trial.
The repeated trial route requires reviewing task granularity and observed call
use before another attempt, rather than silently increasing budgets.

The coordinator subsequently traced v2's complete native chain and actual files:
the final receipt-checking/final-response round was blocked after the requested
effects had completed. Config's product default is 24 calls, whereas the trial
imposed 16. Reviewer agrees that one fresh isolated 20-call-per-trial / 28-call
aggregate attempt is a proportionate fixture correction after this causal
inspection, retaining 1M/1.5M conservative reservation and 180/300-second bounds.
No product code, provider policy, failed Session continuation or uncertain-effect
replay is needed. Its result remains pending; no general efficiency claim follows.

The third broad trial also remains FAIL: direct result inspection confirms 19
forwarded requests / 74.475 s total (69.926 s create) / 938037 conservative reservations, with the next request
refused by the 1M reservation cap. No PASS is inferred from its partial effects.
The reviewer supports stopping that budget-increase route. One selected smaller
integration task may instead validate project skill creation/execution/receipt,
project knowledge and a new-Session reuse, retaining bounded calls/time and all
three failures. Global knowledge has real deterministic Host effects but would
not thereby acquire actual-model autonomous-promotion acceptance.

The live fixture's absolute-directory substring check can falsely reject a
legitimate resource invocation relative to exec.cwd. A narrow fixture correction
must resolve the actual script invocation against the actual task cwd, correlate
the exact native call ID with a completed tool_end.step_id, inspect its input and
artifact, and preserve source/bundle hashes. Mentioning a path in JSON, a cwd or
an unrelated command is insufficient. Any offline correction must preserve the
original result and write a separate post-hoc validation receipt; no additional
model calls are needed solely to repair the observation.

## Final actual-model audit — fourth attempt

The reviewer directly inspected `live-project-two-sessions-v4/result.json`,
`create-result.json`, provider request history, original logs and actual filesystem
artifacts. The final release image hash still matches the approved image.

Result remains FAIL, not a harness-path false-negative: 16 forwarded model
requests, 87.817 s, 934592 conservative reservations; the next candidate request
would add 78043 and was refused by the 1M-per-trial gateway. The create Run has
terminal code -1 and a budget 429; reuse is explicitly NOT_RUN. Its failure
preceded path/report validation. It cannot establish a successful new-Session
autonomous reuse, final answer or task completion.

Independently confirmed partial effects:

* Original first.jsonl and second.jsonl hashes match their captured source hashes.
* first-summary.json contains valid=6, invalid=2, INFO=3, WARN=1, ERROR=2.
* An in-Run reuse-check/second-summary.json contains valid=7, invalid=3, INFO=2,
  WARN=2, ERROR=3. This is a same-Run self-test, NOT the missing new-Session trial.
* A real project skill contains SKILL.md, scripts/summarize_jsonl.py and two
  references. The reviewer independently computed its bundle hash as
  `7d36500fbe20eefa7b6ac244ba47dde7e4169dbd858b2b8fc2e52d58a3588eb0`, matching
  metadata's verified_hash.
* The referenced current-Run run_script calls were read directly. They actually
  invoke the installed script via PowerShell/python and Python subprocess using
  that bundle's resource, with actual resulting reports. Exact event IDs have
  completed tool receipts. A first misordered skill_verify request was correctly
  rejected (`selected tool IDs must follow native result order without
  duplicates`); the subsequent corrected request succeeded.
* A project technical note was written; its actual SHA256 is
  `a1368b9fa4f3d5aaad7b87d4a69748c1f4a54c52652bed9208b0873da033d7c3`, matching
  the immutable filename. No global actual-model claim follows from this selected
  project task; the result explicitly excludes that scope.

These observations show genuine partial model-driven tool effects, not a passing
complete trial. Native metadata authenticity does not make every generated test
assertion correct: for example, one generated self-test compared a source hash
to itself. Independent preserved-source and report checks above are the relevant
evidence. There is no measured speed/token/cost saving or general intelligence
improvement acceptance.

### Final disposition

Accept the source implementation and actual controlled-Host capability boundary
for this local Windows candidate; keep the complete autonomous two-Session point
OPEN / NOT ACCEPTED. Do not promote failed Runs, partial artifacts, adapter cache
counts or a completed tool receipt to successful user Task acceptance. Further
work should investigate excessive model/tool rounds and default context/schema
overhead with a separately bounded efficiency task; this audit does not authorize
another model attempt or product/provider redesign.

Production deployment, installed-device replacement, Linux/macOS execution,
arbitrary system package installation, private-memory automatic promotion,
commit and push remain outside this review. Root OMP delivery/marker state is
unchanged. The reviewer changed only this external audit report.

## Final delivery material check

The reviewer directly read the completed
`apps/handwave-node/docs/evidence/environment-library-0.2.7-20261018.md` and
sanitized `docs/evidence/receipts/environment-library-20261018/acceptance.json`.
Both explicitly keep full acceptance OPEN; the receipt state is
`local_implementation_tested_real_model_acceptance_open`. All four failed trials
and provider usage versus conservative reservations are retained separately.
The source implementation pins remained unchanged on the final reviewer check.
`evidence-secret-scan.json` records 164 JSON files checked and zero matches for
the actual Gateway credential; the reviewer read that receipt, without obtaining
or printing the credential. The reviewer subsequently directly read
`trial-process-cleanup.json`: remaining_count is 0 and processes is empty for
the scoped isolated trial Node/Hub/SSH helpers. This is a retained cleanup
observation, not a claim about unrelated processes or production services.
