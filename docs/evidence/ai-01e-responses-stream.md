# AI-01e: stateless Responses stream and CLI host (WIP)

## Requirement and source

This increment carries the existing stateless `/responses` request body through
an actual HTTP/SSE provider, the shared `ModelProvider` port, and `ara --api
openai-responses`. It covers text/refusal output, JSON function calls, paired
tool results, usage, terminal/error states, cancellation and bounded stream
decoding. The pinned source is OMP
`596f2da7101178214aa27a753529d15e6b7ad91d`:
`packages/ai/src/providers/openai-responses.ts::streamOpenAIResponsesOnce`,
`openai-shared.ts::processResponsesStream`,
`OPENAI_RESPONSES_PROGRESS_EVENT_TYPES`,
`hasExecutableIncompleteResponsesToolCalls`, and
`populateResponsesUsageFromResponse`. The prior request-body evidence is
[AI-01e Responses request](ai-01e-responses-request.md).

ARA owns this in `ara-ai::responses_sse`, `ara-ai::responses_stream`,
`ara-ai::providers::openai_responses::stream`, its `ModelProvider` adapter,
and the CLI protocol switch. Chat Completions remains the default. The
existing Agent, Session writer and tool implementations are unchanged.

## Executed behavior

The controlled upstream receives `POST /v1/responses` with `store:false`.
The provider emits ordered text/tool events and ends on the response terminal
even when the socket stays open. A real `ara` process wrote `response.txt`
with `from responses\n`, journaled the tool result, exited, then a second
process resumed the journal with `--api openai-responses`. The third request
contained the matching `function_call` and `function_call_output` using
`call_write`. The host test checks all three wire requests and the file.

The truncated-turn host fixture sends two `write` calls: the first completes,
the second ends with a JSON prefix at `response.incomplete/max_output_tokens`.
With a one-call budget, neither file is created. The Session records the
assistant `Length` turn with both calls and two `assistant_stop_length`
synthetic results marked `executed:false`. A new `ara` process resumes it;
the second wire request contains both call IDs and their paired non-execution
results, and the model returns `No files were written.` A separate uncapped
real-process fixture proves the Agent resamples within the same run without
executing the partial `write`.

The JSON-mode live fixture holds the fake upstream between text delta and
terminal. The client pipe receives `message_update` after `message_start`
while the `ara` process is still running; one assistant `message_end` and
one `agent_end` follow. Cancelling a partially streamed tool call through
the HTTP provider remains `Aborted`, not `Length`.

The state fixtures cover split UTF-8/CRLF, malformed/oversize SSE, final
snapshot replacement, refusal text, parallel index/ID routing, a prefixed
call alias colliding with an item ID, contradictory keys, incomplete calls,
terminal fallbacks and cache usage. The HTTP fixtures cover request shape,
two-request tool-result replay, EOF after a tool start, and unknown SSE
events that cannot extend the idle deadline.

## Verification and review

- `cargo test -p ara-ai --quiet`: current worktree 78 unit, 48 existing Chat
  fake HTTP, 5 Responses fake HTTP, and 0 doc tests passed.
- `cargo test -p ara-cli --test e2e --quiet`: 20/23 passed with the ambient
  Windows PATH; two failures could not start `bash` and one was the existing
  tool-deadline case. With `C:\Program Files\Git\bin` and `usr\bin` prepended
  to PATH, 22/23 passed; only
  `deadline_during_a_tool_and_zero_budget_exit_nonzero` remained red.
- The four Responses-specific real-process e2e cases passed: tool effect and
  restart replay; mixed complete/partial Length turn and restart replay;
  same-run resampling without a tool effect; terminal-before-exit JSON text
  stream.
- `cargo clippy -p ara-ai -p ara-cli --all-targets -- -D warnings`: passed
  on the current worktree.
- Targeted `rustfmt --check --edition 2024` on the eight changed Rust files
  passed on the earlier stream increment. The three Rust files changed in
  this continuation and `git diff --check` passed on the current worktree.
  A workspace
  `cargo fmt --all -- --check` reports unrelated vendored formatting, so
  that command is not a passing gate.

Independent Codex reviewers `/root/responses_sse_plan` and
`/root/responses_sse_security` reviewed the plan and worktree diff against
the pinned OMP stream tests. They found unsafe promotion of mixed complete
and partial calls, ambiguous identifierless parallel arguments, alias and
identity collisions, missing refusal final text, unknown-event idle resets,
and compatible cache usage gaps. The changed code and focused regressions
address those findings. Both final read-only re-reviews found no remaining
high-confidence P0/P1 issue; the plan reviewer independently ran the earlier
9-case state suite, and the security reviewer performed static review only.
No real-model trial or
full backend verification has passed on this worktree. This point and all
acceptance counts remain open.

For the Length recovery continuation, independent Codex reviewers
`/root/responses_parallel_plan` and `/root/responses_live_plan` reviewed the
scope and diff. Both found no high-confidence P0/P1 issue after tracing
`ara-agent` synthetic results and Session replay. The final snapshot test
proves that a truncated `response.output[]` function item remains `Length`;
its strict `__parseError/__rawJson` representation differs from an open
item's best-effort JSON object. Neither is executed on `Length`.

## Known bounds and next gate

- Parallel argument events with no `output_index` or `item_id` fail closed
  when multiple items are open. Pinned OMP can route some of these by arrival
  order and JSON-prefix matching; ARA does not yet claim that parity.
- A partial open function on an incomplete terminal now records a `Length`
  turn and continues with synthetic non-execution results. A completed
  terminal with an unfinished function, EOF without a terminal, content
  filtering, ambiguous identifierless parallel arguments and cancellation
  remain separate error or aborted paths.
- Native reasoning item replay, provider-hosted tools, strict tool-schema
  normalization, target-specific call-ID spelling, `previous_response_id`
  state, and broad Responses-family model variants remain open.
- The CLI protocol is explicitly chosen on each invocation. `--resume`
  currently requires `--api openai-responses` again. The Chat-only
  `--report-request-text-tokens` option is rejected on this route.
- A bounded real-model task, full backend gate, dependency and fixed-upstream
  checks, and independent point audit remain before acceptance.
