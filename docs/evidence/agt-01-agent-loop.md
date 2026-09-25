# AGT-01 — `ara-agent` loop

Point / requirement / exclusions:
Port the OMP agent loop: the `agentLoop`/`agentLoopContinue` event sequence; streamed assistant turns; abort that keeps only tool calls that reached `toolcall_end`; synthetic results for error, aborted, length and deadline stops; argument validation and the `beforeToolCall` block; shared/exclusive scheduling with completion-order results; deadline; steering and follow-up queues; continue rules.
Excluded (open): telemetry, Harmony recovery, soft tool requirements, pause gate, asides, owned dialects, intent tracing, `transformContext`/`beforeModelCall`/`afterToolCall` hooks and hook argument revision, interruptible-tool steering interrupts, argument streams, transient error tool-turn recovery, pause-turn continuations, and the stateful `Agent` class (AGT-AGENT).

Upstream SHA and source/test location:
`596f2da7101178214aa27a753529d15e6b7ad91d` — `packages/agent/src/agent-loop.ts` (`agentLoop`, `agentLoopContinue`, `runLoopBody`, `streamAssistantResponse`, `retainCompletedToolCalls`, `emitAbortedAssistantMessage`, `prepareToolCallDispatch`, `executeToolCalls`, `createSyntheticToolResultMessage`, `coerceToolResult`) and `packages/agent/test/agent-loop.test.ts`.

Upstream behaviors exercised by equivalent Rust tests:
- B-5f324c4a78: deadline
- B-92975aa93d: abort before events
- B-ac66456ab5: parse sentinel without `__rawJson`
- B-bf34dfaa7a: provider-error synthetic result
- B-879c6b7357: shared parallel, completion order
- B-9395b930fa: drop incomplete calls on abort
- B-06d2dce477: continue on empty context
- B-2d2f6e1d95: `beforeToolCall` block

Not covered: steering interrupt rows B-225e3a4817…B-87325f6054, hook argument revision B-e90cf53240/B-d60f20625c, custom abort reasons beyond deadline B-a4ad214b11.

Delivered commit or exact worktree snapshot:
The commit that adds this file on `dev`; parent `dc7279c`.

Rust entry and host chain exercised:
- `ara_agent::agent_loop`/`agent_loop_continue`/`execute_tool_calls` with a scripted in-process provider.
- One real HTTP chain: `agent_loop` → `OpenAICompletionsProvider` → `ara-testkit` fake upstream → echo tool → second request carrying the `tool` message.
- No CLI host or journal yet.

Environment and sanitized commands:
- `cargo test -p ara-agent` → 19/19 (ran 3× to check for flakiness, 1.21–1.22 s each)
- `python scripts/verify_backend.py` → PASS
- `cargo deny check` → ok (crates marked `publish = false`)

Expected vs actual normal result:
- Simple prompt event order matches the OMP README: `agent_start, turn_start, message_start/end user, message_start/end assistant, turn_end, agent_end`, with 3 `message_update`s.
- A tool turn emits `tool_execution_start/update/end` plus the toolResult message, then a second turn whose provider context ends with the toolResult.
- Steering is injected before the first model call and follow-ups are drained at stop.
- Two shared 300 ms calls overlap (<1 s for two batches). Exclusive calls serialize (≥400 ms, ordered log).
- Over real HTTP, the second request carries `tool_calls[0].id == "call_1"` and the `tool` message.

Actual: as expected.

Expected vs actual failure/cancel/recovery result:
- Validation failure → upstream-format error; the tool never runs.
- Unknown tool → `Tool nope not found`.
- An error result with empty content → `Tool failed with no output.`
- Host block → reason text, no effect.
- Provider error after a completed call → `…ended with an error before the tool could run: 503 overloaded`, with `source: assistant_stop_error`.
- Abort observed live while streaming → aborted message keeps only `done-1`, sets `stopDetails.type = stream_interrupted_after_content`, and emits `Tool execution was aborted: Request was aborted`.
- Abort during a tool → the tool sees cancellation, and no further provider call is made.
- `length` stop → truncation synthetic result, then a re-sample.
- Deadline during stream or tool → aborted with `Deadline exceeded`, and the tool is cancelled.
- A steer dequeued past the deadline is still committed.
- A panicking tool → `Tool boom panicked: kaboom`; its sibling still completes.
- Host drops the run future → the caller token is untouched and the run's child tokens are cancelled by a drop guard.
- An approval hook blocked on the user observes the abort, and both calls report `run was aborted`.
- Continue: empty context and plain assistant tail are rejected. An unpaired tail is **refused by default** (ARA difference) and executed only with `UnpairedTail::Execute`.

Actual: as expected.

Streaming observation before completion:
`abort_during_stream_keeps_only_completed_calls` cancels only after it observes the live `message_update` for `toolcall_start` while the provider is still open.

Real-model route:
Not run: openrouter.ai is blocked by the environment network policy and no key is configured. The loop has no direct model dependency; the host-chain trial is pending at CLI-01.

Independent review and semantic findings:
- `ocr delegate preview` (v1.12.9) was used for scope.
- The Claude Code `code-review` skill (forked reviewer, high) returned 10 findings. All were addressed and the tests re-run:
  - Deadline was not a cancellation → run-scoped cancel with the deadline reason.
  - Panic unwound the batch → per-call `catch_unwind`.
  - No drop guard → `DropGuard` on the run token.
  - Hook lacked cancel → the hook receives the token, and approval is skipped after cancel.
  - Steering lost at deadline → committed.
  - Unpaired tail replay of unknown effects → `UnpairedTail::Refuse` default.
  - Null explanation fallback → fixed.
  - `__rawJson` leaked into events → trimmed.
  - Repeated `definition()` calls → once per batch.
  - Evidence/notices missing → this file and THIRD_PARTY_NOTICES updated.

Unrun/blocked checks and impact:
- No persistence/restart evidence yet (SES-01).
- No real model.
- `dropping_the_run_cancels_running_tools` shows the tool future is dropped with the run. It cannot show an OS subprocess being killed; that belongs to the bash tool evidence.

Decision: tested (loop layer). Not accepted.
