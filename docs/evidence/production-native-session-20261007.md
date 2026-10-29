# Production Native Session checkpoint — 2026-10-07

**Limited WIP; complete OMP acceptance remains 0/111.** Fixed upstream:
`596f2da7101178214aa27a753529d15e6b7ad91d`. Live base is `dev` at
`20aa0e2ef9872f633ba2671376cdf778477e7115`, with existing dirty work preserved.

## Latest final snapshot: v23 — 2026-10-08

The original affected CLI/RPC gate now passes on the final uninstrumented
source: CLI lib **240/240**, CLI bin **81/81**, RPC **61/61**. The complete
repeated-Read and unmatched/invalid startup-selector cases both pass with
unchanged original assertions and WAIT. This closes these two observed
integration failures; it accepts no complete OMP surface.

| Final source path | Change and boundary |
| --- | --- |
| `crates/ara-cli/src/rpc_host.rs` | Creation/reopen, post-publication history and statistics use the owning Session projector and existing public stringify/redaction. Native canonical bodies and post-publication fail-stop remain intact. |
| `crates/ara-cli/src/model_registry.rs` | At only the builtin Manager result boundary, preserve matching-provider built models or change only the provider, then use the existing safe wrapper. All temporary builtin diagnostics are removed; worker timeout, error, join and discovery remain. |
| `crates/ara-cli/src/static_model_registry.rs` | A narrow built-model wrapper reuses opaque-header/safe-metadata handling. Raw ModelSpec construction and its other consumers remain unchanged. |

Fixed OMP registry `model-registry.ts:1936-1947` consumes already-built Manager
Models without rebuilding. Rust had called the raw builder again, rerunning
policy and treating resolved compat as authored compat. Manager raw/custom,
cache, dynamic and collapsed-row builders remain required and unchanged.
Source review and the repaired final process case support this correction;
no isolated performance benchmark or sole-cause timing claim is made.

v22 had first repaired the complete Read case (RPC 60/61) but still failed
startup, in 489.724s with 313s compilation. Its first builtin batch completes
33 managers and 2,895 models; the second is cut off during conversion. One
same-binary isolated original startup case fails in 16.006s, with the last
observed conversion moving from index 10 / 545 models to index 13 / 974 models.
This rules out a fixed index 10 lockup, not every possible slow path. Neither
instrumented attempt is accepted as the final delivered runtime.

v23 applies the two-path reviewed built-model correction and diagnostic
removal on v22.after. Scoped fmt/check and the original combined CLI lib/bin/
RPC command pass, exit0, **454.564s** (454.989s including apply/fmt), with
**294s compilation**. All 751 delivered source identities remain unchanged
across the gate; 749 outside-scope source paths and current index bytes are
preserved. Unchanged Session/Journal/reduction use v21 evidence; unchanged
Agent/Tools/handoff use v20, and Responses/MCP/ValueHeap use their recorded
unchanged-source evidence. Workspace Clippy/dependency/platform/release gates
were not repeated here and are not claimed complete.

The final frozen `ara.exe` SHA256 is
`142395d341408abbdaa0dc769c3f4eee8aa642fecc984061f7de1fd1ebc468c4`.
Packets under `C:/Temp/ara-model-fallback-batch/`:
`root-production-closure-candidate-v23/`, with gate, pins, final source, original
module log and frozen binary. Exact final command: `cargo test -p ara-cli --all-features --locked --lib --bin ara --test rpc_host --no-fail-fast`, with `CARGO_BUILD_JOBS=2`, `RUST_TEST_THREADS=2` and target cache `D:/Temp/ara-build/ara-github-target-20261005`. SOURCE PRE:
`root-startup-built-manager-wrap-independent-pre-v1/REVIEW.md`, SHA256
`1b61b1ace4f902925ccd43763c2febf3b2fcf633cc0ecb41eccc914e435dc44a`.
Final independent POST: Codex `/root/native_realm_pre`: approve the selected final-source gate and two bounded CAS tasks; no full surface acceptance. Report `C:/Temp/ara-model-fallback-batch/root-production-closure-independent-post-v23/REVIEW.md`, SHA256 `0ef175641fb81a25a3f1bc0f78a748ad0b4f3590f70fdae33e5aa821514b71b9`.

