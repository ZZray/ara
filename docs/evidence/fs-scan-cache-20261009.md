# Shared filesystem scan cache — bounded execution

Fixed OMP: `596f2da7101178214aa27a753529d15e6b7ad91d`.
Worktree HEAD: `20aa0e2ef9872f633ba2671376cdf778477e7115`; dirty `dev`.
Git index SHA256: `89d34e26be97c90a14680d44ace937e6b1ae5a86e8ee7b46dd6b90718a922475`.

Status: bounded implementation, selected module execution, actual CAS task and
final independent POST approved. **Formal complete-parent increase: 0.**

## Requirement and scope

Port a process-shared raw owned-entry scan cache, explicit cached glob
consumption and successful WRITE/EDIT/delete/rename invalidation through the
existing Host owner. Original behavior ID `B-566ed432f8` and its nine walker
dependencies are retained; no new IDs or runner were created.

Source: fixed `crates/pi-walker/src/cache.rs`,
`crates/pi-walker/src/lib.rs:1083-1122,1181-1191`,
`crates/pi-natives/src/glob.rs:190-208,289-294`,
`crates/pi-natives/src/iofs.rs:72-85`,
`packages/coding-agent/src/tools/fs-cache-invalidation.ts:1-28`, and the fixed
EDIT/WRITE successful-mutation callers. Exact copied walker sources and hashes
are in the candidate packet's `WALK-FIXED-SOURCE.json`.

Six delivered paths: `ara-walk/src/{lib,cache}.rs`, `ara-tools/src/engine.rs`,
`ara-cli/src/host_edit_session.rs`,
`ara-cli/src/native_observations/host_await.rs`, and `ara-tools/tests/edit.rs`,
all under `crates/`. Existing uncached Glob/Grep consumers remain unchanged.
Cache defaults are TTL1000ms, empty recheck200ms and max16; the empty threshold
applies after pattern filtering. Typed canonical PathBuf keys preserve Windows
case-sensitive roots; deleted operands use canonical-parent resolution.
ARA's Mutex/epoch owner fences invalidated in-flight scans without adding a
dependency. `walk_workers()` is configuration only, not a completed pool.

## Delivered packets and repair

Candidate: `D:/workspace/env/cache/ara-port-candidates/fs-scan-cache-batch-20261008`.
Preparation measured **1061.784 s**; initial scoped formatting **0.554 s**.
Candidate manifest SHA256:
`518d83b0bd930532b6a9361aa1a0982f141a394a6b8bec81b69c156951a12e78`.

Initial execution-v1 retains FAIL101 after **416.197 s**: the observation's
`json!` array-index expression at host_await.rs:387 required parentheses.
No module group ran. Root repaired that expression only and scoped rustfmt;
remaining five source paths and ten direct dependencies matched their pins.

Delivered execution-v2:
`D:/workspace/env/cache/ara-port-candidates/fs-scan-cache-execution-v2-20261009`.
Manifest SHA256:
`2680fee79921492d441aa4637151063a5feb065f42b337cac053f09134a11df7`.
Old manifests and failed receipts are preserved rather than relabeled.

## Original grouped execution

Command: `python -X utf8 scripts/verify_edit_module.py --output <v2>/gate
--groups engine snapshot actual-tools journal-failure write-tools
--delivery-manifest <v2>/delivered-source-manifest.json
--manifest-sha256 2680fee79921492d441aa4637151063a5feb065f42b337cac053f09134a11df7`.

One applicable locked Cargo build: PASS0 **258.543 s**, reused418/compiled3,
warnings83/errors0. Original jobs2/profile/features and D-drive Cargo/Rustup/
target paths were retained. Module PASS5/5, 23 tests, source unchanged,
**292.987 s** including build; actual group execution totals **24.450 s**.

| Original group | Passed tests | Seconds |
|---|---:|---:|
| engine | 14 | 1.470 |
| snapshot | 3 | 0.098 |
| actual-tools | 4 | 0.251 |
| journal-failure | 1 | 22.534 |
| write-tools | 1 | 0.097 |

