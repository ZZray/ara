# Production Extension lifecycle checkpoint

Fixed OMP: `596f2da7101178214aa27a753529d15e6b7ad91d`.
This checkpoint exercises the actual Bun carrier, Rust extension bridge and
original Native Promise driver. Host action bodies are controlled inputs;
the extension does not import the SDK. It does not accept the complete
Extension surface, SDK, CLI/RPC binding or the full OMP marker.

## Snapshot and execution

Packet: `D:/workspace/env/cache/ara-port-candidates/extension-production-batch-20261009`.
All production changes remain in its candidate/assembly, not the live checkout.
The v8 assembly includes the Windows accepted-socket repair, early Bind failure
case and current-driver Host reply queues. It excludes subsequent mutable
SDK and RPC actor changes.

Build command:

```text
cargo test --locked -p ara-cli --test extensions_host_lifecycle
  --test append_only_context_host --test host_bridges
  --no-run --all-features --message-format=json
```

Run command:

```powershell
& 'D:/workspace/env/cache/ara-port-candidates/extension-production-batch-20261009/run-lifecycle-module.ps1' -Revision v8
```

Environment uses Bun 1.4.0 and the existing bundled extension package.
Cargo home, Rustup, target and temporary artifacts remain on D. The runner
uses one sequential module cycle with bounded process timeouts; it preserves
stdout, stderr, source manifest and executable hashes per revision.

| Observation | Result | Elapsed |
| --- | --- | --- |
| v7 selected executable build | PASS, 0 compiler errors | 516.0505042 s |
| v7 actual carrier bootstrap | FAIL, Winsock 10035 / WouldBlock | 1.1632205 s |
| v8 repaired selected build | PASS, 0 compiler errors; 410 reused / 7 built | 403.5817808 s |
| v8 actual lifecycle group | PASS, 1 test | 19.3136759 s |
| v8 append-only context Host group | PASS, 6 tests | 0.6264906 s |
| v8 existing Host bridges group | PASS, 15 tests | 46.5847909 s |

The three v8 groups pass **22/22, zero ignored**, module wall 67.3068684 s.
Their tests cover different boundaries; the count is not an OMP completion
percentage. The original Windows failure remains preserved. The accepted
stream inherited nonblocking mode from its listener; the repaired transport
explicitly makes that stream blocking before its bounded bootstrap read.

## Actual lifecycle observations

- Real guest dispatches `(100, 101, 102, 104) = (1, 2, 3, 2)`.
- Two original lifecycle drivers reuse the prepared modules, with four
  actual Native Promise reactions and expected entries
  `probe:one`, `probe:two`, `shutdown`, `shutdown`.
- Expired first-driver admission and a foreign parent are rejected.
- Dropping a close completion waiter while an active lease blocks disposal
  does not replay shutdown; the original driver finishes the same close.
- A controlled `bind_sdk` error after provisional resolver retention leaves
  the original owner Failed, retires the exact route and keeps the carrier
  healthy, without an additional guest Bind dispatch.
- Assertions require no Unknown effects and no pending work. The actual Bun
  exit receipt records `killed=false`, `success=true`, joined pipe readers.

## Receipts and limits

- `early-check-v7-receipt.json`, preserved original `module-extensions_host_lifecycle.*`
  and `lifecycle-artifacts/production-lifecycle-PBo3P8/carrier/exit.json`.
- `early-check-v8-receipt.json`; v8 manifest SHA256
  `e0c962acca6f513fe1d46c21822c0a3bb15df2ee729bf418d8b1d7e0f7acd70f`.
- `module-v8-receipt.json` and its three per-group logs/receipts.
- `lifecycle-artifacts-v8/production-lifecycle-cykx3e/acceptance.json`,
  original operations journal, carrier frame trace and exit receipt.
- Independent reviewer `/root/extension_integration_review` reads the exact
  SOURCE and execution receipts in `ROOT-SOURCE-REVIEW.md`, SHA256
  `d56b8708797e77d156d213dddd78852b9ce7696118dca322a34a7e034585ae80`.
  It independently rehashed the actual executables and counted the original
  journal dispatches; the bounded v8 POST receipts and artifacts match.
  Latest B/Root/C source was reviewed separately and is not v8 execution.

The review found that an unentered Host request could still enter after queue
revocation. The subsequent B candidate seals that atomic admission; this fix
is **not in v8**, and SOURCE review is not execution of the fix. v8 does not
invoke `setSessionName` through that queue. These Host methods, complete SDK,
whole-driver drop/fallback races, actual CLI/RPC extension tasks and real-model
Extension trials remain unaccepted. No API key or local account configuration
is included here.

Formal totals remain **0/111 complete, 1 tested, 27 implementing, 83 open**.
No commit, push, deployment or live installation change was performed.

## Successor composition diagnostics

