# Joined Edit Host families — 2026-10-08

**WIP; no full surface accepted.** Fixed OMP
`596f2da7101178214aa27a753529d15e6b7ad91d`; ARA base `dev`
`20aa0e2ef9872f633ba2671376cdf778477e7115`, with unrelated dirty work preserved.
Inventory independently recounted: 111 surfaces, 0 complete / 1 tested /
27 implementing / 83 open; full marker remains null.

## Applied scope

Three joined families cover mode/config/details/inspection; streaming Edit
pool/previews/generated-file and abort guards with Host/RPC wiring; and
ACP/plan/LSP writer ports. The combined SOURCE manifest names 21 paths;
24 paths were in the scoped formatting allowlist, including the earlier family.
The source admission was independently approved by Codex `/root/native_realm_pre`:
`C:/Temp/ara-model-fallback-batch/full-edit-joined-independent-admission-v1/REVIEW.md`.

These ports do not establish complete production ACP/editor/LSP services.
Blackbox/repair and AST are now applied in the [current 625/5 composition gate](edit-ast-blackbox-cancellation-20261008.md);
renderer, VCS join and remaining Host service installation remain required. The initial 92-row map's
63/16/13 gap counts predate this integration and are not a current completion count.

## Frozen original batch

Command: `cargo test -p ara-agent -p ara-tools -p ara-cli --all-features --locked --lib --bin ara --test agent_loop --test edit --test model_edit_snapshot --test edit_mode_policy --test rpc_host --no-fail-fast`.

Result: **507 passed / 3 failed, exit 101, 647.358 seconds**, including
482 seconds compilation. The 928-source snapshot stayed unchanged and Git
index bytes were preserved. Earlier formatting and compile failures remain
in the same packet; they are not discarded or reported as passed tests.

| Target | Passed | Failed |
| --- | ---: | ---: |
| Agent lib / loop | 11 / 56 | 0 |
| CLI lib | 242 | 1 |
| CLI bin | 81 | 0 |
| Edit mode policy | 4 | 0 |
| RPC Host | 60 | 1 |
| Tools lib | 37 | 0 |
| Edit tool | 13 | 1 |
| Model Edit snapshot | 3 | 0 |

Packet: `C:/Temp/ara-model-fallback-batch/full-edit-joined-execution-v5`.
The final receipt is `receipt-after-compile-repair.json`; the original log is
`original-module-batch-after-compile-repair.log`.

## Source-backed test corrections and affected reruns

Production Rust was not changed by these three corrections:

- Typed guard: fixed OMP `agent-loop.ts:3005-3034` sends synthetic start/end
  events for a never-executed aborted tool. Replace the incorrect no-start
  observation with a real Edit execute-entry count of zero, and exactly one
  error receipt with `__synthetic=true, executed=false`.
- RPC preview: compare the exact existing Responses composite call ID.
  The failed log already contains both live/final previews and the successful
  streamed-buffer Edit result; the new predicate waited for the wrong bare ID.
- ACP follow-up: the persisted-tag test's second edit must replace a valid body
  row, preserving the TypeScript declaration required by the original engine.
  The engine's parse guard and second real-edit success assertion are retained.

Independent Codex SOURCE PRE approved all 3 paths / 9 hunks, with no actionable
finding: `C:/Temp/ara-model-fallback-batch/full-edit-joined-test-repair-v1/PRE.md`,
SHA256 `33a97a2be2dbb5c8c96c566d6fcb205fad2ececc1563e7304e1db68326c7e474`.
Root read the report and directly checked the fixed source and failed outputs.

Affected reruns use the existing target cache, jobs 2 / test threads 2.
Scoped fmt passed in 0.192s. Typed guard passed 1/1 in 80.076s (body 0.08s);
ACP writer passed 1/1 in 70.154s (body 0.15s). Native RPC preview passed its final
v5 rerun 1/1 in 15.366s (body 5.67s), with scoped fmt 0.191s, source unchanged
and index preserved. Preserve the original failed batch and use the delivered
formatted source hashes for POST; SOURCE approval is not runtime acceptance.

