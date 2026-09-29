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
