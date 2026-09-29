# V1 dogfood log (2026-09-29)

After V1 was accepted on `b179087` ([V1 audit](v1-audit.md)), the REPL is used
on real tasks in this repository. Only what blocks daily use gets fixed. Each
entry records the task, the route, the result and an independent check.

## Common setup

- **Route:** B.AI `https://api.b.ai/v1`, `--api openai-completions`,
  `--api-key-env BAI_API_KEY`, `deepseek-v4.1-flash`.
- **Binary:** `ara.exe` built from the `b179087` code.
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
