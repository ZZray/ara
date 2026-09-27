# AI-01c extension: streamed object-shaped tool arguments (WIP)

## Scope and source

At fixed OMP commit `596f2da7101178214aa27a753529d15e6b7ad91d`,
`packages/ai/src/providers/openai-completions.ts` lines 320–425 and 856–884,
1245–1293 merge MiniMax-compatible object-shaped `function.arguments`
fragments recursively. Strings and arrays distinguish cumulative prefixes
from incremental fragments; unsafe object keys are discarded at every depth.
Object chunks emit empty deltas, followed by one complete JSON delta before
`toolcall_end`. The upstream regression is
`packages/ai/test/issue-2080-repro.test.ts` lines 61–256.

ARA's previous `crates/ara-ai/src/providers/openai_completions.rs` merged
only top-level keys. This is the previously recorded open part of AI-01c,
not a new protocol or an intentional difference. Production changes are
limited to the Chat provider's private argument accumulator and merge helpers.
Agent, Session, CLI, shared message types, retry and tool lifecycle are
unchanged. Other OpenAI-compatible host policies remain open.

## Controlled evidence on Windows

No live credential or model call was used. The fake upstream is a loopback
HTTP/SSE server. The new `openai_http` test sends an `edit` call with three
object fragments. On the old code it failed: final `nested.text` was absent;
on the fix it passed with `nested.text = "hello"`, `oldText = "one two"`,
array `["a", "b"]` and all earlier keys preserved. Concatenating the
`toolcall_delta` values parses to the exact final arguments. A new provider
unit test discards `__proto__`, `constructor` and `prototype` recursively.
Another unit test keeps interleaved calls separate and exercises an
object-to-string mode switch.

The new real `ara` CLI process test receives two streamed object fragments
for one `write` call, writes `fragment.txt` with exactly `hello world\n`,
then makes one follow-up request. The Session journal contains one assistant
tool call with the complete content and one matching tool result. The
existing string-argument CLI write/retry test also passed.

Commands and outcomes on the uncommitted worktree based on `e5aa93b`:

| Command | Result |
| --- | --- |
| `cargo test -p ara-ai --test openai_http object_tool_arguments_merge_fragments_and_emit_concat_safe_delta -- --exact` | Failed on old code as above; passed after fix. |
| `cargo test -p ara-ai --lib providers::openai_completions::tests` | 14/14 passed before the additional interleaving test; its focused run then passed. |
| `cargo test -p ara-ai --test openai_http` | 49/49 passed. |
| `cargo test -p ara-cli --test e2e object_streamed_write_arguments_reach_tool_and_journal_once -- --exact` | 1/1 passed with file and journal assertions. |
| `cargo test -p ara-cli --test e2e empty_stream_retry_reaches_one_tool_task_without_duplicate_effects -- --exact` | 1/1 passed; existing string argument path. |
| `rustfmt --check --edition 2024 --config-path rustfmt.toml` on the three touched Rust files | Passed. |
| `cargo clippy -p ara-ai -p ara-cli --all-targets -- -D warnings` | Passed. |
| `python scripts/verify_backend.py` | Owned formatting and workspace all-target/all-feature Clippy passed. Full tests reached CLI e2e: 42/43 passed; known Windows Bash `deadline_during_a_tool_and_zero_budget_exit_nonzero` exceeded its four-second assertion. Later verifier stages did not run. |
| `cargo deny check` | Exit 0: advisories, bans, licenses, sources OK; existing duplicate-version warnings. |
| `python scripts/omp_inventory.py check` | Passed. |

An isolated invocation of `tool_task_produces_file_and_receipts` outside the
verifier failed because `bash` was not on that process's path; the verifier
sets Git Bash and this case passed there. No Bash production code was changed.
The full delivery gate remains open because of the separate Windows Bash
deadline failure and the absence of a bounded real-model task on a verified
route. This slice remains WIP and does not change any acceptance count.

Independent Codex subagent pre-implementation source/scope review identified
this gap and recommended provider-only code. A separate independent Codex
subagent reviewed the three-file implementation diff against the pinned OMP
merge and stream semantics, found no high-confidence defect, and suggested
interleaving and mode-switch coverage; the extra unit test passed. Neither
reviewer claimed to run Cargo verification.
