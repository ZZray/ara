# Production RPC native Session adoption (WIP)

Fixed OMP: `596f2da7101178214aa27a753529d15e6b7ad91d`. This adds bounded
`new_session` / `switch_session` behavior to the twelve-command entry slice.
The complete denominator remains 42 commands. No whole CA-RPC, A5, P2 or
P1-P6 acceptance, deployment or full OMP marker advancement is recorded.

## Source and scope

- `rpc-mode.ts:541-565,1180-1185`: optional truthy parent, cancelled response,
  and command metadata refresh before successful adoption response.
- `agent-session.ts:7599-7603,7611,7635-7678`: abort before new, fresh provider,
  queues cleared with modes retained, new tool state and current context refresh.
- `session-manager.ts:1108-1127,1484-1488,1687-1691`: fresh ID/header and parent
  copied verbatim; explicit persistent new materializes before returning.
- `agent-session.ts:8740-8763,8832-8852,8879-8915,8963-8984,9081`: join/settle
  old work, same-cwd RPC cancellation boundary, provider reuse/reset and prompt
  refresh. RPC supplies no `onCwdChange`; different recorded cwd returns
  successful `{cancelled:true}`, even when that directory is inaccessible.
- `session/messages.ts:131-144,194-246`: compare provider replay values instead
  of runtime-only timestamps/usage. Responses thinking/native payload handling
  follows these source projections for the four supported Rust message roles.
- Default `--no-session` uses `MemorySessionStorage`; fixed CLI cannot switch
  to a native disk Session (main.ts:964-965, session-manager.ts:3103-3108,
  session-storage.ts:665-675,683-685,715-716,741-744). This is verified source
  inference, not an executed upstream memory-storage test.

Existing CLI discovery, tool/MCP construction, prompt assembly and provider
construction are moved into shared helpers. Initial print/REPL use those same
helpers. New/different/changed Session preparation resets tool context and
provider-native state; unchanged same-file, same-ID replay preserves tools,
provider and reminder state while refreshing the prompt. Native Session IDs
are also distinct under `--no-session`.

Every old Run is cancelled and joined before replacement or target file open,
including a lazy current journal. Its immutable sink records into its original
journal until settlement. Candidate setup precedes recovery writes. The Host
adopts Agent/config/journal/mirror only after preparation succeeds; failed or
cancelled adoption preserves the previous identity, transcript and queued input.
Successful adoption clears queues but transfers steering/follow-up modes.
An explicit persistent new records its model/header and materializes before ACK.

### Remaining boundaries

- Only the launch-selected model route is currently available. An unavailable
  target model retains the launch route and its recorded selection is not
  overwritten. Full role/default/fallback catalogue selection, thinking and
  other saved settings remain open.
- Native custom/Skill/compaction public messages still use existing provider
  projection; complete native public query parity remains open.
- Same-file unchanged tool retention does not reconcile newly discovered Skill
  URLs into the existing tool context. Full runtime capability reconciliation,
  extension/subagent veto/reset, persistent queue admission, branch and all
  other RPC contracts remain open.
- Rebuilt MCP connections retain the existing exact grants and cwd. Existing
  connection-drop cleanup is reused; this does not prove process-tree cleanup
  or full upstream MCP lifecycle parity.
- No live product installation is changed. Full delivery is still WIP.

## Executed verification

`cargo check -p ara-cli --all-targets --all-features` passes after implementation
(0.93 s final check). Final focused/backend/model/audit receipts are recorded
below when complete; compilation alone is not acceptance.

### Actual child-pipe suite

`cargo test -p ara-cli --test rpc_host --all-features -- --test-threads=1`:
**21 passed, 0 failed, 0 ignored**; compile **3.91 s**, execution **5.15 s**,
total **9.259 s**. Eight new adoption scenarios cover:

1. Active real Bash cancellation/settlement, original-journal receipt binding,
   parent header and explicit new materialization, queue clear/mode retention.
2. Changed-cwd cancellation with usable original Session, and new Session
   refresh of same-cwd SYSTEM/AGENTS rather than adopting the foreign context.
3. Responses new/different/changed-conversation/different-ID cold state,
   unchanged and metadata-only same-ID reload warm state; lexical cwd alias
   normalization accepts `nonexistent-component/..` without filesystem lookup.
4. Missing/corrupt native destinations preserve the original identity,
   transcript and file; changed missing cwd cancels with the original usable.
5. Memory-only new creates distinct IDs and no journal; disk switch fails
   explicitly without loading or modifying the target.
6. Runtime unknown-effect recovery after an actual prior write: synthetic
   receipt, no automatic replay, refreshed current context and correct history.
7. Active current-file switch before first assistant: old Run joins and its
   previously lazy journal materializes before open/adoption.
8. Real MCP candidate setup failure after removing only its launcher script:
   new/switch fail without metadata or journal mutation; original live tool
   connection subsequently executes a real marker write with a paired receipt.

Raw final focused log/result:
`C:\Temp\ara-rpc-session-adoption-validation\rpc-host-windows-verified.log`,
`rpc-host-windows-verified-result.json`. The final backend gate below also
exercises the two later allocation-only test assertions fixed for strict Clippy.

