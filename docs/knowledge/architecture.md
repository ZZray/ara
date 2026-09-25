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
| `ara-cli` | Reference host: argument parsing, model/credential binding, journal location, print mode | Product state (HandWave/Lantern/Lumen) |
| `ara-testkit` | Controlled fake upstream and fixtures | Production crates at runtime |

The loop awaits each `AgentEventSink::emit`. A host that persists `message_end` inside the sink has therefore journaled an assistant tool-call message before any of its tools start. By default the loop refuses to re-execute a trailing unpaired tool-call tail (`UnpairedTail::Refuse`), because such calls may already have run and their effects are unknown. A host opts in to execution only when it knows the calls never ran.
