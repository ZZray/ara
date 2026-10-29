# OpenAI custom Chat upgrade — bounded delta accepted

Target comparison: original `596f2da7101178214aa27a753529d15e6b7ad91d`
to fixed `b91db66e9b0b65a2c977e6a5bc74b0f4b1d58d1c`. Complete original
acceptance remains **7/111**. Full upgraded Chat, account login, P1–P6 and
product/release gates remain OPEN; upstream acceptance markers are unchanged.

## Final execution and acceptance

Root and independent Codex execution/point audit accept this **bounded ordinary
JSON throttle/connected-header callback delta**, not the complete in-band helper,
Chat provider or Agent. Union: **62 distinct successful Rust functions**,
58 in the related HTTP target and4 selected Host functions. Exactly2 functions
run on the final test images;60 are explicitly source-bridged historical
successes. They are not reported as62 fresh final-image executions.

Final build:157.182s,504 reused/2 compiled/0 errors; ordinary CLI64bf03d7 is
reused. Final matrix1/1 PASS0.631s; actual Host group1/1 PASS2.158s, with all six
markers proving POST counts1/2/2/2/1/1, one settlement each, zero auth retry and
quota reset, exact text/tool identity and unchanged wire credential/request.
All1200 before/after/current source pins, six delivered hashes and index89d
match. This is real Rust HTTP/Graph Host fixture evidence; no live model or
whole CLI error-journal/restart acceptance is inferred.

Three Cargo invocations total890.742s (14.8min), including the first syntax
failure. Actual test executions total about48.93s, including the failed groups.
The final repair rebuilds only two test programs; no production code changed
after the second build. All failed packets, diagnostic/review misses and raw
outputs are preserved. Commit/push/deployment/installation/model calls:0.

The command/test selection below describes the initial plan. The final plan
`9645a915` selects only the two corrected complete functions; the earlier
successful60 functions are reused through independent source bridges.
Final physical test hashes:

- HTTP: `7c8e844cbaaf055b75e78d24887870d9cdfb97dd2dab0e11cba4bbf0415a20b3`
- Host: `e3f699e17b3670a7ed794535a03b0c118da48bb204f8ee3c8e8b4e686a172ff0`

Final independent receipt is
`omp-upgrade-chat-runtime-root-v3/INDEPENDENT-EXEC-POINT-AUDIT.json`, decision
`APPROVE_BOUNDED_ORDINARY_JSON_AND_HEADERS_WATCHDOG_DELTA`. Root inspected the
raw compiler/tests/six Host markers and index, not merely the PASS summary.
Exact final audit SHA256:
`0c1c91ac2062d4c3aef95ad87f76b35036e565019ff20f225b3dd31da4ba3b68`.

## Requirements and current owners

| Fixed source behavior | Rust owner | Required execution |
| --- | --- | --- |
| Successful headers end the connection watchdog before awaiting `onResponse`; body first-item timeout starts afterwards | `ara-ai/providers/openai_completions.rs::run_prepared`, shared by typed and Graph transport | Slow callback success, callback rejection/cancellation, slow headers and separate body timeout |
| JSON in-band throttle probe runs before the legacy stream-error guard | `in_band_provider_error` then existing `stream_error`; Graph `handle_graph` observation | Typed/Graph numeric and string statuses, code normalization, bounded error unwrapping, precedence, ordinary chunks, permanent/auth/account controls |
| Synthesized opaque 429 detail must not authorize credential rotation | Existing `usage_limit::opaque_status_body` and account/retry owners | Exact informative message plus actual Host credential/reset counters and settlement receipts |
| Classification and local provider retry are separate owners | Existing cause-based replay wrapper; terminal `failure_evidence` flags | Structured status retry before output, no replay after text/tool, statusless transient reaches Session classification without adding a local POST |

Source anchors: `packages/ai/src/providers/openai-completions.ts:946-949,1350-1365`,
`packages/ai/src/error/body-error.ts:185-286`,
`packages/ai/test/openai-first-event-timeout.test.ts:660-693`, and
`packages/ai/test/error-body-rate-limit.test.ts` at the exact new SHA.

Independent source PRE: Codex `chat_upgrade_review`, decision
`APPROVE_BOUNDED_PLAN_WITH_ACCEPTANCE_REQUIREMENTS`, receipt
`omp-upgrade-chat-review-root-v1/PRE.json` SHA256
`bda048bd0a703361c0558646edc20afb542bb3b79a55faba99e885ceb0ae22eb`.
Independent initial source POST approves the initial six-path bounded delta:
`omp-upgrade-chat-review-root-v1/SOURCE-POST.json`, SHA256
`68209eeb5c51ee08f026eea24cff60365c716b4cd1a5083ffbd63d78aaab70b9`.
Root read the report and candidate hunks, checked all before/after hashes and
adopted the six files with index `89d34e26` unchanged. This is source approval,
not delivered runtime acceptance by itself; the final execution/point audit
above completes only the named bounded scope.

## Initial plan and preserved recovery history

One locked/offline incremental build selects `ara-ai --test openai_http`
and `ara-cli --test model_route`. It keeps the existing D target, toolchain,
profile/features and one Cargo job. Cargo may also compile its actual binary
dependencies; artifact counts will be reported from compiler output.
No workspace, all-target or cache-clean command is scheduled.

