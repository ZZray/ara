# AI-RETRYa: replay-safe retry of started OpenAI Chat streams (WIP)

## Requirement and boundary

Fixed OMP `596f2da7101178214aa27a753529d15e6b7ad91d` implements
`withReplaySafeStreamRetry` in
`packages/ai/src/utils/empty-completion-retry.ts`, binds it in
`packages/ai/src/providers/openai-completions.ts:1492-1500`, and exercises it
in `packages/ai/test/empty-completion-retry.test.ts`. Relevant inventory cases
include B-f3204ba5ac, B-d6c439c35f, B-c33421f9ce, B-86ff424237,
B-8176bb72d4, B-c1662ae6aa, B-835c791d85, B-d43bbe6198 and
B-94812de6f0. This ARA slice retries a known-empty stop at most twice and a
typed transient error from an already started 2xx stream at most once. It
buffers pre-output lifecycle events; the first text, thinking or tool event
commits the attempt and is forwarded live. Discarded attempts do not reach
the Agent or its tool dispatcher. Caller cancellation stops retries.

The existing `post_with_retry` owns failures before an HTTP response begins,
including its total-attempt cap, admission rejection and over-cap Retry-After.
The new wrapper requires a `Start` event before it retries a provider error.
This is an intentional ARA boundary for this WIP; fixed OMP's outer wrapper
can issue one additional pre-Start request. Empty-stop retry requires an
explicit provider output count of 0 or 1. Unknown usage is not zero in ARA.
`accept_empty_response` opts out of this wrapper's retries.

**Open parity:** ARA now suppresses retries for selected structured in-band
account-quota codes and clear account-quota messages, while retaining retry
for short-term 429 throttles. This is not OMP's full account-usage-limit
classifier: other provider phrasings and non-2xx HTTP 429 account caps are
still open. OMP's image stream event is not in ARA's current event
protocol, although a terminal image block prevents empty-stop retry. OMP's
full retry classifier and the wider AI-RETRY surface remain open. The final
assistant message contains only the delivered attempt's usage and duration;
these are not aggregate cost or end-to-end latency across discarded attempts.
Do not infer total usage from them.

## Entry and observed behavior

The entry is `ara_ai::providers::openai_completions::stream`, used by
`OpenAICompletionsProvider` and the real `ara` binary. The wrapper is private
to `ara-ai`; it retains typed `ProviderError` for retry classification without
putting discarded attempts on the public event stream. A canceled active HTTP
attempt is stopped before an Aborted terminal waits for a slow consumer.

Controlled `FakeUpstream` HTTP/SSE checks on Windows:

| Case | Expected and observed |
| --- | --- |
| Known-empty stop, then text | 2 requests; one visible Start/terminal; final text `answer`, output usage 2 |
| EOS-only known-empty stops | 3 requests maximum; one delivered empty Stop |
| Empty stop with unknown usage | 1 request; unknown output stays `None` |
| 2xx keep-alive then reset before output | 2 requests; first attempt's markers discarded, `recovered` delivered |
| In-band status | 409/425: 1 request and error; 408/429: 2 requests and `recovered` |
| In-band account quota | `insufficient_quota` with neutral text, account monthly/rate-limit text, and billing quota without a token-limit anchor: 1 request, original 429 error delivered |
| Short-term quota throttles | Concurrent-request and generic rate-limit text with `quota_exceeded`, plus DashScope's billing text with the precise `error-code#token-limit` anchor: 2 requests and `recovered` |
| Tool-call event then reset | 1 request; no replay after the tool event |
| Recovered live text | `live` delta observed before the delayed terminal; later cancellation keeps the text |
| Cancel during retry wait | 1 request; Aborted terminal |
| Full outer event channel then cancel | Test waits for all 256 slots, cancels, delays draining over 2 seconds; exactly one Aborted terminal survives |
| Explicit valid-empty option | 1 request; empty Stop delivered |

The actual `ara` process test
`empty_stream_retry_reaches_one_tool_task_without_duplicate_effects` runs
against `FakeUpstream`: one empty request, one `write` tool-call response, and
one final response. It exits 0; `retry.txt` contains `once\n`; the Session
journal has one assistant tool call with ID `call_w` and one matching tool
result, with no entry for the discarded empty attempt. The fake upstream
records 3 requests, with authorization headers redacted. This is a controlled
host-chain task, not a real-model acceptance trial.

## Verification on the current WIP snapshot

```powershell
cargo fmt -p ara-ai -- --check
cargo clippy --workspace --all-targets --all-features -- -D warnings
cargo test -p ara-ai --all-targets --all-features --quiet
cargo test -p ara-cli --test e2e empty_stream_retry_reaches_one_tool_task_without_duplicate_effects -- --nocapture
cargo deny check
python scripts/omp_inventory.py check
cargo test --workspace --doc --quiet
```

Scoped checks pass: `ara-ai` has 31 unit and 35 HTTP tests, and the real
process test passes 1/1. Workspace strict Clippy, dependency policy (existing
duplicate warnings), fixed OMP inventory, and doc tests pass. The last
unfiltered Windows workspace run, before this quota follow-up, failed at the
two known `ara-tools --test tools` process-tree cases; an earlier unfiltered run
also reached the known CLI deadline failure. A current-snapshot diagnostic run with exactly
those three Bash/CLI cases plus the two existing Windows `pi-edit` hashline
cases explicitly skipped exits 0 across the remaining workspace targets.
These skips are not acceptance evidence for the omitted behaviors. The full
backend gate and bounded real-model task remain open.

Independent Codex review used `ara-git-review` and `ara-provider-review` on
the original retry diff against `e1d0850` and the quota follow-up against
`6997534`, each compared with the pinned OMP source. The first review found
and rechecked full-channel cancellation and 409/425 classification. The
follow-up review found two short-term throttles misclassified as account
caps: DashScope's documented token-limit 429 and a generic
`quota_exceeded`/`Rate limit exceeded` response. Both were fixed and their
HTTP fixtures passed. The reviewer independently ran 13/13 retry HTTP cases,
scoped strict Clippy, format and `git diff --check`; it found no remaining
confirmed P1/P2 issue in the scoped WIP diff. Neither review ran the full
workspace or real-model task. Pre-Start parity, aggregate usage, broad
account-cap classification, and pre-response 429 quota handling remain open.
AI-RETRYa and the AI-RETRY surface are **not accepted**.
