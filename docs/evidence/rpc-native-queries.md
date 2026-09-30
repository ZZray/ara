# RPC command discovery and native Session queries/actions

**Tested and independently audited WIP, 2026-09-30.** Fixed OMP:
`596f2da7101178214aa27a753529d15e6b7ad91d`. Four scoped Rust files build on
`9524f0c`; the delivered baseline also includes the other chat's `a34956b` prompt
change. Complete CA-RPC, CTX-01e, statistics parity and P1-P6 remain open.

## Source and bounded behavior

| Fixed source | Rust behavior |
| --- | --- |
| `slash-commands/available-commands.ts:59-68`, `rpc-mode.ts:1234-1235` | Registered Skill snapshot supplies query/startup/successful new/switch metadata; hidden entries, order, Unicode and description fallback are preserved. Metadata queries do not reopen Skill files; fresh body I/O stays in invocation. |
| `session/agent-session.ts:9754-9782`, `rpc-mode.ts:1428-1431` | `get_branch_messages` selects nonempty ordinary user text from all raw entries, including side branches, with original entry IDs. Skills remain custom. No-session inputs receive stable native IDs when consumed. |
| `session/session-stats.ts:111-199`, `rpc-mode.ts:1418-1421` | `get_session_stats` aggregates completed roles/tool calls and known usage; compacted history uses the existing source projection, preserving summary/Skill identity and excluding replaced usage. |
| `session/session-manager.ts:1286-1290,2221-2256`, `rpc-mode.ts:1439-1451` | `set_session_name` applies JS trim, control cleanup and ASCII-space collapse; records slot/header/title-change, preserves lazy creation, and exposes the applied name in state. |

Ownership: `ara-cli/src/rpc_host.rs` owns wire projection and Session state;
`ara-session/src/lib.rs` owns normalization and durable title mutation. Existing
`ara-cli/tests/rpc_host.rs` and new `ara-session/tests/session_name.rs` exercise
these paths. Core/provider/Skill invocation/dependencies remain unchanged.

Intentional existing persistence policy: a materialized rename uses the journal's
atomic rewrite and restores in-memory title/header/leaf on failure. A fixed-width
slot abbreviates long names on reopen, matching fixed OMP. Slotless v3 headers
restore title/source. `title_change` is metadata in both strict compaction views;
it never enters model messages or invalidates an existing summary.

Remaining statistics work: auxiliary/task usage, premium requests, credits,
routed models and context usage. Unreported Rust usage/cost aggregates are null,
never invented zero. The full six-source command catalogue, independent live
settings and native fork are separate open work. Seventeen of 42 RPC variants
now have bounded implementations; that count is not full behavior acceptance.

## Executed evidence

Receipt root: `C:\Temp\ara-rpc-native-batch`; isolated workspace reused from the
preceding point. `snapshot.json` pins the four files and the eight committed
baseline additions adopted from `a34956b`; unrelated work was never reverted.

- Host unit tests: **5/5**, **48.733 s** including cold compilation.
- Final focused checks: RPC **32/32** plus Session name **8/8**, **40/0/0**,
  **7.083 s**. Exact lists/filters, live nonblocking queries, stale source and
  adoption failures, real tool receipts, raw side branches, lazy/restart/name
  behavior, no-session IDs, compaction and unknown usage are covered.
- First full isolated gate: **1,164/0/1**, **88 suites**, **151.154 s**;
  formatting, strict all-target/all-feature Clippy, target and doc tests PASS.
- After the concurrent committed prompt baseline changed, the current combined
  snapshot gate passed **1,165/0/1**, **88 suites**, **88.515 s**. CLI E2E includes
  the other point's additional fixture. Logs: `backend-current-base.*`.
- `cargo deny check`: advisories/bans/licenses/sources PASS, **3.150 s**;
  dependencies are unchanged. Original output is retained in `cargo-deny.log`.

Failures were retained: the first held-name oracle read a still-lazy file before
an assistant receipt; it now checks lazy state and then reads after joined abort.
The added compacted-stats byte oracle assumed startup did not record a selected
model; it now captures the baseline after ready. Production behavior was not
changed for these two fixture errors. Independent review found and closed two
real dependencies: title receipts must be ignored by strict compaction, and
slotless headers must restore user title/source.

## Actual task and exact boundary

New driver `real_native_trial.py`, SHA-256
`638e61d798a3c6e1153b2b9b7bdf8030f50b6b83eaa80c5d96fbe32dadee5a39`,
was executed once. It freshly verified B.AI `deepseek-v4.1-flash` and Chat
Completions at **10:17:55 UTC**. Limits: two Runs, three calls/60 s per Run,
six total calls, 512 output tokens/call, 150 s wall guard, 4 MiB cumulative
per-stream output. Failure preserves receipts and never replays automatically.

**PASS, 29.996 s, two Runs, four calls, two successful write receipts, 9,286
reported tokens; cost unknown.** A randomized JSON artifact is written, the
Session is renamed, the process exits, the original artifact is deleted, the same
native Session/name/history/IDs resume, and a write-only recall reproduces it.
Rename ACK takes **0.003792 s**; restart/ready/query takes **0.065301 s**.

Receipt: `real-native-7jdx7i3j/summary.json`. Parent rehashed **22 artifacts**,
**10 production sources**, **220 raw frames**, eight native messages and both
Run-local terminals; exact artifact recall, one title receipt and original IDs
pass. `receipts-audit.json` records that audit. Credentials remain environmental.
Provider request bodies were not captured; public/native history is the observed
boundary. The nonempty Skill catalogue is covered by controlled process tests.

Task EXE SHA-256:
`7923d09ce3056c87cbbfca8b034ba3653c758c7cbd66bcb5135427d8f507d5e3`.
This trial preceded adoption of `a34956b`'s prompt. Its native point sources are
unchanged, but the rebuilt combined EXE is different: it is not represented as
having run this trial. Keep current delivery WIP; a later coherent module task
must exercise that final combination before acceptance, without blindly
repeating this completed task.

Independent Codex `rpc_skill_independent_review` read the scoped diff, fixed
source, raw focused/full/dependency logs and source hashes. It found no remaining
confirmed code defects after the two dependency fixes. The current combined
snapshot is tested/audited; final exact-commit Linux and module actual-task
receipts are separate gates. No full RPC/phase/OMP marker advances.

## Delivered Linux checkpoint

Commit `3d37cde74c89bc7c4c5d23767360ecc0ed1997d8` passed
[repository checks 36704283607](https://github.com/ZZray/ara/actions/runs/36704283607):
**1,168 passed / 0 failed / 1 ignored, 88 suites**. The backend step ran
**10:45:15-10:50:16 UTC, 301 s**; formatting, strict Clippy, target and doc tests
passed. Parent re-read the exact job SHA and recalculated totals from the original
`linux-final.log`; SHA-256
`6d6c42007b8d68b7e6979706a84dd1daf2eeb54f28277095ce6784ca7e5090af`.
`commit.json` matches all four delivered Git blobs to the Windows snapshot with
only CRLF-to-LF normalization. The remaining actual-task combination gate keeps
this checkpoint WIP; this Linux success does not advance full module acceptance.
