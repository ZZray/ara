# WRITE remaining Unix22 — reviewed source, execution pending

Fixed OMP: `596f2da7101178214aa27a753529d15e6b7ad91d`.
Checkpoint: 2026-10-11 08:36 Asia/Taipei.

Formal inventory remains **3/111 accepted**. WRITE remains **174/196
observed (88.8%)**. This record adds no runtime or surface acceptance.

## Original scope and change

The remaining original observations are 16 non-root kernel permission cases,
three literal colon filenames and three shebang/chmod cases. All 22 IDs,
original spans/assertions, Rust consumers, owners and language bridges are
in packet `source-map22.tsv`.

Only `crates/ara-tools/tests/edit.rs` changes: the existing Unix helper now
checks original callback cardinality, W2 payload equality, W1/W2 raw EACCES,
and W3 original ENOENT plus recovered EACCES cause. Production code and the
two existing test entries retain their established owners. Root read the
actual narrow diff, fixed original assertions and relevant OS registry paths.

## Independent source review and actual application

Packet: `D:/workspace/env/cache/ara-port-candidates/write-unix22-closure-20261011`.
Independent Codex `/root/print_complete_scope` used ara-git-review plus
ara-rust-core-review and approved the frozen source scope. It read all 22
original bodies, existing consumers/owners and the runner; six original
packets match exact Git-show bytes. Its 1,056 guards have zero mismatches.

One draft runner defect was corrected: any presence of
`ARA_ARCHIVE_HOME_WITNESS_CWD`, including an empty value, must be rejected
because the test otherwise enters its separate early-return witness path.
The Windows rejection smoke exits 2 before compiling or creating output;
this proves refusal only. Python AST parsing passed.

Root applied the single test file with exact before/after guards. The index
hash is unchanged. All 1,039 effective build inputs were copied byte-for-byte
to packet `snapshot/` and checked against the frozen pins; copy/application
took 1.359 seconds. This independent snapshot preserves this batch while
later modules change the working tree. Review additionally confirmed the
112 literal Rust include targets are present in the pinned input set.

| Artifact or source | SHA256 |
| --- | --- |
| `manifest-final-v1.json` | `3fcf8962a0bb5569fdb60c908f18f7bbb966c58bff3bc3b44321c4d9147c1b67` |
| `source-review-v1.json` | `94a780667db1f521896118eb21d2498ed1c213c8716a46a8ec245bb6840e4e7b` |
| `SOURCE-POST-v1.md` | `a2cba9b437655116146d83225eab38e221ff11423c54a70d50c501cc565df36c` |
| `run_unix22.py` | `8dc68af6a0f791b5d3ce9c020f964b092596b981225023fb88cbc559018e1146` |
| E test before | `baec5f41ecf716fd6109a6585aa6631df0bb041e74a3001d8b938a97ceb2cebf` |
| E test after | `55035d088efc6669639d4174f754252b02c3df918fdc0082bef4db9b32541efd` |
| Git index before and after | `89d34e26be97c90a14680d44ace937e6b1ae5a86e8ee7b46dd6b90718a922475` |

Actual application receipt: packet `applied-and-snapshot-v1.json`.

## Required execution and acceptance

Use the frozen snapshot in native Linux storage with nonzero real/effective
UID. The runner verifies actual mode-bit denials, source pins, native Rust/ELF,
absent witness key, zero failed/ignored tests, kernel16 completion and the
six unchanged Unix assertions in the tools entry. It requests one combined
tools/edit Cargo no-run build, then runs those two existing exact entries.
DEV/TEST debug-information settings are recorded for the new Linux target;
their speed benefit is unmeasured.

Cargo and Unix execution are **unperformed**. Local WSL installation permission
is pending; no component installation or reboot follows from source approval.
Runtime evidence and independent execution audit must bind these 22 to the
existing 174 receipts before full WRITE acceptance. No Windows rebuild is
requested because this delta and the missing observations require Unix.

## 2026-10-11 21:44 preparation and execution checkpoint

