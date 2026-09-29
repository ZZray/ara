# V1 dogfood log (2026-09-29)

After V1 was accepted on `b179087` ([V1 audit](v1-audit.md)), the REPL is used
on real tasks in this repository. Only what blocks daily use gets fixed. Each
entry records the task, the route, the result and an independent check.

## Common setup

- **Route:** B.AI `https://api.b.ai/v1`, `--api openai-completions`,
  `--api-key-env BAI_API_KEY`, `deepseek-v4.1-flash`.
- **Binary:** `ara.exe` built from the `b179087` code (dogfood 1 and 2) and
  from `a6b6bd0` (dogfood 3).
- **Workspace:** a fresh local clone under `C:\Temp`, not the working
  repository. A useful change is ported back after it is checked.

## Dogfood 1: the upper-cap test for the summary budget

- **Task:** recheck finding 2 of the run9 fix
  ([V1-COMPACT](v1-compact.md#recheck-of-the-run9-fix-74b80cb-delivery-tree-2026-09-29)).
  Add a case showing that `--max-tokens 100000` does not raise the summary
  budget above 13,107, then run the test.
- **Run:** one prompt and `/exit` on piped stdin, with
  `--max-model-calls 30 --max-time 1500`. Exit 0 after 67 s.
  - The tools used were `grep` ×3, `read` ×1, `edit` ×1 and `bash` ×1
    (`cargo test -p ara-cli --test e2e repl_compact_uses_the_omp_summary_budget`).
  - The answer names the change and the result (`1 passed`).
- **Result:** one hunk, +23 lines in `crates/ara-cli/tests/e2e.rs`. It
  follows the `--max-tokens 700` case and asserts that the summary request
  carries `max_tokens` 13107. Nothing else changed.
- **Usage:** 5 assistant messages, all with usage: input 12,181, output
  1,167, cache read 38,528, total 51,876.
- **Independent check:**
  - In the clone, the test passes.
  - A mutation that sends `--max-tokens` without the cap (`|cap| cap`) fails
    the new assertion (`left: 100000`, `right: 13107`).
  - In the working repository, with the change applied:
    - `cargo fmt -p ara-cli -- --check` exit 0;
    - `cargo clippy -p ara-cli --all-targets -- -D warnings` clean;
    - `cargo test -p ara-cli --test e2e repl_` 21/21.
- **Blockers seen:** none.

## Dogfood 2: a plain `--cwd` path, as in OMP

- **Problem:** on Windows, `--cwd` went through `std::fs::canonicalize`, which
  returns a verbatim `\?\C:\x` path. The prefix reached the Session header,
  the date/cwd reminder (`//?/C:/...`) and summaries (the V1 audit follow-up;
  [run10](v1-trial.md) shows it). OMP's `applyStartupCwd`
  (`packages/coding-agent/src/cli/startup-cwd.ts:47-66` at `596f2da`) keeps
  the plain absolute path from `setProjectDir`.
- **Run:** started in the clone without `--cwd`, as in daily use, with
  `--max-model-calls 40 --max-time 1800`. Exit 0 after 488 s.
  - 38 assistant messages (37 tool use, 1 stop), all with usage: input
    137,338, output 18,779, cache read 2,145,024, total 2,301,141.
  - Tools: `read` 12, `grep` 7, `bash` 14, `edit` 9.
  - One edit was rejected by the hashline guard (the body restated the line
    below the range), and the retry succeeded.
  - The model also ran the whole e2e suite and updated
    `context_files_and_skills_reach_the_model_and_skill_urls_resolve`, whose
    expected path carried the prefix.
- **Result:**
  - `plain_drive_path` in `crates/ara-cli/src/main.rs` rewrites only a
    verbatim drive prefix after `canonicalize`. Verbatim UNC and device paths
    keep their spelling.
  - The new e2e test is
    `absolute_cwd_flag_never_reaches_the_header_or_the_model_request_verbatim`.
- **Review finding, fixed before the port:** the model's test read the
  journal's first line as the Session header, but that line is the title
  entry, so its header check could not fail. The ported test finds the
  `session` entry and asserts that its `cwd` equals the plain path. The
  request-body check is kept.
- **Independent check in the working repository:**
  - `cargo fmt -p ara-cli -- --check` exit 0; the two tests pass.
  - A mutation that keeps the canonical path fails both tests. The header
    assertion shows `left: "\\?\C:\..."`, `right: "C:\..."`.
  - `python scripts/verify_backend.py` (fmt, strict Clippy, all tests):
    PASS, 1037 passed, 0 failed, 1 ignored; e2e 76/76.
- **Review:** the session's own model (Claude Opus 5.5) reviewed the
  REPL's diff line by line. There was no separate subagent review for this
  small host change.
- **Effect on sessions:** `--cwd X` now gets the same default Session
  directory as starting `ara` in `X`. Sessions made earlier with `--cwd` and
  no `--session-dir` stay in the old `------C--…` directory; `--resume <file>`
  still opens them.
- **Blockers seen:** none. The REPL finished a two-file fix with the test
  update on its own; only the vacuous header check needed a human review.

## Dogfood 3: `--print-thoughts` by OMP parity, with a compaction in the middle

- **Task:** finding 5 of the combined V1-COMPACT review (the REPL ignores
  `--print-thoughts`). Read OMP to see where the flag applies, record the
  decision in `v1-compact.md`, run `/compact`, then ask about the earlier
  turn with no tool.
- **Run:** four inputs and `/exit` on piped stdin, with
  `--max-model-calls 30 --max-time 900 --compact-keep-tokens 2000`. Exit 0
  after about 200 s.
  - 11 assistant messages, all with usage: input 18,424, output 2,754,
    cache read 109,952, total 131,130.
  - Tools: `grep` 4, `read` 4, `bash` 2, `edit` 1. The model read the OMP
    checkout by absolute path.
  - Turn 1 cited `packages/coding-agent/src/main.ts:1562`
    (`printThoughts && !isProtocolMode && !isInteractive`), plus lines 1478
    and 1482. Those lines match the source at `596f2da`, and so does
    `modes/print-mode.ts:232`, which it cited in turn 2.
  - Turn 2 changed one table cell and reported `git diff --stat`
    (1 file, +1 −1).
  - `/compact` summarized turn 1 (13 source entries, 6,385 estimated
    tokens down to 2,595). The first kept entry is the turn 2 prompt.
  - The question after the compaction was answered correctly with no tool
    call, from the summary (turn 1) and the kept turn 2.
- **Result:** OMP applies the flag only in single-shot print mode, so the
  REPL ignoring it is parity. No code change. The ported cell says "Decided
  by OMP parity" instead of the model's "Accepted as parity" and links this
  entry; the rest is the model's text.
- **Seen, not blocking:**
  - The REPL progress line for `edit` names no file (`ara: tool edit`). The
    hashline `edit` takes a single `input` whose `[path#hash]` header holds
    the path, and `tool_summary` only looks at `command`, `path` and
    `pattern`. Fixed in the
    [next follow-up](#follow-up-the-edit-progress-line-names-its-file).
  - The turn 2 answer paraphrased the new cell instead of quoting it. That is
    the model, not ARA.
- **Review:** the session's own model (Claude Opus 5.5) checked the diff and
  each OMP line against the source. Doc-only, with no separate review.
- **Blockers seen:** none.

## Follow-up: the edit progress line names its file

- **Why:** every edit in daily use printed a bare `ara: tool edit`, so the
  REPL did not show which file the model was changing (dogfood 3).
- **OMP:** `editToolRenderer.activitySummary`
  (`packages/coding-agent/src/edit/renderer.ts:889-902` at `596f2da`) shows
  the operation and the target path instead of the payload's first line, and
  adds `(+N more)` for more files. `resolveEditCallFacts` (lines 728-774)
  takes the path from `path` or the first parsed `input` entry.
- **Change (`ara-cli` `main.rs`):**
  - `HostSink` keeps the `--edit-mode`.
  - For an `edit` call with only `input`, `tool_summary` names the first file
    that the existing `ara-edit` parser for that mode finds (hashline
    `Patch::parse`, `parse_apply_patch_streaming` or
    `split_sloppy_sections`), plus `(+N more)`.
  - A payload that does not parse, and an `input` on another tool, keep the
    bare line. Replace and patch modes still show their `path`.
  - Intentional difference: the line keeps its `ara: tool edit` prefix and
    has no operation label (OMP's create, delete or move title).
  - The line is REPL progress on stderr only; no tool, request or journal
    data changes.
- **Tests:**
  - Unit test `edit_progress_names_the_target_file`: hashline with one and
    two files, apply_patch, sloppy, an unparseable payload, another tool,
    and patch mode.
  - e2e test `repl_edit_progress_names_the_target_file`: with
    `--edit-mode apply_patch`, a real `edit` call adds `notes.txt` and stderr
    shows `ara: tool edit: notes.txt`.
- **Mutations:**
  - Passing a fixed hashline mode to the sink fails the e2e test.
  - Dropping the `input` fallback fails both tests.
  - The source was restored, with matching sha256.
- **Checks:** `python scripts/verify_backend.py` (fmt, strict Clippy, all
  tests): PASS, 1041 passed, 0 failed, 1 ignored; e2e 78/78.
- **Review:** the session's own model (Claude Opus 5.5) implemented and
  checked it. This is a small display change, so it had no separate
  review.

## Follow-up: a timed-out tool no longer blocks compaction

This follow-up was implemented directly in the working repository, not
through the REPL. The audit named it, and in daily use it stops compaction
for the rest of a long session: a Bash call that hits its timeout (the
default is 300 s, and a long `cargo` build can hit it) left a result that
every later cut had to include.

- **Why it was refused:** the result carried `timedOut: true`, and both
  `has_unknown_tool_effect` (`ara-agent`) and `safe_soft_summary_prefix`
  (`ara-session`) treated that as an unknown effect.
- **Why it is not unknown:** in `crates/ara-tools/src/bash.rs` a timeout and
  a cancel leave the same `select!` loop and run the same process-group kill
  (a Job object on Windows). `bash_timeout_kills_process_group` in
  `crates/ara-tools/tests/tools.rs` shows that a background child does not
  survive. The tool returns its own error result, which is the output plus
  `[Command timed out after N seconds]`. Fix review I1 already decided that a
  cancelled call is summarized with its own result, as in OMP. OMP has no
  refusal of this kind.
- **Partial effects:** the AGT-COMPACTIONa review noted that a timed-out
  command may have partial effects. That holds for a cancel or a non-zero
  exit too. The effects are the tool's own, and the output and marker record
  them; nothing keeps running after the kill.
- **Long output:** the summary input keeps the first 2,000 characters of any
  tool result, as it does for every result. A trailing marker after longer
  output is dropped there, but `is_error: true` still reaches the summarizer.
  The kept session entry keeps the full result.
- **Decision (OMP parity):** timed-out results are summarized like cancelled
  ones. Panicked and synthetic `interrupted_unknown_effect` results are still
  refused, because the tool did not finish normally and its state is unknown.
- **Change:**
  - `timedOut` was removed from both guards, and the doc comment on
    `validate_completed_summary_span` was updated.
  - `prompt_requires_matched_tool_receipts` now expects a timed-out receipt
    to be summarized with `unknown_effect: false`.
  - The cut tests' unknown-effect fixture uses `panicked` instead.
  - New test `a_timed_out_call_does_not_block_later_cuts` in `ara-agent`.
  - New e2e test `repl_compact_summarizes_past_a_timed_out_tool`:
    - a real Bash call times out after 1 s;
    - `/compact` then cuts past that turn;
    - the summarizer sees `[Command timed out after 1 seconds]` and
      `"unknown_effect":false`.
- **Mutations:**
  - Putting `timedOut` back into only the Agent guard fails the new e2e test
    and `prompt_requires_matched_tool_receipts`.
  - Putting it back into only the Session guard fails the new e2e test and
    `projection_rejects_panic_receipts_but_keeps_completed_failures`, which
    now also expects a `timedOut` receipt to be kept.
  - The reviewer found that the Agent-guard mutation also fails
    `a_timed_out_call_does_not_block_later_cuts`.
  - The sources were restored, with matching sha256.
- **Checks:**
  - `cargo test -p ara-agent --test compaction --test compaction_cut`:
    13/13 and 16/16.
  - `cargo test -p ara-cli --test e2e repl_compact`: 4/4.
  - `cargo test -p ara-session --test compaction_projection`: 8/8.
  - `python scripts/verify_backend.py`: PASS, 1039 passed, 0 failed,
    1 ignored; e2e 77/77.
  - Linux CI run 36573002679 on `a6b6bd0`: success.
- **Review:** the session's own model (Claude Opus 5.5) was the implementer.
  Sonnet 5.5 independent static review: approve, no critical or important
  finding. Its six minor findings were resolved: the panicked wording, the
  superseded notes in the AGT-COMPACTIONa evidence, V1-COMPACT and the
  2026-09-26 handoff, the partial-effects and long-output notes, the Session
  projection test, the mutation list and the ledger note.
