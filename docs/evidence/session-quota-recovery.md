# HTTP hints and same-Session quota recovery — bounded WIP

## Requirement and fixed source

Base `9cfdd16e581b69b3d16d81c10326f7db7652f28c`, branch `dev`; fixed OMP
`596f2da7101178214aa27a753529d15e6b7ad91d` (v18.1.8). Continue the
[saved-reset controllers](codex-reset-controllers.md) through an actual
post-emitted quota terminal. Record the rejected account even when visible
text, actual tools, cancellation, disabled retry or exhausted budget veto replay.
Thinking, whitespace and positively unexecuted tools may recover in the same
Session through a healthy sibling, a confirmed saved reset, or the earliest
allowed account window. Preserve the original failed journal entry and annotate
its recovery; never append the original user prompt twice.

Root read the original files with `git show` at the fixed SHA:

- `packages/utils/src/fetch-retry.ts:43-141,247-269,289-295`: first valid
  header/body hint, account-window precedence, retry ceiling and cancellation.
- `packages/ai/src/providers/openai-codex-responses.ts`: default six linear
  fetch attempts; structured error code remains independent of error text.
- `packages/coding-agent/src/session/agent-session.ts:3300-3303` and
  `session/turn-recovery.ts:592-618,2140-2240`: once-only usage feedback,
  sibling-first/reset/earliest-window recovery, retained failed turn and budget.
- The Native rate-limit reason/backoff grammar supplies the existing coarse
  Rust helper corpus. This is source comparison, not executed Bun acceptance.

## Implementation scope

| Files | Behavior |
| --- | --- |
| `ara-ai/src/retry_hint.rs`, `retry_classification.rs`, `lib.rs` | Ordered HTTP/body hints; separate Session maximum-header facts; ASCII case/boundary and JS whitespace semantics |
| `ara-ai/src/providers/openai_{completions,responses}.rs` | Preserve observed headers before body waits, quota/admission vetoes and selected generic/Codex fetch schedules |
| `ara-cli/src/model_route.rs`, `session_quota_recovery.rs`, `main.rs` | Private actual request/lease/route/Session/epoch correlation; owned durable rejection feedback; sibling/reset/earliest-wait outcome and real provider adoption |
| `ara-cli/src/rpc_host.rs`, `rpc_host_quota.rs` | Reader-reachable cancellation of preparation; retained raw entry/retry receipt; plain prompt creates a fresh epoch while keeping provider state |
| `ara-cli/src/auth_{storage,storage_state,broker_store}.rs`, `credential_store_port.rs` | Confirm the actual local block write or this remote write's acknowledgement; settle already owned feedback separately from retry admission |
| Existing provider/route/controller/Broker test families, `retry_http.rs` | Controlled HTTP/SSE, actual CLI/RPC processes, retained tools/journal/blocks and deterministic preparation/admitted-operation controls |
| `scripts/verify_quota_recovery.py` | Module selection and one optional stable comprehensive gate; private configuration and real trials are separate |

No credential, lease or request identity is serialized into an assistant journal.
The capability is generated only by the live typed quota terminal and actual
settled request. Runtime/static/environment authentication cannot claim a saved
row. Recovery additionally matches the complete Agent-retained message, current
Session/model/epoch and positive durable feedback. Stronger Native validation or
tool-output vetoes remain stronger; text resemblance is not correlation.

Canceling preparation permanently revokes its old Host epoch. A fresh prompt may
admit its own epoch. Canceling an awaiting caller cannot undo an admitted consume
or owned feedback; those settle before shutdown without granting a stale replay.
Remote block success requires `Ok(true)` from this write's sole POST; a previous
generic success/cache state is insufficient.

## Executed module receipts

All development, failed and successful receipts are retained under
`C:/Temp/ara-quota-recovery-batch`; each pins source/Cargo/test files before and
after execution. Unchanged passed targets are reused, not counted twice.