The complete related Chat HTTP test target includes three new coarse tests:
`upgraded_chat_connected_callback_boundary`,
`upgraded_chat_structured_inband_matrix`, and
`upgraded_chat_structured_throttle_replay_boundary`; actual Graph observations
share that target. Four selected Host tests cover notification ordering,
callback rejection, new actual route/error/settlement behavior and stable
credential identity across wire retry layers.

Runtime packet: `omp-upgrade-chat-runtime-root-v1` under the existing private
D evidence cache. Driver PRE independently approves exact runner SHA256
`27bf4b24216480b467840237c6d9232752848c4090fac0ebf8ea28c7ae203119`
and execution plan `853b5195a303409e35faffe0e5b226e51ca7d9eb21759fb226e63f0abf312e04`.
Before/after source hashes, Git index, selected physical executables, process
handles, deadlines, stdout/stderr, real POST counts and failures are retained.
The first build started at 10:41:44 UTC, Cargo PID25448, desktop exec
session14320, and terminated with exit101. Actual build372.092s,496 reused,
7 compiled,85 warnings and1 syntax error: new Host JSON macro at
`model_route.rs:2504` lacked one closing brace. No tests ran. Source and index
remained unchanged throughout that attempt; the full failed packet is retained.

Root repaired that one byte against the original raw candidate, preserving
unrelated line endings and WIP. The v2 checkpoint Host hash is
`ea8055bd3dfb6000f1b5e272bc69bba78511c1ea5246257978fe9a4f8f883a4a`;
all other five delivered source hashes are unchanged. Before another Cargo
invocation, six exact changed Rust files passed a non-mutating parser check:
`rustfmt --edition 2024 --config skip_children=true --emit stdout`, with
exit0 and source hash invariance. This is syntax parsing, not a formatting
gate or behavior acceptance. Packet `omp-upgrade-chat-runtime-root-v2` retains
those receipts. Narrow independent source POST v2 APPROVE binds the repaired
Host and unchanged five AI paths: SHA256
`3fe9a617a40e9780cc5325cd4f697fe3e24813a063d2e1a2ca37012e108421ca`.
The second affected-target build terminated PASS:361.468s,503 reused,
3 compiled,0 errors. Exact related HTTP target58 tests executed41.233s:
**57 PASS,1 FAIL**, zero ignored. Its supplemental `camel_dot` account flag
expectation was false but the fixed source classifies the normalized
`throttling_allocation_quota` suffix as UsageLimit as well as Transient.
Both supplemental camel spellings require expected `true`; production
classification matches that source and is unchanged.

Zero-Cargo continuation runs three existing Host functions successfully:
notification1.920s, callback rejection0.175s, stable wire credential identity
0.714s. The new actual Host group fails after four passing scenario markers:
it wrongly equated the Session same-route flag with local output commitment.
Actual text case POST1/content/credential/settlement assertions already pass.
That partial group remains FAIL, not accepted. Independent source diagnosis
SHA256 `699227987f19ba895f01dfde58d9fd214c674ad7dfbead5725bd0df8d2fc2c53`
verifies both incorrect fixture assumptions, all1200 source pins, index and
three physical executables. The first two failed packets remain intact.

Three test-only repairs set the two supplemental account expectations,
explicitly check the Graph UsageLimit flag, and separately assert Session
same-route permission and Graph local `replayBlocked`. All original POST,
text/tool ID, credential, reset and settlement checks remain. Six immutable
syntax checks pass. Production code is unchanged. A final two-target build
and execution of only the two corrected functions subsequently completed after narrow
SOURCE POST v3 APPROVE, SHA256
`ff52665ea575e2592a70b982c4f9fc0a745b7a4793176f43daa3bc6c3c468f97`.
Exec session6871/Cargo PID17972 is terminal PASS; packet `omp-upgrade-chat-runtime-root-v3`.
The57 HTTP and3 Host successes have independent unchanged-source reuse bridges;
identity provenance addendum8d90473c records its historical command/image.
No full-target rerun is scheduled for those60 unchanged successful functions.
New CAS calls: **0**.

## Corrections before compilation and limits

An initially proposed retry side channel was rejected after exact source
inspection: `empty-completion-retry.ts:157` reconstructs
`FinalizedProviderStreamError` from message/status, while `error/retryable.ts`
and `packages/utils/src/fetch-retry.ts:515-519` own local text-based retries.
The body probe's transient flag therefore does not itself authorize another
local POST. Root and independent reviewer corrected their initial layer
inference before adoption; shared retry and other provider constructors stay
outside the delivered delta.

The existing Host slow-callback assertion explicitly expected the old Chat
timeout. Its new-source migration keeps callback settlement and exact POST
count assertions and adds successful output checks. It is a versioned
behavior change, not a discarded regression.

Non-JSON SSE throttle text/page handling, final tool arguments, remaining
custom-wire requirements, full CLI error-journal/restart trials, real model
tasks and login remain OPEN. Two scalar fidelity boundaries are explicitly
unaccepted: UTF-16 detail truncation through an astral pair and preservation
of an existing `HTTP/1.1` prefix in synthesized status messages. The candidate
cannot stand for complete upgraded in-band family or full Chat acceptance.
