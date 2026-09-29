# V1-COMPACT: basic compaction from the A3 pieces

Status: **evidence complete; the decision is in the [V1 audit](v1-audit.md)**.
The sections below are in time order. Later sections supersede the gaps of
earlier ones.

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
  `select_whole_turn_cut`, `summarize_sources` (120 s deadline), strict
  projection validation. The output budget was 1024 tokens until the
  [run9 fix](#run9-fix-the-omp-summary-budget-2026-09-29); it is now OMP's
  default, 13,107, capped by `--max-tokens`.

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
  trial session this way; a clean session compacted fine. (Superseded for
  aborted, errored and length-stopped turns by
  [Finding 1](#combined-v1-review-finding-1-and-fixes-2026-09-29): those are
  now summarized, and the host says why when no cut exists.)
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
| `repl_compact_failure_paths_leave_the_session_usable` | (1) Second `/compact` prints `V1 keeps a single level per session`, no second summary call, still one `compaction`. (2) Summary HTTP 400: stderr `session untouched`, journal bytes identical across the failed `/compact`, next resume turn is answered, still no `compaction`. (3) `/compact` under `--no-session`: `compaction needs a session`, no summary call. (4) One-turn history: `history is already small; nothing to compact`, no summary call. (Messages (3) and (4) changed in `4de4ce1`; see Finding 1 below.) |
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

- ~~**Unix not compiled**~~: done later. Linux CI ran them on `43e79e2` and
  `111c325` (see [V1-REPL/CANCEL](v1-repl-cancel.md#gaps)).
- ~~**No real model**~~: done later, in [V1-TRIAL](v1-trial.md) (runs 5–10).
- ~~**No independent review**~~: done later (the combined V1 review, the fix
  review and both rechecks below).
- **Interrupt determinism:** the hang-summary interrupt case was green on
  this Windows host in the three-run check; a flaky console or slow machine
  could still race `up.served()` vs. the summary request. No second attempt
  was needed.
- **Counts:** set by the [V1 audit](v1-audit.md).

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

### Follow-up: summary failure cause (2026-09-29, WIP)

- **Gap:**
  - In the V1-TRIAL real-model run ([v1-trial](v1-trial.md), run3a),
    `/compact` printed only `summary call failed (summary call failed:
    ProviderError); session untouched`.
  - `SummaryCallError` already carries `provider_status` and
    `provider_message`, but the REPL dropped them.
- **Change:**
  - The `summarize_sources` error arm of `run_compaction`
    (`crates/ara-cli/src/main.rs`) now appends `, HTTP {status}` and
    `: {message}`.
  - The message goes through `sanitize_text`, as `SummaryCallError`'s doc
    comment asks callers to do before display.
  - Nothing else changed: the session is still untouched and the REPL
    continues.
- **Test:** `repl_compact_failure_paths_leave_the_session_usable` now also
  asserts that stderr contains `HTTP 400` and the fake body's message,
  `summary backend down` (e2e.rs:3090).
- **Mutation:**
  - Reverting the print to the old line makes the test **fail** at
    e2e.rs:3090 (`provider cause shown: …`).
  - `main.rs` was restored from a saved copy; `sha256sum -c` reports OK.
- **Checks:**
  - `cargo fmt -p ara-cli --check` exit 0;
  - `cargo clippy -p ara-cli --all-targets --all-features -- -D warnings`
    exit 0;
  - `--bin ara` **8/8**;
  - `--test e2e` **71/71**.
- **Real model:** runs 3b and 3c then showed `HTTP 429: 429 Provider
  returned error`. That is the upstream free-pool limit, not an ARA fault.
- **Open from the same trial:** F2 and F3 in [v1-trial](v1-trial.md):
  - F2: any non-empty summary text is accepted;
  - F3: the summary call's model, response ID and usage are not persisted.
- **Review gap:** no independent review.

## Combined V1 review, Finding 1 and fixes (2026-09-29)

### Review

- **Reviewer:** a Sonnet 5.5 subagent, one combined static review of the V1
  host and Session diff, `e7f94b6..111c325`. HEAD `c07bb3c` had the same
  code.
- **Checks it ran:** read-only `git show`, `git diff` and `grep`. It ran no
  build or tests.
- **Verdict:** changes requested, because of Finding 1. Findings 2–7 were
  minor.
- **No defect found in:**
  - cancellation;
  - journal consistency on cancel or failure;
  - streaming;
  - `/new`;
  - failed resume;
  - exit codes.

### Finding 1 (important): compaction blocked after an aborted or failed turn

- **Before:** `validate_completed_summary_span` rejected every assistant
  whose stop reason was not `stop` or `toolUse`. So after one Ctrl+C
  (`aborted`) or provider error (`error`), no later cut existed:
  - `/compact` said `history is already small`;
  - auto-compaction said nothing.
- **Decision, by OMP parity.**
  - OMP `findValidCutPoints`
    (`packages/agent/src/compaction/compaction.ts:415` at `596f2da`) accepts
    any user or assistant message as a cut point and never a `toolResult`.
    It has no stop-reason filter.
  - ARA now summarizes:
    - aborted, errored and length-stopped turns;
    - a turn that a host budget or deadline ended right after tool results.
  - ARA still refuses:
    - a tool call without its result;
    - a span that ends with an unanswered prompt;
    - developer messages;
    - unknown-effect results (`timedOut`, `panicked`, synthetic
      `interrupted_unknown_effect`).
  - The agent loop already pairs the retained calls of an aborted, errored
    or length-stopped assistant with synthetic `executed: false` results.
    A call that was running when the user cancelled keeps the tool's own
    error result and is summarized
    ([fix review I1](#fix-review-e809a354de4ce1-2026-09-29)).
  - The serialized `stop_reason` tells the summarizer that the turn did not
    finish.
- **Found while testing the fix:**
  - `ara-session` mirrors the rule in `safe_soft_summary_prefix`. The mirror
    still rejected aborted turns.
  - `model_context()` then fell back silently to the raw history: the host
    printed `ara: compacted`, but the next request still carried the whole
    history.
  - The new e2e test caught this before the mirror was fixed.
  - `run_compaction` now checks `compacted_context_projection()` after
    `append_compaction`. On failure it prints `compaction entry written, but
    the session cannot use it (...)`, and the context stays unchanged.
- **Host messages (with Finding 3):**
  - Auto-compaction explains a refusal once per process; `/compact` always
    explains it.
  - The messages are:
    - `there is no earlier turn to summarize`;
    - `no earlier turn can be summarized (<cause>)`, from
      `explain_no_whole_turn_cut`;
    - `this session is already compacted, and V1 keeps a single level per
      session`, only for a `compaction` entry;
    - other snapshot errors, with their own text;
    - `it needs a session (--no-session is set)`.
  - `history is already small` is printed only when the history is within
    `--compact-keep-tokens`.

### Other findings

| Finding | Resolution |
| --- | --- |
| 2 Print-mode auto-compaction runs after the last prompt, with a fixed 120 s deadline | Follow-up; not needed for the REPL daily driver |
| 3 The single-level refusal repeats every turn and mislabels other snapshot errors | Fixed as above |
| 4 Weak asserts (`tokensBefore`, `sourceEntryIds`, resume against the in-memory context) | Fixed: `assert_compaction_provenance` (source IDs equal the message entries before a user `firstKeptEntryId`, `tokensBefore > 0`). The new e2e test asserts that the resumed request starts with the exact in-memory compacted request |
| 5 REPL text mode ignores `--print-thoughts` | Follow-up |
| 6 A `read_line` error ends the REPL with exit 0 and no message | Fixed: `cannot read the next prompt (...); session kept`, exit 1 |
| 7 The REPL return skips the `--report-request-text-tokens` flush | Follow-up |

The reviewer also raised a stale-interrupt race: a Ctrl+C that arrives just
as a step ends stays queued, and the next idle prompt exits with 130. The
Session is kept, so this is a follow-up.

### Code (`4de4ce1`)

- `ara-agent/src/compaction.rs`: the new span rule and
  `explain_no_whole_turn_cut`.
- `ara-session/src/lib.rs`: the mirrored `safe_soft_summary_prefix`.
- `ara-cli/src/main.rs`: refusal causes, the once-per-process notice, the
  projection guard and the `read_line` error exit.

### Tests

- **`compaction_cut`:**
  - `summarizes_aborted_errored_and_length_stopped_turns` covers:
    - the run7 shape (Ctrl+C during Bash, then more turns);
    - an aborted assistant with a synthetically paired call;
    - an aborted assistant with an unpaired call (no cut);
    - errored;
    - length with a retry;
    - length text.
  - `explains_why_no_cut_exists`.
  - The budget-stopped case now gives a cut.
- **`compaction`:** `prompt_rejects_an_unanswered_prompt_but_summarizes_failed_turns`
  covers length, error and aborted; the serialized prompt carries the stop
  reason.
- **`compaction_projection`:**
  `projection_summarizes_aborted_errored_and_budget_stopped_turns`. An
  unanswered prompt still gives `UnsafeSummaryBoundary`.
- **`e2e`:** `repl_compact_summarizes_past_an_interrupted_turn`, with a fake
  upstream and a real interrupt:
  - the cut lands on `third prompt`;
  - the aborted assistant is among the sources;
  - the summarizer sees `aborted`;
  - the fourth request carries the summary and not the aborted prompt;
  - a `--continue` process sends the same messages, plus the new prompt.
- **Changed failure-path expectations:**
  - `compaction skipped: it needs a session`;
  - `compaction skipped: there is no earlier turn to summarize`;
  - a new within-target case: `history is already small`.

### Commands (Windows, `4de4ce1` tree)

| Command | Result |
| --- | --- |
| `cargo fmt` for the ARA-owned packages, `--check` | exit 0 |
| `cargo clippy -p ara-agent -p ara-session -p ara-cli --all-targets -- -D warnings` | exit 0 |
| `cargo test -p ara-agent` | 77/77 (lib 1, agent_loop 38, compaction 13, compaction_call 8, compaction_cut 13, tokenizer 4) |
| `cargo test -p ara-session` | 34/34 |
| `cargo test -p ara-cli --bin ara` / `--test e2e` | 8/8 and 73/73 |
| `python scripts/verify_backend.py` | PASS: 74 test binaries, 1032 passed, 0 failed, 1 ignored |

### Mutations

Each mutation was applied by a script, then the named tests ran, then the
file was restored and its sha256 compared with the saved hash.

| Mutation | Result |
| --- | --- |
| Agent: restore the stop-reason filter in `validate_completed_summary_span` | **fails**: `summarizes_aborted_…`, `prompt_rejects_an_unanswered_…`, and e2e `repl_compact_summarizes_past_…` (no `ara: compacted`) |
| Session: restore the stop-reason filter in `safe_soft_summary_prefix` | **fails**: `projection_summarizes_aborted_…`, and the e2e test (the host guard reports the unusable entry, so `ara: compacted` is absent) |
| Host: report every manual no-cut case as `already small` | **fails**: the one-turn case (`!stderr.contains("already small")`) |

### Real model

- [V1-TRIAL run8](v1-trial.md#run8-compaction-past-an-interrupted-turn-2026-09-29)
  used B.AI `deepseek-v4.1-flash` on the new binary:
  - Ctrl+C during `python slow.py`, then one more turn;
  - `/compact` cut past the aborted turn;
  - after a restart, the model recalled two facts that were present only in
    the summary.
- **Open wording issue:** the run printed `compacted 644 estimated tokens
  down to 816`. The source was small, so the summary plus the kept tail
  were larger than the source. The cut is correct; the wording is a
  follow-up.

### Fix review (`e809a35..4de4ce1`, 2026-09-29)

- **Reviewer:** a separate Sonnet 5.5 subagent. It made a static review of
  `git diff e809a35 4de4ce1` (7 files) and checked the pinned OMP
  `findValidCutPoints`. It ran no build or tests.
- **Verdict:** changes requested (small). No Critical finding.
- **No defect found in:**
  - removing the stop-reason filter, given the loop's pairing rules;
  - `pending`, duplicate-ID and tool-name handling (the Agent rule and the
    Session mirror agree);
  - the `sourceEntryIds` check;
  - the `already compacted` match;
  - a manual `/compact` notice, which the once-per-process flag never hides;
  - the read-error exit path.

| Finding | Resolution |
| --- | --- |
| **I1** (important) Ctrl+C during a running tool: Bash returns an error result with the partial output and `[Command aborted]`, with no unknown-effect flag. It is summarized, while a timed-out Bash (the same kill) is refused. The evidence claimed that no possibly-run tool is summarized | **Decided by OMP parity:** a cancelled call keeps the tool's own result and is summarized. OMP Bash (`packages/coding-agent/src/tools/bash.ts:657-658` at `596f2da`) throws a `ToolError` with the output plus `[Command aborted]`, or `Command aborted` with no output; ARA Bash `Ending::Cancelled` gives the same text. Its text tells the summarizer that the command was aborted, and run8's summary said so. The sentence above and the doc comment on `validate_completed_summary_span` are corrected. New test `a_cancelled_call_is_summarized_with_its_own_abort_result`: the serialized record carries `[Command aborted]`, `is_error: true` and `unknown_effect: false`, and the assistant record carries `aborted`. The timed-out and panicked refusals stay (ARA-stricter than OMP). A timeout therefore still blocks every later cut, and the refusal names the cause. Recorded as a dogfood follow-up |
| **I2** (important) `explain_no_whole_turn_cut` gave a wrong cause: `UnfinishedTurn` for a leading developer message, and `TooManySources` for any history over 256 messages, even when an early blocker caused the refusal | **Fixed:** a leading developer message gives `DeveloperInSummary`; another non-user first message gives the new `NoLeadingPrompt`. The scan now stops at the source limit, as the candidate scan does, so an early blocker is named. `TooManySources` is returned only when no later prompt lies within the limit. Tests: `explains_why_no_cut_exists` (developer first, assistant first) and `explains_an_early_blocker_in_a_history_past_the_source_limit` (305 messages with an early timeout; 303 messages with no prompt inside the limit) |
| M1 An entry the Session cannot project was reported as `already compacted` | **Fixed:** that arm checks `compacted_context_projection()` first and prints `the session has a compaction entry it cannot use (...)`. Untested at the CLI level. The recheck showed the guard is reachable: a `compaction` entry the Session cannot project, such as a non-soft or replay-data entry written by another host, takes this arm. An e2e test with a hand-written journal is a follow-up |
| M2 The once-per-process auto notice had no test | **Test added:** `--no-session` with three turns and `/compact`. `auto-compaction skipped: it needs a session` appears once, and the manual `compaction skipped: it needs a session` also appears once. The notice is still once per process, not once per cause |
| M3 The `None` arm printed nothing | **Fixed:** `no cut was selected`. Not expected to occur |
| M4 The read-error exit had no test | **Test added:** `repl_unreadable_prompt_exits_1_and_keeps_the_session`. After one turn, a line of invalid UTF-8 gives exit 1 and `cannot read the next prompt (...); session kept`. The journal keeps the first turn, and the line after is not sent |
| M5 `agt-compaction-input.md` still described the old rule | **Fixed:** a superseded-rule note. The ledger row lists the intentional differences |
| M6 `UnfinishedTurn` text is also used for a duplicate open call ID | No change: the first call with that ID has no result, so the text holds |
| M7 The Session mirror tests were thinner than the Agent tests | **Tests added:** aborted and length assistants with a call paired by a synthetic `executed: false` result are projected; the same call without its result is refused (`UnsafeSummaryBoundary`) |

The reviewer also noted that `error_message` is not serialized, so the
summarizer sees `error` without its text. OMP behaves the same, so there is no
change.

#### Mutations on the fix

| Mutation | Result |
| --- | --- |
| The explanation scans past the source limit again | **fails** `explains_an_early_blocker_…` |
| A leading developer message gives `UnfinishedTurn` again | **fails** `explains_why_no_cut_exists` |
| Auto-compaction explains every refusal (`load` instead of `swap`) | **fails** `repl_compact_failure_paths_…` (count 1) |
| An unreadable prompt exits 0 | **fails** `repl_unreadable_prompt_…` |
| The Session mirror accepts a prefix that ends with an unpaired call | **fails** `projection_summarizes_aborted_…` |

Each file was restored, and its sha256 matched the saved hash.

#### Commands on the fix tree (Windows)

| Command | Result |
| --- | --- |
| `cargo test -p ara-agent --test compaction_cut` | 15/15 |
| `cargo test -p ara-session --test compaction_projection` | 8/8 |
| `cargo test -p ara-cli --test e2e repl_` | 20/20 |
| `python scripts/verify_backend.py` (fmt, strict Clippy, all tests) | PASS: 74 test binaries, 1035 passed, 0 failed, 1 ignored; e2e 74/74 |

### Recheck (`4de4ce1..74b80cb`, 2026-09-29)

- **Reviewer:** a separate Sonnet 5.5 subagent. It made a static review of
  `git diff 4de4ce1 74b80cb -- crates` and the evidence above. It ran no
  build or tests.
- **Verdict:** approve. No Critical or Important finding.
- It confirmed:
  - `explain_no_whole_turn_cut` returns a cause whenever the candidate scan
    is empty, and cannot index out of range;
  - no exhaustive match breaks on `NoLeadingPrompt`;
  - the new tests fail on the old code, and the stdin and notice-count tests
    are deterministic;
  - the I1 text matches the code.

| Finding | Resolution |
| --- | --- |
| 1 `TooManySources` for one long turn with no later prompt at all | **Fixed:** `TooManySources` only when a prompt lies past the limit; otherwise `EmptySources`. The test gains a 304-message single-turn case |
| 2 A kept prompt with a duplicate ID is reported as `InvalidSourceId` | No change: unreachable from the CLI, because the Session rejects duplicate IDs |
| 3 The M1 guard is reachable (see M1 above) | Evidence reworded; the e2e test is a follow-up |
| 4 The doc comment said every call not run gets a synthetic result | **Fixed:** blocked, invalid and skipped-after-cancel calls get ordinary error results, and are summarized too |
| Nit: Bash with no output returns `Command aborted` without brackets | Doc comment updated |
| Nit: the cancelled-call test named Bash but used the helper's `write` call | Comment reworded |

### Run9 fix: the OMP summary budget (2026-09-29)

- **Found by:** [V1-TRIAL run9](v1-trial.md#run9-and-run10-compaction-on-the-delivered-code-2026-09-29).
  On B.AI `deepseek-v4.1-flash`, `/compact` over the history with the
  aborted turn printed `summary call failed (summary call failed:
  IncompleteResponse); session untouched`. The session was kept, but nothing
  was compacted.
- **Cause:** the host gave the summary call 1024 output tokens. OMP gives
  `min(floor(0.8 * reserveTokens), 16384)`, 13,107 with the default 16,384
  reserve (`packages/agent/src/compaction/compaction.ts:191,203,857` at
  `596f2da`). The summary that later succeeded on the same history is 4,195
  bytes of text, about 1,000 tokens before any reasoning tokens. The failure
  message did not name the stop reason, so a cut-off (`length`) summary is
  the inferred cause, not an observed one.
- **Fix:**
  - The summary budget is now OMP's default, 13,107 tokens. ARA knows no
    context window, so it keeps the default reserve. `--max-tokens`, when
    set, caps it.
  - `SummaryCallError` carries the terminal stop reason. The host message
    now names the stop reason and the output tokens, for example
    `IncompleteResponse, stop reason length, 13107 of 13107 output tokens`.
  - **Intentional difference:** OMP accepts a length-stopped summary and
    rejects only `error`. ARA still refuses a cut-off summary, because it may
    drop facts. The session is untouched and the message says why.
- **Probe:** the run9 journal as the failing `/compact` found it (its first
  47 lines), with the fixed binary and one B.AI call: `compacted 3694
  estimated tokens down to 1149`. The 42 source IDs include the aborted
  `slow.py` turn, and the summary lists that task as "Attempted, aborted".
- **Tests:**
  - e2e `repl_compact_uses_the_omp_summary_budget_and_names_a_truncated_summary`:
    - a summary cut off with `length` prints the stop reason and
      `13107 of 13107 output tokens`, and leaves the session untouched;
    - the next `/compact` succeeds;
    - both summary requests carry `max_tokens: 13107`;
    - with `--max-tokens 700`, the summary request carries 700.
  - `compaction_call`: a cut-off summary reports `length`, from the message
    or from the event; a provider error reports `error`.

#### Mutations on the run9 fix

| Mutation | Result |
| --- | --- |
| The summary budget is 1024 again | **fails** `repl_compact_uses_the_omp_summary_budget_…` |
| `--max-tokens` no longer caps it | **fails** the same test (700 expected) |
| A cut-off summary reports only the message's stop reason | **fails** `one_shot_rejects_incomplete_empty_tool_and_error_responses` |
| `TooManySources` for any history past the limit (recheck 1) | **fails** `explains_an_early_blocker_…` (the new single-turn case) |

Each file was restored, and its sha256 matched the saved hash.

#### Commands on the run9 fix tree (Windows)

| Command | Result |
| --- | --- |
| `cargo test -p ara-agent --test compaction_cut --test compaction_call` | 15/15 and 8/8 |
| `cargo test -p ara-cli --test e2e repl_` | 21/21 |
| `python scripts/verify_backend.py` (fmt, strict Clippy, all tests) | PASS: 74 test binaries, 1036 passed, 0 failed, 1 ignored; e2e 75/75 |

#### Recheck of the run9 fix (`74b80cb..` delivery tree, 2026-09-29)

- **Reviewer:** a separate Sonnet 5.5 subagent. It made a static review of
  the uncommitted diff against `74b80cb` in the five code and test files.
  It ran no build or tests.
- **Verdict:** approve. No critical or important finding.
- **No defect found in:**
  - the budget constant (13,107 = floor(0.8 × 16,384)) and the cap (a
    smaller `--max-tokens` lowers it, a larger one never raises it);
  - the single `summarize_sources` call site, which covers manual `/compact`
    and both auto-compaction paths;
  - `stop_reason` on every error path;
  - the failure message when a part is missing (unknown output usage is
    left out, not shown as 0);
  - the `later_prompt` slice, which cannot panic;
  - reversion detection by the tests.

| Finding | Resolution |
| --- | --- |
| 1 A summary with stop reason `stop` and an HTTP error status prints `stop reason stop, HTTP 500` | Follow-up: true but reads oddly. Not reached on the daily route |
| 2 No test that a `--max-tokens` above 13,107 does not raise the budget | Follow-up (test gap). The cap mutation above is caught; a plain `cap` is not |
| 3 A route whose output limit is below 13,107 may answer HTTP 400 to `/compact`, where 1,024 fit | Follow-up: OMP sends the same default. `--max-tokens` lowers it. B.AI `deepseek-v4.1-flash` accepted it (run10) |
| 4 `--max-tokens 0` gives `InvalidMaxTokens` with no hint that the flag caused it | Follow-up; unlikely in use |