The preferred Manager local gateway remains unavailable. Fresh catalogue
checks confirm Manager's configured CAS route supplies `deepseek-v4.1-flash`;
the existing two bounded tasks run on that route with the same final binary.
Task1 reads the seeded CSV, edits summary.json and reads it back: **6.447s,
4 model calls, 4 tools**, artifact totals 11/34.5/12.75/58.25. Task2 starts a
new process, reopens the same original Session and correctly recalls three
original source values with tools disabled: **2.186s, 1 model call, 0 tools**.
Both terminate normally. Root independently checks CSV arithmetic, edit/read
receipts, raw final answers, journal ID and both stderr Session paths. This
is a bounded actual task and restart observation, not account login, full
Chat compatibility, complete Session/Tools/RPC, platform or product acceptance.

CAS packet: `runtime-integration-cas-receipts/20261007T183808Z/`, receipt SHA256
`1de90936dd5f7367f63e1f20f18ae9ca2344410d81ca4aacbb46ec84cf4e3e20`.
One Session `01a117a8-a81a-7174-b615-fd081e26c57b`; binary/source hashes match
the final gate. Credentials are supplied only through an isolated environment;
no user configuration or installation is modified.

An earlier raw-index hash drift is not silently restored: independent
`--no-optional-locks` mode/OID/stage/path comparison finds all 985 index rows
identical to HEAD, with no staged diff. Current raw bytes
`89d34e26be97c90a14680d44ace937e6b1ae5a86e8ee7b46dd6b90718a922475`
are preserved throughout v22/v23. This proves current semantic equality, not
what caused the earlier byte drift. All unrelated WIP remains preserved.

Formal totals stay **0 complete / 1 tested / 27 implementing / 83 open**,
111 surfaces; full marker null. Actual OpenAI account authorization/task and
all remaining original contracts stay open. The next module work uses an
upstream-behavior-to-current-source gap map before completing one full module;
EDIT has 92 recorded test behaviors, WRITE 204, Chat 455. These are source scope,
not current coverage percentages. Other provider compatibility remains later
required work. No commit, push, deployment, installation or durable knowledge/
memory promotion occurs in this checkpoint.

## Earlier failed batch: v21 — 2026-10-08

Three paths are applied: Live toolResult reduction observes only its consumed
source/image detail slots; two registry paths temporarily record fixed
completion stages using nonpanic stderr writes. Original Graph descriptors,
MCP metadata, COW adoption, startup WAIT and test assertions are preserved.
Scoped fmt/check pass; 748 out-of-scope source paths and index are preserved.
The existing CLI/Session lib/bin/RPC/local_reduction/remote_compaction/Journal
modules ran together; unchanged Agent/Tools/handoff reuse v20 evidence.
Independent SOURCE PRE approves the composition, report
`root-read-and-startup-diagnostic-composition-independent-pre-v1/REVIEW.md`,
SHA256 `c6719cab983c456a51857287a256103e15300fb4bfeaaec759128826e59883a1`.
Packet: `root-production-closure-candidate-v21/` under the existing Temp root.
Gate exits 101: RPC remains **59/61**, while every other selected target passes
(CLI lib/bin 240/240, 81/81; CLI local/remote compaction 3/3 each; Session lib,
journal, local/remote compaction 6/6, 16/16, 3/3, 4/4). Total **601.430s**,
including **421s compilation**. The same two original RPC names still fail:

