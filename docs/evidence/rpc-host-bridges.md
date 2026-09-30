# RPC Host tool and content URI batch

2026-09-30. Fixed OMP `596f2da7101178214aa27a753529d15e6b7ad91d`.
This is a tested bounded implementation checkpoint, not full CA-RPC, P1–P6,
or fixed-marker acceptance. RPC now has **21 of 42 bounded command variants**.

## Source and delivered scope

| Contract | Fixed OMP source | Rust owner |
| --- | --- | --- |
| Tool definitions, active/hidden refresh, calls/updates/results | `modes/rpc/host-tools.ts`, `rpc-mode.ts:568–591,1243–1248`, `session/session-tools.ts:1897–1962` | CLI `rpc_host_tools`, `rpc_host`, shared prompt setup |
| Schemes, URI requests/results, handler replacement | `modes/rpc/host-uris.ts`, `rpc-mode.ts:1250–1256`, `rpc-types.ts:493–536` | CLI `rpc_host_uris`, instance `ContentUriPort` |
| One tool/prompt snapshot per model call, retained prepared adapters | `agent.ts:1439–1446`, `agent-loop.ts:2376,2420+` | Core `ExecutionSnapshot`, `LoopHooks`, `QueueHooks`, loop |
| Read/write content, selectors, rendering, no backing file | `tools/read.ts:2294–2375`, `write.ts:1109–1217`, `read-format.ts`, `path-utils.ts:473–503` | `ara-tools` context, internal URLs, read/write |
| Immediate side channels and EOF cleanup | `rpc-mode.ts:351–376,1591–1618` | RPC reader and connection bridges |

Fourteen source/test files are pinned in `C:\Temp\ara-host-bridges-batch\snapshot.json`.
The original base is `56d6f00`; implementation commit **`c4165e4`** matches the
frozen content. Git normalizes CRLF in two files; the snapshot retains both
tested-file and committed-blob hashes. It is deliberately labelled WIP.
No dependency/provider/REPL/product changes. Host registries survive native Session
replacement; each pending adapter and sink retain their original ownership.
Prompt and active tools publish together. Failed preparation retains the previous
registry. Core defaults retain original behavior when the hook returns `None`.

Intentional Rust boundaries: typed Core accepts UTF-8 text/image blocks and JSON
details; structural wire guards are broader, so malformed typed success terminates
with an explicit error and malformed updates are consumed without projection.
URI interruption records unknown effects in native error text; host tools also
record structured `executed:"unknown"`. After EOF, a connection-only closed flag
prevents *new* URI admission; ordinary bridge `clear` remains reusable. Pending
writes can have unknown effects and are never replayed automatically.

Open: xdev/discoverable presentation, complete runtime catalogue/metadata and
extension UI/hooks, other schemes/router consumers, live model/settings, background
bash, compaction/retry, login/subagents and the remaining full Host contracts.
Only the two new commands and their bounded execution slice are counted here.

## Executed Windows checks

Receipt root: `C:\Temp\ara-host-bridges-batch`.

- Focused `cargo test -p ara-cli --bin ara --test host_bridges -p ara-tools --test host_uris -p ara-agent --test live_tools`:
  **60/0/0**, four suites, **6.468 s**, `focused-final-fixed.log`/result JSON.
- `python -X utf8 scripts/verify_backend.py`: owned formatting, strict
  all-target/all-feature Clippy, target and documentation tests:
  **1,230/0/1**, 93 suites, **100.679 s**, `backend.log`/result JSON.
- `cargo deny check`: advisories/bans/licenses/sources **PASS, 2.752 s**.
- `cargo build -p ara-cli --bin ara`: **PASS, 2.552 s**. Executable SHA-256
  `0c842659386024ff75b181534a83c7594e76b5732175369c36fce2ed91c11272`.

Six real child scenarios exercise same-Run A→B replacement, live host updates,
hidden survivor/new-hidden activation, duplicate/collision rollback, stale IDs,
actual URI read/write/content stripping/read-only refusal, Session join/abort,
EOF unknown receipts/restart without replay, malformed guards and queued work
after EOF. Core fixtures separately hold a provider stream while changing A→B:
the current response dispatches A, the next call uses B. URI tests cover immutable,
mutable/no-file identity, explicit Skill override/removal, fixed selectors and
budgets, oversized first lines, lexical context, empty rows and cancellation.
Existing RPC output-pressure regressions run in the full gate; no separate new
Host-bridge output-pressure fault is claimed.

Retained first failures: `focused-host.log` had an overly specific EOF error-text
assertion. Closed-before-route and closed-after-route correctly reject differently;
the final oracle retains no outbound request, no unknown effect, native error and
bounded exit requirements. `focused-final.log` had an author-written assertion
against the displayContent object instead of its text member. Both were test
corrections; their original logs remain. Independent review found and closed an
EOF admission hang, oversized numbered preview budget, missing lexical boundary
rows and an empty-row display line number. The non-raw 1/3 context padding was
rechecked against fixed source and preserved.

## Bounded real task and raw audit

`real_host_trial.py`, isolated pinned `ara.exe`, and `real-host-yl9yb9ms/summary.json`:
fresh catalogue verification of B.AI `deepseek-v4.1-flash`, OpenAI Chat Completions.
One Run, maximum four calls/512 output tokens per call/60 s Run/100 s wall.
Actual **PASS, 3 model calls, 3 tool operations, 7.831 s**. The random nonce is
provided only by the host URI response; the model stores it through a custom host
tool and a writable URI into JSON and text artifacts containing the same nonce. Live updates,
native receipts and terminal success are inspected. Reported usage is **10,158
tokens**, all three totals available; cost is **unknown**. Provider request bodies
were not captured. The host file effects are controlled temporary trial effects.

Parent `audit_receipts.py` independently checks **14 source hashes, 15 artifact
files, 149 frames, 8 inputs**, three native/live tool-result objects equal in full,
terminal messages, binary/driver hashes, usage and exact requested artifacts.
The executed driver is retained unchanged. `receipts-audit.json` holds final hashes.

## Independent audit and Linux revision gate

Independent Codex reviewer `host_bridges_independent_review` applied
`ara-git-review` plus `ara-rust-core-review` and read fixed blobs, all 14 target
files and raw gates/task artifacts. Final result: **PASS, no remaining confirmed
defects**, all four initial findings resolved. The reviewer separately checked
all 15 artifacts, 149 frames, 8 timed inputs, three complete native/live tool
receipts and seven native messages against their public projection. Native
thinking signatures remain subject to the existing public event redaction.
The real task's requested files, usage and exact binary/driver pins pass.

Exact-code Linux repository run [36715269879](https://github.com/ZZray/ara/actions/runs/36715269879)
passes on `c4165e4`: **1,233/0/1, 93 suites**, backend **231 s**. Parent raw-log
count and `headSha`/successful step checks are retained in `linux-raw.log`, its
result JSON and `linux-status.json`. Fixed Skill invocation `36715269675`,
directory oracle `36715269725` and directory I/O faults `36715269852` also pass
on that revision. RPC transport CI was not triggered by this scope; its existing
Rust regressions ran in the full gate. `commitblob-audit.json` records the two
line-ending differences. Full RPC and phase acceptance remain open.
The same independent reviewer re-counted the Linux raw logs, checked all four
exact-revision CI results and committed blob pins, and approved the final six
documents: **PASS, no new inconsistency**.
