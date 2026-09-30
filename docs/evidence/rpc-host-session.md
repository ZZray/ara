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
Exact commit `840d2daa19678710fa0b52c79ac04cfb12f2804b` Linux repository
run `36674773925` failed: RPC **20/21**, with the remaining backend suites
not reached. The last frames show B1 `agent_end`, B2 success ACK, then B2
`Agent is already running` failure; B2 never starts. This is a production
terminal/Run-settlement race, separate from the corrected Responses fixture.
Raw log: `C:\Temp\ara-rpc-host-session-validation\github\logs-36674773925\0_verify.txt`,
lines 1600-1617. Windows and the prior actual-task receipts remain valid for
their snapshots; they do not close this Linux failure.

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

**Decision at the reviewed snapshot: APPROVE as tested WIP.** The subsequent
exact-commit Linux failure above keeps this slice open. Full RPC, P1-P6, saved
settings and capability reconciliation remain open.

## Terminal settlement correction, 2026-09-30

Fixed `agent-session.ts:750-755,836-842,2361-2371` explicitly holds the external
`agent_end` until in-flight prompts unwind, because clients immediately resume
on that event. The Rust RPC sink now retains only its terminal frame; the
serial Host publishes it after joining the owned task and removing its active
slot. Other live events and awaited journal barriers retain their schedule.
Normal completion, abort and EOF use the same completion path. The original
terminal-before-correlated-error order remains intact; a task without a terminal
does not acquire a fabricated success event. Shared Core/provider/print/REPL
behavior and all twenty-one existing process oracles are unchanged.

Independent Codex `rpc_entry_plan_review` approved this narrow plan before
implementation. The final deterministic oracle holds the real Agent inside its
terminal sink callback while its running guard/transcript lock remain owned.
Before the explicit gate releases, no terminal is exposed; after join/slot
removal, exactly one terminal appears and an immediate next prompt enters the
real Agent. Its zero-call budget avoids network access and also proves the
original terminal-before-correlated-error ordering.

Final local receipts in `C:\Temp\ara-rpc-host-session-validation`:

- `terminal-focused.log` / `-result.json`: **1/1**, **8.327 s** including compile.
- `terminal-mutation.log` / `-result.json`: restoring premature sink emission
  fails the no-terminal assertion, exit **101**, **7.438 s**. Original and
  restored source hashes match; no mutated source is delivered.
- `terminal-rpc-process.log`: default-feature process tests **20/20**, **7.355 s**
  including compile. Full-gate all-feature RPC is **21/21**, **1.21 s** execution;
  the additional test exercises the MCP launcher fixture.
- `terminal-backend-windows.log` / `-result.json`: formatting, strict all-target/
  all-feature Clippy, target tests and doc tests pass, **1,121/0/1 across 86
  suites**, e2e **87/87**, **74.616 s**.
- `terminal-cargo-deny.log`: advisories/bans/licenses/sources all pass,
  **2.791 s**, existing warnings unchanged.
- `terminal-bootstrap.log` and `terminal-inventory.log`: both pass.

`terminal-source-snapshot.json` pins the tested seven paths. Final RPC source
SHA-256 is `31da85dab1a66d0dd0beb0751ed780c93d283bd92905d3f57ef4e9815c55d42b`;
EXE is `0cd88282995252590f98170e76db4f5ad7938aa63306d71585470bcbcae72071`.
The existing twenty-one process tests, dependency bytes, main and Session source
remain unchanged. An initial test compile stopped because two independently
written test modules shared a name; the weaker duplicate was removed before
all final receipts above.

### Repaired executable actual task

The parameterized `real_rpc_adoption_trial_terminal.py` changes only executable/
expected-hash arguments; original driver and all task/limit assertions remain.
Driver SHA-256: `8abf0d22e28c5986e75984276109b4a949d65b39a8941a822ff56e5f883b4a70`.
Live route catalogue at **06:55:51 UTC** confirms 59 models and exact
`deepseek-v4.1-flash`, OpenAI Chat Completions. Authorization remains environment
only, cost unknown. Limits retain three Runs/nine calls, three calls/60 s per
Run, 1,024 output tokens per call, 180 s process guard and 4 MiB stream caps.

Final EXE task **PASS: 18.344 s, 3 Runs, 7 calls (3+2+2), 4 successful receipts,
231 message updates, exit 0**. Proof/independent B marker/source-deletion A recall
all match. A/B IDs remain distinct, journals retain 10/4 messages and 3/1 results.
Reported usage totals **18,386 tokens**; cost remains unknown. All fifteen
original artifact hashes match the final summary's snapshot.
Receipts: `C:\Temp\ara-rpc-host-session-validation\real-adoption-lfrn4sqb\summary.json`;
raw stdout, journal and original artifacts remain in that directory.

Independent actual-task lane `rpc_terminal_real` audited all fifteen hashes,
313 raw frames, ACK/live-state/metadata order, journal isolation, exact deleted-
source recall, seven usage records, final idle/empty queues and one EOF shutdown.
`receipt-audit.json` records PASS; stderr is empty and credential bytes are absent.
Its summary/audit hashes are respectively
`c5f0c01e72c385a168f5246b16da18812eed043c557b58149b6b5fe51b5c2ea3` and
`dcb8d81acc4af489313a6513d7dc59bfef802a54a7a18bf29b7570b0bfbd2c69`.
Independent Codex `rpc_terminal_review` approved the final code (4/4 paths,
zero skipped/actionable defects), read the focused and caught-mutation logs,
and independently recomputed the full gate, CLI/RPC counts and final source/EXE
hashes. `terminal-staged-snapshot.json` matches all six tested source/dependency
paths with the index; unchanged Cargo.toml alone has existing CRLF-to-LF
normalization. The changed Rust file matches exactly.

**Correction decision: APPROVE as tested WIP, pending accurate repaired-commit
Linux verification.** No command or phase acceptance is advanced.