- Read now fails at rpc_host.rs:359 after the actual adapter Run ends, with
  `Session public projection failed after publication` and
  `LegacyBridge("Undefined, BigInt, Function, Symbol or opaque value")`.
  The prior 4376 Superseded and later fresh-store/file/request assertions did
  not execute. Do not claim the complete Read case is repaired or replay its
  published effects as if rolled back.
- Startup's first full refresh/join/engine publication completes. In its
  second full refresh, all three configured producers publish and the
  configured waiters return; no second complete builtin backend/waiter,
  aggregate join or publication exit appears before the original EOF timeout.
  This narrows the pending owner, not its internal root cause.

Independent limited POST retains overall FAILED and no repair acceptance:
`root-production-closure-independent-post-v21/REVIEW.md`, SHA256
`7ae24add7a0cba69b7593cbd285711c29d8d093b117e03cee83a0f233fde2d77`.
All 751 delivered sources/index match across the gate and at POST review.
After that review, Root exactly restores only the two registry paths from
v21/before, preserving 749 other source identities and index. The narrow
Session reduction change remains; `diagnostic-restoration/receipt.json`
records the new current source map. No gate has run after this restoration.
The frozen v21 binary remains explicitly instrumented evidence. No complete
surface or real-model acceptance is claimed.

Current read-only CAS preflight confirms `deepseek-v4.1-flash` through the CAS
route configured in Manager; the local Manager gateway is unavailable.
Receipt: `root-current-cas-preflight-20261008/receipt.json`. This is catalogue
and authorization readiness only. A prepared reuse of the existing two bounded
tasks requires a successful exact-source gate and frozen delivered binary.

## Earlier completed batch: v20 — 2026-10-08

Twelve paths are applied: actual handoff (5), reduction boundary/diagnostic
(2), Bash native settlement (3), its existing Windows observation (1), and
the isolated Ollama cache seed fixture (1). Scoped fmt/check pass. Independent
SOURCE reviews and limited runtime POST approve the scoped WIP. The prior
739 out-of-scope source files and saved Git index are preserved.

The existing Agent/Tools/CLI/Session libraries, CLI binary, handoff,
remote_compaction, local_reduction, Journal and complete RPC host targets
ran in one Cargo batch using the existing cache, build jobs 2 and test
threads 2. Cargo exits 101: RPC is 59/61; every other selected target passes.
Wall time is 667.496s (668.107s including apply/fmt), with 474s compilation.
Build jobs 2 has not demonstrated an acceleration. No real-model trial has
run on this composition. Packet:
`root-production-closure-candidate-v20/` under the existing Temp artifact root.
No new test entrance is added. One old Windows cancellation observation is
explicitly updated to require failed-plus-confirmed native settlement;
non-Windows and real timeout/Unknown checks remain required.

The original auto-classification/preprompt-compaction and new-session
cancellation/isolation RPC cases now pass. The latter executes finish(0)
and its subsequent original file, journal, Skill isolation and request-count
assertions. The two remaining failures are repeated Read (the old receipt
still contains `plain marker` instead of Superseded, rpc_host.rs:4376) and
startup EOF (channel timeout with empty stderr, rpc_host.rs:451).

Agent lib/handoff/local_reduction: 10/10, 4/4, 6/6. CLI lib/bin/
local_reduction/remote_compaction: 240/240, 81/81, 3/3, 3/3. Session lib/
handoff/journal/local_reduction/remote_compaction: 6/6, 3/3, 16/16, 3/3,
4/4. Tools lib: 37/37. These results are execution evidence, not OMP progress.
All 751 delivered source identities are unchanged across the gate; the
739 out-of-scope source paths and index remain preserved.
Independent Codex `/root/native_realm_pre` POST:
`root-production-closure-independent-post-v20/REVIEW.md`, SHA256
`117329e037b32338e34ecca35f811ac27bc6ede048e8ea35bfdb802944b4d529`.
Overall affected-module gate remains FAILED; complete Session/RPC is unaccepted.
Full OMP remains **0 complete / 1 tested / 27 implementing / 83 open**.

