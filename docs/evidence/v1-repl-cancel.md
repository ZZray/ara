# V1-REPL + V1-CANCEL: line REPL with turn cancellation (WIP)

Status: **implementing (WIP)**. The code and tests are on the working tree
after `f20393d`. No independent review has run yet and there is no real-model
trial, so nothing is marked tested or accepted.

## Scope

[Plan](../plan.md#priority-daily-driver-v1) rows V1-REPL and V1-CANCEL. This
is a reference-host subset in `ara-cli`, not a port of OMP's TUI.

- `ara` with no prompt on a terminal, or `--repl` with any stdin, reads one
  prompt per line and runs each as a turn in the same Session journal.
  `--repl` conflicts with prompt arguments (exit 2), and in REPL mode stdin
  is never merged into a prompt.
- `/help`, `/new`, `/compact` and `/exit` (alias `/quit`) are built in; EOF
  also exits 0. `/new` opens a fresh Session file and keeps the previous one;
  under `--no-session` it only clears the in-memory conversation.
- Each turn and each compaction gets its own child of the root cancellation
  token. The first interrupt (SIGINT on Unix; Ctrl+C or Ctrl+Break on
  Windows) cancels only that step. The agent loop records the abort in the
  journal and the REPL prints the next prompt. A second interrupt while the
  step is still winding down exits with 130, as print mode does. An
  interrupt at the idle prompt exits with 130.
- Print mode keeps its previous interrupt behavior and now shares the same
  listener.

Intentional differences from OMP are in the `main.rs` module doc. OMP aborts
a turn with Esc and exits on a double Ctrl+C; ARA uses Ctrl+C for the abort
and a single Ctrl+C at the idle prompt to exit.

## Code

- `crates/ara-cli/src/main.rs`: `listen_for_interrupts`, `interruptible`,
  `ReplSession`, `run_repl_loop`, the `--repl` flag, and `session_dir` hoisted
  so `/new` can reuse it.
- `crates/ara-cli/tests/e2e.rs`: `spawn_repl` (Windows
  `CREATE_NEW_PROCESS_GROUP`), `interrupt` (Unix `SIGINT`; Windows
  `GenerateConsoleCtrlEvent(CTRL_BREAK_EVENT)`), `StderrLog`, `wait_exit`, and
  the six tests below.
- `crates/ara-cli/Cargo.toml`: the Windows dev-dependency `windows-sys` gains
  `Win32_System_Console` and `Win32_System_Threading`.

This change carries the uncommitted V1-COMPACT work of the Cursor session
(`--compact-threshold`, `--compact-keep-tokens`, `run_compaction`,
`ara-session` `append_compaction` and `model_context`,
[receipt](v1-compact.md)) unchanged. That work has not been reviewed
either.

## Executed checks (Windows, 2026-09-29)

Environment: `CARGO_TARGET_DIR=C:\Temp\ara-verify-target`, test/dev debug
info and incremental builds disabled, Git Bash on `PATH`, controlled fake
upstream (no real model).

| Test | Observed |
| --- | --- |
| `repl_runs_turns_in_one_journal_and_exits_on_eof` | Two turns in one file (`model_change, user, assistant, user, assistant`); the second request carries the first turn and its answer; exit 0 |
| `repl_help_and_exit_run_no_turn` | `/help` then `/exit`: no model call, no Session file written, exit 0 |
| `repl_new_starts_a_fresh_session_file` | Two files, each with only its own turn; the second request lacks the first turn; stderr names the kept file |
| `repl_rejects_prompt_arguments` | `--repl "some prompt"` exits 2, no model call |
| `repl_interrupt_during_bash_returns_to_prompt` | Interrupt during `sleep 30` Bash: `agent_end`, `ara: turn 1 cancelled; session kept`, the process survives, the next line is answered; 2 model calls; journal `…, toolResult, assistant(stopReason aborted), user, assistant`; whole test under 20 s |
| `repl_interrupt_at_idle_prompt_exits_130` | Interrupt at `> ` exits 130, no model call |

- The six tests passed three consecutive runs.
- `cargo test -p ara-cli --test e2e`: 60/60.
- `cargo test -p ara-cli --bin ara`: 8/8.
- `cargo test -p ara-session`: 28/28.
- `cargo clippy -p ara-cli -p ara-session -p ara-tools --all-targets -- -D warnings`
  and `cargo fmt --check` on those crates exit 0.

Mutation checks, each restored and compared byte-for-byte afterwards:

| Mutation | Caught by |
| --- | --- |
| A turn uses the root token instead of `cancel.child_token()` | `repl_interrupt_during_bash_returns_to_prompt` fails (the whole REPL is cancelled) |
| `/new` clears the context but keeps the old journal | `repl_new_starts_a_fresh_session_file` fails |
| No Ctrl+Break listener on Windows | both interrupt tests fail |

## Gaps

- **Real console:** Ctrl+C was not tested in a real interactive console
  (`ReadConsole` behavior with a pending `read_line`). The tests use piped
  stdin and Ctrl+Break. This belongs to V1-TRIAL.
- **Unix:** the `cfg(unix)` test code (`libc::kill` with `SIGINT`, the
  `[Command aborted]` assertion) was not compiled on this host. It needs a
  green `ubuntu-latest` run, which requires a push.
- **Windows Bash result text:** on Windows, Ctrl+Break also reaches Bash in
  the same process group, so the test does not assert the tool-result text.
- **Review:** no independent review has run. A Sonnet subagent attempt failed
  immediately with HTTP 429, because the alias routed to a model that needs
  usage credits, and it made no changes. This is an explicit review gap.
- **Full gate:** `python scripts/verify_backend.py` has not run on this
  snapshot.
- **Counts:** unchanged at 28/40 registered points and 1/7 gates.
