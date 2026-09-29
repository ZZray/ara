# V1-REPL + V1-CANCEL: line REPL with turn cancellation (WIP)

Status: **implementing (WIP)**. The REPL and cancel code is in WIP commit
`6a6ef96`; the streaming follow-up is in `43e79e2`, and the failed-turn cause
(F6 in [V1-TRIAL](v1-trial.md)) is in `111c325`. Linux CI and the Windows
gate pass on `111c325`. A real-console Ctrl+C check through ConPTY passes
(below). No independent review has run, so nothing is marked tested or
accepted.

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

## Follow-up: streamed text and tool progress (2026-09-29)

**Gap found:** the REPL printed nothing on stdout until a turn ended, and then
printed only the text of the last assistant message. A user had no sign of
progress during a long turn, and any text before a tool call was never shown.
No earlier record listed this gap.

**Change** (`crates/ara-cli/src/main.rs`, REPL text mode only):

- `HostSink` gains a `stream` flag. When it is set:
  - each `TextDelta` is written to stdout at once;
  - a new text block or a tool call first closes an open line;
  - each tool call prints `ara: tool <name>: <summary>` on stderr when it
    starts, and `ara: tool <name> done|failed` when it ends. The summary is
    the first string among `command`, `path`, `pattern` and `pat`, with
    whitespace folded and cut to 120 characters.
- If a provider delivered an assistant message's text with no delta (for
  example Responses with only `response.output_item.done`), the text is
  printed at `MessageEnd`.
- The REPL loop no longer prints the final answer after the turn. It only
  closes the open line and still prints the last assistant's
  `error_message`.
- Print mode and JSON mode are unchanged. Print mode still prints the final
  text once, as before.

**Changed behavior:** REPL text mode now shows the text of every assistant
message in a turn, including text before a tool call, and no longer only the
last message's text.

| Test | Observed |
| --- | --- |
| `repl_streams_text_and_reports_tool_progress` | The first response streams `Checking first.`, then waits 4 s before its Bash tool call. The text appears on stdout in under 3 s after the prompt line is sent. The start line `ara: tool bash: echo progress-ran; sleep 3` appears while the 3 s tool still runs. stderr order is `Working... (turn 1)` < start < `ara: tool bash done`. stdout is exactly `Checking first.\nAll done.\n`: two deltas join into one line, and nothing is printed twice. 2 model calls |
| `repl_shows_text_a_provider_sent_without_deltas` | `--api openai-responses` with only `response.output_item.done` and `response.completed`: stdout is exactly `Whole answer.\n`; 1 model call |

- The two tests passed three consecutive runs, both before and after the
  string literals in the first test were rewritten with `\n` escapes.
- `cargo test -p ara-cli --test e2e`: 71/71. `cargo test -p ara-cli --bin ara`:
  8/8.
- `cargo clippy -p ara-cli --all-targets --all-features -- -D warnings` and
  `cargo fmt -p ara-cli --check` exit 0.
- `StderrLog::reading` now also collects stdout in the first test; the other
  tests are unchanged.

Mutation checks on `main.rs`, restored from a saved copy and checked by
sha256 afterwards:

| Mutation | Caught by |
| --- | --- |
| m1: deltas are not written (`write!(out, "")`, and `streamed`/`line_open` are not set) | `repl_streams_text_and_reports_tool_progress` fails at `text waited for the message end: 4.02s` |
| m2: no fallback print at `MessageEnd` | `repl_shows_text_a_provider_sent_without_deltas` fails |
| m3: no tool start line | `repl_streams_text_and_reports_tool_progress` fails at `no tool start line` |

This follow-up has no independent review.

## Follow-up: real-console Ctrl+C through ConPTY (2026-09-29)

**Why:** the e2e tests use piped stdin and Ctrl+Break, so they cover neither
a console `stdin` (`ReadConsoleW` pending in `read_line`) nor the Ctrl+C
event itself.

