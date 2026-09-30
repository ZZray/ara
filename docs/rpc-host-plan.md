# Production RPC host plan

**Reviewed design, 2026-09-30; implementation remains WIP.** Target OMP is
`596f2da7101178214aa27a753529d15e6b7ad91d`. The final target is the complete
CA-RPC surface, with source mappings, actual Rust host tasks and independent
acceptance. [RPC-01](evidence/rpc-01-transport.md) accepts transport only.

## Boundaries

- Preserve print/REPL entry behavior, HostSink, journal format and provider adapters.
- Add an isolated reference-host RPC module after shared initialization; identify
  RPC before `read_stdin`, so its protocol pipe is not consumed as a prompt.
- Bind one persistent `Agent` per logical native Session. Add only the Core
  interfaces required by the source-backed Host behavior.
- Keep PKG-WIRE, durable task queues/leases, products and release separate.

## Ownership and required interfaces

| Owner | Responsibility |
| --- | --- |
| `ara-agent::Agent` | Canonical transcript, steering/follow-up queues, queue modes, run exclusion and active cancellation |
| `SessionJournal` | Raw source messages, custom Skill provenance, tool receipts, recovery and compaction source IDs |
| RPC Session owner | Session identity/generation, authorized route, configuration, provider-native state, tools/cwd and disposal |
| Run sink | Immutable Session/journal binding; persistence before tool execution; public completed-message mirror and separate partial snapshot |
| Input dispatcher | Continuous stdin reader, immediate side channels, serial ordinary commands and tracked background bash |
| Output owner | Ordered enqueue/encoding, negotiation transition and actual write/flush receipts |
| `ara-rpc` | JSON values, input lines, v1/v2 frames and physical output; no Agent or product state |

The implemented `prompt_with_config` and `continue_run_with_config` accept
an owned configuration for each Run, retaining the same Agent transcript and
queues. Original methods still use their original defaults. This supports fresh
absolute deadlines and a selected Run route. [API evidence](evidence/rpc-host-run-config.md).

This API is **not** complete OMP configuration parity. Fixed `agent.ts:1439–1446`
refreshes system prompt/tools before each model call, and `1483–1486` reads live
model/reasoning/service tier. The complete Host needs a reviewed interface for
those transitions. Rejecting changes while busy cannot be counted as equivalent.

Compaction needs an idle transcript replacement interface that retains canonical
queues. A Session change deliberately adopts another Agent only after old owned
work and pending side channels settle. Delayed Skill/image preprocessing must
carry its originating generation. Queued Skill messages need owned provenance;
the existing borrowed initial-only `SkillPromptSink` is insufficient.

## Scheduling and persistence constraints

- Ordinary commands serialize without stopping stdin. Extension UI responses,
  host-tool result/update and URI result frames bypass that queue. Bash executes
  in the background and remains tracked until its correlated response is queued.
- Never serve live state/messages by awaiting `Agent.messages()`: the Run holds
  that transcript lock. Update the public mirror from events. RPC forwards full
  partial message snapshots; print's `AgentEvent::printable` drops them.
- Stdin/control handling must not wait for stdout flush. An output actor queues
  frames independently so incoming tool results/abort can still be processed
  when the peer is not reading stdout. Keep fixed unbounded queue semantics
  explicit; output ceilings are not total-memory or inbound limits.
- `Agent.abort()` requests cancellation; abort-and-prompt and Session replacement
  must also await the owned Run. Keep that Run's journal/sink alive until settled.
- Fixed prompt ACK means admission, not Run success or durable queue acceptance.
  Journal persistence is lazy before the first assistant; a durable enqueue
  claim needs its own implementation and evidence.
- Preserve custom Skill source/details rather than persisting only its user
  projection. Skill file errors precede ACK; later invocation failures are
  correlated errors. Local-only handling produces `prompt_result:false`.
- After external host-tool dispatch, cancellation/disconnection leaves effects
  unknown. Preserve that classification in the durable receipt and never replay
  automatically. A pre-dispatch refusal can prove `executed:false`.
- On EOF, settle pending requests, drain accepted serial/background work and
  dispose exactly once. Fixed `rpc-mode.ts:1613–1626` does not await stdoutQueue
  or explicitly join prompt watcher tasks. Rust shutdown delivery/join semantics
  need explicit behavioral evidence and documented differences. Host URI clear
  unregisters schemes/rejects existing waits; it is not a permanent close of all
  future direct requests.

## Complete command denominator

The 42 variants in fixed `rpc-types.ts:30–93` remain required. None is accepted
as a production Rust command by this prerequisite. Side channels and Session
events are additional contracts, not commands omitted from this denominator.

