# ThinkingLoop and original-Session recovery — 2026-10-01

Status: bounded Windows checkpoint; complete AI-STREAM-GUARDS,
CA-TURN-RECOVERY, CA-RPC and P1–P6 remain open. Parent:
`e68c1df36438fd0202f2e77c871bc3c32c496a6c`. The final runner receipt pins the
delivered source snapshot. `ported_through_commit` remains null; RPC stays
27/42 bounded implementations. This batch adds recovery behavior to existing
commands rather than accepting additional command variants.

## Fixed source and implementation

OMP `596f2da7101178214aa27a753529d15e6b7ad91d`:

| Source | Rust owner / observable behavior |
| --- | --- |
| `packages/ai/src/utils/thinking-loop.ts` and its original tests | `ara-ai::thinking_loop`: exact UTF-16 suffix cycles, semantic paragraph/lexicon stalls, separate Gemini header detector, stream latches and one empty failed terminal. Child cancellation preserves the caller token. |
| `packages/ai/src/stream.ts:1048–1106,1753–1768` | Public completion helpers on a guarded provider: only empty ThinkingLoop errors receive three total attempts, 500/1000 ms waits; onAttempt observes every result and caller cancellation propagates. |
| `config/settings-schema.ts`, fixed catalogue compatibility/class facts | CLI native `model.loopGuard` switches, per-call overrides and resolved policy; defined `thinkingLoopGuard:false` still enables semantic detection as upstream requires. Guard metadata never enters the request body. |
| `session/turn-recovery.ts:2232–2253,2405–2429,2525–2552` and original retry tests | Same-route Session retry, native failure IDs, one fixed hidden redirect per retry, existing budget/abort/effect veto. Two looping attempts retain two notices; cancelling backoff retains its already-persisted notice. |
| `session/stream-guards.ts:186–199,243–299` | RPC Gemini-class consumer: warning at 36 headers, original Run cancellation/join, exact aborted assistant branch exclusion, fixed hidden `gemini-tool-call-reminder`, original Agent/Session continuation, generation and cancellation checks. |
| Fixed `thinking-loop-redirect.md` and `gemini-tool-call-reminder.md` | `ara-session::LoopGuardNotice`: exact trusted templates, native custom identity/agent attribution/display false, Developer model projection, source IDs through restart and compaction. Raw interruption receipts remain. |

ARA retains observed usage on the synthetic loop error and keeps absent usage
unknown; fixed OMP creates synthetic zero usage. This follows the existing Core
usage contract. Catalogue/settings/authentication remain Host responsibilities.
The existing tools, authentication, source switches and provider retry ownership
are preserved. Result helpers do not silently replace a caller's compaction or
streamed-tool ownership policy.

## Module and final verification

```powershell
python -X utf8 scripts/verify_recovery.py --module all --output C:/Temp/ara-recovery-batch --full
```

Use `--module ai|host|config|session` during development. The runner compiles
only selected module executables, then `--full` shares the backend,
inventory, dependency and CLI build gates. It reads no local API credentials.

Final stable receipt: `C:\Temp\ara-recovery-batch\module-20261001T082829Z\receipt.json`.
The four module suites pass **24/0/0**: AI 3, RPC 15, configuration 5, Session 1.
The final shared gate passes **1,499/0/20** (passed/failed/ignored), including
fmt, Clippy, all target/doc tests; inventory, cargo-deny and the CLI build pass.
The complete command takes **114.925 seconds**: selected compile 0.389,
four modules 7.520, backend 103.253, inventory 0.434, deny 2.721 and build 0.397.
Its 148 before/after/current source hashes match. The final CLI binary hash
equals the real CAS trial binary below, so that unchanged trial is reused.

The RPC module reuses the existing bounded real-child/socket/journal harness.
Its four added flows cover two-loop recovery → one actual Bash artifact →
two notices → original-Session restart/request projection; disabled guard,
backoff abort and exhaustion; Gemini reminder enabled/disabled → actual tool;
and cancellation before a withheld continuation response followed by an empty
new Session. Existing actual/unknown tool-effect veto and retry-budget flows
run in the same module. Session checks fixed provenance, branch exclusion,
compaction source IDs and rejection of forged reminder content.

First failures are retained, not counted as final passes:

- `081023Z`/`081128Z`: Host class-helper namespace/error conversion compilation
  dependencies; corrected before runtime tests.
- `081202Z` (module-only, not full backend): new abort test awaited an end event already consumed while reading
  the command acknowledgement; corrected to use the harness's retained frames.