## Earlier completed batch: v19

Four applied paths repair the entry/body mapping, correct the CLI tool
declaration observation and remove the Host's premature native Run revoke:
`ara-session/src/native_message.rs`, `ara-session/src/reduction.rs`,
`ara-cli/src/main.rs` and `ara-cli/src/rpc_host.rs` under `crates/`.
Their two independent SOURCE approvals are retained; the abort review is
`root-native-rpc-abort-receipt-owner-independent-pre-v1/REVIEW.md`, SHA256
`9386cf0666d6a1f0d44a1c1249fc9d3791f7a5aa83b13add519c08b89b9dafda`.

Original affected modules run together: **CLI 81/81, CLI local_reduction 3/3,
Session local_reduction 3/3, Journal 16/16; RPC 57/61**.
All three v17 direct regressions now pass. Cargo remains 101 because four
original RPC names still fail. New-session cancellation passes its old-active
receipt/aborted/order assertions and fresh-prompt AgentEnd, then fails at
`child.finish(0)`: exit 2 reports `Session Bash jobs have unknown outcomes:
Host effect 1: outcome_unknown`. The later one-effect file assertion has not
executed. Do not claim this entire case or unknown-effect settlement passed.
The other failures remain preprompt compaction, repeated-read reduction and
startup EOF. No failing assertion was relaxed.

Packet: `root-production-reduction-regression-candidate-v19/`. Wall time is
**367.032s**, including **216s compilation**. All 750 delivered source hashes
match before/after; 746 outside-scope source paths and the saved Git index
are preserved. Scoped fmt/check pass; all 176 original CLI assertion-entry
lines including debug_assert remain byte-identical after formatting.
Independent Codex `/root/native_realm_pre` limited POST confirms the three
regression repairs and retains **full gate FAILED / complete RPC NOT ACCEPTED**:
`root-production-reduction-regression-independent-post-v19/REVIEW.md`, SHA256
`997ac3f2e0cab659efb5b78414d7ba3b2e45237068ba889d8674e1310607f115`.
The affected five targets total **160/164**; this is not product progress.
Complete OMP remains **0/111**.
Responses HTTP/MCP/ValueHeap results below are reused unchanged-scope v17
evidence; no v19 real-model trial has run.

Next-batch SOURCE candidates prepare the missing Native handoff side-request
entry and a narrow no-summary reduction boundary/field-kind diagnostic.
They are not applied, compiled or accepted here. Their execution must retain
the original complete RPC module and its unknown-effect failure boundary.

## Earlier executed batch: v17

The delivered thirteen-path combination executes the original modules once:
Responses HTTP 37/37, MCP stdio 17/17, Journal 16/16, ValueHeap 4/4;
CLI bin 80/81, CLI local_reduction 2/3, Session local_reduction 2/3 and RPC
57/61. **215/222 pass; seven fail; Cargo exits 101.** The two original
Responses tool/journal/restart and unchanged-reload RPC cases now pass.
This test ratio is not OMP completion. All 750 source identities match before
and after. Wall time is **792.430s**, including **644s compilation**.

The thirteen paths contain the normal-finish Responses baseline correction,
nine native receipt/reduction paths, upstream Array.prototype.indexOf and its
Host binding, and a positive response fixture correction. The fixture adds
upstream-required assistant role/completed status; original assertions remain.
Original logs and failed results are preserved in
`root-production-native-integration-candidate-v17/` under the existing Temp
artifact root. Independent Codex `/root/native_realm_pre` reports runtime
**NOT ACCEPTED** in `root-production-native-integration-independent-post-v17/REVIEW.md`,
SHA256 `18586d2e0522d326db588c99cee76bfa431f8792d2d688f6c0545ba0454f6ca8`.

