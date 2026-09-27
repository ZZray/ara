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

The failure-path host fixture sends two `write` calls: the first completes,
the second ends with a JSON prefix at `response.incomplete/max_output_tokens`.
The process exits unsuccessfully, neither file is created, only one HTTP
request is made, and the journal retains only the completed call. The Agent
already filters calls without `ToolcallEnd` on error; the provider never
promotes this turn to `ToolUse`.

The state fixtures cover split UTF-8/CRLF, malformed/oversize SSE, final
snapshot replacement, refusal text, parallel index/ID routing, a prefixed
call alias colliding with an item ID, contradictory keys, incomplete calls,
terminal fallbacks and cache usage. The HTTP fixtures cover request shape,
two-request tool-result replay, EOF after a tool start, and unknown SSE
events that cannot extend the idle deadline.

## Verification and review

- `cargo test -p ara-ai`: final worktree 77 unit, 48 existing Chat fake
  HTTP, 4 Responses fake HTTP, and 0 doc tests passed.
- `cargo test -p ara-cli --test e2e`: earlier candidate 18/21 passed. The
  new Responses success and truncated-tool tests passed; existing Windows
  failures were `deadline_during_a_tool_and_zero_budget_exit_nonzero`,
  `tool_task_produces_file_and_receipts` and
  `resume_runs_tools_in_the_session_cwd` (the latter two need `bash`).
- `cargo test -p ara-cli --test e2e responses_`: final worktree 2/2 passed.
- `cargo clippy -p ara-ai -p ara-cli --all-targets -- -D warnings`: passed
  on the final worktree.
- Targeted `rustfmt --check --edition 2024` on the eight changed Rust files
  and `git diff --check`: passed on the final worktree. A workspace
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

## Known bounds and next gate

- Parallel argument events with no `output_index` or `item_id` fail closed
  when multiple items are open. Pinned OMP can route some of these by arrival
  order and JSON-prefix matching; ARA does not yet claim that parity.
- A partial open function on an incomplete terminal is an error in ARA.
  Pinned OMP can retain a `Length` turn and continue; this is a deliberate
  safe-side WIP difference pending complete recovery semantics.
- Native reasoning item replay, provider-hosted tools, strict tool-schema
  normalization, target-specific call-ID spelling, `previous_response_id`
  state, and broad Responses-family model variants remain open.
- The CLI protocol is explicitly chosen on each invocation. `--resume`
  currently requires `--api openai-responses` again. The Chat-only
  `--report-request-text-tokens` option is rejected on this route.
- A bounded real-model task, full backend gate, dependency and fixed-upstream
  checks, and independent point audit remain before acceptance.
