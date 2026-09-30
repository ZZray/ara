# Core and host ownership

**Decision, 2026-09-25:** ARA is the reusable Rust Agent Core. This repository is a new Git root; prior ARA code and historical claims are reference material only. No behavior has been accepted here yet.

The Core owns typed conversation content, Session and Run state transitions, tool-call orchestration, cancellation semantics, context assembly, durable event/receipt contracts, and externally reviewable decisions. It exposes explicit ports for model transport, tool execution, persistence, clock, approval, and background work. A host binds those ports and supplies authorization; the Core must not import product-specific state.

The host owns Project and Task identity, users and grants, data location, Gateway routing, budgets, process lifecycle, scheduling, review, and UI/RPC/MCP transport. A successful Run produces evidence; the host's Task reviewer makes the acceptance decision. Background jobs and timers need durable ownership, bounded polling, cancellation, and lease renewal while work is active. A timer firing is a request to run, not a successful task or permission to replay an uncertain tool effect.

Keep `Project`, `Task`, `Session`, `Run`, and scheduled `Job` distinct. A Session can span Runs; a Run has one execution and its own outcome. Persist native Session identity and tool receipts. On restart or failed resume, report the failure and preserve history rather than creating a fresh conversation silently. Stream event IDs and correlation IDs let a UI resume observation without redefining the Core.

The first wire protocols are OpenAI-compatible Chat Completions/Responses and Anthropic Messages. Protocol-specific frames live in adapters; model choice and endpoint configuration belong to the host. OpenRouter free and local CAS are test routes, not Core dependencies.

This is an architecture target. Source-backed Rust behavior and host tests are required before any part can be marked implemented. See [verification](verification.md).

## Rust crate layout

**Decision, 2026-09-25**, from the P1 slice-1 implementation:

| Crate | Owns | Must not depend on |
| --- | --- | --- |
| `ara-ai` | Message model, assistant stream protocol, provider adapters (`ModelProvider` port), argument validation, history pairing guard | Session storage, tools, hosts |
| `ara-agent` | Agent loop, `AgentTool` port, `LoopHooks` (permission gate, steering/follow-up queues), `AgentEventSink` port | Storage, process/file effects, product state |
| `ara-session` | Session journal format and recovery | Providers, hosts |
| `ara-tools` | Built-in tool implementations (file/process effects) behind `AgentTool` | Hosts, products |
| `ara-mcp` | Host-side bounded MCP stdio process and `AgentTool` adapter | Session storage, product state |
| `ara-rpc` | Fixed RPC JSON value semantics, v1/v2 framing, input lines and ordered output | Agent, Session, providers, product state |
| `ara-cli` | Reference host: argument parsing, model/credential binding, journal location, print mode | Product state (HandWave/Lantern/Lumen) |
| `ara-testkit` | Controlled fake upstream and fixtures | Production crates at runtime |

**Decision, 2026-09-30 (RPC-01 transport accepted on `69de671`):** `ara-rpc` owns the
fixed OMP JSONL transport without importing Agent, Session or provider crates.
Its local UTF-16/f64/ordered-value representation preserves wire semantics
which ordinary Rust strings and `serde_json::Value` cannot fully express.
V2 decoding belongs to reception of server output; server input remains direct
JSON lines. Physical/logical output ceilings do not define inbound or host
queue limits. Rust writer completion is an awaited write/flush receipt, and
cancelled or failed partial output makes that writer terminal. No-op terminal
compaction borrows the original value; chunk emission keeps only one base64
physical line at a time, while serialized logical data remains resident.
The reference host will bind scheduling, authority and native Session
ownership separately. See [RPC-01 evidence and runtime differences](../evidence/rpc-01-transport.md).

**Decision, 2026-09-30 (production Host dependency, tested WIP):** the stateful
Agent accepts an owned configuration snapshot per Run through `prompt_with_config`
and `continue_run_with_config`; original APIs retain their original defaults.
Transcript and queue ownership remain with the same Agent. A snapshot is not
the fixed OMP live configuration port between model calls. The production RPC
Host needs an independent event mirror for live queries because `Agent.messages`
waits for the Run's transcript lock, and its sink must retain a fixed journal
binding until that Run settles. Stdin/side-channel dispatch and queued stdout
have independent owners. The [Host plan](../rpc-host-plan.md) maps all 42 commands
and remaining lifecycle/provenance interfaces; [API evidence](../evidence/rpc-host-run-config.md)
does not accept production RPC, durable queue receipts or native resume.

**Decision, 2026-09-30 (first production RPC entry, WIP):** the reference Host
has a separate command/Run owner and ordered output actor after shared CLI
initialization. Live state reads an event mirror of completed messages, with
partial snapshots separate, rather than waiting for the Agent transcript lock.
Each Run's sink permanently retains its Session journal and cancels before
further tools when persistence fails. Reconcile canonical queues after both
admission and completion; explicit abort suppresses autonomous queue drain.
EOF joins accepted Runs and awaits queued output, so a connected peer which
never reads stdout can hold final shutdown. Startup native recovery is reused;
the initial mirror uses provider-context projection and does not preserve all
custom/compaction public roles. Runtime Session adoption, RPC Skill provenance,
live settings and remaining commands retain the [full-plan](../rpc-host-plan.md)
boundary. See [actual entry evidence](../evidence/rpc-host-entry.md).

**Decision, 2026-09-28 (implemented WIP, not accepted):** The CLI owns MCP server launch and exact `(server, tool)` grants. `ara-mcp` translates a granted stdio tool into the existing `AgentTool` port; the Agent loop and Session writer retain their existing ordering and recovery behavior. The child receives only explicitly mapped environment variables plus the Windows OS installation path, and a call with an uncertain effect is recorded as unknown without reconnect or replay. This first path covers a direct executable and pinned MCP `2025-11-25`; HTTP/SSE, OAuth, catalog resources/prompts and Windows process-tree containment are separate work. See [CA-MCP stdio evidence](../evidence/ca-mcp-stdio.md).

**Decision, 2026-09-28 (implemented WIP, not accepted):** Each MCP stdio connection has one task that owns its child, pipes and Windows Job Object and reads server messages while tools are idle. Granted tools share a single-call permit and bounded command channel; dropping the last tool or abandoning an in-flight call closes the connection and releases the child. The task answers server `ping` with `{}` even without an active call, matching the [fixed OMP client and transport](upstream.md). Outbound frame validation occurs before queueing: a rejected or cancelled pre-dispatch call reports `executed: false`, while interruption after queueing remains `executed: unknown` and is never replayed automatically. See the [real-child and host evidence](../evidence/ca-mcp-stdio.md). This decision does not claim race-free descendant containment or point acceptance.

The loop awaits each `AgentEventSink::emit`. A host that persists `message_end` inside the sink has therefore journaled an assistant tool-call message before any of its tools start. By default the loop refuses to re-execute a trailing unpaired tool-call tail (`UnpairedTail::Refuse`), because such calls may already have run and their effects are unknown. A host opts in to execution only when it knows the calls never ran.
