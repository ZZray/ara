# V1-RESUME: `--continue` / `--resume` on the recovered Session (WIP)

Status: **implementing (WIP)**. Evidence is from the working tree after
`6a6ef96`. No independent review has run and there is no real-model trial, so
nothing is marked tested or accepted.

## Scope

[Plan](../plan.md#priority-daily-driver-v1) row V1-RESUME: `--continue` and
`--resume <path>` enter the same REPL on the recovered Session. A failed resume
is reported and never replaced by a new conversation.

This receipt covers the four required end-to-end tests in
`crates/ara-cli/tests/e2e.rs`, using the controlled fake upstream (no network,
no real model). Production code is unchanged: the resume path already existed
in `crates/ara-cli/src/main.rs` (`run()` around `let session_dir =` and the
`run_repl_loop` entry), and these tests did not expose a defect against the
plan row.

## Code changed

- `crates/ara-cli/tests/e2e.rs` only: four tests below, plus the
  `kill_mid_tool` helper (Windows `taskkill /F /T`, Unix `kill` + `pkill -f`)
  that builds the same end state as the Unix-only
  `crash_mid_tool_resume_reports_unknown_effect_without_replay`.
- Production code: **none**.

## Tests

| Test | Observed |
| --- | --- |
| `repl_continue_replays_the_prior_turn_in_the_model_request` | Process 1: `--repl` one turn (`alpha-turn` → `First answer.`), EOF exit 0. Process 2: `--repl --continue` (`beta-turn` → `Second answer.`). The second model request carries `alpha-turn` and the assistant text `First answer.`. Still exactly one Session file; journal `model_change, user, assistant, user, assistant` with user texts `alpha-turn, beta-turn`. |
| `repl_resume_named_file_appends_to_that_session` | Two print-mode runs create two Session files. `--repl --resume <older>` runs `third turn`. The older file gains `first turn, third turn`; the newer file still holds only `second turn`. The third model request includes `first turn` and never `second turn`. No extra Session file. |
| `repl_failed_resume_exits_without_a_new_session` | `--repl --resume <missing path>` exits 2, prints `opening session <path>`, `up.served()==0`, `session_files()==[]`. `--repl --continue` with an empty session directory exits 2, prints `no session to continue in <dir>`, `up.served()==0`, `session_files()==[]`. |
| `repl_resume_after_interrupted_tool_reports_unknown_effect_without_replay` | A print-mode run is killed mid-`bash` (unique tag in the command, `runs.log` written) so the journal ends with `call_crash` and no result. `--repl --resume` prints `call_crash were interrupted before a result was recorded; their effects are unknown and they were not re-run`. `runs.log` still holds a single `run` line (not replayed). The next REPL turn is answered (`Resumed after checking.`). Journal `model_change, user, assistant, toolResult, user, assistant`; the `toolResult` has `details.source=interrupted_unknown_effect` and `isError=true`. The resumed model request carries a `tool` message for `call_crash` containing `effects are unknown`. |

## Commands and pass counts (Windows, 2026-09-29)

Environment: `CARGO_TARGET_DIR=C:\Temp\ara-verify-target`, test/dev debug
info and incremental builds disabled, Git Bash on `PATH`, controlled fake
upstream.

| Command | Result |
| --- | --- |
| `cargo fmt --check -p ara-cli` | exit 0 |
| `cargo clippy -p ara-cli --all-targets -- -D warnings` | exit 0 |
| `cargo test -p ara-cli --test e2e` | **64/64** (60 prior + 4 new) |
| `cargo test -p ara-cli --bin ara` | **8/8** |
| new tests, three consecutive runs (`--exact` on the four names) | **4/4 × 3**, all green |

## Mutations

Each named production change that should make one test fail. Two were applied
one at a time; `main.rs` was restored with a byte-for-byte `cmp` against a
saved copy after each.

| Test | Named mutation | Applied? | Result |
| --- | --- | --- | --- |
| `repl_continue_replays_the_prior_turn_in_the_model_request` | `--continue` ignores `latest_session` and always creates a new Session | not applied (named only) | — |
| `repl_resume_named_file_appends_to_that_session` | `--resume <path>` ignores `path` and resumes the newest Session | not applied (named only) | — |
| `repl_failed_resume_exits_without_a_new_session` | **Mutation B:** `SessionJournal::open` failure falls through to `SessionJournal::create` | **applied** | test **fails**: exit 0 (REPL on a new conversation) instead of 2 |
| `repl_resume_after_interrupted_tool_reports_unknown_effect_without_replay` | **Mutation C:** skip `recover_interrupted_tool_calls` | **applied** | test **fails**: no unknown-effect warning on stderr and no synthesized `toolResult` |

## Review (2026-09-29)

- **Implementer:** an external agent (MiMo v2.6), working from a
  self-contained task prompt.
- **Reviewer:** the coordinating Claude Code session (Opus 5.5), reviewing
  against the working-tree diff.
- **Scope check:** only `crates/ara-cli/tests/e2e.rs` (+176 lines) and this
  file changed. `main.rs` has no diff.
- **Reruns on the same tree:**
  - `cargo fmt --check -p ara-cli` exit 0;
  - `cargo clippy -p ara-cli --all-targets -- -D warnings` exit 0;
  - `cargo test -p ara-cli --test e2e` **64/64**;
  - the four new tests 4/4 × 3.
- **Extra mutation:** the reviewer applied the mutation the implementer only
  named for the resume-a-named-file test: `--resume <path>` ignores the path
  and resumes `latest_session`. `repl_resume_named_file_appends_to_that_session`
  fails at the older-journal assertion. `main.rs` was restored and matches its
  saved copy byte-for-byte.
- **Result:** no blocking findings.
- **Nits (not fixed):**
  - The `kill_mid_tool` doc says it kills the process tree. That is true
    only of the Windows `taskkill /T` arm.
  - On Unix, as in the existing crash test, `pkill -f <tag>` may miss the
    `sleep` child, which can then linger for up to 30 s.

## Gaps

- **Unix not compiled:** `kill_mid_tool`'s `cfg(unix)` arm (`child.kill` +
  `pkill -f`) and the existing `cfg(unix)` crash/interrupt tests were not
  compiled here. Linux CI with `-D warnings` is still required. The Windows
  `taskkill` arm is the one exercised above.
- **No real model:** every request went to the controlled fake upstream.
- **No real console:** the REPL tests use piped stdin and EOF; Ctrl+C at a
  real console is V1-TRIAL territory.
- **Full gate:** `python scripts/verify_backend.py` has not run on this
  snapshot.
- **Counts:** unchanged; nothing is marked tested or accepted.