Original engine observations exercise owned metadata, ignores/depth, Windows
junction follow policies, real cache hits, ordinary fresh glob, relative and
deleted-path selective invalidation, global invalidation and ranking/limit.
Two bounded child configurations execute TTL250/recheck12/max1/worker0 and
TTL0; observed auto workers28. They are part of the original coarse family.

Actual NativeRun Host mutation observations, recorded in
`gate/journal-failure-stderr.txt`, execute WRITE/EDIT/fresh-Session rename/delete
with completed effect receipts and actual disk assertions. Invalidated scans
discover externally seeded markers in **4/11/13/10 ms**, all below TTL1000ms.
Pre-cancel, callback-cancel and callback-error leave the target absent and
retain cached contents; callback refusals settle `not_dispatched`.
Expected caught panic sentinels in the same original group are negative
observations, not group failure. Other original supplier checks remain present.

## Bounded actual model task

Procedure: `<v2>/real-cas-task.py`, SHA256
`8c81873ae1b2032caf35cd8587b8e82015349019c5473935da880d6f23133641`.
It reuses the existing task, verifies manifest/source/production-binary pins,
isolates ARA_HOME/HOME/USERPROFILE/temp and supplies the private key via env.

Live Manager-configured CAS catalogue confirms `deepseek-v4.1-flash`;
actual messages confirm `openai-completions`. The Manager local gateway is
unavailable (URLError); the configured CAS direct route succeeds. Do not
describe this task as passing through that unavailable local gateway.

Bounds: two runs, 6/2 maximum model calls, 120 seconds/run, 2048 output tokens
per call, tools read/write/edit followed by no tools, retry/fallback disabled.
WRITE incorrect grand_total0 -> read -> EDIT -> read succeeds in **8.021 s**,
6 calls/6 tools. Actual file is pen5.5/book11.5/cup17/grand_total34, matching
the input CSV. Second process uses the same Session
`01a11c56-6e64-70e0-afc0-9e20a29fcec2`, returns quantities2/4 and book price11.5
without tools in **2.149 s**, 1 call. Both final assistant terminals are stop.
Per-call usage is retained: reported totalTokens28822; cost is not asserted.
Root checked raw tool ordering, disk bytes, recall, route and terminal fields.

Receipt: `<v2>/real-cas-receipts/20261008T162625Z/receipt.json`, SHA256
`fb18d718f6007cb43bc5e626f20e7d048410d5c984ca69cc374229eadec68744`.
Production binary SHA256:
`18cd39ec69168de52c3fc1f13c006235a12e11c9de377f6fd1122eb511e2cdb5`.
The model task proves production mutations/continuation; Native observations
separately prove raw-cache hits and invalidation. Run success alone is not
Task or complete OMP-parent acceptance.

## Review and open boundaries

Independent Codex `/root/finish_edit_renderer` uses ara-git-review and
ara-rust-core-review. Candidate PRE/PRE-FREEZE and delivered SOURCE-v1 are
bounded approvals. Delivered `<v2>/SOURCE.md` APPROVE SHA256:
`48ce8b9e581f215848f3c94bc7fa558d966c151c0779540f942f4eba937c9f87`.
Final `<v2>/POST.md` APPROVE SHA256:
`d738b8c96c97f9491d996b7dfc486474fe05e2b9186148b5416ac732a0debff7`.
Root read the reports and original source/output/receipt/bytes/Session evidence
and accepts this bounded batch. SOURCE notes rustfmt's same-binding import
ordering and trailing commas alongside the macro fix; no two-byte-only
formatting claim is made. POST independently checks all six source paths,
ten dependencies, original executed binaries and the raw actual-task evidence.

Full parallel walker/native directory-error/follow/platform surfaces, Unix
FIFO runtime, post-effect unknown failure and owning EDIT/WRITE/FS parents
remain OPEN. Workspace/clippy/deny release gates were not repeated for this
bounded batch; they remain stable-milestone obligations. No parent inventory
increment, commit, push or deployment is claimed.