Diagnostic failures are retained: the initial suite ran before the command
metadata fix (14/20, 101.076 s); the next diagnostic was 19/21 (10.085 s).
The MCP fixture initially used a non-absolute executable despite the existing
MCP contract; its correction passes in 0.867 s. Responses Host reuse was true
and controls unchanged, but the fake completed output omitted `annotations`.
Fixed `openai-responses-wire.ts:4276-4296` requires that array; native history
preserved the absent field whereas cold canonical history included `[]`, so
safe chain-prefix comparison declined the delta. Completing the fixture
preserved the exact warm-state assertions and passed (2.532 s). Production
provider behavior was unchanged; temporary diagnostic logging was removed.

### Delivered-code backend and policies

- `python scripts/verify_backend.py`: formatting, strict workspace all-target/
  all-feature Clippy, all-target tests and documentation tests pass. **1,120
  passed, 0 failed, 1 ignored, 86 suites**, CLI e2e **87/87**, **71.299 s**.
- `cargo deny check`: advisories/bans/licenses/sources all pass, **7.623 s**;
  existing license/version warnings remain. The first full-gate attempt stopped
  at Clippy after 4.237 s for two test-only cloned slice comparisons; these were
  corrected before the final gate, with no production change.
- Bootstrap/links/Skills/marker, inventory consistency and scoped diff checks
  pass. The full OMP marker stays unchanged.

Logs/results and six-file source snapshot:
`C:\Temp\ara-rpc-host-session-validation\backend-windows.log`,
`backend-windows-result.json`, `cargo-deny.log`, `cargo-deny-result.json`,
`source-snapshot.json`. The final test file hash is
`a030b108e20ee4800d873673e2203cbc26c1fff070bcdb619fa46d38c302d987`.
Original, isolated trial and final full-gate executable hashes all match:
`bba61c5769eaa1f2540b30626779b566c04a40fb7245cc0fd6751a7eb3fb3e12`.
Exact pushed-commit Linux verification remains pending for this Session slice.

### Bounded actual-model task

Live catalogue at **2026-09-30 05:29:29 UTC**: authenticated HTTP 200,
59 models, exact `deepseek-v4.1-flash` supports OpenAI/Anthropic endpoints.
Trial route is **B.AI `https://api.b.ai/v1`, OpenAI Chat Completions**;
credential bytes remain only in `BAI_API_KEY`, outside retained artifacts.

Driver: `C:\Temp\ara-rpc-host-session-validation\real_rpc_adoption_trial.py`,
SHA-256 `89a1771aa368d9fd7f97d9ee34948010ace1ae02603d88ea54fca9b74311f39c`.
Receipts: `C:\Temp\ara-rpc-host-session-validation\real-adoption-2altco6s\summary.json`.
Bounds: three Runs, three calls/60 s per Run, nine calls total, 1,024 output
tokens per call, 180 s process guard and 4 MiB per captured stream.

**PASS: 19.341 s, seven model calls (3+2+2), four successful paired tool
receipts, 274 full message-update events.** Session A reads an unpredictable
fixture and writes exact nonce/sum/sorted JSON. Explicit new materializes B
before any assistant, keeps queue modes and clears messages/queues; B writes
an unrelated marker. The fixture is removed before switching back to A. The
last Run uses only `write`, reproducing A's exact artifact from restored history.
A/B journals contain 10/4 messages and 3/1 tool results, with distinct IDs
and no cross-Session receipt contamination. All three Runs expose live streaming
state before completion, ACK precedes start, both adoption responses succeed,
EOF emits one shutdown and exit code is 0 with empty stderr. Reported usage
totals **18,544 tokens**; cost remains unknown.

This task proves real native Session switching and historical recall. Controlled
process tests separately prove Responses native-state rules, concurrency,
setup failures and unknown effects. It does not prove full RPC parity or
automatic Task acceptance. Final independent receipt audit is recorded below.

## Final independent audit, 2026-09-30

Independent Codex `rpc_entry_plan_review` approved the implementation plan and
reviewed **14/14 changed paths, zero skipped, no unresolved actionable defect**.
Four source-review corrections were closed: successful metadata-before-response,
same-path native ID isolation, preservation of the non-RPC memory-only header,
and lexical cwd comparison. It independently confirmed the controlled Responses
fixture correction from the fixed required wire schema, with no provider change.

The final audit recomputed the full backend/CLI counts and all dependency
policies, checked the allocation-only test corrections, rehashed all fifteen
model artifacts and six tested source/dependency paths, and matched original,
isolated and final executables. It compared all 356 raw stdout frames with the
observations, verified ACK/adoption metadata order, live queries, A/B journal
ownership, pre-assistant B materialization/parent/modes, exact artifacts and
source-deletion recall, all seven usage records and one EOF shutdown. No Cargo
or optional model task was repeated for this audit. The actual-task lane also
retained its raw receipt audit in `receipt-audit.json`; the parent independently
rehashed the fifteen artifacts and matched tested files with the staged snapshot.

**Decision: APPROVE as tested WIP.** Exact pushed-commit Linux remains pending;
full RPC, P1-P6, saved settings and capability reconciliation remain open.
