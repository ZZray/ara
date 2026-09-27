# AI-01e Responses reasoning and native history (WIP)

## Scope and fixed source

Fixed OMP `596f2da7101178214aa27a753529d15e6b7ad91d`:
`packages/ai/src/providers/openai-shared.ts::processResponsesStream`,
`finalizeReasoningThinking`, `buildResponsesInput`, and
`packages/ai/src/utils.ts::sanitizeResponsesHistoryItem`, plus
`packages/ai/src/providers/openai-responses.ts` session warmup policy.
ARA implements the bounded slice in `ara-ai::responses_stream` and
`ara-ai::providers::openai_responses`, with `AssistantMessage.providerPayload`
persisted by the existing Session journal. `ara-agent::event` removes opaque
replay data from printable assistant messages while preserving tool arguments
and result details. The CLI accepts explicit `--reasoning` only with
`--api openai-responses`; the host must confirm that model capability. The
flag must be repeated on a separate `--resume` process.

## Executed behavior

- Reasoning summary and raw deltas become Thinking events. Multiple summary
  parts use the fixed OMP double-newline boundary. A divergent summary done
  snapshot does not emit duplicate text or silently change later partials.
  The complete reasoning item is saved as `thinkingSignature`.
- On a successful terminal turn, ordered native output items are saved in
  `providerPayload` with `dt:true` (incremental) and a 32 MiB cumulative cap.
  Successful incomplete and hidden-only turns also save native items;
  hidden-only items do not warm or replay. Failed, aborted and
  over-limit turns have no native payload. There is no signature-only opaque
  fallback. See [incomplete history](ai-01e-responses-incomplete-history.md).
- Replay requires the same Responses API, provider, model and endpoint SHA-256
  fingerprint, a
  successful stop reason, matching Thinking signature and unchanged visible
  content/tool call. Output-only IDs/status are removed; message `phase` and
  valid encrypted reasoning are retained. Tool outputs use the emitted call
  ID. Invalid native items fall back to ordinary visible history. The Session
  payload stores the endpoint fingerprint rather than URL credentials.
  SHA-256 prevents plaintext storage; it is not a password protection scheme
  against offline guesses of a low-entropy URL.
- A CLI Responses provider starts cold for each process. Cold history uses
  visible assistant text and tool calls, without the prior opaque reasoning
  payload. A completed response with a replayable native payload warms that
  provider for later calls in the same process. A failed request, including a
  retryable empty attempt, does not warm it. Direct stateless encoder calls
  without a host session state retain their earlier native replay behavior.
  Warmup is transient, scoped by provider and not saved to Session. Old ARA
  payloads without `dt` remain incremental. Explicit `dt:false` or malformed
  `dt` falls back to visible history; full snapshot replacement requires
  separate validation support.
- A real `ara` process with a controlled fake upstream wrote `reason.txt`.
  The same-process tool continuation contained the encrypted reasoning item,
  commentary-phase message, function call and paired function output. A new
  process after `--resume` used visible text and paired tool history without
  encrypted reasoning. All three requests included
  `reasoning.encrypted_content` when `--reasoning` was supplied. The Session
  JSONL contains the opaque snapshot; JSON client output does not contain it.
  The fake upstream supplied the encrypted item, so this does not prove an
  actual model will return one.
- A reasoning part boundary followed by a dropped SSE connection is not
  retried; an empty reasoning start can retry. These cases use two scripted
  upstream responses and assert the served request count.
- A five-request fake HTTP sequence verifies cold failure, a retryable empty
  attempt, its successful retry, and later warm replay of the seed's opaque
  reasoning item. Both attempts of the retried call remain cold.

## Checks on the cold/warm WIP snapshot before the incomplete-history extension

- `cargo test -p ara-ai --quiet`: 92 unit, 48 Chat HTTP, 9 Responses HTTP
  passed; doc target passed.
- `cargo test -p ara-cli --test e2e responses_ --quiet`: 7/7 real-process
  Responses cases passed, including tool effects, restart and JSON output.
- `cargo clippy --workspace --all-targets --all-features -- -D warnings`,
  `cargo test --workspace --doc --quiet`, `cargo deny check`,
  `python scripts/omp_inventory.py check`, `rustfmt --check --edition 2024`
  for changed Rust files and `git diff --check`: passed. `cargo deny` reported
  existing non-fatal duplicate/license-field warnings.
- `cargo test --workspace --all-targets --quiet` failed in existing Windows
  CLI cases: deadline exit status and Bash unavailable for two tool cases.
  The new reasoning CLI case passed (23/26 CLI e2e cases passed overall).
  After adding `C:\Program Files\Git\bin` to PATH, CLI e2e passed 25/26;
  only the existing Windows tool deadline timing assertion failed.
- `cargo fmt --all -- --check` still reports unrelated vendored `pi-edit`
  formatting. Changed Rust files pass direct rustfmt check.

Independent Codex agents `/root/reasoning_plan` and `/root/reasoning_risk`
reviewed the plan and diff. They identified duplicate/divergent Thinking
deltas, summary boundaries, reasoning ID sanitization, cross-endpoint opaque
replay, event projection overreach, and an unrequested CLI include. The
reviewed fixes have focused regressions. Their final review raised two
further stream consistency cases; both were corrected and tested. A later
review found that storing the endpoint URL could persist URL credentials;
it is now stored as SHA-256 with a Session serialization regression. The
independent final re-review found no remaining high-confidence P1/P2 in this
slice. The full gate and dependency audit recorded here ran after that fix.
For cold/warm replay, independent agents `/root/cold_source` and
`/root/cold_risk` reviewed fixed OMP source and the plan; `/root/cold_risk`
reviewed the diff. Its `dt:false` finding was fixed with a fallback regression.
No remaining high-confidence P1/P2 issue was reported in this bounded slice.

## Open parity and delivery gaps

ARA now applies the fixed OMP cold/warm native replay policy to CLI Responses
sessions and saves native items for successful incomplete turns. It does not
implement `dt:false` full-history snapshot replacement, server-side
`previous_response_id`, or per-item filtering of malformed native tool calls
in a mixed truncated output. A terminal `response.output` that first
adds encrypted data after an earlier `output_item.done` is not merged into
the stored item; fixed OMP generic stream has the same gap. Cross-provider
adaptation, hosted tools, actual model trial, complete backend gate and
point audit remain open. No AI-01e or P0-P6 acceptance is claimed.
