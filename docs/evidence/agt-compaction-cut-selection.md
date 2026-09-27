# AGT-COMPACTIONf: provisional whole-turn cut selection (WIP)

## Requirement and source

Fixed OMP commit `596f2da7101178214aa27a753529d15e6b7ad91d`,
`packages/agent/src/compaction/compaction.ts:499-560`, walks backward over
estimated message tokens and chooses a valid cut near `keepRecentTokens`.
ARA's existing `whole_turn_cut_candidates` offers only complete-turn user
boundaries. This slice selects among those candidates while preserving that
intentional temporary restriction. It does not implement OMP's assistant or
split-turn cuts.

`ara-agent::compaction::select_whole_turn_cut` counts settled raw messages
with `tokenizer::count_message`, picks the earliest complete-turn candidate
whose retained suffix estimate meets the target, or the latest candidate when
the newest turn alone exceeds it. It builds the candidate's summary prompt
before returning. If that prefix contains an unsupported image or exceeds the
prompt size limit, it tries earlier boundaries so that content can stay raw.
The result exposes the candidate ID/index, estimated retained raw tokens and
whether that estimate exceeds the target. An already-small history or one
without a valid cut returns no selection.

This is a planning result, not a model context-fit verdict. The raw-message
estimate omits some payloads, including user images, and does not cover tool
definitions, provider framing or proxy transformation. Before use, a host
must authenticate the source IDs against a strict Session snapshot, recheck
the branch leaf, prepare the actual provider request, accept a completed
summary, and persist it atomically. Agent loop, Session writer, CLI and replay
were not changed. AGT-COMPACTION and A3 remain WIP.

## Executed evidence

- `cargo test -p ara-agent --test compaction_cut --quiet`: 11/11 passed on
  Windows. Tests cover exact target boundaries, target inside a turn, zero
  target, oversized newest turn, unfinished tool tail, prompt image/size
  rejection, earlier-boundary fallback and the 256-source prefix limit.
- `cargo test -p ara-agent --all-targets --quiet`: 1/34/12/8/10/4 groups
  passed on the pre-review candidate. After the fallback fix, the final-code
  workspace run passed the Agent 1/34/12/8/11/4 groups, AI 108 unit, 48 Chat
  HTTP and 13 Responses HTTP tests, then failed the existing Windows CLI
  `deadline_during_a_tool_and_zero_budget_exit_nonzero` timing assertion
  (27/28 CLI e2e). Strict workspace Clippy passed.
- Changed-file rustfmt, `cargo test -p ara-agent --doc --quiet`, `cargo deny
  check` (with existing duplicate-version warnings) and
  `python scripts/omp_inventory.py check` passed. `cargo fmt
  --all -- --check` remains red in unchanged vendored `pi-*` files.
- Independent Codex preimplementation review checked the fixed OMP algorithm
  and the pure-slice scope. Independent diff review found that the first
  version rejected a later image/large-text prefix even when an earlier
  serializable cut could keep it raw; fallback and regression tests fixed it.
  Final re-review found no remaining high-confidence issue and independently
  reran the focused test 11/11 plus `git diff --check`.

No real-model context-overflow task or Session restart test was performed for
this planning function. These are required for later compaction acceptance.