Three direct regressions are identified: custom entry/body selection breaks
two persisted reduction families, and the CLI declaration helper's whole-root
export rejects private Symbol metadata. The three-path entry-aware/selected-Get
repair retains strict stale checks and every original assertion; it has SOURCE
approval in `root-native-reduction-entry-observation-independent-pre-v2/REVIEW.md`,
SHA256 `16b82cf89f65523214a42b0355a66753704b19cbfa7f715f188029557efccef2`.
Its application and successful affected regression execution are recorded
in v19 above; the failed v17 result remains unchanged.

Current original RPC failures are preprompt compaction, missing old active-tool
abort receipt, repeated-read reduction and unmatched-scope startup EOF.
The test-only v18 diagnostic preserves assertions and restores all source:
162.173s total, 16.814s case bodies. EOF-before-handoff produces only the
classifier request, a failed maintenance/no-headroom notice and prompt_result;
new_session still publishes no old-active toolResult; repeated-read notices
include an undecodable entry and a non-Wire field observation. These are
observations, not one inferred root cause. Startup's existing v16 await-stage
evidence is reused without rerunning its unchanged timeout.

Complete totals remain **0 complete / 1 tested / 27 implementing / 83 open**.
The next module batch must repair the actual failures, retaining the fixed OMP
target; OpenAI custom protocol and account login follow production Session.

## Earlier executed results

| Executed module / flow | Result | Boundary |
| --- | --- | --- |
| CLI bin tests, v9 | 81 passed, 0 failed | Selected existing tests; not complete CLI parity. |
| MCP stdio, v9 | 17 passed, 0 failed | Existing actual process fixtures. |
| RPC Host, v11 | 55 passed, 6 failed | Five v9 failing names now pass; no new failing names. This change alone does not isolate cause. Cargo exit 101. |
| Responses HTTP, v11 | 37 passed, 0 failed | Original HTTP suite; native RPC tool/reload assertions still fail. |
| Journal, v9 | 16 passed, 0 failed | Executed on the same delivered v9 source. |
| Real CAS file task, v7 | PASS; 4 model calls / 4 tool calls, 5.487s | Prior binary: read CSV and old JSON, edit, then actual readback. |
| Restart same Session, v7 | PASS; 1 model call / 0 tools, 2.081s | Prior binary: `--continue`, same data root; prompt contains no answer values. |

Real model: `deepseek-v4.1-flash`, protocol `openai-completions`. The Manager
local OMP endpoint returned `URLError`; the executed route is CAS configured
in ARA Manager. Credentials were supplied privately through the environment.
The CSV independently calculates pen 16.5, book 11.5, cup 4.25, total **32.25**;
the actual JSON and final tool readback agree. Restart recalls quantities 6/1
and book unit price 11.5 under the same Session ID.

The real task stderr also records an auto-compaction `LegacyBridge` failure.
Successful file effects and recall do **not** accept compaction, account login,
full RPC, complete OMP or a complete ARA product. No v9/v11 real-model trial has
been run; the v7 result does not prove later compaction is repaired.

## v11 delivered Responses batch and current blockers

Six paths join actual native Responses history/request/scaffolding, shared
ChainLease completion and POST transport: `openai_responses.rs`,
`openai_responses/actual.rs`, its `history.rs`, `request.rs`, new `chain.rs`,
and `responses_stream/actual/transport.rs`, all under `crates/ara-ai/src/`.
Original HTTP/RPC gates ran once on the delivered composition, total
**448.160s**, with **300s compilation**, HTTP **1.75s**, RPC **142.55s**.
The 750-entry before/after source maps match, preserving 744 other paths and
the saved index. Complete module acceptance is withheld: six RPC failures are
Responses tool artifact, unchanged-reload chain, preprompt compaction ordering,
new-session aborted-tool receipt, repeated-read reduction and startup EOF.

