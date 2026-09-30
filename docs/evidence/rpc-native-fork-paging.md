# Native RPC fork and message paging checkpoint

Fixed OMP: `596f2da7101178214aa27a753529d15e6b7ad91d`.
Base: `a032434`. Status: **tested/audited WIP**; complete CA-RPC, P1-P6 and
the full upstream marker remain open. This is one coherent native Session
batch, following [native queries and naming](rpc-native-queries.md).

## Source and bounded implementation

| Fixed source | Rust behavior |
| --- | --- |
| `rpc-mode.ts:541-565,1180-1185`; `session/agent-session.ts:9096-9191` | `branch` validates any raw ordinary-user entry before cancellation, excludes the selected user, joins the original Run, prepares a fresh provider and native journal, adopts once and emits one metadata update before the exact text/cancelled response. |
| `session/session-manager.ts:2722-2803`; `SessionEntryIndex.pathTo/labelsInEffect` | `fork_at` preserves retained raw IDs/provenance, title/source, effective labels and non-root additional directories. It creates a new identity and durable file with parentSession, or a real memory journal without file effects. Root branching clears directories and inherits the title through a new title receipt. Missing ancestors/cycles retain the fixed tolerant walk. |
| Entire `modes/rpc/rpc-messages.ts`; `rpc-mode.ts:1469-1493` | Pure stable-snapshot paging: default 100/max 256, 768 KiB JSON budget, canonical unpadded base64url, strict UTF-8, JS safe integers/UTF-16 IDs, snapshot-bound cursors, exact busy/stale errors and lossless oversized first messages through negotiated v2 framing. |

Files: CLI Host and its real-process tests; Session library and new
`session_fork` tests; RPC module registration and new `messages` implementation
and tests. No dependency, provider, REPL, product or ARA customization changes.
The necessary no-session dependency replaces the user-only ID list with a
canonical native memory journal for all messages, Skill receipts and title
changes. Memory context projection remains separate from durable compaction
source evidence: a memory snapshot cannot claim durability.

Advanced extension cancellation, advisor/memory lifecycle hooks, full saved
model/settings reconciliation and complete Session storage remain open. Export
HTML is a separate viewer/assets batch. Full statistics, command catalogue and
the remaining host/protocol contracts remain open. There are now **19 of 42
bounded RPC command implementations**, not 19 fully accepted OMP commands.

## Executed Windows checks

Receipt root: `C:\Temp\ara-rpc-native-batch`. The seven-file frozen snapshot and
actual executable are pinned by `native-session-snapshot.json`.

- Focused: `cargo test -p ara-cli --test rpc_host -p ara-session --test session_fork -p ara-rpc --test messages`:
  **52/0/0** (37 process, 8 Session, 7 pure RPC), **6.336 s**;
  `native-session-focused-fixed.log` and result JSON.
- Full: `python -X utf8 scripts/verify_backend.py`: owned formatting, strict
  workspace/all-target/all-feature Clippy, all target and documentation tests
  **PASS, 1,187/0/1, 90 suites, 90.030 s**;
  `native-session-backend-fixed.log` and result JSON.
- `cargo deny check`: advisories/bans/licenses/sources **PASS, 2.695 s**;
  `native-session-deny.log` and result JSON. Dependencies are unchanged.
- Final binary: `cargo build -p ara-cli --bin ara`, **PASS, 2.56 s**.
  SHA-256 `9098231f415f6f6be806d2c23a45b2ef9cb6258482ad7e999e4fa5f30d3b13f4`.

Actual child-pipe tests cover persistent and memory forks, duplicate text/images,
Skill/tool provenance, restart after deleting Skill source, side-path selection,
empty-parent root, unavailable target directory retaining the source Session,
held Run cancellation and queue disposal, busy paging before terminal delivery,
exact snapshot reconstruction, leaf-only/count/Session stale cursors and a
1,400,000-byte UTF-8 first message round trip through real negotiated v2 output chunks.
A Host unit test preserves an unpaired UTF-16 correlation ID on a coded error.