The same packet then combines B's original graph actor and retirement fix,
Root's actual SDK Bind port and read-only native capture edges, and C's
prepared logger/prompt plus sixteen utility exports. This successor remains
candidate code; it is not substituted for the executed v8 snapshot.

`cargo check --locked -p ara-cli --lib --bin ara --all-features --message-format=json`
initially failed in v9 after 115.9794089 s with one E0277: `NodeId` does not
implement `Ord`. The only v10 source replacement changes the visited-node
membership set to `HashSet<NodeId>`; this set is not iterated, so roots, queue
traversal and deterministic capture names retain their original order.

v10 passes after **108.6996688 s**, zero errors, 412 reused / two checked,
manifest SHA256 `984ddf710e57212b5ba2ffb3a78642228d945763ef470ba6c64c228816762cc8`.
Original v9 failure and both source manifests/receipts remain in the packet.
This is library/binary typecheck evidence, with zero tests executed. It does
not execute the latest Host mutation/retirement fix or SDK materialization.
The reviewed utility root remains Partial and all 1,347 fixed SDK inventory
entries retain their acceptance requirements.

## Actual SDK import diagnostics, v11-v13

The same packet adds the original Native Map clone owner, Session SDK root,
ambient primitives and current Host read/tool owners. These are candidate
implementations, with complete SDK and Host parents still open.

| Revision | Observation | Elapsed |
| --- | --- | --- |
| v11 | Selected original four-group build FAIL, E0308 in one Arc borrow | 353.8822495 s |
| v12 | Same build PASS, 415 reused / 3 built; no compiler errors | 162.5229587 s |
| v12 | Actual lifecycle FAIL before factory registration | 2.6878644 s |
| v13 | Original lifecycle fixture adds import-error artifact; build PASS | 14.7937805 s |
| v13 | Actual lifecycle FAIL; original SDK import error preserved | 1.5679806 s |

The v13 artifact `lifecycle-artifacts-v13/production-lifecycle-evzOdT/module-import.json`
records `SDK namespace access requires an admitted invocation`, two SDK
namespaces and 226 shared nodes. Materialization occurred, but the Bun plugin
callback lacks the invocation context required to load/enumerate the actual
SDK namespace. The loader preserves the failed import rather than registering
the factory. This is not a passing SDK or Map observation; those assertions
were not reached. Original failures, killed carrier receipts and joined
readers remain preserved. The other three groups stopped before execution.

The v12 manifest SHA256 is
`3227ebf8e35838c92adf2de7b741bd45236fc455d7d03f872957b1d09d9cd0b7`;
v13 is `4faa8cb2c69550f96d0ba21e0dc39767a17b86e2853b75c65a7bea7b0f90767b`.
The actual v13 lifecycle executable SHA256 is
`709077893dfbfba583d46c3acc5679a1f3c55f129398fb74b706047acae84f19`.
The independent reviewer rechecked both failures in the existing
`ROOT-SOURCE-REVIEW.md`, SHA256
`f17e700b64d4df114e84677aff514ceef6c38e4722342082d1499b20fae16deb`.

The next repair is confined to runtime import context ownership. A TypeScript
delta may inherit this exact executable with a new runtime source manifest
and revision; it will not be labeled a new Rust build. No production source
has been applied to the live working tree by this packet. Formal complete
count remains 0/111.

## Runtime-only repair and original module execution, v14-v17

The existing runner now accepts an explicit inherited build revision and a
new runtime source manifest. It checks all runtime source pins and selected
executable hashes before the run; it does not manufacture a new build receipt.
The v13 lifecycle executable stayed exactly
`709077893dfbfba583d46c3acc5679a1f3c55f129398fb74b706047acae84f19`.

| Revision | Original group / observation | Result | Process elapsed |
| --- | --- | --- | --- |
| v14 | Exact import tag/namespace context repair | FAIL, before Bind | 1.1524893 s |
| v15 | Same original group with temporary import-catch stack logging | FAIL, exact stack captured | 1.0529362 s |
| v16 | Actual selected SDK / lifecycle, stack diagnostic removed | PASS, 1/1 | 26.7540742 s |
| v17 | Append-only context Host | PASS, 6/6 | 0.7094366 s |
| v17 | Existing Host bridges | PASS, 15/15 | 44.6772185 s |
| v17 | Original value heap group | PASS, 4/4 | 0.1053554 s |

All four affected original groups pass **26/26, zero ignored**, combined group
process time **72.2460847 s**. No Cargo invocation was needed for v14-v17.
The v17 groups inherit their unchanged v12 executables and production Rust;
v13 changed only the separate lifecycle fixture. v16 and v17 pin the same
283 runtime source rows. Build ancestry and runtime manifests remain distinct:
v16 runtime SHA256 `9c9355f076f7532cef03cfc1e7dad306f71d98bf9371f8a106146e71657d9faf`;
v17 runtime SHA256 `c592be24a0c8e64ac47026a8c482c2b63982f1f8dc960cc7fe04c3beb839a6cf`.

