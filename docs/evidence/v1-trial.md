# V1-TRIAL: bounded real-model REPL task (2026-09-29, WIP)

Status: **partial (WIP)**. The coding task, turn cancel, resume and a real
failed-summary path ran on a real model through the REPL. Run 5 produced a
*useful* real-model summary (`google/gemma-4-31b-it:free`). A recall that
depends on that summary is **not** shown: both recall attempts got HTTP 429.
Earlier, the fixed qwen pool answered 429 to every summary call, and the free
router returned a safety-classifier line as the "summary". No independent
review has run; nothing is marked tested or accepted.

## Point and scope

Plan row V1-TRIAL ([plan](../plan.md)): a bounded real-model daily task
through the REPL on an OpenAI-compatible route, with artifact check, receipts,
usage (unknowns kept) and latency ([acceptance](../acceptance.md)). It
exercises V1-REPL, V1-CANCEL, V1-RESUME and V1-COMPACT together; each of those
keeps its own evidence file.

## Snapshot

- Base commit `7aa59b2` plus the uncommitted working-tree diff of this date
  (REPL streaming and tool progress, the reset-test fix in
  `crates/ara-ai/tests/openai_http.rs`, and the summary-error diagnostic
  below). The trial binaries were copied out of `C:\Temp\ara-verify-target`.

| Runs | `ara.exe` sha256 | `crates/ara-cli/src/main.rs` sha256 |
| --- | --- | --- |
| run1, run2, run3a | `1d77a281…6f56783` | `cfdf3f92…d4470b27` (streaming, before the diagnostic fix) |
| run3b, run3c, run4, run5 | `66f87802…6da442b27` | `11039ec3…d1e40e1d9` (with the diagnostic fix; committed as `43e79e2`) |
| run5b | `febb5f10…83325209` | `602920a8…a2813d619` (`43e79e2` plus the REPL failed-turn cause, F6; `ara-tools/src/read.rs` `1a60b2ea…71894d35`) |

## Route and model (checked live before use)

- OpenRouter, `https://openrouter.ai/api/v1`, `--api openai-completions`
  (Chat Completions, streamed). Key from `OPENROUTER_API_KEY` only.
- Catalogue (`GET /models`, 2026-09-29): 460 models, 15 `:free` with
  `tools`. `qwen/qwen3.8-27b:free`: context 262,144, `tools`, `tool_choice`,
  `max_tokens`, price 0/0. `openrouter/free`: context 200,000, same
  parameters, price 0/0.
- Served model from `GET /generation?id=` for every journal `responseId`:
  - all 19 `qwen/qwen3.8-27b:free` calls: `qwen/qwen3.8-27b-20260814:free`
    via provider ModelRun, `total_cost` 0;
  - the run4 recall turn on `openrouter/free`:
    `nvidia/nemotron-3-nano-omni-30b-a3b-reasoning-20260428:free` via Nvidia,
    cost 0.
- `google/gemma-4-31b-it:free` (run5, run5b). The public endpoint list,
  saved after run5b as `logs/gemma-endpoints.json`, shows one endpoint:
  - Google AI Studio, context 262,144, `tools`, `tool_choice` and
    `max_tokens`, price 0/0.
  - The served model of the run5 summary call is not recorded (F3). Both
    recall calls failed with 429, so they have no generation record.
- A scan of the 2,753 trial and scratch files (binaries excluded) found no key
  value. The probe logs held an account `user_id` in OpenRouter's error body;
  it was redacted in place and is not reproduced here.

## Harness

- Driver `v1_trial.py` (session scratchpad, not committed):
  - starts `ara --repl` in its own hidden console (`CREATE_NEW_CONSOLE`),
    with piped stdin, stdout and stderr;
  - detects a turn's end by counting the `> ` prompts on stderr;
  - interrupts with a helper process that attaches to ara's console and calls
    `GenerateConsoleCtrlEvent(CTRL_C_EVENT, 0)`.
- Common flags: `--session-dir C:\Temp\ara-v1-trial\sessions --cwd
  C:\Temp\ara-v1-trial\work --max-model-calls 8 --max-time 300`. The budget
  applies per turn.
- Fixture: a git repo, baseline `6a93b52`, holding:
  - `stats.py`: `median` returns `ordered[mid]` for even lengths;
  - `test_stats.py`: 4 tests, one failing;
  - `slow.py`: writes `slow.started`, prints `progress i/90` once a second,
    then `slow finished`.
- **Smoke finding (harness, not ARA):**
  - The first smoke Ctrl+C never reached ara. The "ignore Ctrl+C" console
    attribute is inherited by child processes, and the parent shell had it
    set.
  - The driver now calls `SetConsoleCtrlHandler(NULL, FALSE)` before spawning
    (`clear_ignore=1` in each driver log), as a normal terminal would start
    ara.