Packet `root-production-responses-candidate-v11/` has pins SHA256
`738cddd21f2963e913b76cfba6ffacd997365ac88f172d16fb22e974455f84d0`,
gate JSON `32ffae426b8e3fe439d9eef363845af92ed9c0b2ad9ab429ec123d5d366a9a70`,
and log `bf6637c37319ddde3c8a54547af8a05c7e9a4ca1ac8bd685abb8c7d501fcb586`.
Independent Codex `/root/native_realm_pre` confirms limited WIP in
`root-production-responses-independent-post-v11/REVIEW.md`, SHA256
`1b02d1f109a828127a077e36cb01f61aa18e6889230b1793fdcd3a22fbe8806f`.

Temporary v12 instruments the five original data/receipt failures without
changing assertions: total **23.929s**, test compilation **12.70s**, all five
failures reproduced. Actual notices confirm `raw entry has no original message
object` before auto-compaction, and checked Graph projection LegacyBridge on
repeated-read reduction. Cancelled old-tool journal lacks its result receipt;
Responses tool fails before execution; reload input loses the fixture's
assistant role. These observations constrain the repairs, not acceptance.
Temporary v13 identifies startup's unmatched `--models` scope case: at the
15s EOF timeout, `child.try_wait()` is `Ok(None)`, zero frames and empty stderr.
Total **29.353s**; the unreturned startup/cleanup await remains unlocated.
Both diagnostic receipts assert original bytes and all source restored.
Artifacts: `root-production-rpc-diagnostics-v12/` and
`root-startup-child-diagnostics-v13/`, under the Temp evidence root below.

## v9 delivered batch and remaining failures

The six-file batch repairs native custom-message envelopes and their
reopen/fork consumers, narrow budget observations and same-file replay
comparison, plus observed assistant/error-tool frames after receipt failure.
It retains the original canonical bodies and persistence rollback owner.

| Delivered file | Responsibility |
| --- | --- |
| `crates/ara-session/src/lib.rs` | Native custom staging, durable envelope and reopen/fork binding. |
| `crates/ara-session/src/native_message.rs` | Custom field mapping with exact UTF16 keys; v9 fixes the v8 WireString API mismatch. |
| `crates/ara-cli/src/actual_session.rs` | Production native journal encoding and source rebinding. |
| `crates/ara-cli/src/agent_message_projection.rs` | Selected budget observation, original-domain reload comparison and complete summary proof digest. |
| `crates/ara-cli/src/native_budget_observation.rs` | Private budget/replay consumer; no canonical transcript replacement. |
| `crates/ara-cli/src/rpc_host.rs` | Publish already observed assistant failure frames and refused-tool observations while retaining effect cancellation. |

Root read the unchanged original assertions and actual log. The ten newly
passing RPC cases include Skill receipts/queues and reopen/fork, same-file
reload, persistence-failure write refusal, active model/tool observations,
failed switch, provider recovery and stdout backpressure. The remaining eleven
are two Responses stateful cases, five catalogue/model-selection timeouts,
one startup EOF timeout, and one each for preprompt compaction ordering,
new-session aborted-tool receipt and read-result reduction. These groups are
observed failure boundaries; this record does not assign them one root cause.

Full custom normalization, Date/opaque domains, complete native tokenizer
domains and replay getter/exotic equality remain explicitly unaccepted.

## Identity and execution receipts

Artifacts are under `C:/Temp/ara-model-fallback-batch/`:

- Current delivered packet: `root-production-native-session-candidate-v9/`.
  Its gate compares 749 source identities before/after, all unchanged.
  `gate-results.json` SHA256:
  `d457e7bc6ab07f38e84127b25499ecb3fe5a1e1633d7be5325223efca6fce8ee`.
  `combined-modules.log` SHA256:
  `b042f758bc5126b8662eba826f66330b9cb1d02a697a68707f9b53a926b0fec7`.
  `failure-comparison.json` retains the ten resolved and eleven remaining names.
- Frozen v9 production binary: `root-production-native-session-candidate-v9/frozen-binary/ara.exe`,
  SHA256 `f4591e83c4d8459e729f7a254364cf871f2f0e658b4109867cd1fa3b84514078`.