Retained first failures: one test accidentally included the Session header UUID
among retained entry IDs; the assertion now explicitly distinguishes the header
from native entries. The first full gate stopped at a new test's Clippy
`is_multiple_of` spelling. Both originals are retained; production behavior was
not changed to fit these failures. Independent pre-review found and closed one
actual parity difference: raw `parentId:""` is a root in fixed OMP.

Independent Codex `rpc_skill_independent_review` read all seven scoped files,
fixed source, raw focused/full/dependency logs and hashes. **Final source review
PASS, zero remaining confirmed code defects.** Final real-task artifacts and
exact-code Linux evidence are separate checks below.

## Module task and Linux evidence

`real_session_trial.py` ran once on the final executable and exact code commit
`dd1b2a62447fc6cd17f4d584f38529fd5e1dd9fd`. Driver SHA-256:
`7b710768c1096e375ad6d0f0e7dd20e4a43de4ce95aac528f31379a1137c9435`.
It freshly verified B.AI `deepseek-v4.1-flash`, Chat Completions, at
**11:32:21 UTC**. Limits: two Runs, three calls/60 seconds per Run, six total
calls, 512 output tokens/call, 150-second wall guard, cumulative 4 MiB per output
stream. Failures retain receipts and never automatically replay.

**PASS, 11.082 seconds, two Runs, four calls, two successful write receipts,
12,221 reported tokens; cost unknown.** The first Run writes a random JSON
artifact. Five snapshots are reconstructed through 29 limit-one page responses.
Rename and branch each make the preceding cursor stale. A new native fork keeps
the original records/IDs, title and source-file parent; its second Run recalls
the exact artifact after the original file is deleted, using write only. A third
process only queries the fork and verifies exact history/identity/name restore;
it starts no Run. The source journal remains byte-identical after the fixture.

The branch selector is a deliberately appended **unconsumed user draft**, added
after the first process exits. It is a controlled native persistence fixture,
not another model turn. The fork excludes it. Process tests separately cover
consumed selections, Skill/tool history and active cancellation. Provider
request bodies were not captured; the observed boundary is public/native output.
This trial exercises the current combined prompt/native-query implementation,
closing the previous checkpoint's old-binary task gap without reusing its result.

Receipts: `real-session-08w328zp/summary.json` and its raw inputs/stdout/native
journals/paging/artifacts. Parent audit `native-session-receipts-audit.json`
verified **35 artifact hashes, 12 production source hashes, 245 raw frames,
56 input commands, five paging snapshots and both Run-local terminals**.
All source hashes also match exact committed blobs after CRLF normalization.
Rename ACK **0.003034 s**, branch ACK **0.003948 s**, source restart/query
**0.071501 s**, query-only fork restart **0.078690 s**.

Exact-code Linux repository run `36708859220` passes on `dd1b2a6`:
**1,190/0/1, 90 suites**; full backend step **11:29:36-11:34:51 UTC, 315 seconds**.
Formatting, Clippy, target/doc tests and all repository checks pass. Receipts:
`native-session-linux.log`, job JSON and audit JSON. Saved log SHA-256:
`e56fe583827a3eb5aad495d58996b93f10db72d0e0071827a779250582a86cd5`.
Fixed RPC transport `36708859361` and fixed Skill invocation `36708859155`
also pass on this exact code commit.

Independent Codex `rpc_skill_independent_review` audited the complete driver and
original task artifacts: all 35 artifact hashes, 12 source hashes and seven final
snapshot hashes; 245 frames match observation, all 56 inputs have correlated
responses, five pages/history snapshots match native/public projections, and
both write receipts/terminal outputs/retained IDs/unchanged-source/fork restart
are consistent. **Raw task audit PASS, zero confirmed defects.** The unconsumed
fixture and uncaptured provider request-body limits above remain explicit.
This closes this batch's current-binary task and Linux evidence gaps. No phase,
complete CA-RPC command or full OMP acceptance is claimed from these bounded checks.