- **Caveat:** the interrupt is a console-delivered `CTRL_C_EVENT` on ara's
  `ctrl_c()` path, not a human keypress in a visible terminal. A manual
  keypress check is still open.

## Runs

All runs use one Session file name,
`2026-09-29T07-24-40-285Z_01a0ec0d-….jsonl`. Runs 1–4 append to it in
`sessions/`. Run 5 and run5b use `sessions-run5/`, which holds a copy of the
file saved before run4 (`logs/session-before-run4.jsonl`), so run4's bad
summary is not in it. Journal entry numbers are line indexes in the file.
"Calls" counts assistant messages; "usage" is the sum of the journal's
`usage` fields.

| Step | Input | Wall time | Calls | Result |
| --- | --- | --- | --- | --- |
| run1 turn 1 | run the tests, fix `stats.py` without touching `test_stats.py`, rerun | 35.9 s | 5 | Tool calls: read `.`, read `stats.py`, read `test_stats.py`, then bash (tests, `isError`), edit `stats.py`, bash (4 OK). Answer: the even-length `median` bug, all 4 tests pass. `test_stats.py` was untouched in this turn. |
| run1 turn 2 | add `mode(values)` with tie → smallest and `ValueError` on empty, add tests, run the suite | 28.9 s | 5 | Edits: `stats.py`, `test_stats.py`, then a blank-line fix. The bash run shows 8 tests pass. |
| run1 turn 3 | run `python slow.py` in the foreground | 8.3 s | 2 + 1 aborted | Order of events: `slow.started` seen at 3.0 s, `CTRL_C_EVENT` at 6.1 s, prompt back 2.2 s later. stderr: `interrupt received, aborting…`, `tool bash failed`, `turn 3 cancelled; session kept`. Journal: entry 29 is the bash result, `isError`, `progress 1/90 … 6/90` then `[Command aborted]`. Entry 30 is the assistant message with `stop: aborted`, `usage {}` (unknown, not zero) and `responseId` null. |
| run1 turn 4 | which bug did you fix; rerun the tests | 16.0 s | 2 | bash shows 8 OK; the answer recalls the `median` fix. Then `/exit`, exit 0. |
| run2 turn 1 | `--continue`; which function did you add (no tools) | 2.0 s | 1 | `mode(values)` with the right contract, answered from the resumed journal. |
| run2 `/compact` | default `--compact-keep-tokens` 4000 | 0.2 s | 0 | `history is already small; nothing to compact`: a correct no-op. |
| run2 turn 2 | which bug did you fix first | 3.8 s | 1 | Correct answer. Then an idle `CTRL_C_EVENT` at the prompt ends with exit **130**. |
| run3a `/compact` | `--continue --compact-keep-tokens 300` | 35.9 s | 0 accepted | `summary call failed (summary call failed: ProviderError); session untouched`, with no cause shown. This led to the fix below. |
| run3b, run3c `/compact` | same, with the fixed binary | 36.3 s, 35.7 s | 0 accepted | `summary call failed (…ProviderError, HTTP 429: 429 Provider returned error); session untouched`. The journal has no `compaction` entry. |
| run3a, b, c follow-up | which bug first, then which function (no tools) | 4.2, 1.8, 1.8 s | 1 each | Correct answers in the same process, with the full history intact. |
| run4 `/compact` | `--model openrouter/free --continue --compact-keep-tokens 300` | 6.0 s | summary accepted | Journal entry 45 is `model_change` (`openrouter/openrouter/free`). Entry 46 is a `compaction` with `method: soft`, `tokensBefore` 4119, `firstKeptEntryId` = entry 25, and 22 source IDs (entries 3–24). stderr: `compacted 4119 estimated tokens down to 1587`. **The summary text is `User Safety: safe` / `Response Safety: safe`**. See F2. |
| run4 follow-up | same question | 10.4 s | 1 | A correct answer, served by the Nvidia model above. It does **not** show that the summary carried context: the kept raw tail (entries 25–44) already restates both facts in entries 34–44. |
| run5 `/compact` | `--model google/gemma-4-31b-it:free --continue --compact-keep-tokens 300` | 11.6 s | summary accepted | Entry 45 is `model_change` (`openrouter/google/gemma-4-31b-it:free`). Entry 46 is a `compaction` with `method: soft`, `tokensBefore` 4119, `firstKeptEntryId` = entry 25 and 22 source IDs (entries 3–24). stderr: `compacted 4119 estimated tokens down to 1888; summary persisted with source IDs`. The summary is a structured task summary; see "Summary coverage". |
| run5 recall | two facts found only in entries 3–24: the tie-test input list and the blank-line fix | 36.3 s | 1 failed | Entry 48: `stopReason: error`, `errorStatus` 429, `errorMessage` `429 Provider returned error`, `usage {}`. stderr showed only `ara: turn 1 ended in error; session kept` (F6). |
| run5b recall | `--continue` on the compacted file, no second `/compact`: "quote the expression your `mode()` uses to break ties", no tools | 36.1 s | 1 failed | Entry 50: the same 429. With the F6 fix, stderr shows `ara: turn 1 ended in error (429 Provider returned error); session kept`. Exit 0. Not retried, to keep within the free-tier request budget. |