The v15 original stack locates `worker.current -> hostQueryValues -> hostQuery
-> sdkCheck(extension.ts:8:17) -> extension.ts:31`: the first top-level
`formatDuration` call lacked its original invocation. The repair confines
context restoration to the active import tag, actual registered source path
and original Frame; it preserves native export aliases. The module resource
enters this context before its body and restores its previous context at EOF.
Import finally retires the exact tag and graph path owners. No latest-frame
fallback or relaxed Native admission was introduced. The temporary loader
stack diagnostic is absent from v16/v17; all earlier failed artifacts remain.

`lifecycle-artifacts-v16/production-lifecycle-Ef1EXH/acceptance.json` records:

- Two actual SDK namespaces, 226 shared nodes, sixteen utility exports exercised
  at import, factory and both original Session probes.
- Fresh branded Native Map clone, root/map cycles, aliases, internal slot
  insertion, no overridden `.set` Get, and actual getter order twice
  `key,key-leaf,value`.
- Original dispatch counts `(100,101,102,104)=(1,2,3,2)`, four Native reactions,
  original entries `probe:one`, `probe:two`, `shutdown`, `shutdown`.
- Original caller-drop close completion, expired/foreign admission rejection,
  zero Unknown/unsettled records, and actual carrier exit
  `killed=false`, `success=true`, joined readers.

Independent reviewer `/root/extension_integration_review` directly rehashed
all four executables and both runtime manifests, compared the runtime rows,
and checked the original 1,074-row journal and actual artifacts in the same
`ROOT-SOURCE-REVIEW.md`. Its bounded POST matches this representative
source/execution checkpoint; it does not accept the complete Extension or
the unexercised B9/35/P2 failure case.
Final review report SHA256:
`2dfccb2752550df992e04debd79b54a07bfc2d7411ad995e8b32e53e5fef856c`.

Still OPEN: failed/TLA evaluation may skip EOF restoration; retiring the
import token does not prove cleanup of the Bun resource or its children.
Failure followed by another owner on that resource, globalThis lexical
shadowing/language-domain fidelity, full lazy imports, concurrent shared
Windows graphs, CJS and cache/reload behavior require their original complete
requirements. Existing expired/foreign Session assertions do not cover these
evaluation-resource failures. Full 1,347 SDK inventory, all 35 Host methods,
main/RPC integration and representative real-model Extension task remain OPEN.
No additional real-model helper trial was run, no live source was applied,
and formal complete-parent count remains **0/111**.

## Composed send source and evaluation recovery, v18-v21

The v18 snapshot combined the three actual send/next-turn owner files and
the two original evaluation-recovery files. The selected four-target build
failed in 415.084243 s with one E0382 in the lifecycle fixture: `drive(self)`
consumed `first` while the async body still referenced `first.effects()`.
No tests ran on v18. Its manifest SHA256 is
`ca4e3c6bc5a89ffd1fe4a0baa7f1c9ff43dab59c57fe59dd08f7ea930fbff79b`.

Two fixture-only Arc clone changes produced v19. The same selected build
passed in 62.5901849 s (415 reused artifacts, three compiled targets). Build
manifest SHA256 is
`436712bb7caffddf703a438cfc96318cd7f4c014cf2ccee6d239d7ee4deb18a2`.
The original lifecycle executable
`3902f9d71056336849cfb51772c6dfcda8b47506971f2ca827b5d13c3b3a449c`
failed in 5.7856046 s: the second Import100 returned
`Failed to load extension: foreign module generation value` before the
expected `ara-evaluation-throw` marker. Artifacts are retained under
`lifecycle-artifacts-v19/production-lifecycle-LX4D9b`. The original runner
stopped at this failure; this is not successful evaluation recovery.

The remaining original groups then ran as v20 on the same v19 build, with
all 283 runtime sources and exact executable hashes pinned in
`runtime-v20-manifest.json` (SHA256
`ddd2436b8a93b7655c2794ddb6507c332b67645b7b570e943cf86bb4f35e8857`).
Append-only context passes 6/6 in 2.1791795 s, Host bridges pass 15/15 in
46.3082684 s, and value heap passes 4/4 in 0.0798088 s: **25/25, zero ignored,
48.5672567 s**. No second Cargo run was used. These groups do not exercise
the new actual Extension send consumer or close all 35 Host actions.

One v21 runtime-only diagnostic retained the same lifecycle executable and
re-threw the original import error after logging its stack. It failed in
4.3380831 s. The stack is `worker.opaque -> hostQueryValues -> hostQuery ->
Array.from -> synthesizeBundledModuleSourceFromModules`, not the requested
throwing user module body. Original stderr and receipts remain under
`lifecycle-artifacts-v21/production-lifecycle-nO04rO`; no Cargo or real-model
cycle was added. The diagnostic is temporary, pending a source-backed repair.

