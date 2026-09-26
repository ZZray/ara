# AI-RETRYa: replay-safe retry of uncommitted OpenAI Chat attempts (WIP)

## Requirement and boundary

Fixed OMP `596f2da7101178214aa27a753529d15e6b7ad91d` implements
`withReplaySafeStreamRetry` in
`packages/ai/src/utils/empty-completion-retry.ts`, binds it in
`packages/ai/src/providers/openai-completions.ts:1492-1500`, and exercises it
in `packages/ai/test/empty-completion-retry.test.ts`. Relevant inventory cases
include B-f3204ba5ac, B-d6c439c35f, B-c33421f9ce, B-86ff424237,
B-8176bb72d4, B-c1662ae6aa, B-835c791d85, B-d43bbe6198 and
B-94812de6f0. This ARA slice retries a known-empty stop at most twice and a
typed transient error from an uncommitted attempt at most once, including a
failure before the 2xx stream emits `Start`. It
buffers pre-output lifecycle events; the first text, thinking or tool event
commits the attempt and is forwarded live. Discarded attempts do not reach
the Agent or its tool dispatcher. Caller cancellation stops retries.

The existing `post_with_retry` owns the inner HTTP retry policy (six attempts
by default, matching fixed OMP `openai-http.ts`). The wrapper can issue one
additional attempt before `Start`, giving at most twelve requests for two
exhausted transient inner rounds. Admission rejection and over-cap Retry-After
block both retry layers, including when the error body stalls. Empty-stop retry requires an
explicit provider output count of 0 or 1. Unknown usage is not zero in ARA.
`accept_empty_response` opts out of this wrapper's retries.

**Open parity:** The broad outer account-cap classifier is documented below.
The inner HTTP layer can still repeat account-cap 429s before the final error
is classified. ARA now has the OMP `image_end` event shape and commits the
outer retry attempt on it, but its current OpenAI Chat adapter does not emit
output images. OMP's wider AI-RETRY surface remains open. The final
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

## Verification on the prior started-stream WIP snapshot

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
AI-RETRYa and the AI-RETRY surface were **not accepted** on that snapshot.

## Pre-Start follow-up on the current WIP snapshot (2026-09-26)

Code commit: `ff4f47d` (WIP; local `dev`).

Fixed OMP `empty-completion-retry.ts` retries an uncommitted finalized transient
error without requiring `Start`. ARA removed that gate while retaining its
typed side channel. `post_with_retry` now passes a private no-retry decision
for a terminal non-2xx account cap, concurrency admission rejection, or
over-cap `Retry-After`. Header decisions are recorded before awaiting the
error body, so a first-event timeout cannot erase them. The public terminal
error keeps the final status and readable detail; discarded attempts never
reach the Agent or Session journal.

Controlled FakeUpstream checks on this WIP code:

| Case | Observed result |
| --- | --- |
| Pre-Start 408, 429 throttle, or 503 with inner cap one | Two requests; one visible Start and `recovered` terminal |
| Pre-Start structured `insufficient_quota` or monthly account cap | One request, original 429 terminal, no Start |
| 429 admission marked in body or response header | One request, original 429 terminal, no Start |
| Admission or over-cap Retry-After header followed by a stalled error body | First-event timeout terminal; one request, no outer replay |
| Cancel during Pre-Start outer backoff | Aborted terminal; one request |
| Inner cap three exhausted in both outer rounds | Six total requests; final 502 detail `gateway` |
| Two slow headers or two pre-output resets | Two bounded attempts; final timeout/error without a tool call |

The actual `ara` process test
`pre_start_http_retry_reaches_one_tool_task_without_duplicate_effects`
receives six immediate 503 responses (`Retry-After: 0`), then one `write`
tool-call response and one final response. It exits 0 with stdout
`Wrote retry.txt once.`; the ephemeral work directory contains `retry.txt`
with `once\n`. The Session journal has one assistant tool call `call_w` and
one matching tool result, with no entry for the six discarded HTTP attempts.
FakeUpstream records eight requests and redacts the authorization header.
This is a real host chain against a controlled upstream, not a real-model
trial.

| Command on current WIP worktree | Result |
| --- | --- |
| `cargo fmt -p ara-ai -p ara-cli -- --check` | Exit 0 |
| `cargo test -p ara-ai --all-targets --all-features --quiet` | Exit 0: 31 unit and 40 HTTP tests |
| `cargo test -p ara-cli --test e2e pre_start_http_retry_reaches_one_tool_task_without_duplicate_effects -- --nocapture` | Exit 0: 1 real-process fake-upstream tool task |
| `cargo clippy --workspace --all-targets --all-features -- -D warnings` | Exit 0 |
| `cargo test --workspace --doc --quiet` | Exit 0 |
| `cargo deny check` | Exit 0, with existing duplicate and license-field warnings |
| `python scripts/omp_inventory.py check`; `git diff --check` | Exit 0 |
| `cargo test --workspace --all-targets --quiet` | Exit 1: 15/18 CLI e2e tests passed; `deadline_during_a_tool_and_zero_budget_exit_nonzero`, `tool_task_produces_file_and_receipts`, and `resume_runs_tools_in_the_session_cwd` failed on this Windows environment. The latter two report Bash not found or its missing output file. The run stopped at this target; it does not prove later targets. |