### Summary coverage (run5, entry 46)

Headings: Goal, Constraints & Preferences, Progress/Done, Key Decisions,
Critical Context. Checked against the journal and `work/`:

- **Correct:** the even-length `median` fix; the `mode` contract; the four
  `mode` test names (they match `test_stats.py`); "8 tests passed"; the
  functions `mean`, `median` and `mode` (they match `stats.py`).
- **Summarized-only fact kept:** the tie-breaking expression
  `min(counts, key=lambda v: (-counts[v], v))`. In the journal it appears
  only in entries 16–17 (summarized span) and in entry 46. It is not in the
  kept tail (entries 25–44). This is what the run5b recall asked for.
- **Omitted:** the tie-test input list `[4, 1, 3, 1]` and the blank-line fix
  to `test_stats.py`. The run5 recall asked for both, so a correct answer
  there would have come from somewhere other than the summary.

### Direct probes of the qwen pool

Five small non-streaming requests went straight to the same model:
- with and without `tools`;
- with and without `max_tokens: 1024`.

All returned HTTP 429 with `metadata.provider_name: ModelRun` and
`limit_source: upstream_provider_shared_pool` ("temporarily rate-limited
upstream").

The summary request differs from a normal turn in two ways: it has no tools,
and it sets `max_tokens` 1024 while normal turns send none. Neither changed
the outcome. The ~36 s per failed summary is the pre-stream 408/429/5xx retry
with backoff.

**Unverified:** follow-up turns that reuse the conversation's cached prefix
(`cacheRead` 8960–9216) succeeded within seconds of those 429s. Whether cache
routing explains that was not tested.

### Usage by step (journal, tokens)

| Step | Calls with usage | Unknown usage | input | output | cacheRead | reasoning |
| --- | --- | --- | --- | --- | --- | --- |
| run1 turn 1 | 5 | 0 | 8,199 | 578 | 25,856 | 237 |
| run1 turn 2 | 5 | 0 | 2,281 | 1,284 | 37,376 | 863 |
| run1 turn 3 | 2 | **1 (aborted)** | 480 | 139 | 16,640 | 83 |
| run1 turn 4 | 2 | 0 | 605 | 484 | 17,152 | 412 |
| run2 | 2 | 0 | 268 | 258 | 17,920 | 180 |
| run3a / b / c follow-ups | 1 each | 0 | 249 / 343 / 194 | 77 / 83 / 74 | 8,960 / 8,960 / 9,216 | 24 / 17 / 22 |
| run4 follow-up | 1 | 0 | 7,476 | 388 | 0 | 387 |
| failed summary calls (run3a–c) | — | **unknown** (no response) | — | — | — | — |
| run4 summary call | — | **unknown** (not persisted, F3) | — | — | — | — |
| run5 summary call | — | **unknown** (not persisted, F3) | — | — | — | — |
| run5, run5b recall (429) | 0 | **2** (`usage {}`) | — | — | — | — |

OpenRouter's normalized `tokens_prompt` is 5,645–8,076 for the qwen calls.
This is a different count from the journal's input + cacheRead; both are kept
as reported. Every reported cost is 0. Provider latency was 214–917 ms and
generation time 0.75–14.1 s.

## Artifact check

- In `work/`, `python -m unittest -v` gives `Ran 8 tests … OK`
  (`logs/run1.own-test-run.txt`).
- `git diff` against `6a93b52`:
  - `stats.py`: the even-length `median` fix, plus `mode`, which returns
    `min(counts, key=lambda v: (-counts[v], v))`;
  - `test_stats.py`: the import changed and 4 `mode` tests were added; the
    original 4 tests are untouched.
- **Model quality:**
  - The "tie" test `mode([4, 1, 3, 1]) == 1` is not a tie, since 1 occurs
    twice. The implementation does handle real ties: `mode([3, 1, 3, 1])`
    and `mode([2, 2, 1, 1])` both return 1 (my own check).
  - `stats.py` has one blank line, not two, before `mode` (a style nit).

## Findings

- **F1 (fixed, WIP):** a failed summary call showed no cause. The REPL printed
  only `summary call failed: ProviderError`.
  - `run_compaction` now appends `, HTTP {status}` and the sanitized
    provider message (`crates/ara-cli/src/main.rs`, error arm after
    `summarize_sources`).
  - Evidence: [v1-compact](v1-compact.md) "Follow-up: summary failure
    cause".
- **F2 (open, question for the user):** compaction accepts any non-empty
  final text as the summary.
  - With `openrouter/free`, the summary call returned a safety-classifier
    line. It was persisted and now stands in for 22 entries in the model
    context. The raw entries stay in the journal.
  - The source model of that line is inferred from its format, not verified
    (see F3).
  - Whether V1 should check summary content, or refuse router models for
    compaction, is a product decision. Upstream OMP behavior for this case
    was not checked.
- **F3 (open, question for the user):** the `compaction` entry does not
  record the summary call's provenance.
  - `AcceptedSummary` carries `model_id`, `response_id`, `usage`,
    `duration_ms` and `ttft_ms` (`crates/ara-agent/src/compaction.rs:484-494`).
  - `run_compaction` persists only the text, the first-kept ID, the source
    IDs and `tokensBefore` (the `append_compaction` call in
    `crates/ara-cli/src/main.rs`).
  - So the summary's served model and usage are unknown after the fact.
  - Whether OMP persists these was not checked.
- **F4 (observation):** tool progress lines.
  - `ara: tool edit` prints no argument summary, because the edit argument
    key is `input`.
  - For parallel calls, the `done` lines carry no path (three identical
    `ara: tool read done`), so they cannot be matched to their calls.
- **F5 (observation):** `model_change` records
  `openrouter/openrouter/free`, the provider prefix plus the router's own
  ID.
- **F6 (fixed, WIP):** a failed REPL turn showed no cause. In run5 the
  journal held `429 Provider returned error`, but stderr said only
  `ara: turn 1 ended in error; session kept`. Print mode already printed the
  message.
  - The REPL `RunEnd::Error` arm now appends the sanitized `errorMessage` of
    this turn's last assistant message when its stop reason is `error`
    (`crates/ara-cli/src/main.rs`, the REPL match on `report.end`). It only
    looks at messages added in this turn, so an earlier turn's error is never
    shown again.
  - Test: `repl_failed_turn_shows_the_provider_cause_and_keeps_the_session`
    in `crates/ara-cli/tests/e2e.rs`. A fake upstream returns HTTP 400
    `turn backend down`, then a normal answer. The test checks the stderr
    line `ara: turn 1 ended in error (400 turn backend down); session kept`,
    that turn 2 is answered, and that both prompts are journaled.
  - Mutation: restoring the old line fails that test (the panic message
    prints the old stderr line). The source was restored and its sha256
    re-checked.
  - Real binary: run5b, above.
- **Not observed:** whether the `python slow.py` process was killed after the
  cancel. The trial did not inspect the process table. Process-tree
  termination belongs to [windows-bash-process-tree](windows-bash-process-tree.md)
  (TOOLS-01c, changes requested).

## Checks on the diagnostic change

| Command | Result |
| --- | --- |
| `cargo fmt -p ara-cli --check` | exit 0 |
| `cargo clippy -p ara-cli --all-targets --all-features -- -D warnings` | exit 0 |
| `cargo test -p ara-cli --bin ara` | **8/8** |
| `cargo test -p ara-cli --test e2e` | **71/71** |

## Checks on the F6 change (with the read-probe fix in the same tree)

| Command | Result |
| --- | --- |
| `cargo fmt -p ara-cli -p ara-tools -- --check` | exit 0 |
| `cargo clippy --workspace --all-targets --all-features -- -D warnings` | exit 0 |
| `cargo test -p ara-cli --all-targets --all-features` | bin **8/8**, e2e **72/72**; the other ara-cli targets 17/17, 6/6, 3/3 |
| `cargo test -p ara-tools --all-targets --all-features` | all targets pass (lib 30/30, `tools.rs` 16/16) |

The read-probe fix is recorded in
[tools-01a-multi-range-read](tools-01a-multi-range-read.md), "Follow-up:
Linux long literal path".

## Gaps and decision

- A real-model compaction with a useful summary is shown (run5). A recall
  that depends on that summary is **not shown**: run5 and run5b both got
  HTTP 429. The next attempt is one run5b retry on the same file.
- The manual keypress Ctrl+C in a visible terminal is not run.
- The trial ran on Windows only. On Linux CI for `43e79e2` (GitHub Actions
  run 36538669183), `ara-cli` e2e passed 73/73, including
  `repl_interrupt_during_bash_returns_to_prompt` and
  `repl_interrupt_during_compact_leaves_no_compaction_entry`. The workflow
  failed later, in `ara-tools` `tools.rs`, on the read long-path case. That
  case is fixed in the working tree and has not run on CI yet. Cargo stops at
  the first failing test target, so the targets after it did not run.
- No independent review.
- **Decision:** changes requested (WIP). The counts are unchanged.