- Independent SOURCE approval: `/root/native_realm_pre`,
  `root-production-native-session-independent-pre-v8/REVIEW.md` and
  `root-production-native-session-independent-pre-v9/REVIEW.md`.
  v9 independent limited WIP POST approval is in
  `root-production-native-session-independent-post-v9/REVIEW.md`, SHA256
  `06d2641475015ec1d415d949dc605eed41eaddface432b7af695826d667af1f6`.
  It confirms the six-file identity, original failure-set comparison and
  selected original assertions; the integration gate remains failed and v9
  real-model behavior was not reviewed. Root also checked the raw logs/source.
- Frozen candidate: `root-production-native-session-candidate-v7/` (26 paths).
  All 26 live hashes and the CAS source before/after map were checked unchanged.
- Binary SHA256: `01ecb37ce41e5ae727b307ce135b6b92b5758e71ca73d3fac523427db5ebe1a6`.
- CAS: `runtime-integration-cas-receipts/20261007T122410Z/receipt.json`, SHA256
  `52635ba366116459e61e4045c431ff0dfd58c36c26fdc74ef72308d5896a302d`.
- Independent reviewer `/root/native_realm_pre`: limited WIP POST approval in
  `root-production-native-session-independent-post-v7/REVIEW.md`, SHA256
  `ca78eb354813e2f2d3f75b6cdb562fdccd206353ab9bf29c24cbac00df60028b`.
  Root also read the actual logs, stderr, CSV/JSON, commands and receipt identities.

The v7 command was `cargo test -p ara-cli --all-features --locked --bin ara
--test rpc_host --test mcp_stdio --no-fail-fast`. v5/v6/v7 elapsed times total
657.877s (about 11 minutes); compiler summaries total about 521s. The v6 and
v7 failed-test name sets match exactly. Retained failure logs are not overwritten.
Workspace Clippy, full release gates and account-login tasks have not passed here.

The retained v8 attempt exited 101 in 16.082s before executing tests, because
`WireValue::insert` accepts `&str`, not the original `WireString` key. The
reviewed v9 correction uses exact Wire Object entries. v9 runs the original
CLI/RPC/MCP/Journal modules in one command, adding `-p ara-session --test journal`
to the v7 gate; it exits 101 after **511.651s**. Reported compilation is
**390s**, test bodies total **116.63s**. This batch introduces no new tests or
weakened assertions. The saved Git index SHA256 remains
`3879fed248a828447b744371bdcea1da2578792b938205d7466ad1ff809b6eb0`.

## Formal progress and next batch

Recounted `upstream/inventory/surfaces.toml`: **0 complete / 1 tested /
27 implementing / 83 open**, 111 total. Edit's Windows behavior evidence
remains [285/286, with Unix execution pending](edit-engine-parity-20261007.md).
The old bounded-point and V1 percentages are not complete OMP progress.
`ported_through_commit` remains null; no reliable complete-product ETA exists.

Next batch closes the actual Responses stateful owner using the existing
Host stateful/reload assertions. v1 was rejected for history return types and
premature baseline publication; v2 repaired those and exposed a direct Done /
next-turn active-slot race. v3 repairs publication/release under the shared
mutex and receives independent SOURCE approval in
`root-native-responses-stateful-independent-pre-v3/REVIEW.md`, SHA256
`9ae6ca3280b6cac25020dc7b3c60164d7d4e4e34cfe72ac9c8f95df7644f240a`.
The formatted six-file composition is applied as
`root-production-responses-candidate-v11/`; its completed existing module gate
and limited independent POST are recorded above. The next batch closes the
remaining six original failures. Use existing module tests in one compilation batch
and reuse unchanged evidence. OpenAI custom routes and account login precede
the remaining Provider compatibility work.
No original behavior or failing assertion is removed to increase the count.