| Receipt directory | Target | Passed / failed / ignored |
| --- | --- | --- |
| `ai-groups/module-20261003T130710Z` | AI library | 141 / 0 / 0 |
| `ai-groups/module-20261003T131549Z` | HTTP hints | 4 / 0 / 0 |
| `ai-groups/module-20261003T132512Z` | Chat / Codex HTTP | 50 / 0 / 0; 6 / 0 / 0 |
| `cli-groups/module-20261003T134647Z` | Storage / Session / requests | 201 / 0 / 0; 59 / 0 / 0; 18 / 0 / 0 |
| `final-modules/module-20261003T141100Z` | Controllers | 18 / 0 / 0 |
| `final-modules/module-20261003T141209Z` | Responses / RPC retry / Broker processes | 33 / 0 / 0; 19 / 0 / 0; 1 / 0 / 0 |

Combined: **550 passed, 0 failed, 0 ignored**. Provider/Host tests use synthetic
credentials and isolated data roots. The successful controller receipt has
compile exit0 (10.846s), execution20.430s and `sourceUnchanged=true`. Its final
test SHA256 is `1065aab6fef74e32e0ce63d207cb5d5439dd57d2d1e540bb39979f26b7f2215b`.
The Responses receipt includes both Compatible and Codex exact quota-code,
usage-limit and replay-veto assertions in an existing coarse family.

### Failed observations and corrections

Failures are retained; they are not relabelled as successful whole runs.

1. Initial CLI compilation caught an incorrectly assumed Core re-export;
   both callers now use `ara_agent::agent_loop::retain_completed_tool_calls`.
   A subsequent Windows PDB/link failure coincided with insufficient build
   capacity. Restored capacity and a successful compile precede process results.
2. `diagnostic-groups/module-20261003T140359Z`: controller13/5. The actual
   terminal preserves Stream kind, `usage_limit_reached`, UsageAdmission and
   replay veto. Three captured sequences already contain successful replay
   `agent_end` before `auto_retry_end`; the test consumed it and waited again.
   The helper now checks the already observed successful terminal and recovery
   receipt. Production parser and frame ordering were unchanged.
3. Raw block queries omitted the credential-type suffix. The actual durable key
   is `openai-codex:oauth`; exact SQL lookup requires that key. All three queries
   were corrected while retaining the block, account and window criteria.
4. The original EOF closed admission immediately after the Agent terminal,
   before Host recovery preparation. Unknown-consume tests now wait for the real
   POST; no-hint tests wait for the actual maximum-delay refusal before EOF.
5. `ordering-groups/module-20261003T140953Z`: controller17/1. The captured
   `new_session` case had only usage/credit GETs and no model POST: startup usage
   rendering also lists credits. Only the new preparation family opts into a
   Gate after the actual model request, with an explicit POST-count assertion.
   Original manual/listing Gates retain their semantics. Final controller18/0
   exercises every loop that the earlier failures had prevented reaching.

No assertion was removed to permit model replay, reset spending, missing blocks,
unknown tool effects, lost journal identity or cross-Session continuation.

## Stable gate, real task and audit

The one stable gate in `final-20261003T141304Z` has
`sourceUnchanged=true`. It reports backend1831/0/20 in984.777s; ARA-owned format,
all-target/all-feature Clippy, all-target tests and doc tests PASS. Inventory
PASS in0.448s; explicit binary build PASS in0.406s. Dependency audit FAIL in2.686s
on existing `ttf-parser`0.25.1, `RUSTSEC-2026-0192`, through fontdue/snapcompact.
There is no safe version upgrade in that advisory and no waiver. The whole
runner correctly exits1; a successful backend does not erase this boundary.

The built binary was copied to an immutable trial path after the explicit build.
`binary-source-identity.json` links the build receipt and before/after source
pins to SHA256 `d631a44fc04f0c3512feceaedefc717b070aee68b727ae4801274e7136b95c40`.
The57 preserved vendor/EOL hashes match the baseline; graph and unrelated WIP
remain untouched.

### Bounded actual task

