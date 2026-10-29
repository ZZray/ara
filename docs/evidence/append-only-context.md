# Append-only context — bounded module-tested WIP

Checkpoint: 2026-10-04 17:23 UTC. Fixed OMP source:
`596f2da7101178214aa27a753529d15e6b7ad91d`. This records the applied
Core/cache/normalizer/intent, inline catalogue/prompt and Host policy/lifecycle
batch. It does not accept full Agent, Provider, Settings, SDK or ARA surfaces.

## Requirement and source mapping

| Observable contract | Fixed source | Applied Rust consumer |
| --- | --- | --- |
| Stable prefix, reset/rewrite/cache and isolated side projection | `packages/agent/src/append-only-context.ts`, `agent-loop.ts` | `ara-agent::{append_only_context,provider_context,agent_loop,agent}` |
| Tool metadata normalization and intent before final Assistant persistence | `packages/agent/src/agent-loop.ts`, `types.ts`; `packages/ai/src/types.ts` | `ara-agent::{provider_context,tool_intent,tool,event}`, `ara-ai::{types,tool_schema}` |
| Native schema display, complete inline tool catalogue and prompt selection | `packages/coding-agent/src/system-prompt.ts`, catalog inventory/TypeScript helpers and fixed tests | `ara-agent::tool_inventory`, `ara-context::SystemPromptOptions` and prompt builder |
| Auto/on/off, sparse authored donor, Session-start flags, one admitted snapshot | fixed append/inline mode config and AgentSession consumers | `ara-cli::{append_only_context,model_registry,rpc_host_settings,rpc_host_tools,main}` |
| Same owned side snapshot, no prefix/log/cursor mutation | fixed side/handoff/remote-compaction consumers | `ara-cli` side-request adapters and `ara-agent::provider_context` |
| Explicit startup selection fails before fallback/session/provider effects | `packages/coding-agent/src/main.ts:1145-1178` | typed `StartupModelResolutionFailure`, main exit mapping and unchanged RPC process oracle |

Exact Native exports and blob/hash manifests are retained under
`C:/Temp/ara-model-fallback-batch/native-append-context-source`,
`native-append-inline-prompt`, `native-append-inline-tests`, and the Host exports.
The application receipt is
`C:/Temp/ara-model-fallback-batch/append-context-applied-20261004T162733Z/applied.json`:
28 changed files match approved drafts byte-for-byte, with before backups.
The earlier five-file AI dependency has its own retained application receipt.

## Executed module batch and retained failure

Command:
`python -X utf8 scripts/verify_append_context.py --output C:/Temp/ara-model-fallback-batch/append-context-receipts`.
Single Cargo owner/job, two test threads, one compilation and23 families.

| Receipt / step | Actual result | Time |
| --- | --- | --- |
| `module-20261004T162733Z/receipt.json` | Compile exit101, MSVC LNK1318 PDB LIMIT; zero modules executed; source unchanged | 221.963s compile |
| `module-20261004T164321Z/receipt.json` | Compile PASS;23 groups executed;917 passed/1 failed/0 ignored; source unchanged | 289.702s compile,704.454s total |
| New Core / inventory / Host groups | 4/3/5 groups pass, including actual RPC, two Session settings/reload/drop, side isolation and precommit intent | Original per-family logs retained |
| Only failed group | RPC59 passed/1 failed: original startup15-second negative timed out in debug | 173.125s RPC family |

Receipt root is `C:/Temp/ara-model-fallback-batch/append-context-receipts`.
The original failed receipt remains FAILED. Debug cold-start sensitivity is
unresolved; this batch does not prove its complete performance root cause.
All537 before/after/current source pins and23 executed binary pins were
independently recomputed. Expected caught fixture panic is not a failed oracle.
No previous green batch was relabeled as execution of this snapshot.