**Method:** [`scripts/windows_conpty_ctrl_c.py`](../../scripts/windows_conpty_ctrl_c.py)
starts `ara` inside a pseudoconsole (ConPTY), the host Windows Terminal uses.
It passes no `--repl` flag, so the REPL is chosen by `stdin.is_terminal()`.
The script writes the prompt lines and Ctrl+C (`\x03`) to the pseudoconsole
input pipe, as a terminal does for a key press. The console host turns
`\x03` into a CTRL_C_EVENT. The model is the controlled fake upstream: a
`bash` call `echo started; sleep 30`, then `After the abort.`, then a third
response that must not be requested. The binary is the debug build of the
committed `111c325` sources.

**Inherited ignore flag.** A process created with `CREATE_NEW_PROCESS_GROUP`
has Ctrl+C disabled, and its children inherit that. The process that ran the
script (this session's tool runner) passed the flag on, as the A/B below
shows. A shell in Windows Terminal has Ctrl+C enabled. `--enable-ctrl-c`
calls `SetConsoleCtrlHandler(NULL, FALSE)` before `ara` starts, so `ara`
inherits the enabled state.

| Run | Observed |
| --- | --- |
| `--enable-ctrl-c`, six runs: four with a scratch copy of the script, then two with the committed script (one before the mutation below, one after the restore) | All checks pass every time. Ctrl+C during the Bash tool prints `ara: interrupt received, aborting …`, `ara: tool bash failed` and `ara: turn 1 cancelled; session kept, type the next prompt` about 0.05 s after the byte is written. The process survives, the next prompt answers `After the abort.`, and Ctrl+C at the idle prompt exits 130 within about 0.01 s. There are 2 model calls, and each run takes about 2.8 s. The journal is `session, model_change, user, assistant, toolResult, assistant, user, assistant` with stop reasons `toolUse, aborted, stop`. The tool result ends with `[Command aborted]` and has `isError: true`. |
| Flag kept (A/B control) | FAIL, as expected. Ctrl+C has no effect: `sleep 30` runs to completion (tool result `started`, not an error), the canned second response becomes turn 1's answer, 3 model calls are made, and Ctrl+C at the idle prompt does not exit within 10 s |

After the runs, no `sleep.exe` or `bash.exe` process was left.

This check also asserts the tool-result text, which the Windows e2e test
cannot (see Gaps).

**Mutation** (`main.rs`, restored and checked by sha256 afterwards): the
Windows listener waits only on Ctrl+Break (`let got = b.recv().await`), and
the Ctrl+C handler stays installed. `cargo test -p ara-cli --test e2e
repl_interrupt` still passes 3/3, because the e2e tests send Ctrl+Break. The
ConPTY check fails on the same six checks as the A/B control. After the
restore, both pass again on a rebuilt binary.

**Observation (not changed):** when `ara` inherits the ignore flag (for
example from a runner that uses `CREATE_NEW_PROCESS_GROUP`), Ctrl+C in its
console neither cancels a turn nor exits at the idle prompt. Ctrl+Break still
works. `ara` does not clear the flag itself.

**Limits:** these are bytes on the ConPTY input pipe, not a physical key
press, and the model is the fake upstream. The legacy console window
(conhost without ConPTY) was not tested.

## Gaps

- ~~**Real console**~~: shown through ConPTY (above). A physical key press in
  a visible window has not been done.
- ~~**Unix**~~: done. Linux CI ran the REPL tests on `43e79e2` (e2e 73/73,
  both `repl_interrupt_*` tests) and on `111c325` (every step green, e2e
  74/74).
- **Windows Bash result text:** on Windows, Ctrl+Break also reaches Bash in
  the same process group, so the e2e test does not assert the tool-result
  text. The ConPTY check does assert it.
- **Review:** no independent review has run. A Sonnet subagent attempt failed
  immediately with HTTP 429, because the alias routed to a model that needs
  usage credits, and it made no changes. This is an explicit review gap.
- ~~**Full gate**~~: done. `python scripts/verify_backend.py` passed on
  `111c325`.
- **Counts:** unchanged at 28 accepted points and 1/7 gates.