Independent reviewer `/root/extension_integration_review` verified v18's
compiler error, v19's exact executable and actual failure artifacts, and the
final user-intent source delta in `ROOT-SOURCE-REVIEW.md` (SHA256
`587550c77e23d50650e423222f02c7c39acd59fd2b584b0632e93e7f4bd110d2`).
Evaluation recovery is not accepted. The next original driver persistence
dependency group is candidate implementation, not reviewed execution.
Complete SDK, actual Host actions, main/RPC integration and full Extension
remain OPEN; no live source was applied and the formal complete count is 0/111.

## Composed original driver and canonical evaluation, v22-v25

The v22 composed original driver/persistence and canonical Symbol source group
failed its selected build in **471.561603 s** with three fixture compile errors.
No tests ran. The v23 repair only supplied the original Session initializer's
Arc journal and extension read state and explicit AgentMessage channel types.
It passed in **228.1799196 s**, with 415 reused and four compiled artifacts.
The v23 build manifest is
`81aabc6121b50b5aa31776cd3a43d71fb1a2fd164c791dba9edd76dc58b32b18`.

On that build, the v24 original four groups pass **26/26, zero ignored**:
lifecycle 1/1 in 35.2127037 s, append-only context 6/6 in 2.2600361 s,
Host bridges 15/15 in 47.8360027 s and value heap 4/4 in 0.1101057 s.
The group total is **85.4188482 s**, without another Cargo run. Runtime source
and executable identity are pinned in `runtime-v23-manifest.json`, SHA256
`0234ebc62c8da35e3e062ee07c288a2eaed44b4e702d4ad8a0e05ef3720f4cee`.

The original recovery artifact at
`lifecycle-artifacts-v24/production-lifecycle-n3agdJ` records the precise
throw and TLA rejection markers and successful subsequent module owners on
carrier `15153457731430473516`. Its original carrier exit records
`killed=false`, `success=true`, and `pipe_readers_joined=true`. Failed-module
child-effect cleanup and full loader coverage are not established by those
observations. Independent reviewer `/root/extension_integration_review`
confirmed the selected source/build/runtime/executable identities and actual
artifacts in the same `ROOT-SOURCE-REVIEW.md`, at checkpoint SHA256
`985ca2252558806578a81a65d9a6d188aa77f4b0fd20cff4e03de2ef29d28831`.

The additional exact original Host send test failed on v23 at Bind in
**1.8870819 s**. It did not execute send. A tests-only v25 repair uses the
original SdkRegistry materialization and PreparedNativeSession session ports,
preserving the strict production namespace identity requirement. Only the
affected `ara` bin test was rebuilt: **132.3284333 s**, 411 reused artifacts,
one compiled artifact, zero errors. Its build manifest is
`1ad983aced580b72ab3bc95a6e6bf6b6facfc422c4f882e4ffda34cda1fd7437`.

The exact v25 test selected one test and failed in **26.5512372 s** while the
actor awaited its original native request at `rpc_host.rs:8127`. Bind completed,
but send/driver/persistence behavior is not accepted. The actual artifact is
`lifecycle-artifacts-v25/native-host-send-T9uoYc`; its carrier was killed during
failure cleanup (`success=false`, readers joined). Original operation/wire
receipts are retained for diagnosis; do not replay an unknown effect or extend
the timeout as a repair. The unaffected v24 four-group receipts remain valid.
The runtime manifest is
`3747a38d96b916491815694c7a928bf23e77c30ff07d48d7021b4a19f0911b47`.

Complete SDK, all 35 Host actions, main/RPC integration, a representative
real-model Extension task and the complete original Extension parent remain
OPEN. No production source was applied to the live checkout; formal complete
count remains **0/111**.

## v26 composed build and original runtime groups — 2026-10-09

The seven-file actual Host/catalog and loader composition builds successfully
in **543.7158328 s**, with 410 reused artifacts and zero compiler errors.
Build manifest: `f0452860040d08874a9a0867ddea1fee68c2d9d3f7de63148ef30e2d9ec5075d`.
The runtime manifest pins 283 runtime files and the four actual executables:
`runtime-v26-manifest.json`, SHA256
`abd38d3b4d4604d6d34ed7a7489f123861c1896ac8be294e5b67be64bdb1b101`.

Executed results on that composition:

| Original group | Runtime revision | Actual result | Seconds |
| --- | --- | --- | --- |
| Exact `rpc_host::tests::actual_extension_send_native_host_coarse` | v26 | FAIL, 0 passed / 1 failed; exact selection matched | 7.787515 |
| `extensions_host_lifecycle` | v27, same v26 build | FAIL, 0 passed / 1 failed | 20.4828241 |
| `append_only_context_host` | v28, same v26 build | PASS, 6 passed / 0 failed / 0 ignored | 0.79718 |
| `host_bridges` | v28, same v26 build | PASS, 15 passed / 0 failed / 0 ignored | 46.3743309 |

