# Edit engine upgrade to b91db66e — 2026-10-15

Status as of 2026-10-18: **ACCEPTED complete CRATE-PI-EDIT at b91db66e**.
Independent Codex `/root/edit_recovery_review` final EXEC36f8e673 APPROVE and
POINT-AUDITa367d2f9 ACCEPT_COMPLETE_POINT close the complete engine and its
declared direct dependencies. Root rechecks current1206sources/77paths/
14Windowsimages, immutable receipts and unchanged index before writing
`ROOT-ACCEPTANCE.json` in the recovery packet below. Original baseline remains
7/111; complete upgraded modules are Mock, Append, Retry, Snapcompact and
CRATE-PI-EDIT=5. Full CA-TOOL-EDIT and product gates remain OPEN.

## Current recovery — 2026-10-18

The original Windows batch is terminal FAILED, not still compiling. Its actual
1454.048s build reused501/compiled18 artifacts; execution23.173s produced
159 successful functions and3 failed functions. Two image functions lacked
existing Bun bindings; one snapshot function had a stale raw terminal-LF
expectation. The summary and ACP filters also missed their actual namespaces;
the driver correctly refused to count those execution groups as complete.

`runtime-windows-reused-v2` then passed5 functions in11.159s on the same images
without Cargo: two actual Bun codec functions, two summary functions and one
ACP plan function. Exact Bun1.4.0 and2555 private Host source pins were checked
before/after. No browser, new download or model task was run.

Recovery packet:
`D:/workspace/env/cache/ara-port-candidates/omp-upgrade-edit-engine-recovery-review-20261018`.
Independent `/root/edit_recovery_review` traced the remaining snapshot failure
to line222, the raw assertion. Ordinary read at line221 had already passed and
must still exclude the terminal LF. The earlier two-LF v6 candidate is rejected
and was never adopted. V7 changes only the raw expectation, preserving every
other byte; production READ and all other source files remain unchanged.

Guarded adoption preserves current index
`5fdd85af453d826620be5cd3ddaddcab6d5c4d708ed6dee0a62409222100043c`.
The older89d34e26 index is a historical checkpoint, not a required current Git
state. Current HEAD isbe0a3aa; changes outside Core are not imported into this
point. Root verified all1206 source pins before recovery and only the single
snapshot-test source differs afterward. Unique implementation/adaptation paths
are now77, with exact delivered hashes in `ROOT-FINAL-EXECUTION.json`.

`runtime-windows-snapshot-v3` selects only ara-tools/model_edit_snapshot and
the previously failed function. It passes1/1,0failed/ignored in0.079s. Build
231.995s reused446/compiled6 artifacts; this was not a one-artifact rebuild.
The full applicable Windows execution union is165 distinct successful functions:
159 earlier successes +5 prerequisite/filter repairs +1 final raw fixture.
Root checked raw log hashes,1206 current source pins,77 delivered paths and
14 current Windows executable hashes. Windows165 is not165 fresh final-image
executions. Vendor415 and Unix Host18 remain explicit source/dependency bridges;
the changed snapshot test is outside those native test closures.

Cumulative execution across retained failed/successful batches:8 Cargo builds,
1961.648s compilation and41.719s test execution. These are process times across
multiple dates, not wall time, idle time or a whole-workspace result. Final
independent EXEC and complete-point audit now pass with no remaining point
blockers. Full CA-TOOL-EDIT/READ/AST, normal CLI product, real-model and release
gates remain OPEN. The accepted execution includes explicit historical source
bridges; it is not a claim that every test reran on the final image.

The checkpoint sections below retain the original October15 state and failures;
pending Windows statements there are superseded by this recovery section.

## Version and scope

Fixed old `596f2da7101178214aa27a753529d15e6b7ad91d` to target
`b91db66e9b0b65a2c977e6a5bc74b0f4b1d58d1c`. This point is CRATE-PI-EDIT
plus its necessary direct AST/Host consumers, not complete CA-TOOL-EDIT,
READ, WRITE, dynamic grammar, terminal presentation or product acceptance.
The ported-through marker remains null. No model call, normal ara.exe rebuild,
commit, push, installation or deployment is claimed.

Root packet: `D:/workspace/env/cache/ara-port-candidates/omp-upgrade-edit-engine-root-v1`.
Sibling author packets end `vendor-v1` and `host-v1`; independent Codex
`/root/snapcompact_pre` reports are in sibling `pre-v1`.

- Exact old/current/new vendor map:169 paths,56 upstream changes,113 unchanged.
  New target358 test identities:353 source token-equivalent bodies and5
  explicitly declared ARA test-body adaptations. Original32 removed/renamed test dispositions
  retain explicit replacement/change records rather than asserting equivalence.
- New engine behavior includes URL answers/aliases, strict UTF8 staging,
  updated patch/hashline/sloppy, snapshots/recovery and path policies.
- Necessary AST dependency: statement-sequence/climb boundaries and9 original
  new tests; existing static Lua registry is retained. Full AST is not accepted.
