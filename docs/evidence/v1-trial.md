# V1-TRIAL: bounded real-model REPL task (2026-09-29, WIP)

Status: **partial (WIP)**. The coding task, turn cancel, resume and a real
failed-summary path ran on a real model through the REPL. Run 5 produced a
*useful* real-model summary (`google/gemma-4-31b-it:free`). Three recall
attempts on OpenRouter got HTTP 429. Run 6, on B.AI
`deepseek-v4.1-flash`, then recalled a fact that the request carried only in
that summary; a replay of the same file shows what the request held.
Earlier, the fixed qwen pool answered 429 to every summary call, and the free
router returned a safety-classifier line as the "summary". No independent
review has run; nothing is marked tested or accepted. F2 and F3 are decided
by OMP parity, and the Ctrl+C console path is covered by a ConPTY check.

## Point and scope

Plan row V1-TRIAL ([plan](../plan.md)): a bounded real-model daily task
through the REPL on an OpenAI-compatible route, with artifact check, receipts,
usage (unknowns kept) and latency ([acceptance](../acceptance.md)). It
exercises V1-REPL, V1-CANCEL, V1-RESUME and V1-COMPACT together; each of those
keeps its own evidence file.

## Snapshot

- Base commit `7aa59b2` plus the working-tree diff of this date (REPL
  streaming and tool progress, the reset-test fix in
  `crates/ara-ai/tests/openai_http.rs`, and the summary-error diagnostic
  below), later committed as `43e79e2` and `111c325`. The trial binaries were copied out of `C:\Temp\ara-verify-target`.

| Runs | `ara.exe` sha256 | `crates/ara-cli/src/main.rs` sha256 |
| --- | --- | --- |
| run1, run2, run3a | `1d77a281…6f56783` | `cfdf3f92…d4470b27` (streaming, before the diagnostic fix) |
| run3b, run3c, run4, run5 | `66f87802…6da442b27` | `11039ec3…d1e40e1d9` (with the diagnostic fix; committed as `43e79e2`) |
| run5b, run5b2, run6 | `febb5f10…83325209` | `602920a8…a2813d619` (`43e79e2` plus the REPL failed-turn cause, F6; `ara-tools/src/read.rs` `1a60b2ea…71894d35`; committed as `111c325`) |

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
  - The served model of the run5 summary call is not recorded (F3). The
    three recall calls failed with 429, so they have no generation record.