The first link failed with E free484,847,616 bytes and a515,517,554-byte PDB.
Root moved only12 checked older generated cache directories to a C-drive backup,
with per-file metadata/count/length verification and restoration mapping; source,
WIP, executables and receipts were retained. Actual E free became4,531,560,448
bytes. Logical moved bytes are not the physical reclaimed size. The next debug
compile succeeded; no assertion or build profile changed for that invocation.
Recovery mapping: `generated-cache-backup-20261004T1643Z/receipt.json` under the
same Temp root. This observation does not establish a4GB PDB format limit.

## Same-source optimized original oracle

Receipt:
`C:/Temp/ara-model-fallback-batch/startup-optimized-receipts/20261004T165522Z/receipt.json`.
Compile:
`cargo test -p ara-cli --test rpc_host --release --all-features --locked --no-run --message-format=json`.
Compile339.519s PASS; exact original
`rpc_startup_unmatched_scope_and_invalid_explicit_selector_never_run_fallback`
passes1/0/0 in7.498s. Both original selectors and15-second deadline remain.
It confirms exit1, not-found stderr, no Session, no ready frame and no model
request. Source unchanged; all539 pins and three release binaries independently
match. This supplements the missing selected release scenario and does not
rewrite the original debug917/1/0 into a fresh full-green receipt.

Selected Host:
`E:/repos/ara-github/target/release/ara.exe`, SHA256
`8042a3bee060760efc2a06ef132eddece55620f4de13f5c08760077db6d74b95`.
The build used `test-fixture`; production packaging without that feature remains
a separate final gate.

## Actual bounded CAS task

Receipt:
`C:/Temp/ara-model-fallback-batch/append-context-cas-receipts/20261004T170436Z/receipt.json`.
Script `verify-append-context-cas.py` SHA256
`ec00e15657a642f34abcca77c06474441d8fccea66144b0bf5e6494a701c11ad`.
The reviewed debug-script version was preserved before switching to the pinned
current release binary and adding before/after source pins. Private key is a
child environment value; isolated config stores only its environment name.

- Model/protocol: `deepseek-v4.1-flash` / `openai-completions`, verified in the
  live enabled configured CAS catalogue. Actual route is CAS configured in ARA
  Manager. The local OMP management route probe returned URLError; that transport
  chain is not accepted by this receipt.
- Isolated `appendOnlyContext:on`; no model fallback; bounds6 task calls plus2
  recall calls,120s each and8192 token limit.
- Random CSV pen5*2.75/book3*11.5/cup3*4.25; actual edit/readback artifact has
  13.75/34.5/12.75 and total61.0. Artifact SHA256
  `b30cbd7a3c204390ca9833c022617191fba0d41ad8e3f99e782678ad557bc9ee`.
- First task4 calls/4 successful tools in6.545s; edited artifact was read after
  completion. Second task1 call/zero tools in2.248s; answer-free recall prompt
  yields pen quantity5/book unit price11.5/cup quantity3.
- Both processes and actual v3 journal retain Session
  `01a107df-ef80-77a8-bd20-3d527c506101`; all Assistant terminals stop with the
  selected route. Five calls/8.793s total;538 source/script pins unchanged.

Task/Run evidence requires independent acceptance; it does not automatically
accept a product Task, real OpenAI account login, KV-cache performance, remote
freeform, computer/MCP login or all Providers.

## Independent audit and decision

Independent Codex `/root/startup_boundary_review` read original logs, compiler
artifacts, actual CAS events/artifact/journal and the unchanged original oracle;
recomputed source/executable hashes and the random arithmetic oracle.
Final report `C:/Temp/ara-model-fallback-batch/append-context-exec-review.md`,
SHA256 `9f36c0ef190e0e26859349be06d03378d6b5b87d68c26d6b3807108aaf1a0f01`:
**bounded EXEC APPROVE**. Root independently read the verdict and critical
original logs, verified artifact arithmetic, binary and report hashes.

Decision: bounded module-tested WIP, with selected optimized runtime and actual
CAS trial independently approved. Full backend/dependency/platform/account gates
were not rerun here. Parent surfaces,111/60 formal accounting, P1-P6 and full
marker stay unchanged. Core hooks/interrupt and Broker server/management proceed
in separate scratch batches. No commit, push, deployment, memory update or
temporary daily package delivery.