The earlier installation-permission status above is historical. User has
authorized WSL on D and executed the administrator script. Its actual receipt
confirms MSI exit0,Microsoft signature Valid,component Enabled,restartRequired
false and rebootCalled false. Distro import exit0 registers ARA-Write-Unix as
WSL2 with BasePath/VHD on D; Root verifies the registration before launch.

Reviewed nonroot bootstrap actually completes in235s with Rust/Cargo1.97.1,
dedicated ara-verify UID/GID1000,and native /opt/ara-env cache/target/temp.
Receipt is `/opt/ara-env/logs/preparation-20261011T132447Z-578.json` in the
D-backed distro. This is tool preparation,not WRITE acceptance.

Root stages all1039 frozen inputs byte-for-byte into native storage,8.853s;
source-pins SHA256
`857d12d478755ec21efe17383e923a13c7b780e94d3f0cb3e70cbdde71bc35c4`.
The unchanged runner begins native nonroot execution with fresh output
`/opt/ara-env/logs/write-unix22-v1`,build timeout3600s/test timeout180s.
Filesystem and real mode-bit denial prerequisites pass before Cargo.
First crates.io index population takes about13minutes before downloads begin;
no compilation/test PASS is claimed at this checkpoint. A proposed stop-only
guard observes downloads have started and refuses to terminate Cargo.

Root also compares this historical snapshot with current E: later changes
include ara-tools edit.rs/edit_post_apply.rs/edit_stream.rs and other inputs.
Therefore its eventual execution is not automatically current-v21 acceptance.
A current pinned snapshot is being prepared for affected incremental execution
using the same Linux cache. All old receipts/failures remain retained; formal
WRITE stays174/196 and overall5/111 until execution and independent audit.

[Current handoff](../handoffs/2026-10-11-write-closure.md).

## 2026-10-11 22:12 current-input Unix22 execution

The historical1039-input first attempt ends FAIL exit-15 after1552.337s;
the guarded termination confirms the exact Cargo PID/cwd and no compiler
outputs. It was still acquiring dependencies. No original ID executes in
that attempt, and its logs remain `/opt/ara-env/logs/write-unix22-v1`.

To avoid repeated acquisition,116 existing Windows Cargo registry archives
(58,606,973 bytes) and420 index files are seeded into the D-backed native
registry in20.201s, with archive checksums verified against current Cargo.lock.
No credentials, Windows toolchain or Windows target are copied. Receipt:
`/opt/ara-env/logs/registry-seed-v1.json`. The successful build is offline.

Current-v2 freezes1749 current inputs but omits the runner-required upstream
lock; preflight refuses in0.091s without Cargo. Current-v3 adds only that
unchanged verification lock, retaining runner/map/six fixed-original packets.
Native snapshot `/opt/ara-env/workspace/write-unix22-current-v3/snapshot`
has1750 source pins SHA256
`eaca8e671224bc5393b62250cfb20adf0e0391b6f0f0276cda54e9be110374a8`.
Current manifest SHA256
`55c847983581155cd3f6f5a02485ed777b9c44edecd349bc6150b4a96a576aef`.
Current staging takes4.381s. Previous snapshots and failed receipts remain.

The unchanged runner executes on native ext4 with UID/EUID1000. Real create,
unlink and lstat denials each return EACCES13. Existing tools/edit test entries
each report1 passed,0 failed/ignored; edit stderr confirms
`WRITE_KERNEL16_EXECUTED: uid=1000 observations=16`. All22 original IDs in
result.json execute. Build130.308s; edit0.315s; tools9.086s; total143.099s.
Root checks all24 command stdout/stderr hashes against the actual files.

Actual logs: `\\wsl.localhost\ARA-Write-Unix\opt\ara-env\logs\write-unix22-current-v3`.
Result state is EXECUTED_22_PENDING_INDEPENDENT_AUDIT, not full WRITE acceptance.
Independent Codex P is reviewing the22 runtime observations, current source
bridge and existing174 evidence against complete WRITE196. No Cargo rerun
or full-workspace gate is requested by this audit.