- First full `082351Z`: the old pipe-pressure fixture emitted repeated `x`
  prose, which the new exact guard correctly stopped. Same-size numeric filler
  replaces it, with every original backpressure/effect/order assertion retained.
  The repaired existing test passes (`pressure-repaired.log`); guard behavior
  remains enabled. No timeout or product expectation was relaxed.

## Actual OMP test reuse

`thinking-oracle/receipt.json` records the existing verified Bun 1.4.0 runtime,
source/export/extraction/input/output hashes and commands. Unchanged original
detector/header test blocks run **19 passed, 0 failed, 33 expectations**;
the supplemental original-detector corpus passes **46/46**. Ten shared input
families match expanded Rust test literals exactly. Rust execution is the AI
module above; literal equality is not a second Rust run or a per-case reason
string comparison. The original stream/retry tests and complete OMP suite were
not executed by this extracted detector oracle.

## Bounded real CAS task and independent acceptance

Artifacts: `C:\Temp\ara-recovery-batch\live\20261001T082351Z`.
The script privately reads the current Manager `用户.ry_switch`, confirms the
enabled route and live catalogue, then uses Manager → OMP management → CAS
`deepseek-v4.1-flash`, OpenAI Chat Completions. A local relay holds the actual
credential; the Rust RPC child receives only a dummy relay key.

Bounds: two Runs, eight calls and 120 seconds per Run, 4,096 output tokens per
call, ten relay requests total. The first two responses deliberately inject
stream loops; subsequent responses come from actual CAS. Observed: seven
requests total, five real CAS requests, three completed read/write/read tools,
two native recovered-error IDs and two fixed notices.

The answer-free task computes invoice rows into `invoice-summary.json`:
marker 15, folder 25.5, lamp 34.5, grand total 75. Actual restart keeps Session
`01a0f690-0f75-75fb-af2e-c37cc8922683`; with tools disabled, recall correctly
returns original marker quantity 4, folder price 8.50 and total 75.0.
Task 10.154 seconds; restart/recall 1.184 seconds. `audit.json` pins raw events,
requests, artifact and the initial receipt; it records observed usage, with
synthetic loop usage unknown and cost unreported rather than assumed zero.

The initial live harness verdict was FAIL solely because decimal display strings
were compared literally (`8.50`/`75.0` versus `8.5`/`75`). Root verified the saved
raw results numerically against unchanged expected invoice values and wrote
separate **PASS** `audit.json`; the initial receipt is preserved and no paid task
was repeated. This is the native prompt's `<number>` contract, not a weakened
answer expectation. Binary SHA-256:
`73e1e745be966bd6c86e3d1fa546d8383f99478ec1a95fe2ef6f808bf3f5e1f9`.

Independent Codex `ctx_oracle_diff_review` reviewed the pre-plan, AI/route
diff and Host/Session/test-runner diff against fixed source. Receipts are
`ai-route-post-review.json` and `host-session-post-review.json`; no blocking
finding. Their final gate binding independently recounts the backend log and
checks all 148 source hashes, the repaired pressure fixture, original CSV,
numeric output, request/model sequence, tool receipts, original Session journal,
unknown usage and retained initial failures. Root repeats the source/export,
artifact, numeric, tool, journal and final-binary checks in
`C:\Temp\ara-recovery-batch\root-point-audit.json`. This accepts the bounded
Windows checkpoint as tested/audited WIP; it does not accept a complete parent
surface, platform matrix or fixed-OMP parity.

Final receipt SHA-256 values:

- Module/full gate: `868fd1fa86c2df7cd6c4bb380c6ea09ac098b9b89601378b2ee7108f6ae98cce`.
- AI/route review (final-source addendum retains the earlier segment hashes):
  `4f1056d1d88be343773b38e2597514ecc730093239afc9ccb98aad2ca619c1f1`.
- Host/Session review: `900b40d94fbedb3aa0e142972bf087633837d2ebcb36991161e39799c6f5544f`.
- Live numeric/artifact audit: `8033e2246b870209b1e05bd75495bee977f11426c32c7a32fba81a0ecd474c7f`.

Documentation bootstrap and `git diff --check` pass after the ledger/plan update.

Explicit evidence limits: controlled Gemini and loop faults are not a live
Gemini task or a spontaneous real runaway. The old-retry → header-pending →
EOF/generation race window and new notice journal-write failure are source
reviewed, without dedicated execution. Linux execution for this snapshot and
actual OpenAI account authorization remain separate checks. Other stream
healing/tool-loop guards, overflow/interrupted recovery, native special stream
caps, full registry/auth/fallback/usage and later Provider contracts remain
required work; this checkpoint accepts none of their parent surfaces.