Independent Codex review applied `ara-git-review` and
`ara-provider-review` to the four-file worktree diff. It found the missing
header-only concurrency admission check and the stalled-body timeout window.
Both were fixed; the reviewer independently ran the two new fixtures (1/1
each), checked the final diff against the fixed OMP source, and found no
remaining confirmed P1/P2 defect in scope. A bounded real-model task, the
unfiltered full Windows gate, wider account-cap classifier, inner non-2xx
account-cap suppression, image event, and aggregate retry accounting remain
open. The point stays **implementing (WIP), not accepted**.

## Account usage-limit classifier follow-up (2026-09-26, WIP)

Code commit: `c285bd1` on local `dev`. The private
`ara-ai::usage_limit::account_usage_limit` classifier replaces the narrow
OpenAI Chat account-cap check at both terminal HTTP and in-band stream error
sites. It is adapted from fixed OMP
`packages/ai/src/error/{rate-limit,flags,retryable}.ts` and
`packages/utils/src/fetch-retry.ts` at
`596f2da7101178214aa27a753529d15e6b7ad91d` (MIT). It changes only the
outer replay decision after a finalized error. The existing inner HTTP 429
attempt cap and public provider contract are unchanged.

Controlled cases distinguish opaque 429 account caps; subscription, daily
free-model, ClinePass, Chinese quota, billing, and structured Google RPC
account caps; from per-minute, concurrent, Chinese transient, and DashScope
token throttles. Google `RATE_LIMIT_EXCEEDED` uses a five-minute boundary
for relative `RetryInfo` milliseconds/seconds, reset-in text, and absolute
English/Chinese reset timestamps. Absolute reset dates take precedence over
short `Please retry` hints, following fixed OMP. Short or expired hints keep
the transient retry path. HTTP and in-band fixtures assert the actual request
count and terminal outcome; unit fixtures cover 402/403 and code precedence.

| Check on `c285bd1` | Result |
| --- | --- |
| `cargo test -p ara-ai --all-targets --all-features --quiet` | Exit 0: 35 unit, 42 HTTP tests |
| `python scripts/verify_backend.py` | Owned-package formatting and workspace strict Clippy pass; unfiltered all-target tests stop at 3 existing Windows CLI e2e failures (15/18 pass): Bash not found, tool deadline exit, resume file missing. The script does not reach doc tests. |
| `cargo test --workspace --doc --all-features --quiet` | Exit 0 (separate run after the stopped verifier) |
| `cargo deny check`; `python scripts/omp_inventory.py check`; `git diff --cached --check` | Exit 0; deny retains existing duplicate/license-field warnings |

Independent Codex review first found that `300000ms` and `reset in 10
minutes` were missed, then found absolute reset timestamps were missed. Both
findings were fixed with HTTP and in-band regressions. Final read-only
re-review found no further confirmed P1/P2 in this scoped diff; the reviewer
did not rerun tests. The current worktree was tested locally, and the code
was committed as WIP because the full delivery gate and bounded real-model
task have not passed. Aggregate retry usage, provider image output, the wider
AI-RETRY surface, and the prior inner HTTP account-cap behavior remain open.

## Image event commit follow-up (2026-09-26, WIP)

Code commit: `accd018` on local `dev`. Fixed OMP
`packages/ai/src/types.ts:1341` defines an `image_end` event with content
index, `ImageContent`, and a current assistant snapshot;
`packages/ai/src/utils/empty-completion-retry.ts:48` treats it as meaningful
output. `ara-ai::event::AssistantMessageEvent` now carries that variant and
prints a tagged `type: image` content object without repeating the partial
snapshot. `ara-ai::replay_safe_retry` immediately commits the attempt on the
event. The existing Agent and CLI generic event paths need no state-machine
change. ARA's existing `ImageContent` only covers its base fields; optional
upstream image metadata and a provider that actually emits output images are
still open.

An in-process synthetic provider attempt emits `Start` and `ImageEnd`, waits
before its terminal 503, then finishes with an error. The test observes the
image before the terminal and verifies exactly one attempt, one Start, and no
replay after the 503. A separate event-shape test checks `contentIndex`, the
tagged image JSON, the current snapshot, and nonterminal status. These tests
prove the retry wrapper's contract, not an OpenAI Chat image-output route.

