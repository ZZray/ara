# RPC Session retry checkpoint — 2026-10-01

Status: **tested WIP** on `8d20531514951550ccb3c23a231ef1fb971e42cf`.
This batch implements bounded `set_auto_retry` and `abort_retry` behavior and
same-route Session recovery: **27/42 bounded RPC implementations**. Complete Retry, CA-RPC, fixed OMP
parity and P1–P6 remain open; the fixed marker does not advance.

## Fixed source and implemented behavior

Fixed OMP: `596f2da7101178214aa27a753529d15e6b7ad91d`.

- `session/turn-recovery.ts:654–790,1153–1425`: actual request API,
  failed-message eligibility, native pending error IDs, recovery metadata and
  unsafe output/tool-effect vetoes.
- `turn-recovery.ts:2064–2113,2120–2605`: wait hints, saga, budget, native
  continuation, phase-specific abort, terminal settlement and queue handling.
  Ordinary hints only increase exponential backoff; stale Responses has its
  independent zero-delay/provider-reset branch.
- `session/retry-fallback-chains.ts:72–77`: exponential delay capped at
  8,000 ms with 0–25% downward jitter. Continuation is deferred another 1 ms.
- `config/settings-schema.ts`, `settings.ts`: native retry defaults
  `enabled=true`, `maxRetries=10`, `baseDelayMs=500`, `maxDelayMs=300000`.
  Numeric settings retain negative, fractional and zero semantics.
- `modes/rpc/rpc-mode.ts:1390–1398`: persistent policy and abort dispatch;
  `agent-session-events.ts:34–51`: retry start/end and recovered error receipts.
- `packages/ai/src/error/{flags,retryable}.ts` and provider constructors:
  typed failure/status/flags and bounded, non-secret wait facts.

The Host joins the failed Run before starting its Session-owned saga. The
original Agent, transcript, queues and authorized route remain owned; provider
inner/outer retry budgets remain separate and unchanged. Waiting uses an
independent deadline/token and stays responsive to serial commands. Policy
disable does not cancel previously admitted work. `abort_retry` cancels waiting;
an already started continuation retains its own Run cancellation. EOF settles
accepted retry work. Retry suppresses autonomous queue drain/compaction while
the failed turn is pending.

The Run sink retains actual IDs returned by native append. Only an eligible
failed active tail is removed; raw journal evidence remains. Fully proven
synthetic `executed:false` tool pairs are retained. Missing, actual, panicked
or unknown-effect tool results do not authorize destructive replay. Recovery
rewrites exact failed native entries before publishing successful events;
persistence failure stops the connection. Authorized current-Session Bash
receipts can extend the expected transcript during backoff.

Provider `failureEvidence` keeps kind, status, safe code/wait and flags.
`replayBlocked` preserves the original provider veto; `sameRouteBlocked`
controls Session admission. A long hint blocks provider replay while the
Session applies its own ceiling; admission/usage vetoes remain distinct.
Non-2xx header facts are saved before waiting for a body, so first-event
watchdog cancellation cannot erase them. New HTTP attempts/success clear
previous header evidence. Full headers and credentials are not retained.

## Delivered files and Windows verification

Fifteen source/test files: eight `ara-ai` paths (`error`, `lib`, three provider
adapters, `retry_classification`, `types`, `usage_limit`); five `ara-cli`
paths (`main`, `rpc_host`, `rpc_host_retry`, `rpc_host_settings`, `rpc_retry`);
and Session journal/recovery tests. Receipt root: `C:\Temp\ara-retry-batch`.

| Final check | Result | Time |
| --- | --- | --- |
| `python -X utf8 scripts/verify_backend.py` | 1,316/0/1 across 99 suites; fmt, all-feature Clippy, target/doc tests pass | 105.980 s |
| `cargo deny check` | Pass | 2.657 s |
| `cargo build -p ara-cli --bin ara --all-features` | Pass | 0.341 s |

`full-receipt.json` pins all fifteen final files before/after the 108.982 s
pipeline. Binary SHA-256:
`736f4f087b275a08e9ca119494b9db85ed86c76d339cc23e2d2a03cb6838ce88`.
`committed-source-audit.json` verifies twelve exact raw/blob matches;
`error.rs`, `types.rs` and `usage_limit.rs` differ only by tested CRLF to
committed LF normalization. The existing ignored ara-ast fixture remains.

Earlier focused checks passed 8 classifier, 3 safety, 13 settings, 3 native
metadata and 11 actual RPC process tests. Final full execution includes all
those tests plus the new HTTP header/body-stall oracle: three actual APIs ×
admission/long-wait cases, each bounded to two seconds with a 100 ms watchdog.
It checks one request, terminal Timeout, retained HTTP 429/wait and both vetoes.
No redundant focused rerun is claimed after the final full gate.

The eleven RPC child cases exercise provider-budget exhaustion, same-Session
retry and one actual tool effect/restart; waiting abort; disabled/new/switch/
restart policy; Session budget exhaustion; HTTP/SSE hints and wait ceiling;
visible text veto; retained synthetic false pair; controlled incomplete native
journal with unknown effect and no replay; long provider hint with independent
Session ceiling/cancel; current Bash during backoff; and zero budget with one
attempt-0 end and no start/replay. Journal truncation is a controlled fixture,
not proof of every real crash window.

