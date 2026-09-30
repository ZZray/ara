# Production RPC entry and owned Runs (WIP)

**2026-09-30.** Fixed OMP: `596f2da7101178214aa27a753529d15e6b7ad91d`.
This first Host snapshot implements a bounded task chain. It is not acceptance
of CA-RPC, A5, P2, or the 42-command surface in the [full plan](../rpc-host-plan.md).
The final parity marker remains unchanged.

## Requirement and scope

The real `ara --mode rpc` process reads commands on stdin after the existing
route/Session/context/tool/provider setup. It owns one persistent `Agent`,
canonical queues, immutable Run/journal binding, complete live events and
independent input/output scheduling. Print and line REPL behavior are preserved.

| Fixed source and contract | Rust implementation |
| --- | --- |
| `cli/args.ts::Mode`, `main.ts` RPC dispatch | `main.rs::Mode::Rpc`; detect RPC before `read_stdin` |
| `rpc-mode.ts::runRpcMode`, ordered writer and successful negotiation transition | `rpc_host.rs::serve`, `write_output`; existing `ara-rpc` framing/input |
| `rpc-mode.ts:1093–1177`, prompt/queue/abort dispatch | Persistent Agent, owned active task/token; ACK enqueued before task spawn; abort awaits settlement |
| `rpc-mode.ts:1192–1225`, `1465–1470`, live state/messages | Completed public message mirror; no access to the locked Agent transcript |
| `agent-session.ts:6765–6827`, idle queue drain | Reconcile after admission and completion; explicit abort suppresses autonomous resumption |
| `agent-session.ts:10249–10279`, last assistant text | Skip aborted empty-content candidate; JS trim; omit undefined text |
| `rpc-mode.ts:1613–1626`, EOF drain/disposal | Join input and active Run, emit shutdown once, then await output drain |
| Complete public Session events | `AgentEvent::full`; partial/message/error snapshots with existing assistant replay redaction |

Twelve commands have bounded implementations: `negotiate_protocol`, `prompt`,
`steer`, `follow_up`, `abort`, `abort_and_prompt`, `get_state`, `get_messages`,
`get_last_assistant_text`, `get_available_commands`, `set_steering_mode`,
`set_follow_up_mode`. This count does not mean twelve parity acceptances.
`get_available_commands` reports an empty supported slash-command catalogue.
Unknown and remaining commands return explicit failures.

## Explicit remaining boundaries and differences

- Model/base route is still required before ready. Model-less startup remains open.
- Runtime Session new/switch/branch, RPC Skill/custom provenance, prompt templates,
  external tools/URI/UI, background bash, compaction/retry controls, live settings,
  login and subagent/query contracts remain open. RPC Skill syntax fails explicitly.
- Startup native resume uses existing journal recovery. Its public mirror is seeded
  from `model_context`: custom Skill and soft-compaction summary messages are
  provider projections. It does not establish native custom-message query parity.
- Images are passed as existing direct image blocks. Catalogue-based image
  preprocessing/capability selection remains open; command content must be valid
  UTF-8 representable text. Correlation IDs retain transport UTF-16 semantics.
- State reports the implemented wait interrupt behavior, disabled maintenance/fast
  mode and current configured model/tools. Full catalogue/context-usage fields and
  live settings are unimplemented.
- ACK means admission, not durable queue acceptance or task success. Lazy journal
  materialization and canonical in-memory queues retain their existing semantics.
- Persistence failure latches the Session, cancels before subsequent tool effects,
  emits its concrete reason and refuses subsequent Runs. An unknown effect on
  startup recovery is paired explicitly and never replayed.
- Output and input queues retain fixed unbounded semantics. Physical transport
  ceilings do not bound total Host memory. The Rust EOF path awaits queued stdout
  delivery; fixed shutdown does not explicitly await its stdout queue. A connected
  peer which never reads stdout can therefore hold final shutdown indefinitely.
- An aborted/error/budget Run produces a correlated failure after admission while
  the protocol remains usable. Native process exit, journal fault and broken pipe
  are tested separately; Run success never implies Task acceptance.

## Verification and review

Final production source SHA-256 values:

| Path | SHA-256 |
| --- | --- |
| `crates/ara-cli/src/main.rs` | `458ddc36cca130da2db3042e10ea200d6afaa9e995cea80461a8ae72b69dcfb3` |
| `crates/ara-cli/src/rpc_host.rs` | `10032025b1484756ebc926635f33214c7f3b130a3632090f1b735d59f26ec37a` |
| `crates/ara-agent/src/event.rs` | `6aaf443c580e97a220f5a3002f8d8e2b33064639e883c510dfbdccfe9ee23648` |
| `crates/ara-cli/tests/rpc_host.rs` | `3c480928223f9fe78aab86581a9a7639d3ac431155809f62406f25773af92eb6` |

