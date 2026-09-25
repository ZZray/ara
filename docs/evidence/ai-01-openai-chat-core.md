# AI-01 — `ara-ai` message model and OpenAI Chat Completions adapter

Point / requirement / exclusions:
Port the OMP message model and assistant stream protocol, and the OpenAI-compatible Chat Completions adapter's default-compat path: request shaping, pre-stream transport retries, SSE chunk handling, finalization, timeouts and cancellation. Also port the provider-side tool-call/result pairing guard and tool-argument validation subset. Build a controlled fake upstream for deterministic faults.
Excluded, with ledger rows kept open: per-host compat tables (Mistral ids, DeepSeek/Kimi reasoning replay fields, strict tools, prompt-cache keys, OpenRouter routing), markup healing, `reasoning_details` signatures, object-argument deep merge, the replay-safe whole-stream retry wrapper, cost pricing, the Anthropic and Responses APIs. The host chain (CLI → agent loop → provider) and a real-model trial belong to later points.

Upstream SHA and source/test location:
`596f2da7101178214aa27a753529d15e6b7ad91d`:
- `packages/ai/src/types.ts` and `packages/catalog/src/types.ts` (Usage)
- `packages/ai/src/providers/openai-completions.ts` (`streamOpenAICompletionsOnce`, `buildParams`, `convertMessages`, `mapStopReason`, `parseChunkUsage`, `isOpenAICompletionsProgressChunk`)
- `packages/ai/src/providers/openai-shared.ts` (`calculateOpenAIUsageAccounting`)
- `packages/ai/src/utils/openai-http.ts` and `packages/utils/src/fetch-retry.ts`
- `packages/ai/src/providers/transform-messages.ts` (second pass)
- `packages/ai/src/utils/validation.ts` (`validateToolArguments`)
- `packages/utils/src/json-parse.ts`

Upstream behavior items exercised by equivalent Rust tests:
- B-7edcc8d33c, B-29c9e10810, B-53b9ad9ea2: `[DONE]` termination
- B-ad54ce72a7, B-c3cbb73cd9: error finish reasons
- B-e81cb471fa, B-c61e7715a5, B-836aa079c9: in-band error envelopes
- B-8780a8bd5c: premature close
- B-e9c8a8c67a, B-0ec7b527b3, B-cb8efe88d5: synthetic, non-duplicated results
- B-6df7685621, B-e50f7de88d: `reasoning_content` with null content

Not covered: B-4593deda43 (empty-close retry wrapper), B-58206af471, B-9df9da761f (late real result pulled into its window), B-82a2371a2f and siblings (idle deadline sliding with local work).

Delivered commit or exact worktree snapshot:
The commit that adds this file on branch `dev`. Parent: `71a9554db16cba2635afb17861ee8aa8e24bf639`. The tests below ran on the staged tree of that commit.

Rust entry and host chain exercised:
`ara_ai::providers::openai_completions::stream` over real HTTP (`reqwest`, loopback TCP) against `ara-testkit::FakeUpstream`, which is a real socket server that scripts SSE frames, delays, hangs, resets and HTTP errors, and records requests with the authorization header redacted. No agent loop or CLI host is involved yet, so this is **not** host-chain evidence.

Environment and sanitized commands:
Linux container, rustc 1.94.1, cargo-deny 0.20.2, ocr v1.12.9. No API key was used.
- `cargo test -p ara-ai` → 29 unit + 17 HTTP tests pass
- `cargo test -q -p ara-ai --test openai_http`, repeated 5× → 17/17 each run, 2.63–2.68 s
- `python scripts/verify_backend.py` → PASS (fmt check, clippy `-D warnings` on the workspace with all targets, all tests, doc tests)
- `cargo deny check` → `advisories ok, bans ok, licenses ok, sources ok`
- `python scripts/omp_inventory.py check` → OK

