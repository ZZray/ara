# Hard tool directives foundation — bounded tested WIP

Fixed OMP: `596f2da7101178214aa27a753529d15e6b7ad91d`. This records the
four existing Rust choices and their deferred ownership. Full D remains open.

## Requirement, implementation and source

Native `packages/agent/src/agent.ts` and `agent-loop.ts` own dynamic hard
selection, active admission, same-Agent deferred claims, clear/reset and final
provider refusal. The approved PRE plan is
`C:/Temp/ara-model-fallback-batch/core-tool-directives-d-pre-plan/PLAN.md`
(SHA `f2f7ff7e4e174b02c433220e0936409f04ada549245aaacbfb7f9bcc7532601e`).

`ara-agent::{agent,agent_loop,tool_directives}` implements
`ToolChoiceSelection`, fallible selector/unavailable hooks and per-Run origin
ownership. Only Auto/None/Required/Tool are implemented in this foundation.
Select from admitted tools before request preparation. A closed, nonaborted
provider refusal retains only its own dynamic claim after the awaited final
RunReport; error, panic, late abort/deadline and served claims do not restore it.
Static fallback is not claimed. Deferred names are rechecked, and the filtered
Agent result is authoritative. Owned dialects retain logical selection while
suppressing the wire choice as before.

`clear_all_queues` also clears retained directives; individual queue clear and
`replace_idle_messages` preserve their earlier contracts. New `reset_idle`
uses existing state/transcript lock order and refuses Busy/NeedsRecovery and
unpaired effects before mutation. It is a guarded Rust adaptation of Native
reset. Host `RpcHooks` and `ToolLoopHooks` forward both selector methods.

## Exact SOURCE and application

- Core v2 pins: `3fe66f2289f6f522a40df6d6379b517791e1c7b4d411a17f65ff9c1aa21ebe47`.
- Core normalized diff: `50dfab26a6cac6de1ba2445fbf27b1c305add436eef8253f6f39681e19bc6fd9`.
- Host v2 pins: `890868757bcbe072edd861d28b493aa8fee7a12194ba2e6c3cb4a3046f7403f6`.
- Host diff: `31395418baf37da5198e30c6d2494c7c00cf10ec176611ab836760fb3b0891ee`.
- Independent Codex SOURCE reviewer `/root/broker_server_review`: APPROVE,
  report SHA `3481dcb6dcd5cb93241dc23c6ff43235d4b8b472909e8e6244e78948c0477cd0`.
- Exact seven-file application with before backups:
  `C:/Temp/ara-model-fallback-batch/d-foundation-joint-applied-20261004T204256Z/receipt.json`.
  Earlier CRLF Core and whole-file Host drafts remain preserved and were not applied.

## Concentrated execution

Root ran `python C:/Temp/ara-model-fallback-batch/verify-d-foundation.py` with
`CARGO_BUILD_JOBS=1`, `RUST_TEST_THREADS=2`. The runner reused existing targets
and compiled them once with all features/locked/no-run before direct execution.

| Existing group | Passed | Failed | Seconds |
| --- | ---: | ---: | ---: |
| core-hooks | 9 | 0 | 1.573 |
| core-owner | 9 | 0 | 0.073 |
| core-loop | 56 | 0 | 2.719 |
| core-live | 10 | 0 | 0.070 |
| host-hooks | 73 | 0 | 2.149 |
| repl | 95 | 0 | 21.966 |

Six D-related groups pass **252/0**. Compile **317.428s**; the original
seven-group invocation totals **349.781s**, **254/1**, with source unchanged.
Its separate Broker failure is retained and corrected by the affected-only
[Broker evidence](auth-broker-server.md). This is not a freshly green
seven-group invocation or a comprehensive gate.

The existing Core D-H aggregate observes actual provider CallOptions, real
controlled tool effects, catalog replacement, prompt/continue retention,
clear/reset, source/unavailable/preparation/rejection faults, sink unwind and
late cancellation. Actual internal Host gate aggregate continues on the same
Agent and Session, retains Required without a second source read and verifies
provider options. It does not expose or test a new external RPC directive command.

Receipt: `C:/Temp/ara-model-fallback-batch/d-foundation-receipts/module-20261004T204408Z/receipt.json`.
Receipt SHA: `bb9673ee41f256d9336cb2d34efd0d733f211dacb283269781af5ed48660aa79`.
Seven-file scoped rustfmt --check passes in **0.216s**, source unchanged:
`C:/Temp/ara-model-fallback-batch/d-foundation-fmt-receipts/20261004T204651Z/receipt.json`.
This does not claim workspace/vendor fmt passes; the original full fmt exit1 remains.

## Current main binary and bounded real-model task

Raw compile JSON confirms the non-harness debug `ara.exe`, built with the
`test-fixture` feature, SHA
`ec00620b7740d2cbcf6996304b1d34382c2b9bfd61a568fd99b68c73ffb63cd2`.
PE stack reserve is the normal **1,048,576 bytes**. Artifact identity is in the
same batch's `production-binary-pin.json`; it is not a final release package.

Root used the currently enabled CAS configuration from ARA Manager,
`deepseek-v4.1-flash`, `openai-completions`. The local OMP management route's
catalogue request returned URLError; the configured CAS route supplied the
model. No claim is made that the local Gateway passed. Credentials stayed in
child environment variables and local private configuration.

In four calls/7.856s the actual CLI read CSV, edited summary.json and read it
back: pen 2*2.75=5.5, book 1*11.5=11.5, cup 4*4.25=17, grand total **34**.
A restarted CLI continued Session `01a108ae-a7d1-76fd-8b9c-409ba9cd0084` and
returned the earlier quantities/price in one call/3.390s with zero tools and no
answers in the new prompt. Total **five calls/11.246s**. Both exits0, actual
model/protocol, terminal messages, per-call usage and journal pairing were checked.
Artifact hash and raw tool frames are in the receipt. This proves default-path
integration, not live selector-driven hard choices, account login or OS interaction.

CAS receipt:
`C:/Temp/ara-model-fallback-batch/runtime-integration-cas-receipts/20261004T205022Z/receipt.json`.
SHA `62e43db127c490dc9aad8b9216fd6b8777dad6a74b2bd5276f6f3cb29e0ff0a5`; source/binary unchanged during execution. Later v5 changed
only a Broker test accessor; the executed runtime and main binary remain identical.

## Independent execution acceptance and remaining work

Independent Codex `/root/broker_server_review` approved bounded SOURCE/EXEC,
actual compile artifacts, CSV/edit/readback, original-Session journal and recall:
`C:/Temp/ara-model-fallback-batch/core-directives-d-custom-cas-independent-bounded-exec-review.md`,
SHA `1c5596c2e0196dd368d6c572d8f384bff19bc32999b71b09629660db598a579a`.
Root independently checked relevant runtime hunks, original failed logs, current
binary stack/artifact identity, disk artifact arithmetic and exact recall answer.

Full eight-shape hard algebra/native marker/provider mapping, raw AgentMessage,
soft reminder/predicate/escalation, actual Harmony same-turn recovery,
pause/pause_turn/asides/thunks/before-yield, remaining telemetry/argument streams
and original debug startup15s remain required. No complete D/surface/module,
formal111/60/P1-P6 acceptance or full upstream marker is advanced. Final stable
workspace/dependency/platform/account/product gates remain open. No commit/push,
installation or deployment occurred in this batch.
