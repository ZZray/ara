# CLI-01 — `ara` print-mode host chain

Point / requirement / exclusions:
The actual Rust host binds the Core end to end:
- CLI → OpenAI-compatible route → real tools → session journal, with text and JSON output
- resume/continue, including interrupted-tool recovery
- SIGINT, deadline and model-call budget
- key isolation and output sanitization

Excluded (open):
- interactive/TUI, RPC/ACP, extensions, skills and context files
- upstream system prompt templates (ARA uses a short default)
- model registry/auth storage, `@file` arguments and images on the CLI
- **bounded real-model trial** (blocked: see below)

Upstream SHA and source/test location:
`596f2da7101178214aa27a753529d15e6b7ad91d`:
- `packages/coding-agent/src/modes/print-mode.ts` (`runPrintMode`, `printableEvent`)
- `cli/initial-message.ts` (stdin prepend)
- `pi-utils` `sanitizeText`

Upstream behavior items: `test/print-mode-*.test.ts` (CA-PRINT-MODE, 20 cases), mostly tied to plan mode and advisors (not ported).

Delivered commit or exact worktree snapshot:
The commit adding this file on `dev`, with parent `9c63015`.

Rust entry and host chain exercised:
The compiled `target/debug/ara` binary, spawned as a real process by `crates/ara-cli/tests/e2e.rs`. The chain is HTTP to `ara-testkit` FakeUpstream on loopback, then real `read`/`write`/`bash` effects in a temp workspace, then a JSONL journal in a temp session directory. `scripts/fake_chain_demo.sh` runs `ara` and `ara-fake-upstream` as two separate processes.

Environment and sanitized commands:
- `cargo test -p ara-cli` → 3 unit + 13 e2e pass, repeated (13 e2e in 1.66 s).
- `python scripts/verify_backend.py` → PASS.
- `cargo deny check` → ok.
- `scripts/fake_chain_demo.sh <dir>` → `exit=0`, `OK: fake chain produced notes/plan.md, 2 tool receipts, redacted request log`. Artifact sha256 is `743475353f7d00d371964d5eb8a13a186fb71503956e731996eac5b669664694` for `# Plan\n- port agent loop\n`.

Expected vs actual normal result:
- **Text mode:** stdout is the final text followed by a newline, stderr shows `Working...`, and the exit code is 0. The journal holds `model_change`, `user`, `assistant`, with usage input 40. The recorded request carries a redacted `Authorization` header, and the key never appears in stdout or stderr.
- **Tool task:** the model writes `hello.txt`, then `cat`s it through bash, then answers. The file content is exact. The journal reads `model_change, user, assistant, toolResult ("Successfully wrote 12 bytes to hello.txt"), assistant, toolResult ("hi from ara\ndone"), assistant`, and the third request carries the tool message.
- **JSON mode:** the session header line comes first, and each event is one line. `message_update` carries only the delta.
- **Continue and stdin:** `-c` with a stdin prompt continues the same file, and the second request contains the prior turn.
- **Stdin prepend:** stdin is prepended to the first prompt (`--- a\n+++ b\nreview this diff`).
- **Resume cwd:** resuming from another directory runs tools in the session's recorded cwd.

Actual: as expected.

Expected vs actual failure/cancel/recovery result:
- **Upstream 401:** exit 1, stderr `401 bad key`, stdout empty; the journal's last assistant has `stopReason error` and `errorStatus 401`.
- **SIGINT during `bash sleep 30`:** after the live `tool_execution_update`, the process exits 1 within 5 s. stderr shows `interrupt received` and `Request was aborted`. The journal has a toolResult ending in `[Command aborted]` and an aborted assistant, and no further model call is made.
- **`kill -9` mid-tool, then `--resume`:** stderr shows `call_crash were interrupted before a result was recorded; their effects are unknown and they were not re-run`. `runs.log` still holds a single `run` line (no replay). The journal gains an `interrupted_unknown_effect` result, and the resumed request carries that tool message.
- **`--max-time 1` hit during streaming or during a running tool:** exit 1 with `Deadline exceeded`, in under 4 s.
- **Model-call budget:** `--max-model-calls 1` exits 1 with `model call limit reached (1)` after 1 request. `--max-model-calls 0` makes 0 requests.
- **OpenRouter key isolation:** with only `OPENROUTER_API_KEY` set and a non-OpenRouter base URL, no `authorization` header is sent.
- **Usage errors:** an unknown tool gives exit 2 and leaves a resumed journal byte-identical. `--no-session --resume` gives exit 2.
- **Journal write failure:** it cancels the run so the message's tools never run (unit test `journal_failure_cancels_the_run`).
- **Output sanitization:** ANSI/OSC and control characters are removed from printed model text and errors (unit test).

Actual: as expected.

Streaming observation before completion:
`json_mode_streams_events_before_the_run_ends` reads the `first ` delta line while `child.try_wait()` is still `None`, with the upstream holding the rest for 1.5 s.

Real-model route:
**Not run — blocked.**
- `curl https://openrouter.ai/api/v1/models` through this environment's proxy returns `CONNECT tunnel failed, response 403`: openrouter.ai is not in the network allowlist.
- No `OPENROUTER_API_KEY` is configured.
- The ARA Manager → CAS route is local to the user's machine and cannot be reached from this cloud container.

`scripts/real_model_trial.sh` is prepared with fixed bounds: ≤6 model calls, ≤180 s, ≤1024 output tokens per call. Its task (a `fib.txt` artifact checked with `wc -l`) runs through the same host chain, and it reports the model id, calls, tool receipts, usage (unknown stays unknown), artifact check and final text. It has **not** been executed.

Observed output, tool effects, usage and latency:
Fake upstream only (see above). Usage values come from the fake's usage chunks, and runs without usage chunks record no usage fields.

Independent review and semantic findings:
- Claude Code `code-review` skill (forked reviewer, high). 10 findings, all fixed and re-tested:
  1. A deadline hit during a tool exited 0. Fixed: the Core now reports `RunEnd`.
  2. Journal failure still ran tools. Fixed: the run is cancelled.
  3. The budget stop was inferred and `--max-model-calls 0` exited 0. Fixed via `RunEnd::ModelCallBudget`.
  4. `--no-session` silently dropped `--resume`. Fixed: clap conflict.
  5. Stdin was ignored when a prompt was given. Fixed: it is prepended.
  6. Fallback keys were sent to arbitrary base URLs. Fixed: route-bound keys.
  7. Validation ran after journal writes. Fixed: all args are validated first.
  8. Resume ignored the session cwd. Fixed: the session cwd is used and a warning is printed on explicit mismatch.
  9. Output was not sanitized. Fixed: `sanitizeText` is ported.
  10. Exit-code and sequencing differences were undocumented. Fixed: recorded in the module header and the ledger.
- `ocr delegate preview` was used for scope.

Unrun/blocked checks and impact:
- **Bounded real-model task: open** (mandatory for this integration point).
- Windows is not supported.
- Like upstream, piped stdin that never closes blocks startup; scripts must pass `</dev/null`.
- Power-loss durability is not exercised.

Decision: tested with a controlled upstream. Not accepted: the real-model trial is missing.

Decision: accepted for the behaviors exercised in [real-model trials 2026-09-25](real-model-a1-a2-20260925.md) (Agnes `agnes-2.5-flash`, OpenRouter `openrouter/free`); items listed there as not exercised stay open.