The Host artifact `lifecycle-artifacts-v26/native-host-send-HMR30V` reaches
the actual handler, but fails the retained physical ReplyJob assertion:
`rpc_host.rs:8164`, observed pending count 0, expected 1. Its journal has only
the Session/header records; no successful actual send persistence is claimed.
The lifecycle artifact `lifecycle-artifacts-v27/production-lifecycle-DmXYhm`
records evaluation recovery, rejected retired-child effects and pre-Go
cancellation, then fails at `extensions_host_lifecycle.rs:794` with the first
actual Native driver timeout. These are failing complete-group receipts;
intermediate artifacts do not turn either group into a pass.

The existing runner stops a group sequence on failure. Separate runtime
revisions execute the remaining independent groups with the explicit same-build
manifest, without another Cargo invocation. Unaffected v24 value-heap evidence
remains retained. No full workspace/model cycle is added at this diagnostic
checkpoint. Independent final runtime acceptance remains pending.

The next `appendEntry` candidate is excluded from this build: its queued
recording does not satisfy fixed upstream synchronous read-after-call behavior.
Implementation must record in the same live Session records owner before
returning void and expose that owner through actual SessionManager reads.
The original Extension parent and **0/111** formal complete count remain open.

## v31 build and v31/v32 original failures — 2026-10-09

The selected original Host/lifecycle repair build passed in **493.6610862 s**,
with 410 reused artifacts, 6 checked artifacts and no compiler errors. Build
manifest: `3ebd18b39926ef8346194db15300d4370d4230add1fcf2c4e4d01a6a0176a2c1`.
Runtime manifest pins 283 runtime sources and the two actual test executables:
`dc6935904ab120979c326c65a39dda8a954a3f59eb2d7d02a9465a649654b636`.

- v31 exact original Host send: FAIL 0/1 in 8.1849309 s. The retained
  physical ReplyJob assertion remains observed 0 / expected 1. Artifact
  `lifecycle-artifacts-v31/native-host-send-h7J2xp/idle-send-before-unlock.json`
  records physical_pending=0, name_reads=0, frames=[]; it does not identify a
  successful send or an explicit error notice.
- v32 original lifecycle, same v31 executable: FAIL 0/1 in 20.3590476 s.
  Artifact `lifecycle-artifacts-v32/production-lifecycle-t1Ouhr` fails at
  line 795 with Host effects 117 and 97 outcome_unknown. The async AWAIT220
  repair has not established a passing lifecycle.

Independent source/artifact diagnosis is pending. No timeout/oracle change,
extra Cargo invocation, live source application or acceptance is claimed.
The complete surface total remains **0/111**. Full AGT-APPEND-CTX closure also
retains an explicit actual ArkType callable materialization gap; a projected
JSON schema cannot substitute for that original behavior.

## v33 coherent composition and build failure — 2026-10-09

The candidate composition contains 46 frozen files for the synchronous journal
writer and shared records, Graph SessionManager reads, original-tool append
normalization, Native driver membership wake and original owned-reader test
migration. It remains isolated from the live source checkout. Composition
manifest `early-typecheck-v33-manifest.json` SHA256 is
`5b7eedbbf62497f714de0b1172399d3f86e99ffd460334bb1af2380eff8383ef`.

The single selected executable build on this composition failed with exit 101
in **111.8685407 s**, reusing 404 artifacts and compiling one artifact. Seven
compiler errors remain in the directly affected Session code: six stale
`leaf`/`ids`/`entries` field uses in `model_usage.rs` and one `Entry` versus
`&Entry` comparison in `lib.rs`. Original tests were not executed. Exact command,
diagnostics and timings are retained in `early-check-v33-receipt.json` and
`early-check-v33-errors.txt` in the current candidate packet.

The independent reviewer also identified three stale inline reduction-test
path field accesses before that test target compiled. The author is collecting
these migration repairs together, preserving the original failure injection
and assertions. Neither composition nor this failed build accepts a behavior.
The complete surface total remains **0/111**.

## v34-v37 integration and build-input diagnosis — 2026-10-09

### Follow-up: cumulative time and automatic freshness preparation

The terminal v39 receipt records 176.8229785 s, 424 reused artifacts,
five compiled artifacts and two compiler errors in the original
`ara-session/tests/local_reduction.rs` owned-reader callers. No tests ran.
After accounting for that receipt once, tracked Cargo time is
6,164.8181909 s (102.7469698 min); executable test time remains
444.9347704 s (7.4155795 min). These are cumulative attempts, not one
full rebuild. The completed build handle has been cleared.

The existing selected build runner now invokes the reviewed source freshness
guard automatically before Cargo and records the guard receipt path/hash in
its build receipt. Guard failure prevents Cargo from starting. The original
package/target selection, features, profile and two jobs remain unchanged.
The PowerShell parser reports zero errors and a comparison confirms unchanged
Cargo arguments/environment; this tooling change performed zero Cargo runs.
Its measured time saving remains unverified until the next necessary build.