- B.AI (run6, suggested by the user after the 429s):
  `https://api.b.ai/v1`, `--api openai-completions`, key from
  `BAI_API_KEY` via `--api-key-env`. `GET /models` returned 200 with 58
  models, including `deepseek-v4.1-flash` (`supported_endpoint_types`
  `openai`, `anthropic`). The journal records the response model as
  `deepseek-v4.1-flash`; no generation lookup exists for this route.
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
  `ctrl_c()` path, not a human keypress in a visible terminal. A later
  ConPTY check ([V1-CANCEL](v1-repl-cancel.md#follow-up-real-console-ctrlc-through-conpty-2026-09-29))
  drives Ctrl+C through a pseudoconsole with a console `stdin`; a physical
  keypress is still not run.

## Runs

All runs use one Session file name,
`2026-09-29T07-24-40-285Z_01a0ec0d-….jsonl`. Runs 1–4 append to it in
`sessions/`. Runs 5, 5b, 5b2 and 6 use `sessions-run5/`, which holds a copy of the
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
| run5b recall | `--continue` on the compacted file, no second `/compact`: "quote the expression your `mode()` uses to break ties", no tools | 36.1 s | 1 failed | Entry 50: the same 429. With the F6 fix, stderr shows `ara: turn 1 ended in error (429 Provider returned error); session kept`. Exit 0. |
| run5b2 recall | the same prompt and binary, on the file after run5b (51 entries; copy `logs/session-before-run5b2.jsonl`) | 35.5 s | 1 failed | Entries 52–53: the same 429, with the same stderr line. Exit 0. Logs `logs/run5b2.*`. |
| run6 recall | the same prompt and binary, `--model deepseek-v4.1-flash --base-url https://api.b.ai/v1 --api-key-env BAI_API_KEY --continue`, on the file after run5b2 (53 entries; copy `logs/session-before-run6.jsonl`) | 7.8 s | 1 | Entry 54 `model_change`, 55 the prompt, 56 the answer (`stopReason: stop`, thinking + text). It quotes `min(counts, key=lambda v: (-counts[v], v))`, which matches `work/stats.py:26`. It also answers the two unanswered run5 questions with "I don't know": the summary omits both facts. Exit 0. Logs `logs/run6.*`. |

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

### Replay of run6's request (controlled fake upstream)

The real request body is not logged. To see what run6 sent, the same binary
(`ara-run5b.exe`) ran the same REPL command on a copy of
`logs/session-before-run6.jsonl`, against `ara-fake-upstream --record`.
Only the base URL and the key variable differed. Script: `replay6.py`
(scratchpad); output under `C:\Temp\ara-v1-trial\replay6`.

- One request: model `deepseek-v4.1-flash`, 6 tools, 26 messages.
- Messages 0–1: system prompt and project context.
- Message 2 is a user message: the date/cwd reminder, then
  `[Compacted summary of earlier turns; source entries withheld]` and the run5
  summary text (entry 46).
- Messages 3–21 are the kept tail, from the run1 `slow.py` prompt (entry 25)
  onward, with its tool calls and results.
- Messages 22–25 are four user prompts in a row: the run5, run5b and run5b2
  prompts, whose assistant turns ended in error, and the new prompt. The
  errored assistant messages are not sent.
- The tie-breaking expression occurs **once** in the whole body, inside the
  summary. The tie list `[4, 1, 3, 1]` does not occur.

So run6's answer came from the persisted summary, not from the raw entries
16–17. This is a reconstruction on the same inputs, not a capture of the real
request.

Observation, not a finding: because the failed turns' prompts stay in
context, the model answered them too in run6.

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
| run5, run5b, run5b2 recall (429) | 0 | **3** (`usage {}`) | — | — | — | — |
| run6 recall (B.AI) | 1 | 0 | 7,584 | 650 | 0 | 453 |

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
- **F2 (decided 2026-09-29: keep, OMP parity):** compaction accepts any
  non-empty final text as the summary.
  - With `openrouter/free`, the summary call returned a safety-classifier
    line. It was persisted and now stands in for 22 entries in the model
    context. The raw entries stay in the journal.
  - The source model of that line is inferred from its format, not verified
    (see F3).
  - Fixed OMP does not check the content either.
    `summarizeConversationWindow`
    (`packages/agent/src/compaction/compaction.ts:931-1016` at `596f2da`)
    throws only on `stopReason === "error"`, then returns the text blocks
    joined with newlines, even when that text is empty. ARA already differs
    in one way: it refuses an empty summary.
  - The user left the choice to this session ("按照目标推进就行了。不用问我").
    V1 follows OMP, adds no content check and does not refuse router
    models. The practical guidance is to compact with a fixed model, not
    `openrouter/free`. A later check would be ARA's own evolution
    ([agent-evolution](../knowledge/agent-evolution.md)), not parity.
- **F3 (decided 2026-09-29: keep, OMP parity):** the `compaction` entry does
  not record the summary call's provenance.
  - `AcceptedSummary` carries `model_id`, `response_id`, `usage`,
    `duration_ms` and `ttft_ms` (`crates/ara-agent/src/compaction.rs:484-494`).
  - `run_compaction` persists only the text, the first-kept ID, the source
    IDs and `tokensBefore` (the `append_compaction` call in
    `crates/ara-cli/src/main.rs`).
  - So the summary's served model and usage are unknown after the fact.
  - Fixed OMP does not persist them either. Its `CompactionEntry`
    (`appendCompaction`, `packages/coding-agent/src/session/session-manager.ts:2409-2438`)
    stores `summary`, `shortSummary`, `firstKeptEntryId`, `tokensBefore`,
    `tokensAfter`, `method`, `providerReplayThroughEntryId`, `details`,
    `fromExtension` and `preserveData`. There is no model, response ID or
    usage. The summary call's usage goes only to OMP telemetry
    (`instrumentedCompleteSimple` with `oneshotKind: "compaction_summary"`),
    and ARA has no telemetry sink (AGT-TELEMETRY is open).
  - V1 keeps the entry as it is. Trial receipts record the summary call's
    usage as **unknown**, never zero.
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
  (TOOLS-01c, accepted on `111c325`).

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

- A real-model compaction with a useful summary is shown (run5), and a
  real-model recall that depends on it is shown (run6, B.AI
  `deepseek-v4.1-flash`, with the replay above). The summary and the recall
  came from different models; the Session carried the fact between them.
- On OpenRouter, run5, run5b and run5b2 all got HTTP 429 from the single
  gemma endpoint, while the run5 `/compact` call on the same model
  succeeded. Each failed model call may send up to 6 HTTP requests
  (`RetryPolicy` default: the first plus 5 retries); the exact count was not
  logged.
- Ctrl+C through a real console input path is shown by the ConPTY check in
  [V1-CANCEL](v1-repl-cancel.md#follow-up-real-console-ctrlc-through-conpty-2026-09-29)
  (fake upstream). A physical keypress in a visible terminal is not run.
- The trial ran on Windows only. On Linux CI for `43e79e2` (GitHub Actions
  run 36538669183), `ara-cli` e2e passed 73/73, including
  `repl_interrupt_during_bash_returns_to_prompt` and
  `repl_interrupt_during_compact_leaves_no_compaction_entry`. The workflow
  failed later, in `ara-tools` `tools.rs`, on the read long-path case. With
  that fix, run 36541328597 on `111c325` passed every step: 74 test binaries,
  1030 passed, 0 failed, 1 ignored; `ara-cli` e2e 74/74. The Windows gate
  (`verify_backend.py`) on `111c325` also passed: 1028 passed, 0 failed, 1
  ignored; e2e 72/72.
- F2 (any non-empty summary text is accepted) and F3 (the summary call's
  model, response ID and usage are not persisted) are decided: V1 keeps OMP
  parity, with no code change (see Findings).
- No independent review.
- **Decision:** changes requested (WIP). The counts are unchanged.
