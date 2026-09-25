# Core and host ownership

**Decision, 2026-09-25:** ARA is the reusable Rust Agent Core. This repository is a new Git root; prior ARA code and historical claims are reference material only. No behavior has been accepted here yet.

The Core owns typed conversation content, Session and Run state transitions, tool-call orchestration, cancellation semantics, context assembly, durable event/receipt contracts, and externally reviewable decisions. It exposes explicit ports for model transport, tool execution, persistence, clock, approval, and background work. A host binds those ports and supplies authorization; the Core must not import product-specific state.

The host owns Project and Task identity, users and grants, data location, Gateway routing, budgets, process lifecycle, scheduling, review, and UI/RPC/MCP transport. A successful Run produces evidence; the host's Task reviewer makes the acceptance decision. Background jobs and timers need durable ownership, bounded polling, cancellation, and lease renewal while work is active. A timer firing is a request to run, not a successful task or permission to replay an uncertain tool effect.

Keep `Project`, `Task`, `Session`, `Run`, and scheduled `Job` distinct. A Session can span Runs; a Run has one execution and its own outcome. Persist native Session identity and tool receipts. On restart or failed resume, report the failure and preserve history rather than creating a fresh conversation silently. Stream event IDs and correlation IDs let a UI resume observation without redefining the Core.

The first wire protocols are OpenAI-compatible Chat Completions/Responses and Anthropic Messages. Protocol-specific frames live in adapters; model choice and endpoint configuration belong to the host. OpenRouter free and local CAS are test routes, not Core dependencies.

This is an architecture target. Source-backed Rust behavior and host tests are required before any part can be marked implemented. See [verification](verification.md).