The next integration pass must repair related owned-reader callers together
before another build. Existing module executables and unchanged evidence are
reused; broad regression remains at stable integration checkpoints. Changes
to jobs, debug information, linker or crate boundaries are deferred until
timings justify a controlled comparison, because each can invalidate caches.

These are later checkpoints; historical counts above remain historical. Formal
inventory is now **1/111 accepted** because AGT-APPEND-CTX was independently
accepted separately, not because this Extension candidate passed.

| Build | Actual result | Seconds | Reused / compiled | Executed tests |
| --- | --- | ---: | --- | ---: |
| v34 | FAIL: one original retry-recovery temporary borrow | 327.7226 | 405 / 17 | 0 |
| v35 | FAIL: five owned-reader caller errors and incremental compiler ICE | 323.4299687 | 412 / 1 | 0 |
| v36 | Interrupted; no terminal receipt or known exit code | unknown | 411 / 0 observed | 0 |
| v37 | FAIL: two original `journal.rs` temporary borrows | 272.2348022 | 396 / 22 | 0 |

v36's four-file/five-hunk owned-reader patch passed independent SOURCE review
and was applied to assembly only. Actual CRLF files are line-for-line equal to
the reviewed LF candidates; their actual hashes are in the 48-path freeze
`early-typecheck-v36-manifest.json`, SHA256
`948616a8610fe74fd5a3a7b52a2dffc4808bca958da6c739523c2d7f52e2526f`.
The interrupted log and missing process/handle observation are preserved in
`early-check-v36-observation.json`; it is not reported as a successful or
completed failed build.

### Stale local artifacts from a different source root

The same target was used by live and assembly with copied source mtimes. v36
declared assembly's Session artifact `fresh=true`, but its existing rlib and
dep-info came from live. The compiler diagnostic places `SessionJournal` at
line 1739, matching live; assembly has `SessionJournalRecords` at 2018 and
`SessionJournal` at 2179. The old dep-info lacks `journal_writer.rs`. Similarly,
assembly's Tools has `EditFileFallbackUnsettled` at line 93 while live does not,
and the old reused metadata falsely reports the type missing. Required
Session/encoder methods exist in assembly. Independent Codex confirmed these
facts: they are cache/input errors, not missing source APIs.

The source-hash guard advances mtimes of different inputs without changing
source bytes, deleting external artifacts or accepting behavior. Its initial
prepared-input receipt pins 1,000 inputs. Two bin-only incremental cache groups
were moved to a verified target-local quarantine; dependencies and the accepted
Append executable/test artifacts are preserved. The target must retain a stable
owner, or every source-root transition must use the same guard; bypassing it can
reintroduce the stale-artifact problem. A prepared snapshot does not prove a
successful build. These are local candidate controls, not a product change.

v37 uses the same 48 frozen source bytes, manifest SHA256
`b137325eb9a88a220740217d91a301b946e6a6edffb78cb34bddef5f3433da23`.
It now actually compiles Session/Tools/CLI (`fresh=false`), and Session dep-info
contains `journal_writer.rs`. The false missing-API diagnostics disappear.
Two real original-test lifetime migrations remain at `journal.rs:222,1120`;
their assertions and expectations are retained for the next source correction.

### Measured cost and initial guard correction

v37 adds `--timings`, preserving the existing features/profile/two build jobs.
Its report is retained as
`v37-cargo-timing-20261009T031703252Z-f3b249721f47d5bd.html` in the packet.
The CLI library costs 138.32 s: frontend 85.32 s and codegen 53.00 s. AI library
costs 58.43 s and the selected Agent handoff test 40.54 s. These parallel task
durations are not summed as wall time. This failed build does not measure final
CLI binary/test linking, nor prove that a new profile is faster.

Independent guard review found that its initial pin-domain expansion treated
145 unchanged vendor files as changes, causing avoidable local vendored builds.
The original guard is archived as `prepare-source-owned-build-v37.py`; the
current guard seeds equal rows from an explicit previous source root instead
of equating missing pins with changes. The already-running build was allowed
to finish; this one-time cost is not hidden or presented as an optimization win.
No full dependency clean, optional real-model cycle or acceptance advance was
performed. Original Session/Host/lifecycle execution remains pending.

## v40 original module execution and v41-v42 corrective source

Packet: `extension-production-batch-20261009`, same fixed upstream and candidate
assembly. v40 manifest SHA256
`7b78bd63e54da712eddc0a599e361f9d71a93a3ac45da7055900df64855b7711`.
Its selected build passed in 47.7347066 s (431 reused/four compiled).
The original 21 groups ran in 88.2375948 s: 118 passed/five failed/zero ignored.
The five failing groups are Host append RPC, Session projected compaction,
remote compaction, retry recovery and native Host send. The production
Extension lifecycle group passed 1/1 in 35.1834273 s. All original assertions
and timeouts remain; this does not accept the complete Extension parent.

