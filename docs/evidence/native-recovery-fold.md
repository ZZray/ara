# Responses context recovery and native summary fold checkpoint

Base `6bc87b7665af58a0ea0dcf9e3c072013c39ad0c3`; fixed OMP
`596f2da7101178214aa27a753529d15e6b7ad91d`. This
bounded WIP does not accept CA-TURN-RECOVERY, AGT-COMPACTION or a phase.
RPC remains 27/42; P1–P6 remain open; the complete port marker remains null.

## Source and observable scope

Fixed agent `compaction.ts:765–958,1543,1734–1756` supplies output budget,
message-window planning, carried summary, clamp and overflow replanning.
Native inputs are `compaction-summary-cap`, `compaction-oversized-input`,
`compaction-reserve-provenance` and `openai-responses-stream-retry`.
Exact source/test blob receipts are under `C:\Temp\ara-native-recovery-batch`.

| Fixed behavior | Executable Rust mapping |
| --- | --- |
| B-b77a105738, B-760933ee31 | Fitting one-call path / multi-window fold in the grouped Core module |
| B-a7cbc7e1d2, B-db1f683fc8 | Typed overflow halving / small-window floor in the same module |
| B-7fab66a327, B-334ed36fa2 | Non-overflow failure / caller-provided previous summary |
| B-600e58711c, B-115b87725d | Large raw-reserve cap / proportional smaller reserve; actual Host wire cap |

These are bounded mappings. Remote cap B-be04f9381a, split-turn cap
B-92ae50c4fa and readable cross-provider boundary B-b3d3cfc199,
B-183a484fb1, B-f17d67ff52 are not accepted by these tests.

- Responses parsing records typed context-recovery evidence. Only proven
  content-only output can bypass the old same-route veto for overflow context
  rewriting. Function/native/unknown output, usage admission and Codex event
  validation remain vetoes. Missing journal metadata retains the old policy.
  Transparent Provider replay and ordinary Session retry use their prior gates.
- Summary output uses `min(floor(0.8 * rawReserve), 16384)`, default reserve
  16384; the Host's explicit generation cap can further limit the request.
  Retry-fit uses its separate resolved reserve. The prior reviewer statement
  that effective reserve controls the summary cap is withdrawn in the new PRE
  receipt; previous historical receipts are preserved.
- Validate the full original span once, pack at message boundaries, clamp one
  oversized message and carry each successful summary into the next call.
  Genuine typed overflow can halve/replan the current window down to the native
  floor. One cancellation token/deadline spans all calls. A failed later window
  returns no accepted partial summary; complete success retains every source ID
  for one checked Session commit.
- Core success/error results retain per-call receipts with optional usage,
  terminal evidence and replanning disposition. Existing top-level usage and
  response metadata describe the last call, not aggregate billing. Unknown
  usage is never replaced with zero.

## Verification entry

`python -X utf8 scripts/verify_native_recovery.py` runs Provider, summary and
actual RPC Host modules. Add `--full` for the single shared backend gate.
Only Root runs Cargo; authors freeze before verification. Live trials privately
read the user's Manager configuration and verify the current catalogue before
using CAS `deepseek-v4.1-flash`; no credentials enter repository artifacts.

Final snapshot: `module-20261001T115156Z/receipt.json` under the artifact root.
Modules: Provider 32 / summary 11 / Host 10, all passing (53/0/0).
The final gate takes 338.253 seconds: backend 322.1 seconds, 1,512 passed /
0 failed / 20 ignored; formatting, Clippy, inventory, deny and final build pass.
Source fingerprints are unchanged. Earlier failed compile/lint receipts are
retained: one Root test path needed borrowing; one test callback needed a type
alias. Neither repair relaxes expectations or lint policy.

Final live receipt: `live/20261001T120003Z/receipt.json`, 29.558 seconds,
Manager → OMP management → CAS `deepseek-v4.1-flash`, Chat Completions.
Eleven actual CAS calls across three processes preserve Session
`01a0f755-ffd5-709b-8c2d-9bc37b965de5`. Two real no-tools summary calls use the
configured bounded window; the second carries the first summary. The invoice
artifact totals 75; after compaction the adjustment artifact is
`{original_total:75,shipping:15,grand_total:90}`. Tool-disabled reopen recalls
quantity 4, unit price 8.5 and adjusted total 90. One compaction contains exactly
the eight original message IDs before the kept entry. Root inspected original
wire, journal, artifacts and final binary, rather than only script exit status.
Final binary SHA256:
`53eec32da3248960189fecce0b068a4f172710d31c5f1269f8ed2b9579934615`.

Independent Codex `/root/ctx_oracle_diff_review` approved PRE and completed POST
`PASS_WITH_EXPLICIT_GAPS`. It directly read final frames, wire, journal and
artifacts, and bound 13 source hashes to gate before/after and current files.
POST receipt `post-review.json` SHA256:
`e21f400d75c1ce6a54bfee75e7bcba02b6577ae5b40f04774f9ddca6f9cf9654`.
Its new PRE receipt explicitly corrects the previous reserve-source misreading.
Root `root-point-audit.json` independently binds source, gate, binary and live
outputs. The bounded WIP execution/audit passes; complete parent surfaces remain
open.

## Remaining required parity

Whole-span admission still limits input to 1 MB / 256 sources, including cut
selection. Token counts use the selected reconstructed Claude tokenizer when
available; other models retain an approximate estimate. UTF-16 prefix boundaries
keep whole Rust scalars, so lossless split-surrogate text remains open.
Core per-call receipts are not yet persisted/exposed by the Host; executed
per-call evidence is in Core fixtures and live wire artifacts. Cross-fold cancel
is exercised; a distinct second-window non-overflow failure and cross-fold
deadline scenario were not separately added. Existing deadline/failure tests
and direct common-deadline source review cover their narrower boundaries.

Legacy REPL keeps its default-reserve entry; complete native settings integration
there remains open. Remote/handoff/shake/pruning, split-turn selection, native
length/incomplete/terminal handling, promotion/durable-discard Host callers,
rescue, full registry and other inventory contracts remain mandatory. Real
OpenAI account authorization is still open; a synthetic Responses route or a
real CAS Chat task does not establish it.