First failures are retained: `focused-compile-first-*` (PostError assertion),
`focused-value-order-first-*` (YAML root ordering), `full-clippy-first-*`,
`full-clippy-second-*`, and `full-attempt-first-*` (terminal attempt regression).
The budget assertion was not relaxed. Settings tests compare every retained
root key/value and length, with exact nested values. Failed gate count fields
sum only successful suites; failure status/raw logs are authoritative.
`full-before-delay-*` and `full-delay-*` preserve intermediate passing snapshots.

## Bounded actual task and failure disposition

`real-retry-b2mt98r0` freshly verifies B.AI `deepseek-v4.1-flash` and its
Chat Completions endpoint. The relay injects twelve HTTP 503 responses to
exhaust the existing 6×2 provider budgets, then forwards at most three real
requests. Each Run allows three model calls, 1,024 tokens/call and 35 seconds;
the total wall bound is 120 seconds and native retry budget is one. Only the
relay holds the real environment credential; the child receives a dummy key.

Observed **PASS**, **6.786 s**, **two Runs**, one failed logical call and two
successful real calls, **14 proxy requests = 12 fake + 2 actual**. The original
failed entry `1c38b7e2` remains sourced and receives matching native/event
recovery metadata. Same Session ID is retained. One native `write` receipt
matches the exact unpredictable nonce/completed artifact; the final assistant
answers `done`. Idle native reopen retains metadata without another request or
rewrite. The first thirteen request bodies are equal; the last is the actual
tool follow-up. Captured successful usage reports 2,952 + 2,960 total tokens;
failed/physical remote-attempt usage and cost remain unknown.

The executed driver, inputs/stdout/frames, proxy bodies/SSE, journals, artifacts
and source/binary pins remain under that trial directory. Driver SHA-256 is
`d0524a04d68aed091fefca5becaab5ae5007544037677b76274f6940ad797cdc`.
Later real forwarding waits for the previous actual request's successful
terminal proof and settlement. Any early disconnect/upstream failure blocks
further forwarding; only a local peer close after a proven terminal permits
draining the remaining trailers.

The first trial `real-retry-vs06kgmx` remains **FAIL**, 4.909 s, twelve fake
responses and one actual call. Raw SSE proves a tool-call terminal, usage and
DONE; native storage and artifact prove the single write. Request fourteen
was blocked before remote forwarding. The error was WinError 10053; its raw
summary has no stack to prove the exact throw site. After independent raw
diagnosis the driver was corrected only around local `wfile.write/flush`
after terminal proof. The successful trial used a fresh isolated task/nonce;
the failed Session/effect was not replayed. Cleanup admission and truncated
stream races found during driver preflight were also corrected before trials.

## Independent audit and remaining mandatory work

Independent Codex `host_bridges_independent_review` reviewed the plan, Host,
Session, policy, safety and eleven child oracles. Three P2 findings were fixed:
authorized Bash transcript drift, zero-budget terminal omission, and hints
shortening backoff. Independent `ctx_oracle_diff_review` covered the eight
provider/classifier paths and reviewed the body-stall repair. Their source
receipts and final pins remain in `source-review-root.json` and
`source-review-provider.json`. Driver preflight and first-failure diagnosis are
recorded separately. `retry-final-independent-review.json` independently
passes 46 raw checks: exact Windows log/counts/pins, tested/committed newline
boundary, stdout/inputs/Run terminals/native write and recovery, request bodies,
successful SSE terminals, idle reopen and distinct failed-trial retention.

Exact-code Linux [repository run 36750535284](https://github.com/ZZray/ara/actions/runs/36750535284)
passes **1,319/0/1 across 99 suites**, including fmt, all-feature Clippy, target
and doc tests. Backend RUN-to-PASS takes **276.926 s**, including fresh
compilation. `linux-final.log`, `linux-final-status.json` and `linux-audit.json`
retain exact SHA, commands, counts and raw hash
`293309d12b59ac66213d5c227f23131faf9b28e634e3c30e56d503f872d55042`.
Same-code Skill directory-fault/invocation runs also succeed (36750535281,
36750535225); their status is not a repeated independent surface audit.

Final independent raw/document audit: **BOUNDED_FINAL_PASS** on the pinned
code. Complete Retry and OMP acceptance remain open.

All remaining fixed behavior is mandatory reproduction work: usage-backed
overflow/compaction continuation; ThinkingLoop guard/redirect and special
stream limits; full provider flags and interrupted-turn recovery; native
credentials, OAuth, usage/preflight, request identity and rotation; native
model catalogue/roles/aliases, fallback chains, API/context/effort and reserve
checks; cooldown/revert, Fireworks intrinsic downgrade, fallback before final
exhaustion; and full settings overlays/migration/conflict/quarantine behavior.
Neither these commands nor a test count close those contracts. Continue all
R2/R3/R4 and every required inventory row before ARA customization.