| Check on `accd018` | Result |
| --- | --- |
| `cargo test -p ara-ai --all-targets --all-features --quiet` | Exit 0: 37 unit and 42 HTTP tests |
| `cargo test -p ara-agent --all-targets --all-features --quiet` | Exit 0: 32 loop, 12 compaction, 8 call, 7 cut, and 4 other tests |
| `python scripts/verify_backend.py` | Owned formatting and strict workspace Clippy pass; all-target tests stop at the same 3 Windows CLI e2e failures (15/18 pass): Bash start, deadline exit, resume file. Doc phase not reached. |
| `cargo test --workspace --doc --all-features --quiet` | Exit 0 separately |
| `cargo deny check`; `python scripts/omp_inventory.py check`; `git diff --cached --check` | Exit 0; deny retains existing policy warnings |

Independent Codex plan and diff reviews checked the fixed OMP event and retry
semantics, the changed two files, and Agent/CLI consumers. Final read-only
review found no reachable defect in scope and did not rerun tests. No
controlled upstream in the current ARA provider stack produces `image_end`,
so the real Provider → Agent → CLI path and bounded real-model task remain
unverified. The point remains **implementing (WIP), not accepted**.

## Statusless in-band transient error follow-up (2026-09-26, WIP)

Code commit: `2a2a6b5` on local `dev`. Fixed OMP
`packages/ai/src/providers/openai-completions.ts:643-665` turns an SSE error
envelope without a numeric status into a provider response error. Its
`utils/empty-completion-retry.ts:147-180` wrapper asks the shared classifier
whether an uncommitted error may be retried. The classifier in
`error/retryable.ts:42-60`, `error/flags.ts:159-160,306-312,791-819`, and
`packages/utils/src/fetch-retry.ts:393-395` recognizes transient message
phrases and statuses embedded in text, but excludes account usage caps and
permanent 4xx errors. ARA previously treated every statusless in-band error
as `ProviderError::Stream`, which could not trigger this outer retry.

`ara-ai::replay_safe_retry` now applies those fixed-OMP message/status
patterns only to an uncommitted `Stream` error. It consults the existing
account-cap classifier first and keeps the one-extra-attempt budget. The
provider wire request, public events, Agent, CLI, Session, and inner HTTP retry
policy are unchanged. This is a parity repair: fixed OMP also retries ordinary
inner 429 responses except its concurrency-admission and over-cap delay cases.
Suppressing every inner account-cap 429 on its first response would be a
separate intentional difference, not a remaining fixed-OMP parity step.

The new controlled HTTP fixture for `Service unavailable` failed on the old
code: one request and a terminal error. On the repaired code, a first 200 SSE
response with a statusless transient error followed by a healthy response
produces two requests, one visible `Start`, one terminal, and text `recovered`.
The same result holds for an upstream connect error, socket closure, stream
read error, truncated JSON diagnostic, and `HTTP 408`/`HTTP 501` text. Negative
fixtures keep `HTTP 429` with an opaque body, `HTTP 401 Service unavailable`,
an account rate cap, and an invalid schema at one request. A transient error
after text `partial` also stays at one request and ends with that text and an
error; committed output is never replayed. The `HTTP 429` negative fixture
first exposed a false retry in the initial candidate and passed after the
status-in-message and account-cap gates were added.

| Check on final code | Result |
| --- | --- |
| `cargo test -p ara-ai --all-targets --all-features --quiet` | Exit 0: 37 unit and 45 HTTP tests |
| `python scripts/verify_backend.py` | Owned formatting and strict workspace Clippy pass; unfiltered tests stop at the existing Windows CLI tool-deadline case (17/18 e2e pass, verifier exit 101). The doc phase was not reached. |
| `cargo test --workspace --doc --all-features --quiet` | Exit 0 separately; no doc test cases |
| `cargo deny check`; `python scripts/omp_inventory.py check`; `git diff --check` | Exit 0; deny retains existing policy warnings |

An independent Codex plan review established the fixed-OMP gap and warned
against changing inner account-cap 429 behavior. Independent diff review found
two candidate defects: false replay when a statusless message embeds a
permanent or opaque HTTP status, and missed socket/read/408/other-5xx transient
phrases. Both were fixed with HTTP fixtures. Final read-only re-review covered
both changed files and reported no remaining confirmed defect; it did not run
the full host chain or real-model trial. The unfiltered Windows backend gate,
bounded real-model task, and wider AI-RETRY surface remain open. AI-RETRYa is
still **implementing (WIP)**, and no acceptance count changes.