Independent reviewer `/root/extension_integration_review` directly checked
all 21 executable hashes and 283 runtime source hashes: no mismatch. The
Session failures share two projection leaf reads from an empty post-reset
model suffix and native custom metadata skipping the reserved marker's
no-payload rule. The reviewed candidate preserves shared records, model
suffix selection and opaque generic metadata, restores original strict error
classification, and obtains leaf identity from the full validated raw branch.
It changes only four hunks in Session lib; original negative tests remain.

Host send artifacts show the original current queue count changes from one
to zero before journal release, while name reads and frames remain zero.
This rules out a missing enqueue or a different current queue; the actor
outcome still requires observation. The existing RPC timeout did not record
its waiting stage. Two reviewed diagnostic changes retain the original tests:
test-only actor Ready/Driver/Error classification, and RPC timeout callsite
plus at most eight observed frames. They change no predicate or budget.

v41 compilation failed in 16.5132126 s (412 reused/zero completed new artifacts,
one error/zero tests): `WireValue` implements Drop and its String control must
be borrowed. v42 applies only `ref custom_type` to that candidate. Its 49 pins
were independently checked, manifest SHA256
`f6bae228579295c803ac4c0345febd3ce2b6d7dfff7345a6b5ffe847ac99aa7c`.
The final v42 source review is APPROVE; actual build/runtime is still required.
Current review report SHA256
`7f8346f4b33fa77a750d868c0f6049a84358bf78572f5541ae401e17ca31f382`.

## v42 terminal execution and build cost

The preceding SOURCE approval preceded execution. The selected build is now
PASS: 402.7438062 s, 412 reused/23 compiled, zero errors. The original 21 groups
ran in 91.61152 s: 120 passed/three failed/zero ignored. Projected compaction
19/19, remote compaction 4/4 and retry recovery 4/4 pass. The remaining failures
are Host first-turn cancellation, native Host physical ReplyJob disappearance,
and the second lifecycle bind/retained close's ExpiredCapability. Retain the
current lifecycle failure alongside the historical v40 pass.

The original Session inline bank passes 6/6 in 1.0464502 s. Its separate
no-run build costs 64.9578599 s (106 reused/66 compiled); the narrower package
selection creates dependency feature variants even with no changed source.
Receipts are `early-check-v42-receipt.json`, `module-v42-receipt.json`,
`session-inline-check-v42-receipt.json` and
`session-inline-module-v42-receipt.json` in the same packet. Independent Codex
reviewer `/root/extension_integration_review` checked all 21 executable hashes,
283 runtime source hashes and the inline executable hash without mismatch.
No live application or additional feature acceptance is claimed.

The original Cargo timing artifact records CLI library 180.6 s (frontend
100.5/codegen 80.1), production ara binary 104.8 s, ara binary test 90.5 s,
and Session library 23.8 s. Parallel unit times must not be summed as wall
time; binary durations are not measurements of linker time alone. The
source-hash freshness guard is automatic in `build-composed-v37.ps1` and
preserves unchanged artifacts. Stable feature selections and coherent source
batches avoid additional variants and repeated downstream builds; a four-job
trial is planned but has no measured speed claim yet.

## v43/v44 terminal results and D-only successor

The v43 selected build passed in 647.223669 s, with 412 reused and 23 compiled
artifacts. Four build jobs were used for this trial; observed memory pressure
included four concurrent MSVC links and less than 0.4 GiB available physical
memory. The trial did not demonstrate a speed improvement. The shared runner
is restored to two jobs, retaining its target, features, profile and automatic
source-hash freshness guard. No full cache clean was performed.

The original 21 groups on D passed 121 tests and failed two, with zero ignored,
in 64.9582039 s (`module-v43-receipt.json`). Host append RPC now passes 6/6:
the reviewed first-turn fix permits the existing lazy journal before physical
materialization, while retaining failed-rewrite and strict compaction barriers.
Native Host send failed with
`Protocol("Native effect notice has no admitted graph job")`, cancel=false.
The lifecycle group exceeded its unchanged 20-second deadline.

The already completed v44 comparison used the same v43 executables and source,
with only the runtime artifact root on E. It passed 122 tests and failed one
in 29.9350323 s (`module-v44-receipt.json`). The lifecycle executable
`a37eaaefb23d524513709f27c9dfefd31c6d30cb98fa168f2c02107bf9543807`
took 20.6489003 s and failed on D, versus 1.3038897 s and PASS on E. Native
Host send executable
`2101a85332d144364ed035db27f0a546f88094ffd1445596485a6da987e0ed78`
failed on both volumes with the same original effect-routing error. Independent
Codex reviewer `/root/extension_integration_review` rechecked all 21 executable
and 283 runtime-source hashes. This comparison preserves the D failure and
does not accept the complete Extension surface.

