# V1-COMPACT: basic compaction from the A3 pieces (WIP)

## Scope

Single-level soft compaction for daily-driver v1 on OpenAI-compatible Chat
(`--api openai-completions`). A threshold from an explicit host estimate
(`--compact-threshold`, default 32,000 estimated tokens) plus a manual
`/compact` REPL command. The summary is persisted in the Session journal with
source entry IDs; raw history stays in the journal and resume reads the
projected context. A second compaction on an already-compacted session is
refused: chained summaries need the still-open A3 persistence decision, so V1
keeps one level. This is a reference-host subset, not full A3 parity.

## Code

- `crates/ara-session`: `append_compaction` (kind `compaction`, `method:
  soft`, `summary`, `firstKeptEntryId`, `sourceEntryIds`, `tokensBefore`) and
  `model_context` (latest valid projection; summary sent as a lower-trust
  user message, kept/later messages raw; falls back to `build_context`).
- `crates/ara-cli`: `--compact-threshold`, `--compact-keep-tokens`,
  `/compact` in the REPL, auto-compact after each completed turn (REPL and
  print), startup/resume context via `model_context`.
- Reuses A3 WIP unchanged: `compaction_source_snapshot`,
  `select_whole_turn_cut`, `summarize_sources` (1024 output tokens, 120 s
  deadline), strict projection validation.

## Real-model trial (2026-09-28, `openrouter/free`, OpenAI-compatible)

1. Clean session, two completed turns (fix `add.py` `a-b` to `a+b`; read
   `notes.txt`, maintainer Li). Both exit 0.
2. Third turn with `--compact-threshold 100 --compact-keep-tokens 60`:
   `ara: compacted 555 estimated tokens down to 343; summary persisted with
   source IDs`, exit 0. Journal holds the `compaction` entry with the fix and
   the maintainer in its summary text.
3. Fourth turn (`--continue`, auto-compact off): maintainer/fix question
   answered from the summary-only prefix (thinking receipt carries both
   facts; the free model returned no text block, so print showed nothing).
   Exit 0, same session file.

## Boundary observations

- A session whose tail holds an interrupted/unknown-effect tool call yields
  no valid cut (the A3 unknown-effect rule); the host reports nothing on
  auto and keeps the session. A budget-aborted first turn poisoned the first
  trial session this way; a clean session compacted fine.
