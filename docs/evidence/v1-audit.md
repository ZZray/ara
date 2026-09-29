# V1 audit: daily-driver points (2026-09-29)

Combined point-delivery audit for plan rows V1-REPL, V1-CANCEL, V1-RESUME,
V1-COMPACT and V1-TRIAL ([plan](../plan.md#priority-daily-driver-v1)). It
follows `.ara/skills/point-delivery-audit` once for the five points, as the
user's faster V1 strategy decided.

## Snapshot and scope

- **Source:** OMP v18.1.8 at `596f2da` ([lock](../../upstream/omp.lock.json)),
  plus the ARA decisions in the plan (user decision 2026-09-28).
- **Delivered code:** `b179087`, the commit that first carried this audit.
  Its code is the tree after `74b80cb` plus the run9 fix
  ([V1-COMPACT](v1-compact.md#run9-fix-the-omp-summary-budget-2026-09-29)).
  Run10's `ara.exe` was built from this code. The later edits were a test
  string fix and mutations, each restored with a matching sha256.
- **Route:** OpenAI-compatible Chat Completions (`--api openai-completions`)
  with the key from an environment variable.
- **Excluded:**
  - OMP's TUI (CA-TUI-MODE stays open);
  - A3 remote, handoff, shake and pruning compaction;
  - Anthropic and OAuth routes;
  - print-mode auto-compaction timing (finding 2).

## Points

| Point | Requirement (plan exit criterion) | Executed evidence on the delivered code | Failure paths exercised | Real model |
| --- | --- | --- | --- | --- |
| V1-REPL | A multi-turn edit-and-test task in one process, every turn in one journal; EOF and `/exit` leave a valid Session | e2e `repl_runs_turns_in_one_journal_and_exits_on_eof`, `repl_help_and_exit_run_no_turn`, `repl_new_starts_a_fresh_session_file`, `repl_streams_text_and_reports_tool_progress`, `repl_shows_text_a_provider_sent_without_deltas`; streaming mutations m1–m3 caught ([V1-REPL/CANCEL](v1-repl-cancel.md)) | `repl_rejects_prompt_arguments`; `repl_failed_turn_shows_the_provider_cause_and_keeps_the_session`; `repl_unreadable_prompt_exits_1_and_keeps_the_session` | Run10 process a: four turns, one journal, the fix and `mode` with 7/7 tests, `/exit` 0 |
| V1-CANCEL | Interrupting a running `bash` returns to the prompt, the journal shows the abort, and the next turn works; Ctrl+C at an idle prompt exits | e2e `repl_interrupt_during_bash_returns_to_prompt`, `repl_interrupt_at_idle_prompt_exits_130`, `repl_interrupt_during_compact_leaves_no_compaction_entry`; the ConPTY real-console check with an A/B control | A crash mid-tool: `repl_resume_after_interrupted_tool_reports_unknown_effect_without_replay` | Run10 a3: Ctrl+C 3 s into `python slow.py`; the journal has the tool's aborted result and `assistant(aborted)`; a4 works. Run10 b: idle Ctrl+C exits 130 |
| V1-RESUME | Exit, restart with `--continue`, and the model uses the earlier turn's result; a failed resume is never replaced | e2e `repl_continue_replays_the_prior_turn_in_the_model_request`, `repl_resume_named_file_appends_to_that_session`, `repl_continue_uses_the_persisted_summary` ([V1-RESUME](v1-resume.md)) | `repl_failed_resume_exits_without_a_new_session`; the unknown-effect resume test above | Runs 6, 7, 9, 10: `--continue` recalls on B.AI |
| V1-COMPACT | A long session compacts, keeps working, and resumes after restart from the persisted summary | e2e `repl_manual_compact_then_keep_working`, `repl_auto_compact_threshold_and_disabled_companion`, `repl_compact_summarizes_past_an_interrupted_turn`, `repl_compact_uses_the_omp_summary_budget_and_names_a_truncated_summary`; `ara-agent` `compaction_cut`/`compaction_call`; `ara-session` `compaction_projection`; mutations caught ([V1-COMPACT](v1-compact.md)) | `repl_compact_failure_paths_leave_the_session_usable` (refusal causes, provider error, cut-off summary); interrupt during `/compact` | Run10 b2/b3/c1: a soft compaction that summarizes the aborted turn; the replay shows that c1's answer is present only in the summary |
| V1-TRIAL | A bounded real-model daily task through the REPL: artifact check, receipts, usage with unknowns kept, latency | [V1-TRIAL run10](v1-trial.md#run9-and-run10-compaction-on-the-delivered-code-2026-09-29): `--max-model-calls 8 --max-time 300` per process, B.AI `deepseek-v4.1-flash` | Run9 found a real failure (the summary budget); the session stayed untouched, and run10 closed it | Artifact 7/7 by an independent `python -m unittest`; per-step latency; usage per process with the aborted message counted as unknown |

### Commands on the delivered code (Windows)

| Command | Result |
| --- | --- |
| `python scripts/verify_backend.py` (fmt, strict Clippy, all tests) | PASS: 74 test binaries, 1036 passed, 0 failed, 1 ignored; e2e 75/75 |
| `cargo test -p ara-cli --test e2e repl_` | 21/21 |
| `cargo test -p ara-agent --test compaction_cut --test compaction_call` | 15/15 and 8/8 |
| Linux CI (GitHub Actions run 36567392227, `verify_backend.py` on `b179087`) | PASS: 74 test binaries, 1038 passed, 0 failed, 1 ignored; e2e 77/77 (the `cfg(unix)` tests included) |

## Reviews

All reviewers were separate Sonnet 5.5 subagents making static reviews
(read-only `git`, `grep` and file reads). None ran a build or tests; the
implementer ran the gate and the mutations.

| Scope | Verdict | Outcome |
| --- | --- | --- |
| Combined V1 review, `e7f94b6..111c325` (host and Session) | Changes requested (Finding 1) | Finding 1 fixed in `4de4ce1`; findings 3, 4 and 6 fixed; 2, 5 and 7 are follow-ups ([record](v1-compact.md#combined-v1-review-finding-1-and-fixes-2026-09-29)) |
| Fix review, `e809a35..4de4ce1` | Changes requested (small) | I1 decided by OMP parity with a test; I2 and M1–M7 fixed or answered ([record](v1-compact.md#fix-review-e809a354de4ce1-2026-09-29)) |
| Recheck, `4de4ce1..74b80cb` | Approve | Recheck 1 and 4 fixed; 2 and 3 recorded ([record](v1-compact.md#recheck-4de4ce174b80cb-2026-09-29)) |
| Recheck of the run9 fix, `74b80cb..` delivery tree | Approve | Four minor follow-ups ([record](v1-compact.md#recheck-of-the-run9-fix-74b80cb-delivery-tree-2026-09-29)) |

## Self-inspection

- **Ownership:**
  - `ara-agent` depends only on `ara-ai` and generic crates, and
    `ara-session` likewise. Neither names a product.
  - The host (`ara-cli`) owns the Session location (`--session-dir`), the
    working directory, the route and credentials, and the terminal.
- **Permissions:** the REPL uses the same tool set and working-directory
  rule as the print host (CLI-01b). V1 adds no permission surface. The tool
  allowlist is not an OS sandbox.
- **Cancellation:**
  - Ctrl+C aborts the turn and keeps the process. A second Ctrl+C during a
    turn exits at once, and Ctrl+C at an idle prompt exits 130.
  - Cancelling `/compact` leaves no compaction entry.
  - The stale-interrupt race is a recorded follow-up.
- **Unknown effects:**
  - A tool that returns after a live cancel keeps its own result. For Bash
    this is the partial output and `[Command aborted]`
    (`packages/coding-agent/src/tools/bash.ts:657-658` at `596f2da`).
  - A tool that a crash or kill interrupts is paired on resume with a
    synthetic `interrupted_unknown_effect` result. It is never replayed.
  - Compaction refuses timed-out, panicked and interrupted-unknown results.
  - **Plan wording:** the plan row says in-flight effects "are recorded as
    unknown". By OMP parity, a live cancel keeps the tool's aborted result
    instead. The ledger row states this.
- **Credentials:**
  - Keys are read only through `--api-key-env`. They are not persisted in
    the journal.
  - Scans of the run9, run10 and probe directories found no key value.
- **Usage:**
  - Unknown usage is kept unknown; the aborted message has none.
  - The summary call's model, response ID and usage are not persisted, as
    in OMP (F3).
- **Regression:** the full gate on the delivered code on Windows and on
  Linux CI, and mutation checks on each fix.

## Follow-ups (not blocking daily use)

- **Combined review:** 2, print-mode auto-compaction deadline; 5,
  `--print-thoughts` in the REPL; 7, the `--report-request-text-tokens`
  flush.
- **Run9-fix recheck:** 1, 3 and 4 (stop-reason wording with an HTTP status,
  routes with a lower output limit, `--max-tokens 0`). Finding 2, the
  upper-cap test, was fixed after acceptance by
  [dogfood 1](v1-dogfood.md).
- **Cancel and resume:**
  - the stale-interrupt race;
  - a physical keypress in a visible terminal (ConPTY covers the console
    path).
- **Compaction:**
  - a timed-out tool result blocks every later cut;
  - an M1 e2e test with a hand-written non-soft compaction entry;
  - the summarizer reads a user Ctrl+C `[Command aborted]` as an unexpected
    abort, and OMP has the same text;
  - ~~the `//?/C:` working directory appears in prompts and summaries~~:
    fixed after acceptance by [dogfood 2](v1-dogfood.md);
  - the "down to" wording.

## Decision

**Accepted on `b179087`:** V1-REPL, V1-CANCEL, V1-RESUME, V1-COMPACT and
V1-TRIAL. The mandatory execution and audit evidence passes on the delivered
code:

- the Windows gate and Linux CI;
- the focused tests with their failure paths and mutations;
- run10 through the REPL on B.AI `deepseek-v4.1-flash`, with an
  independent artifact check;
- four scoped reviews, with every critical and important finding resolved.

The follow-ups above do not block daily use. Registered points move from
28/45 to 33/45 (73.3%). The formal gates stay at 1/7. `ported_through_commit`
is unchanged.