ELF edit SHA256 `06a50d568406a7ae4af2a1663a708a643ee93454f052ba64e99e8ccc48f9d464`;
tools SHA256 `df44d5c8df971c5221e745497c10132df04aca6b2ead35a0292e48316717ee2a`.
DEV/TEST debug0 and jobs2 are recorded. Download avoidance is measured;
different inputs/platform/profile do not prove a general compile multiplier.

## Independent execution and complete-source decision

Independent Codex `/root/print_complete_scope` approves Unix22 runtime and
exact original-ID accounting: Conflict93 + Windows59 + xdev22 + Unix22 =196,
zero missing/duplicate IDs. It checks113 selected current E/D/native source
relationships,24 command outputs, both ELF hashes and exact tests/marker.
Prior174 observations are explicitly reused through reviewed source impact;
they are not relabeled as a new whole-surface execution on raw-identical input.
Root reads the complete audit and verifies its hash before recording this.

Report `D:/workspace/env/cache/ara-port-candidates/write-unix22-current-v3/WRITE196-RUNTIME-SOURCE-AUDIT-P-v1.md`,
SHA256 `4710ad7297c93732942b9dcaa0feaceb0f6aaff14e917866e5681ac986755a7e`;
machine receipt SHA256
`6fa45baf746445f5cb98cb971c9b73b282dfae682be9b495d5b604a591525a0b`.

Complete fixed-source CA-TOOL-WRITE remains CHANGES REQUESTED/implementing.
Two remaining gates are identified from fixed write.ts, not new product scope:

1. Lines995-1043: per-file bulk success alongside a different file's actual
   splice failure. Current Rust branch exists but its original93 tests leave
   that combination unexecuted. Prepare one same-binary controlled Host run,
   including a file with an earlier in-memory splice followed by locate-miss,
   and inspect disk/history/aggregate receipts. No Cargo or CAS is required.
2. Lines1160-1168 and1637: special `xd://report_issue` plain-text dispatch and
   dedicated call preview. Current generic Native DeviceWrite requires normal
   mounted-member JSON/schema input and cannot stand in for that special owner.
   Approval-tier recognition alone does not connect this behavior. The owner
   is in report-tool-issue.ts (CA-TOOL-MISC); port this exact dependency with
   Host consent/persistence/background network ownership before acceptance.

Original observations are196/196; complete surface count remains5/111,
delta0. WSL/admin/user action is complete and no restart is needed. The
historical CAS SQLite effect/terminal failure, snapshot drift and failed
acquisition/preflight attempts remain preserved. No full product is accepted.
# Supplemental bulk execution closure — 2026-10-11 22:43 Asia/Taipei

The earlier two-gate complete-source review remains history. The bulk gate
now has independently approved runtime evidence from the unchanged normal v21
binary. Runner `D:/workspace/env/cache/ara-port-candidates/write-bulk-mixed-host-candidate-v1-20261011/verify_mixed_bulk_conflict_host.py`
SHA256 `938217dd512503e0d5dd627ded5c675c2c8601c002c5a3cba3bfa2fa5ad87f93`;
run receipt `D:/workspace/env/cache/ara-port-candidates/write-bulk-mixed-host-run-v1-20261011-ROOT/receipt.json`
SHA256 `10f722fd98128257c53969bdd282331a1c8d81575ed491d5334e5cede0c1717c`.
One Native CLI process takes3.123s with6 controlled fake requests,0 CAS,
exit0 and9 exact start/end/persisted tool receipts (6 successes/3 expected errors).
A persists chosen theirs and invalidates its resolution ID. B keeps external
bytes and both unresolved IDs after lower scratch success then upper locate
failure. Root verifies actual disk/journal/output; independent Codex P EXEC
APPROVE `WRITE-BULK-MIXED-INDEPENDENT-EXEC-P-v1.json` SHA256
`946511ab437d428f3811279dd8cebb6843800f9942324865a1e87e3030396a74`.
No new original ID, no fresh196-wide execution and no full WRITE acceptance.
The remaining confirmed blocker is dedicated report_issue Host dispatch/call
rendering, now in D-only implementation after independent B PRE approval.