`live/20261003T143131Z/receipt.json` and `real-task-audit.json` record the current
configured CAS route and catalogue-confirmed `deepseek-v4.1-flash`, using explicit
OpenAI Completions. The local management proxy was unavailable; the enabled CAS
configuration supplied the direct fallback. Endpoint/key/local configuration
values remain outside Git. Limits: two Runs, six Agent model calls/Run,
120s/Run,8192 output tokens/call and140s outer observation.

- Task: read sales.csv, write exact summary.json and read it back. Four actual
  model calls and three successful correlated tool receipts (read/write/read),
  exit0,6.285s. Exact artifact is
  `{"pen":7.5,"book":24,"cup":21,"grand_total":52.5}`;
  SHA256 `0ea48896a15edf6b5eb0343122b74c6585aa9de0e786f68e98d3023741a971d9`.
- Resume: the same Session ID/journal, one actual model call, zero tools, exit0,
  1.641s. Exact formatted reply: `pen_quantity=3; cup_unit_price=5.25; grand_total=52.5`.
  Independent review found that this prompt supplied those answer values, so
  this Run proves format/continuation only and is not historical-recall evidence.
- Corrective history Run: `recall-receipt.json` preserves a new question with
  no answer values, asking original book quantity, pen unit price and cup quantity.
  One actual model call, zero tools, the original Session ID, exit0,1.257s;
  actual JSON `{"book_quantity":2,"pen_unit_price":2.5,"cup_quantity":4}` matches
  the original CSV. The book quantity was absent from the earlier echo prompt.
  Separate correction limits:one Run/one call/30s/1024 output tokens; the artifact
  and original two Run receipts are retained, with no tool/effect replay.
- Observed task usage: input3653/output387/cacheRead9472/total13512 tokens.
  Resume: input218/output21/cacheRead3328/total3567. Corrective history Run:
  input3006/output22/cacheRead0/total3028. Combined six calls/9.183s across three
  Runs. Monetary cost is unknown.
- Root independently inspected original events, all three actual tool receipts,
  persisted journal, artifact and source/binary identity. The requested bounded
  artifact/continuation/history criteria pass only with the corrective history
  Run; neither this task nor the fixtures prove
  real OpenAI account login/reset, live Broker or complete OMP acceptance.

Independent Codex `/root/broker_integration_review` used `ara-git-review`,
`ara-provider-review` and `ara-rust-core-review`. Final source/execution/document
POST is **APPROVE TESTED BOUNDED WIP**, recorded in
`C:/Temp/ara-quota-recovery-batch/post-integration-final.md`. Its earlier reports are
`ai-source-final.md`, `host-source-final.md` and
`host-diagnosis-supplement.md`. Root independently checked the material Native,
typed-terminal, actual event-order and exact durable-key facts.
`C:/Temp/ara-quota-recovery-batch/real-task-audit-correction.json` explicitly
limits the retained original audit's `RecallExact` field to exact format and
links the separate answer-free historical-recall receipt. Root inspected this
correction and the actual recall events; the original audit remains unchanged.

## Required differences and parent work

- Wide JavaScript Date.parse, extreme numeric/date/timer and UTF16 bounded-dot
  grammar remain required; the supported date forms/coarse corpus do not prove
  their complete equivalence.
- Fixed Codex renews its pre-response watchdog per fetch attempt. The current
  aggregate first deadline remains a recorded difference.
- Fixed fetch-retry returns a final retryable response before extracting its
  cloned-body/hints. ARA still extracts final hints and may set the delay veto.
- Complete configured model reserve/fallback/revert, REPL/TUI/ACP, Broker server
  and management, other provider/account/platform consumers and product hosts
  remain required. No real account login or saved reset is consumed by fixtures.
- Existing `ttf-parser` dependency acceptance remains open until the actual
  dependency gate says otherwise; there is no waiver in this batch.

No new point or complete surface is accepted:111 surfaces27 implementing/1
tested/83 open/0 complete;60 points35 limited accepted/23 implementing/2 tested.
Started28/111=25.2%; limited35/60=58.3% is not overall completion. P0/V1 accepted,
P1-P6 open, RPC27/42, full marker null. The full objective remains active.
