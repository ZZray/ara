# AGT-COMPACTIONd: complete-turn cut candidates (WIP)

Fixed OMP source: `596f2da7101178214aa27a753529d15e6b7ad91d`, `packages/agent/src/compaction/compaction.ts:415-560,1369-1422`. OMP can cut at an assistant message and summarize a split-turn prefix separately. ARA has only one-shot complete-span validation, so `ara-agent::compaction::whole_turn_cut_candidates` currently offers the narrower boundary before a user message only. Its preceding span must end at a complete assistant answer, pass tool-call/receipt and source-ID validation, and contain no developer message that would lose priority when summarized. It returns the first kept index and caller-provided entry ID. A future caller must take those IDs and messages from a strict Session snapshot.

The function enumerates structurally safe boundaries only. It does not check whether the summary prompt can encode an image or fit the 1 MiB input limit; a caller must build that prompt before using a candidate. It does not choose a cut by token budget, authenticate Session entries, summarize text, persist an entry, or change replay. It scans at most the current 256-message summary-input limit. Failed/incomplete turns and unknown tool effects remain visible in the raw suffix because no candidate after them is offered. Developer messages in the suffix remain raw; no candidate may place them in the summarized prefix.

Windows checks on the candidate:

| Check | Result |
| --- | --- |
| `cargo test -p ara-agent --test compaction_cut` | 7 passed: complete turns, unfinished tail, tool receipt/final answer, failed and unknown-effect turns, developer priority, invalid first-kept ID, structural boundary versus image-prompt rejection, 256-source limit, and no-op cases. |
| `cargo test -p ara-agent` | On the final candidate, 22 Agent loop, 11 compaction input, 8 summary call, 7 cut, and 4 tokenizer tests passed; doc tests passed. |
| `cargo clippy -p ara-agent --all-targets --all-features -- -D warnings`; `cargo fmt -p ara-agent` | Passed. |

An independent Codex reviewer read the fixed OMP cut path and both code/test files. It found that the first candidate contract implied prompt payload support although the function only validates structure. The contract now says a caller must build the summary prompt before using a candidate, and a test shows an image-containing candidate is rejected by prompt serialization. The reviewer rechecked the corrected diff and found no remaining P0/P1 issue; it ran `git diff --check` but did not independently rerun tests or Clippy.

This is a deliberate temporary difference from OMP's split-turn behavior. A real context-fit decision needs a verified model request projection. Session metadata, locking, leaf recheck, durable compaction, restart projection, and a long-context trial remain open. AGT-COMPACTION and A3 are WIP.