| Family | Commands | Fixed dispatch in `rpc-mode.ts` | Production status |
| --- | --- | --- | --- |
| Protocol (1) | `negotiate_protocol` | 1083–1087 | open |
| Prompting (5) | `prompt`, `steer`, `follow_up`, `abort`, `abort_and_prompt` | 1093–1177 | open |
| Session adoption (3) | `new_session`, `switch_session`, `branch` | 541–565, 1180–1185 | open |
| State/bridges (9) | `get_state`, `set_fast_mode`, `get_available_commands`, `set_todos`, `set_host_tools`, `set_host_uri_schemes`, `set_subagent_subscription`, `get_subagents`, `get_subagent_messages` | 1192–1294 | open |
| Model (3) | `set_model`, `cycle_model`, `get_available_models` | 1301–1336 | open |
| Thinking (2) | `set_thinking_level`, `cycle_thinking_level` | 1340–1351 | open |
| Queue modes (3) | `set_steering_mode`, `set_follow_up_mode`, `set_interrupt_mode` | 1357–1369 | open |
| Compaction (2) | `compact`, `set_auto_compaction` | 1376–1384 | open |
| Retry (2) | `set_auto_retry`, `abort_retry` | 1390–1398 | open |
| Bash (2) | `bash`, `abort_bash` | 1404–1413 | open |
| Session queries/actions (6) | `get_session_stats`, `export_html`, `get_branch_messages`, `get_last_assistant_text`, `set_session_name`, `handoff` | 1418–1459 | open |
| Messages (2) | `get_messages`, `get_messages_page` | 1465–1493 | open |
| Login (2) | `get_login_providers`, `login` | 1500–1558 | open |

## Implementation order and proof

1. **Owned Run configuration dependency:** implemented and Windows tested;
   remains WIP pending production integration. No accepted Host command.
2. **First production task chain:** actual CLI startup, ready/commands metadata,
   protocol negotiation, prompt/images/Skill, live events/state/messages,
   canonical queue/modes, abort/abort-and-prompt and startup resume. Exercise
   new/switch Session early to prove identity, late-preparation and provider-state
   isolation. The current Rust route requires model/base URL; fixed OMP can
   become ready before login/model selection. Keep model-less startup mapped.
   The first [entry snapshot](evidence/rpc-host-entry.md) implements twelve
   bounded commands and actual owned Run/sink/mirror/output binding. It remains
   a historical WIP checkpoint: runtime adoption and Skill provenance were open
   there and are addressed by the subsequent slices below. Remaining task-chain
   contracts continue to return explicit unsupported errors.
   The [native Session slice](evidence/rpc-host-session.md) adds bounded new/switch
   implementations (fourteen of 42 variants implemented, not full command parity).
   Fixed RPC cancels changed-cwd switches; unchanged same-ID replay retains
   provider state, while new/different/changed Sessions reset it. Full saved
   model/settings and runtime capability reconciliation remain open.
   The [owned RPC Skill point](evidence/rpc-host-skills.md) is accepted on
   `97567f0`: pre-ACK fresh source, command-specific images/default queue behavior,
   opaque Core input provenance, custom receipts, Run-local public terminals and
   active-branch native restoration. Complete CTX-01e and CA-RPC remain open.
   The [native query/action batch](evidence/rpc-native-queries.md) adds registered
   Skill metadata, branch selectors, bounded statistics and Session naming:
   seventeen bounded variants, with full catalogue/statistics/fork contracts open
   at that checkpoint. The [native fork/paging batch](evidence/rpc-native-fork-paging.md)
   then adds canonical memory journals, real native forks and snapshot paging:
   nineteen bounded variants. The [Host tool/URI batch](evidence/rpc-host-bridges.md)
   then adds live tool/prompt snapshots, actual bidirectional calls and instance
   content routing, bringing the bounded count to twenty-one of 42. Complete
   xdev presentation, runtime catalogue and the remaining Host contracts stay open.
   Complete CA-RPC and full advanced lifecycle hooks
   remain open; this is tested/audited WIP, not full command acceptance.
3. **Bidirectional execution and maintenance:** host tools/URI/extension UI,
   background bash, live configuration, compaction/retry and command metadata.
   Unsupported known commands fail explicitly until implemented.
4. **Remaining Session/auth/subagent contracts:** branches, paging, export/name/
   stats/handoff, login and subscription/query behavior. Complete every row and
   its source tests before CA-RPC/A5/P2 acceptance.

Use unchanged fixed `rpc-input-frame` tests for dispatch/shutdown; `rpc-skill-command`
and `rpc-prompt-result` for ACK/local-only behavior; `rpc.test` for model/tool/
Session tasks; client start/restart, messages, host-tools/URI, subagent and UI
tests for their respective contracts. [Executed 15-test dispatch baseline](evidence/rpc-host-run-config.md)
does not prove production EOF or native AgentSession.

Production proof must use actual Rust child pipes: observe events before Run
completion; hold stdout while sending controls; test malformed lines, background
abort, correlated late errors, persistence failure, unknown tool outcomes,
restart and Session generation. Inspect real artifacts and journal receipts.
Then run a bounded task through a currently verified real-model route, the
delivered-code backend/dependency gates and independent diff/artifact audit.

Three independent Codex views (`rpc_host_pragmatism`, `rpc_host_architecture`,
`rpc_host_performance`) proposed and cross-reviewed this ownership design.
Consensus: the owned Run configuration prerequisite, canonical Agent queues,
isolated RPC sink/mirror and independent input/output scheduling. Remaining
dependencies and differences above stay open. The full OMP marker remains null.
