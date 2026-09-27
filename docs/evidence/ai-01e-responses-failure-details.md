# AI-01e extension: Responses failure details (WIP)

## Requirement and boundary

Fixed OMP commit `596f2da7101178214aa27a753529d15e6b7ad91d` formats
Responses failures in `packages/ai/src/providers/openai-shared.ts` lines
3196–3277. Its `packages/ai/test/azure-openai-responses-stream.test.ts` lines
339–400 exercises `response.failed.response.error`,
`response.failed.response.incomplete_details.reason`, and a failed
`response.completed.response.status_details.error`. This receipt uses the
Azure test as evidence of the shared terminal shape, not a claim that ARA
implements the Azure adapter.

Previously `ara-ai::responses_stream::response_error` kept only a direct
message. It lost the structured code and nested reasons, making failed Runs
hard to diagnose. The production change is local to Responses error
extraction. It covers direct and nested error objects, incomplete and status
reasons, and generic stream `error` frames. JSON `null` at the direct error
position falls back to the nested or top-level details as in OMP's `??`
selection. Agent, Session, CLI production code, status mapping and retry
classifier are unchanged.

## Executed evidence

Target code: local WIP `1da7d19` on `dev`, based on `aac0b57`. The checks ran
on that exact code tree before commit; no code changed afterward. Windows;
fake loopback HTTP/SSE upstream; no live key or model call. The new
seven-shape HTTP regression first failed on the old code: it returned
`backend exploded` instead of `server_error: backend exploded`. On the fix,
all cases produce one terminal error and retain the streamed `draft` text:
direct error, incomplete reason, failed final status with nested error,
direct `error:null` with nested error, cancelled status reason, generic error,
and generic `error:null` with top-level code/message. A separate fake HTTP
test confirms a pre-output `server_error` makes one replay-safe retry and
publishes only the recovered attempt. This is a consequence of the existing
transient-message classifier seeing the newly preserved code; a visible
partial output still blocks replay.

The actual `ara` CLI process receives a partial Responses text event and a
coded failure. It exits 1, prints `server_error: backend exploded` on stderr,
retains `draft` and the same error in the Session journal, and makes one
request. The provider HTTP suite includes normal responses, tool and history
paths, cancellation, and other failure cases.

| Command | Observed result |
| --- | --- |
| `cargo test -p ara-ai --test openai_responses_http failed_response_frames_preserve_nested_codes_and_reasons -- --exact` | Failed on old code; passed after fix and independent review corrections. |
| `cargo test -p ara-ai --test openai_responses_http pre_output_server_error_retries_once_without_duplicate_client_events -- --exact` | Passed; two upstream requests, one public Start and terminal Done. |
| `cargo test -p ara-ai --lib` and `cargo test -p ara-ai --test openai_responses_http` | 122/122 unit and 31/31 Responses HTTP tests passed on final code. |
| `cargo test -p ara-cli --test e2e responses_failed_detail_reaches_cli_and_session_journal -- --exact` | Passed; exit 1, exact stderr and journal error, partial text retained. |
| `python scripts/verify_backend.py` on the final code tree | Owned formatting and workspace all-target/all-feature Clippy passed. Workspace tests reached CLI e2e: 43/44 passed; the existing Windows Bash `deadline_during_a_tool_and_zero_budget_exit_nonzero` exceeded its four-second assertion. Later verifier stages were not run. |
| `cargo deny check --hide-inclusion-graph` | Exit 0; advisories, bans, licenses and sources OK with pre-existing duplicate warnings. |
| `python scripts/omp_inventory.py check` | Passed. |

Independent Codex pre-implementation review identified the bounded gap.
A separate independent Codex post-diff reviewer found two reachable `null`
fallback defects; both were corrected and covered with SSE cases. The reviewer
also flagged the retry-classifier interaction, which the second controlled
test now exercises. No further confirmed defect was reported.

The full delivery gate and bounded real-model task remain open, so this
extension does not change any accepted-point or P0–P6 gate count. The
separate Windows Bash process-tree issue remains documented in
[`windows-bash-process-tree.md`](windows-bash-process-tree.md).