- Host supplies explicit editable-file grants, aliases, selectors, immutable
  and plan scope. Preview resolves outside the Session lock; apply discards
  preview answers and reauthorizes source/move targets. Late preview cannot
  replace apply authority. Unknown write effects are not replayed.
- Initial73 adoption paths plus3 new caller-repair paths give76 unique paths.
  `COMPOSED-ADOPTED.json`, repair `ADOPT-LIST.json` files and
  `CURRENT-CHECKPOINT.json` retain exact paths, before/after bytes and backups.
  Existing staged/unstaged/untracked work is preserved; index stays89d34e26.

## Executed batches

Each build selects existing affected targets, uses locked/offline dependencies
and jobs1, and records source pins, actual images, logs, reuse and exit status.
Windows and Linux Cargo are serialized on the D-backed storage. Native tests
run as UID/EUID1000 on ext4 in the existing ARA-Write-Unix environment.

| Packet | Build seconds | Execution |
| --- | ---: | --- |
| vendor-v1 |21.070| Compile failed at upstream unstable Option::map_or_default;0 tests |
| vendor-v2 |55.558;213 reused/16 compiled|359 edit/diff plus56 AST tests PASS,3.708s |
| unix-host-v1 |69.278;438 reused/9 compiled| Compile failed at3 streaming fields/2 ToolError Display calls;0 tests |
| unix-host-v2 |52.462;449 reused/2 compiled|18 functions:14 PASS/4 FAIL,2.919s |
| unix-host-v3 |40.740;450 reused/1 compiled| Only4 failed functions:3 PASS/1 FAIL,0.566s |
| unix-host-v4 |36.497| Only final failed function:1 PASS,0.115s |

Native totals so far:275.605s compilation and7.308s test execution, including
failures. These are process timings, not total wall time, review time or a
whole-workspace result. Successful vendor415 and Host14+3 are retained by
explicit unchanged source/helper/dependency correspondence; final1 is fresh.
Do not describe all18 Host functions as executed on the final image.

The target358 mapping observes357 on Linux. Sole Windows identity
`path_policy::tests::strips_verbatim_prefix_from_windows_paths` awaits Windows.
Two vendor functions are local carries. Twelve hashline fixture files contain
304 named cases/541 calls exercised by original parent loops, not individually
reported stdout tests or native TypeScript execution. Real FIFO and original
Unix executable-bit file operations execute in the native batches.

Windows plan `windows-host-plan.json` c5a477ef selects13 execution groups in
one incremental build. It includes Windows path policies/portable cwd,
edit/post-apply, READ/WRITE consumers, snapshots/URI/SQLite, actual CLI Graph
and bin-test ACP/RPC. Root exec14132 owns `runtime-windows-host-v1`; observe
its existing receipt before starting another build. Its result is pending.

## Failure analysis and source reviews

- Source capture initially included an existing nested build cache. Archive
  hardlinks failed regular-file validation before Cargo. V2 excludes precisely
  that cache without deleting it:1750 to1206 source files,253MB to55MB archive.
- Stable Rust adaptation uses equivalent `map_or_else(Vec::new, ...)`.
  Independent source POSTb421bcea approves it before vendor-v2.
- Caller repair supplies streaming=false for complete READ contexts and uses
  ToolError.0. The same sixth test caller was found before rebuilding.
  Source POST00349a17 approves five files/six hunks.
- Four old Host composite fixtures assumed previous READ/router contracts.
  Source POST5310d0a9 approves exact terminal-LF/CR/BOM output, an explicit
  one-line selector preserving the50KiB budget, and traversal refusal wording.
- V3 exposes one final stale invalid-UTF8 display assertion hidden by the
  earlier failing assertion. Whole-function reinspection confirms it is last.
  Fixed READ decodes after a bounded binary sniff, records the whole decoded
  snapshot and prints a hash header/N: rows. Source POST2e041aa6 approves
  v5's exact expected header derived from fixture bytes, retaining every row,
  original invalid bytes and1996-line continuation. V4 executes that function.
- No production READ change was made to fit these fixtures. Display tags do
  not imply strict UTF8 EDIT staging acceptance. Raw CR/BOM display versus
  normalized edit addressing, including seen-line default policy, remains a
  separate full READ/EditTool boundary. Initial reviews missed these fixture
  migrations; all failed receipts remain intact.

Vendor SOURCE07889857, Host SOURCEb3d4e67f and vendor EXECd51a22ca approve
their declared scopes. Unix EXECv3 `2623397f` correctly requests changes for
3PASS/1FAIL. Final Unix EXECv4 `3d11b11b` approves the18-function union with
explicit17 source bridges and unchanged vendor415. Root independently checks
all1206 Windows/native/current pins in `ROOT-SOURCE-CORRESPONDENCE.json`;
only five Host files differ from vendor-v2, outside the vendor dependency
closure. Windows EXEC and complete-point audit remain pending. No source review
or passing narrow fixture accepts the point.