The user subsequently cancelled the proposed E target migration because of
capacity. No target migration started. Preserve the existing comparison
receipts; future Cargo caches, target and temporary execution artifacts stay
on D. No further E artifact comparison or cache allocation is scheduled.

The exact route repair has independent PRE/SOURCE APPROVE. In the original
effect owner, reservations now distinguish builtin Graph notices from physical
receipt-only operations. `OperationTicket::mirror` uses the latter; its original
receipt, join, cancellation, Unknown and wake paths remain. Receipt-only
operations cannot publish Graph updates, and unknown Graph jobs still fail.
This two-file repair is applied only to the D assembly and awaits execution.

The independently reviewed twelve-file tooling successor (freeze
`1ccea3755f0a8bd999ca6ebabb12fde0c136988e86c154a433125b116d97784e`)
is merged into that newer assembly through narrow patch hunks. The receipt-only
route and v43 Host diagnostics survive; actual resulting hashes are recorded in
`tooling12-root-merge-receipt.json`. It is not compiled or accepted. Production
tooling/SDK binding and native exec termination candidates continue separately,
to form one coherent successor before the next selected build. Full Extension
parents, strict35 production owners and SDK remainder remain open. Formal
inventory remains 1/111 accepted.

## v45 coherent build and original D execution

The route/tooling/native-exec successor is now compiled on D with two jobs.
Its 65-pin manifest SHA256 is
`c1a52976dbe66d9bb1d61b21fb474998776e0331025167e4f193772fefeb35fc`.
Independent Codex SOURCE review rehashed all pins and the four-file exec merge
without mismatch. `early-check-v45-receipt.json` records PASS in 575.818932 s,
424 reused/11 compiled artifacts and zero errors. Different changed inputs
prevent treating the difference from v43 as a controlled concurrency benchmark.

The unchanged original 21 groups on D ran in 115.0935595 s: 122 passed,
one failed, zero ignored (`module-v45-receipt.json`). Host append RPC passes
6/6. D lifecycle passes 1/1 in 35.4042409 s with its original per-operation
budgets, preserving both the earlier D failure and this new pass. All future
temporary artifact roots remain D; no additional E comparison was performed.

Native Host send progressed beyond the former unadmitted Graph-job notice
and missing ReplyJob: the original actor reports Driver and Ready, and the
idle-send/nextTurn assertions proceed. The complete coarse group still FAILS
in 44.8218208 s at `rpc_host.rs:8456`,
`original synchronous custom owner stalled: Elapsed(())`. The actual journal
contains `custom-normal`; operation receipts also show the file-open failure
scenario proceeds. The timeout belongs to `host-coarse-custom-file-rewrite`,
not the first normal append. Its final nested operation receipt is returned with
cancelled=true, and its parent operation is Unsettled with
`guest_terminal_uncertainty_after_go`. Preserve this unknown terminal boundary;
do not replay the entered operation or declare the whole Host group repaired.

The operation journal contains 265 rows for custom-normal, 20 for the file-open
failure and 665 for the rewrite scenario. These are diagnostic counts, not a
measurement of which IO or callback caused the timeout. No deadline, assertion,
durable barrier or original fixture was weakened. Runtime manifest SHA256 is
`f644f2526742336e07e964f4e106bfa7a70a153d28d9b7d119b423c5a00f650c`.
Production tooling activation and remaining strict35 owners are isolated
successors; this cycle does not validate their not-yet-integrated behavior.
Independent Codex reviewer `/root/extension_integration_review` checked all
65 source, 283 runtime-source and 21 executable hashes without mismatch, and
matched all group summaries to their receipts: zero outer timeout or selection
miss. The reviewer confirmed the rewrite-stage uncertainty and retained the
complete Host-group FAIL. Formal accepted inventory stays 1/111.

## Reviewed admission-pair source successor

Root implemented the approved one-file change in an isolated D candidate and
merged it into D assembly after independent Codex
`/root/extension_integration_review` SOURCE APPROVE. Before SHA256 is
`a3cb8180bbe15b0717af323e71231f199197336b531df3938c85a0b2c81e9a71`;
after SHA256 is
`a96b72c690d5448c35cc8dea6835740c7a4da826171121e8e4464421c7496b0c`.
`admission-pair-root-merge-receipt.json` records the exact application.

The original ticket guard still covers admission and the actual wire writer.
FileReceiptJournal writes both ordered metadata rows under its existing mutex,
then performs one flush/sync_all before writer entry. The default custom-journal
method retains both record calls and original ordinal faults. Shared sync
failure retains Writer sequence n+2 before NotStarted n+3; sequence exhaustion
keeps the original sequential path. Single records, WriterFinished, terminal
barriers, cancellation, mirror and Unknown/no-replay are unchanged.

Rustfmt and scoped diff whitespace checks completed. No Cargo/runtime was run
for this helper; the successor awaits the coherent production/control module
freeze and original affected process/module execution. No speed gain or repair
of the retained rewrite failure is claimed. Formal acceptance stays 1/111.