The final RPC fixture retains only completed calls. An unfinished generated
call has no synthetic placeholder; the completed invalid call has one
`__synthetic=true, executed=false` placeholder. Journal size is two: one real
Edit result plus that invalid placeholder. Responses IDs are composite IDs.
The paused final-preview fixture now waits after `output_item.done`, which
emits ToolcallEnd, and before `response.completed`; `arguments.done` is not End.
Independent Codex SOURCE/affected-group review approved this exact v5 correction:
`C:/Temp/ara-model-fallback-batch/full-edit-rpc-synthetic-repair-execution-v5/SOURCE-PRE.md`,
SHA256 `010b0b2ea92ee50d62b6056af64d498a4db9669c44df98f70b9075f5d3dc8ebc`.

Final RPC packet: `C:/Temp/ara-model-fallback-batch/full-edit-rpc-synthetic-repair-execution-v5`.
The 507 original passes and three separate affected-group passes are distinct
receipts, not a single full batch with 510 passes.

Rerun packet: `C:/Temp/ara-model-fallback-batch/full-edit-joined-test-repair-execution-v1`.

## Retained initial same-binary actual CAS Host trial: failed

The allocation repair and subsequent same-binary CAS edit/readback and
original-Session recall now pass; see [current repair evidence](repl-allocation-repair-20261008.md).
The failed initial run below remains retained. The original REPL interruption
journal gate still fails; complete EDIT and lifecycle acceptance remain open.

Executed `python -X utf8 C:/Temp/ara-model-fallback-batch/verify-joined-edit-host-cas-v1.py`.
The script verifies delivered source/index receipts and binary identity, checks
the current Manager-configured CAS catalogue for `deepseek-v4.1-flash`, and uses
the `openai-completions` route with an isolated temporary profile/workspace.
Bounds: two Runs, at most 6/2 model calls, 120 seconds per Run, 2048 output tokens.
Requested outcomes: REPL edits and reads back `summary.json`; a new process then
recalls original CSV facts in the same Session with tools disabled.

Binary SHA256: `f332ed28fe606d5c431d84e3bd8261b71b537756f779e007de9bba77e3c2748c`.
First actual task failed after 6.851s, exit `3221225725` (`0xC00000FD`). Stderr
reports main-thread stack overflow. Raw stdout contains the actual model stream
and two Read ToolcallEnd events for `sales.csv` and `summary.json`; no assistant
message-end, tool execution-start/end or Agent terminal event was emitted.
The initial artifact was not corrected; the dependent recall was not executed.

The script's `modelCalls=0` and `usage=[]` count completed assistant messages
only. They do not mean no provider request or zero usage; live usage is unknown.
Source and binary remained unchanged. At this initial execution, the failed
actual Host trial prevented joined-family delivery acceptance even though the
deterministic groups passed. Its unknown usage is not overwritten by later
successful trials.

Packet: `C:/Temp/ara-model-fallback-batch/full-edit-host-cas-receipts/20261007T212645Z`.
Subsequent independent POST accepts the narrow repair and exercised families;
the complete lifecycle gate remains open. This receipt does not claim workspace strict Clippy, release,
complete Edit, actual OpenAI account, or product acceptance.

## Independent engine closure audit

An independent read-only audit found 167 engine/diff vendor files and 11 recorded
dependency sources identical to existing Windows evidence. The lock drift is
only an unrelated MCP dependency edge; no new engine implementation gap was found.
The 286-row denominator still includes one unexecuted Unix symlink case.
Complete engine acceptance needs that same-source Unix execution and applicable
engine/dependency gates; other surface and release gates keep their own scope.
Local WSL is unavailable; the checked existing development host has no Rust
compiler. No environment installation was made.

Report: `C:/Temp/ara-model-fallback-batch/full-surface-closure-audit-v1/REVIEW.md`,
SHA256 `6e586510599e9560df7ddb3a516440190c5282a9a32fe9da043f0407e43f53bd`.

No commit, push, installation, deployment or durable memory promotion occurred.