Expected vs actual normal result:
- Streamed text, reasoning and indexed tool calls assemble in order.
- `stop` is promoted to `toolUse` when tool calls exist.
- `responseId` and `upstreamProvider` are captured.
- Usage accounting subtracts cached tokens: `{prompt 10, completion 5, cached 4}` → input 6, cacheRead 4, total 15.
- The recorded request has `stream: true` and `stream_options.include_usage`, one system message per prompt, the user message, a `tools` array, and a redacted `authorization` header.
- A usage-only trailing chunk ends the response without `[DONE]` in under 2 s. A finish without usage ends after the 2.5 s post-finish grace (measured 2.4–6 s window).

Actual: all as expected (tests `streams_text_usage_and_records_request`, `text_reasoning_and_usage`, `indexed_tool_calls_and_stop_promotion`, `finish_with_usage_completes_without_done_sentinel`, `finish_without_usage_ends_after_grace`).

Expected vs actual failure/cancel/recovery result:
- 429 (`retry-after: 0`) then 503 then success → 3 requests, success.
- 401 → no retry, `errorStatus 401`, `"401 bad key"`.
- 500/500/502 → retries exhausted, `"502 gateway"`.
- An HTTP-date Retry-After beyond the cap → no retry.
- Connection refused → transport error after bounded attempts.
- A builder/config error fails fast in under 1 s.
- Idle stall with keep-alive comments and empty deltas → idle timeout; keep-alives do not reset it, and partial text is retained with the text block closed first.
- Slow headers → first-event timeout.
- Reset without a terminal frame → incomplete error.
- Reset after finish plus usage → completed response kept.
- An empty 2xx body or a non-SSE JSON body → `OpenAI completions stream ended without any event` (ARA difference below).
- A malformed `data:` frame → error.
- `content_filter` → error.
- Cancel mid-stream → `aborted` with `Request was aborted` and partial text kept.
- Cancel while the consumer has stopped reading → the provider stops within 2 s.

Actual: all as expected.

Streaming observation before completion:
`cancellation_mid_stream_aborts` receives the live `text_delta` "working" while the upstream socket is still open (`end: hang`), and only then cancels.

Real-model route:
Not run. openrouter.ai is denied by this environment's network policy (proxy 403), and no `OPENROUTER_API_KEY` is configured. A real-model trial is required at the later host-chain integration point, not at this adapter point.

Observed output, tool effects, usage and latency:
Fake upstream only; values are as asserted above. No tool effects at this layer.

Independent review and semantic findings:
- `ocr delegate preview --format json` (v1.12.9) on the staged diff: 26 reviewable files, 1 excluded (`Cargo.lock`).
- The Claude Code `code-review` skill (forked reviewer, effort high) on the staged diff reported 10 findings. Resolutions:
  1. Empty `finish_reason` was treated as a finish. **Fixed**; regression test added.
  2. Unreported cache buckets and total became zero or were invented. **Fixed**: they stay `None`.
  3. An empty or non-SSE 2xx body became a successful empty turn. **Fixed**: now an error.
  4. Malformed frames were dropped silently. **Fixed**: now an error.
  5. Builder errors were retried. **Fixed**: now fail fast as `Config`.
  6. A read error after finish discarded a completed response. **Fixed**.
  7. A provider could park on a full channel after cancel. **Fixed** with a cancel-aware push.
  8. HTTP-date Retry-After was ignored. **Fixed** with `httpdate`.
  9. Streaming argument re-parse was O(n²). **Mitigated** by a geometric throttle as upstream does. Per-event snapshot cloning remains; this is an open performance item.
  10. The first-event deadline cuts retries short. **Kept**, because upstream documents that retries must not extend the first-event deadline.

  Tests were re-run after the fixes, with the counts above.

Unrun/blocked checks and impact:
- No real-model trial.
- No agent-loop/CLI host chain.
- Per-event partial snapshot cost is unmeasured on large outputs.
- The first-event timeout message hides the last HTTP status when retries are cut short.

Decision: tested (adapter layer). Not accepted: the host-chain and real-model evidence required by `docs/acceptance.md` are pending.

Decision: accepted for the behaviors exercised in [real-model trials 2026-09-25](real-model-a1-a2-20260925.md) (Agnes `agnes-2.5-flash`, OpenRouter `openrouter/free`); items listed there as not exercised stay open.