- `default_session_dir` now also replaces `?` (verbatim `\\?\` prefix made
  the default dir illegal under Windows; found during this work).

Checks on the final code: workspace Clippy for the touched crates with
`-D warnings`, `ara-session --all-targets` suites green, `cargo fmt`
applied. Full backend gate and V1-TRIAL hardening remain open; formal counts
unchanged.

## Automated tests (2026-09-29)

Status: **implementing (WIP)**. These tests run against the controlled fake
upstream (no real model, no network). No independent review has run and
nothing is marked tested or accepted.

### Code changed

- `crates/ara-session/tests/model_context.rs` (new): five integration tests.
- `crates/ara-cli/tests/e2e.rs`: five V1-COMPACT e2e tests plus small helpers
  (`compaction_entries`, `request_text`, `tools_absent_or_empty`).
- Production code: **none**. The existing `run_compaction` / `model_context`
  behavior satisfied every test against the plan row.

### Tests

| Test | Observed |
| --- | --- |
| `model_context_replaces_the_summarized_prefix_with_a_summary_user_message` | After `append_compaction`, `model_context()` is `[summary user, kept user, kept assistant]`. The summary user text starts with `[Compacted summary of earlier turns` and contains the fake summary. Summarized `question one` / `answer one` are absent. `firstKeptEntryId` is a real user entry; `method` is `soft`. |
| `raw_summarized_entries_stay_in_the_journal_file` | The journal file still holds every source id and the raw summarized text. `model_context()` does not rewrite the file. |
| `reopen_yields_the_same_model_context` | Two `SessionJournal::open` calls yield the same role/content projection (the synthesized summary user message gets a fresh `timestamp`, so full `Message` equality is not asserted). |
| `missing_first_kept_id_falls_back_to_build_context` | A `firstKeptEntryId` that names no message entry makes `compacted_context_projection` fail (`MissingKeptMessage`); `model_context()` equals `build_context()` (four raw messages). |
| `non_user_first_kept_falls_back_to_build_context` | A `firstKeptEntryId` that points at an **assistant** message is rejected (`UnsafeSummaryBoundary`); `model_context()` equals `build_context()`. The code requires the kept entry to be a **user** message whose preceding span passes `safe_soft_summary_prefix`. |
| `repl_manual_compact_then_keep_working` | `--compact-threshold 0 --compact-keep-tokens 1`, two turns, `/compact`, third turn. Summary request has no tools and its prompt contains `first prompt`. Journal has exactly one `compaction` (`method: soft`, fake summary); `firstKeptEntryId` is a user entry; every `sourceEntryIds` id precedes that entry. The third request carries the summary and kept turn (`second prompt` / `Second answer.`) and drops the summarized raw `first prompt` / `First answer.`. Exit 0, stderr `ara: compacted`. |
| `repl_auto_compact_threshold_and_disabled_companion` | `--compact-threshold 1` after two turns runs a summary call (served 3) and writes one `compaction`. Companion `--compact-threshold 0` makes no summary call (served 2) and no `compaction`. |
| `repl_continue_uses_the_persisted_summary` | After `/compact`, restart with `--repl --continue`. The first model request of the new process carries `[Compacted summary of earlier turns` and the fake summary, not the summarized raw `first prompt`, and does carry the kept turn. The answer is appended to the same Session file; still one `compaction`. |
| `repl_compact_failure_paths_leave_the_session_usable` | (1) Second `/compact` prints `V1 keeps a single level per session`, no second summary call, still one `compaction`. (2) Summary HTTP 400: stderr `session untouched`, journal bytes identical across the failed `/compact`, next resume turn is answered, still no `compaction`. (3) `/compact` under `--no-session`: `compaction needs a session`, no summary call. (4) One-turn history: `history is already small; nothing to compact`, no summary call. |
| `repl_interrupt_during_compact_leaves_no_compaction_entry` | Hang summary + `interrupt`: stderr `session untouched`, REPL returns to the prompt, no `compaction` entry, the next line is answered (served 4). Deterministic on Windows within one attempt. |

### Commands and pass counts (Windows, 2026-09-29)

Environment: `CARGO_TARGET_DIR=C:\Temp\ara-verify-target`, test/dev debug
info and incremental builds disabled, Git Bash on `PATH`.

| Command | Result |
| --- | --- |
| `cargo fmt --check -p ara-cli -p ara-session` | exit 0 |
| `cargo clippy -p ara-cli -p ara-session --all-targets -- -D warnings` | exit 0 |
| `cargo test -p ara-session` | **33/33** (7 projection + 8 source + 13 journal + 5 model_context) |
| `cargo test -p ara-cli --test e2e` | **69/69** (64 prior + 5 new) |
| `cargo test -p ara-cli --bin ara` | **8/8** |
| new tests, three consecutive runs | **5/5 session + 5/5 e2e × 3**, all green |

### Mutations

Each named production change that should make a test fail. Three were applied
one at a time; both production files were restored with a byte-for-byte `cmp`
after each.

| Test | Named mutation | Applied? | Result |
| --- | --- | --- | --- |
| `model_context_*` / `repl_manual_compact_*` / `repl_continue_uses_*` | **A:** `model_context` returns `build_context()` unconditionally (`ara-session/src/lib.rs`) | **applied** | **fails**: `model_context_replaces_…` (4≠3), `reopen_…` (4≠3), `repl_manual_compact_…` and `repl_continue_uses_…` (third/first request lacks the summary) |
| `repl_compact_failure_paths_…` | **B:** `run_compaction` appends a compaction entry even when `summarize_sources` fails (`ara-cli/src/main.rs`) | **applied** | **fails**: journal bytes change across the failed `/compact` (`MUTATION failed summary still persisted` present) |
| `repl_auto_compact_threshold_and_disabled_companion` | **C:** auto-compaction ignores `--compact-threshold` (including 0) (`ara-cli/src/main.rs`) | **applied** | **fails**: companion case with threshold 0 makes summary calls (served ≠ 2) |
| `repl_interrupt_during_compact_…` | append a compaction entry on cancel/failure (covered by B's family) | named with B | — |

### Context-window finding

The plan row asks for a threshold "from the model's context window (or an
explicit host value)".

- **ARA `ara_ai::Model`** (`crates/ara-ai/src/types.rs:371-383`) has
  `id`, `api`, `provider`, `base_url`, `reasoning`, `max_tokens`,
  `tokenizer` — **no context-window field**. The current threshold is the
  explicit host value `--compact-threshold` (default 32,000 estimated
  tokens) in `crates/ara-cli/src/main.rs:150-152`.
- **OMP** derives the threshold from the model window plus a reserve /
  configured percent: `packages/coding-agent/test/compaction.test.ts`
  (`shouldCompact` B-20fa248471, B-6c14f00a14, B-705d8b7c5f, B-8ae02454f7,
  B-5cc7a0a453, B-e675e77cb8) and
  `packages/agent/test/compaction-reserve-provenance.test.ts`
  (B-cf0804a347: proportional fallback, threshold clamped below the window).
  Catalog/registry expose `contextWindow` on models
  (PKG-CATALOG / CA-MODEL-REGISTRY inventory rows).
- The default is **not** changed in this task. A future slice would need a
  context-window field on `Model` (or host discovery) before the
  window-derived threshold can match OMP.

### Gaps

- **Unix not compiled:** the `cfg(unix)` interrupt paths in e2e were not
  compiled here. Linux CI with `-D warnings` is still required.
- **No real model:** every request went to the controlled fake upstream.
- **No independent review:** self-reported.
- **Interrupt determinism:** the hang-summary interrupt case was green on
  this Windows host in the three-run check; a flaky console or slow machine
  could still race `up.served()` vs. the summary request. No second attempt
  was needed.
- **Counts:** unchanged; nothing is marked tested or accepted.

### Review (2026-09-29)

- **Implementer:** an external agent (MiMo v2.6), working from a
  self-contained task prompt.
- **Reviewer:** the coordinating Claude Code session (Opus 5.5), reviewing
  against the working-tree diff.
- **Scope check:** only `crates/ara-cli/tests/e2e.rs` (+306 lines),
  `crates/ara-session/tests/model_context.rs` (new) and this file changed.
  `main.rs` and `ara-session/src/lib.rs` have no diff.
- **Reruns on the same tree:**
  - `cargo fmt --check -p ara-cli -p ara-session` exit 0;
  - `cargo clippy -p ara-cli -p ara-session --all-targets -- -D warnings`
    exit 0;
  - `cargo test -p ara-session` **33/33**;
  - `cargo test -p ara-cli --test e2e` **69/69**;
  - `cargo test -p ara-cli --bin ara` **8/8**.
- **Extra mutations:** both were applied to `main.rs` one at a time. The
  file was restored after each and `cmp` confirmed it matches its saved copy.
  - **D:** after a successful compaction, skip
    `*context = journal.model_context()`. `repl_manual_compact_then_keep_working`
    **fails** at e2e.rs:2882, while `repl_continue_uses_the_persisted_summary`
    still passes. So the tests separate the in-memory swap from the persisted
    projection.
  - **E:** `context.clear()` in the `summarize_sources` error arm of
    `run_compaction`. This drops the whole in-memory history after a failed
    or cancelled summary call. Both
    `repl_compact_failure_paths_leave_the_session_usable` and
    `repl_interrupt_during_compact_leaves_no_compaction_entry` **still pass**.
- **Finding (important, not blocking):**
  - The two failure-path tests do not prove that the *same* REPL process
    keeps its history:
    - the failed-summary case checks the next turn only in a new
      `--continue` process;
    - the interrupt case checks `served()` and the journal, but not what the
      follow-up request carries.
  - The production code is correct today: the error arm returns without
    touching `context`. Nothing pins that, though.
  - **Fix:** assert that the follow-up request in the same process carries
    the prior turns and no summary. Mutation E must then fail.
- **Nits (not fixed):**
  - The summary failure uses HTTP 400 rather than the suggested 500. That is
    acceptable, since both take the same error path.
- **Result:** no blocking findings. The test fix is recorded below; nothing
  is marked tested or accepted.

### Follow-up fix: Mutation E pinned (2026-09-29)

- **Implementer:** the coordinating session (Opus 5.5), at the user's
  request. Only `crates/ara-cli/tests/e2e.rs` changed; `main.rs` has no diff.
- **Changes:**
  - `repl_interrupt_during_compact_leaves_no_compaction_entry` now also
    asserts that the follow-up request (the 4th) carries `first prompt`,
    `First answer.`, `second prompt` and `Second answer.`, and no
    `[Compacted summary of earlier turns`.
  - `repl_compact_failure_paths_leave_the_session_usable` has a new
    sub-case, and the separate-process case is kept. It builds two turns,
    then in one `--repl --continue` process sends `/compact` (the summary
    gets HTTP 400) and `next turn`. It asserts:
    - exit 0, stderr `session untouched`, stdout `Next answer.`;
    - served 2 (no retry of the 400);
    - the journal still starts with its earlier bytes, and there is no
      `compaction` entry;
    - the user texts are `first prompt, second prompt, next turn`;
    - the follow-up request carries both prior turns and no summary.
- **Commands:**
  - `cargo fmt --check -p ara-cli` exit 0;
  - `cargo clippy -p ara-cli --all-targets -- -D warnings` exit 0;
  - `cargo test -p ara-cli --test e2e` **69/69**, both before and after the
    mutation run;
  - the two changed tests **2/2 × 3**.
- **Mutation E, re-applied:** each test now **fails** at its new assertion:
  - the failure-paths test with `first prompt survives the failed summary`;
  - the interrupt test with `first prompt survives the cancelled summary`.

  `main.rs` was restored from a saved copy, and `cmp` confirmed it is
  identical.
- **Review gap:** the reviewer wrote this fix, so it has no independent
  review.