Actual Windows commands/results on those bytes:

```text
cargo test -p ara-cli --test rpc_host -- --test-threads=1
python scripts/verify_backend.py
cargo deny check
python scripts/verify_bootstrap.py
python scripts/omp_inventory.py check
git diff --check
```

- Focused real-process tests: **13 passed, 0 failed**, compile 1.24 s,
  execution 3.08 s. Full gate repeats all thirteen and executes the complete
  event-projection privacy/print-regression unit test.
- Windows full backend: **1,112 passed, 0 failed, 1 ignored, 86 suites**,
  CLI e2e **87/87**, elapsed **81.348 s**. Formatting, strict all-feature
  workspace Clippy, all-target and documentation tests passed.
- Dependency policies: advisories/bans/licenses/sources passed, **7.259 s**;
  existing license/version warnings remain. Bootstrap/links/Skills/marker and
  inventory consistency passed.
- Logs and machine-readable results:
  `C:\Temp\ara-rpc-host-entry-validation\backend-windows.log`,
  `backend-windows-result.json`, `cargo-deny.log`, `cargo-deny-result.json`.

The process suite checks malformed/final input, negotiation/explicit unsupported
errors, live queries before completion, direct image request/history, canonical
queue modes and idle drain, EOF, joined abort/replacement, provider error recovery,
Responses warm state and cold native restart, lost tool receipt recovery without
replay, user/assistant journal faults and broken stdout with stdin kept open.
The pressure precondition is a real first-Run write marker after large snapshots
and the following held model call. The replacement file appears before the
stdout reader resumes. The assistant persistence fault reaches an actual tool
call but returns the existing cancelled-before-execution receipt and produces
no file. Tests do not change established synthetic start/end cancellation events.

### Bounded actual-model task

Live authenticated catalogue at **2026-09-30 04:28:23 UTC** confirmed
`deepseek-v4.1-flash`, 58 models, OpenAI and Anthropic endpoint types. Actual
trial uses **B.AI `https://api.b.ai/v1`, OpenAI Chat Completions**;
`BAI_API_KEY` stays in the environment and no credential bytes are retained.

Driver: `C:\Temp\ara-rpc-host-entry-validation\real_rpc_trial.py`, SHA-256
`26d0764974b51ada71b53f53ffdb1542556f4d170772d229f02ce5c48fdff78c`.
Final receipts:
`C:\Temp\ara-rpc-host-entry-validation\real-rpc-ez8k12z9\summary.json`.
Bounds: two Runs, four calls/60 s per Run, eight calls total, 1,024 output
tokens per call, 120 s process guard and 4 MiB per captured output stream.

**PASS: 12.117 s, five model calls (3+2), three successful paired tool
receipts, 196 complete message-update events.** The first Run reads an
unpredictable fixture and writes exact nonce/sum/sorted-value JSON. The fixture
is removed before the second Run writes the same JSON using retained context,
with no read call. Both exact artifacts match. State queries observe
`isStreaming:true` before each Run ends with the same native Session ID.
Ten public completed messages match ten journal messages; all three tool
results pair with the observed read/write/write calls. EOF emits one shutdown
and process exit 0. Per-call usage is retained; cost remains unknown.

Original, isolated trial copy and final full-gate `target/debug/ara.exe` all
have SHA-256 `4d1c47a46be9547cfdf9669cb41b714088ba7a6b8a8f9b3ee899385ad893d561`.
The model task proves same-Agent context continuation; cold native restart
and unknown-effect recovery are proved by controlled child tests separately.
It does not prove runtime Session adoption or full RPC parity.

Independent Codex `rpc_entry_plan_review` approved the bounded plan before
implementation. Its initial complete code review found last-assistant selection
and disk-failure feedback defects; both were corrected against actual source.
The Tokio stdin shutdown concern was withdrawn after verifying the explicit
`std::process::exit` production path; real broken-output and persistence failure
tests now prove bounded exit with stdin still open.

Final independent audit by `rpc_entry_plan_review`: **approve, tested WIP only;
13/13 paths reviewed (six code/dependency/test and seven documentation), zero
skipped, no unresolved actionable finding.** It independently recomputed all
Windows/predecessor Linux totals, checked all four dependency policies, rehashed
all twelve actual-model artifacts plus summary/driver/source/executable, and
inspected raw ACK ordering, live queries, redacted snapshots, journal pairs,
exact artifacts, five usage records and one shutdown. No optional Cargo rerun
was needed after these final results. Delivered-commit Linux CI remains pending.

Next: verified same-cwd and changed-cwd Session adoption with fresh provider state,
then owned queued Skill provenance and remaining contracts in the full plan.
