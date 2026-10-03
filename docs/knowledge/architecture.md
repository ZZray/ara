# Core and host ownership

**Decision, 2026-09-25:** ARA is the reusable Rust Agent Core. This repository is a new Git root; prior ARA code and historical claims are reference material only. No behavior has been accepted here yet.

The Core owns typed conversation content, Session and Run state transitions, tool-call orchestration, cancellation semantics, context assembly, durable event/receipt contracts, and externally reviewable decisions. It exposes explicit ports for model transport, tool execution, persistence, clock, approval, and background work. A host binds those ports and supplies authorization; the Core must not import product-specific state.

The host owns Project and Task identity, users and grants, data location, Gateway routing, budgets, process lifecycle, scheduling, review, and UI/RPC/MCP transport. A successful Run produces evidence; the host's Task reviewer makes the acceptance decision. Background jobs and timers need durable ownership, bounded polling, cancellation, and lease renewal while work is active. A timer firing is a request to run, not a successful task or permission to replay an uncertain tool effect.

Keep `Project`, `Task`, `Session`, `Run`, and scheduled `Job` distinct. A Session can span Runs; a Run has one execution and its own outcome. Persist native Session identity and tool receipts. On restart or failed resume, report the failure and preserve history rather than creating a fresh conversation silently. Stream event IDs and correlation IDs let a UI resume observation without redefining the Core.

The first wire protocols are OpenAI-compatible Chat Completions/Responses and Anthropic Messages. Protocol-specific frames live in adapters; model choice and endpoint configuration belong to the host. OpenRouter free and local CAS are test routes, not Core dependencies.

This is an architecture target. Source-backed Rust behavior and host tests are required before any part can be marked implemented. See [verification](verification.md).

## Rust crate layout

**Implemented boundary, 2026-09-30:** Core `LoopHooks::execution_snapshot` optionally
provides matching tools and system prompt once per model call. The whole response
keeps that snapshot, including prepared adapter Arcs; the next call may refresh.
RPC owns tool registries and pending transport, while `ContentUriPort` only routes
instance-bound text read/write through ordinary tool permissions. Context clones
share the port slot; a fresh context has a fresh slot. A Run sink retains its
original Session. URI `clear` is reusable, so EOF needs a distinct permanent
connection admission close under the same mutex as pending insertion: rejecting
current waits alone can let accepted queued commands register routes and hang.
See the [bounded implementation and executable evidence](../evidence/rpc-host-bridges.md).
Live model/settings and full presentation remain open.

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


### Native RPC Session replacement

[Native adoption evidence](../evidence/rpc-host-session.md) maps the fixed RPC
boundary: a changed recorded cwd cancels; the Host joins old accepted work before
opening or replacing journals. Run sinks remain attached to their original
Session. Prepare target tools/context/provider before adopting once; failures
retain the prior native identity and transcript. Explicit persistent new creates
a restart boundary before ACK. Unchanged same-file, same-ID provider replay
retains native provider/tool state; new/different/edited Sessions reset it.
Compare replay content rather than timestamp/usage metadata. Prompt refresh and
fresh setup share the initial CLI helpers; complete saved-model/settings and
capability reconciliation are still open.

### RPC terminal and Run settlement

**Decision, 2026-09-30 (verified correction `9491fa1`, RPC remains WIP):**
Clients can send their next prompt immediately after `agent_end`. The internal
Agent loop emits that event while its running guard/transcript lock are still
owned; the reference RPC Host must retain the terminal in its fixed Run sink
and publish it only after joining the owned task and clearing the active slot.
All earlier live events and journal barriers retain their schedule. Completion,
abort and EOF share this boundary; publish the terminal before its correlated
error, and never fabricate a terminal for a task which emitted none.
This matches fixed OMP `agent-session.ts:750-755,836-842,2361-2371`. The
[Session evidence](../evidence/rpc-host-session.md) contains the actual Linux
failure, gated real-Agent oracle, caught premature-emission mutation and final
Windows/Linux/real-task receipts. This rule does not change Core/print/REPL or
accept full RPC parity.

### Owned RPC Skill input and public provenance

**Decision, 2026-09-30 (bounded point accepted on `97567f0`):** Core queues own
`AgentInput` envelopes with a model message and optional opaque `Arc<Value>`
provenance. Never recover custom input identity by model-message equality or
timestamp: two identical model messages may have distinct user sources. Owned
hooks and the awaited input callback carry each envelope through queue modes,
rollback and pending deadline/budget commits. Core does not interpret metadata,
and provider context, transcript and RunReport contain model projections only.

The RPC sink owns custom presentation and the consumed input's journal receipt.
Its local completed-message sequence supplies the Run terminal, published only
after task join. A Session's global mirror is a live query view, not a substitute
for Run identity. Restore recognized Skill public messages from active raw
branch entries; the historical file may no longer exist. Keep provider replay
and public native history as separate projections. CLI setup owns discovery;
adopt config and its Skill snapshot together after successful preparation.

The [point evidence](../evidence/rpc-host-skills.md) includes equal-value provenance
oracles, real process failures, source deletion/restart/switch, exact Windows and
Linux gates and the audited actual task. Arbitrary custom roles, full compaction
public rendering, live/saved settings and durable queue admission remain open.

### Native user Bash and retained Session writers

**Decision, 2026-09-30 (bounded RPC checkpoint, full parity open):** User-shell
jobs belong to the reference Host and have tokens separate from model tools.
Input dispatch captures the target before ordinary serial work can settle;
only the serial completion owner writes receipts. Native `bashExecution`
identity stays in Session storage/public history and projects to User context.
Core idle transcript edits share Run admission's atomic state guard and cannot
hide unresolved tool effects or discard queued input.

A pending job must not retain a stale journal writer when that file is reopened.
Reuse the captured Session's journal lock and replace its reopened view before
adoption. Transition destinations retain their own advancing branch parent;
detached or branch receipts cannot enter the new Agent. Current receipts defer
until the owned Run joins, before new prompts or automatic queued continuation.
Branch append preserves the live leaf only: fixed OMP rebuild selects the last
raw entry after restart. A new persistent leaf marker would change that contract.
Valid context-excluded Bash is transparent to unknown-effect recovery adjacency;
included/malformed receipts remain barriers. The [checkpoint evidence](../evidence/rpc-native-bash.md)
records source locations, actual process/journal faults and the verified restart
task. Complete shell/settings/maintenance behavior remains open.

### Native Session retry ownership

**Decision, 2026-10-01 (bounded same-route checkpoint, full parity open):**
Provider replay and Session recovery have different owners and budgets. A
Session retry begins only after its failed Run joins, retains its original
Agent/queues and captures native append IDs. Backoff has its own cancellation;
aborting that wait does not abort an already started continuation. Eligible
failed active tails may leave the model projection while their raw source
remains. Successful recovery rewrites exact failed native IDs before emitting
recovery receipts; I/O failure is fail-stop.

Header facts must survive a dropped body future: provider veto, Session route
veto and Session wait ceiling are distinct. Unknown tool effects do not
authorize replay. [Retry evidence](../evidence/rpc-session-retry.md) records
the tested scope and mandatory auth/model/overflow/interrupted-turn branches.
The Host owns those registries and authorized route adoption; Core must not
acquire product identity or credential storage.

### Complete model route and request authentication

**Tested foundation, 2026-10-01; full registry/auth/fallback remains open:**
the Host owns exact catalogue metadata, native credentials and route selection.
`PreparedRoute` carries the execution model, protocol options, lazy resolver and
generation. One logical model call acquires one private credential lease; inner
HTTP and outer provider-stream retries retain it. Its immutable actual identity
owns settlement even if another request has since selected another account.
Native row revision is a captured global DB revision, not a per-row counter.

Credential headers must be in that same lease: a static Authorization/x-api-key
override can otherwise send account A while attributing usage/failure to B.
Prepared protocol options reject static credential material. Startup explicitly
moves authorized overrides into its Runtime lease. A settlement write failure
preserves the provider's actual terminal/content/usage/native ID, reports error
and vetoes replay; it cannot erase evidence to manufacture a clean retry.

Core `ExecutionSnapshot` optionally replaces model/provider/per-call options
atomically with tools/prompt. One response and its entire tool batch retain that
snapshot; the next call refreshes it. Run budgets, deadlines and hooks remain
Run-owned. Pure catalogue lookup does not resolve authentication, and catalogue
metadata that execution cannot yet represent is rejected rather than discarded.
See the [bounded evidence and remaining contracts](../evidence/model-auth-foundation.md).

**Native composition ownership, 2026-10-02 (tested bounded WIP):** safe model
metadata is separate from opaque Host-owned header/API-key sources. Preserve
absent/undefined/null/source presence; plain records are literal until a native
operation wraps them in Live. A collapsed family inherits the original input
firstMember donor, which can differ from its default/request ID. Never restore
private headers by looking up the final ID. Materialized bundled rows share
references and must not be rebuilt merely because they entered the registry.

The static adapter composes explicit snapshots without I/O, discovery or
credential authority. External availability callbacks run outside its mutex:
a Host may legitimately reenter lookup during authentication observation.
Inspect actual composed header names before executing helpers. Cancellation
must reach every nested Live source and the resolver's actual helper-start
gate; started synchronous work still settles before normal exit. The daily
Host reasserts provider authHeader from its effective CLI/environment lease
so attribution and the actual bearer agree. Full loader/auth/runtime lifecycle
remains separate required work; see [composition evidence](../evidence/model-composition.md).

**Command-value ownership, 2026-10-02 (tested bounded WIP):** native synchronous
models configuration is materialized after the reference Host knows the
effective cwd. Provider headers precede eager key installation because a header
helper can create input needed by the key helper. Raw source chains belong to
the private lease resolver, with the native process-shared command cache; they
are not model or journal metadata. Ordinary wire retry keeps its lease. Selected
pre-output HTTP 401 refresh invalidates the provider's key and all model/override
header commands, settles the old request and records a separate fresh identity
only when a changed bearer permits another request.

Joining a Run is insufficient for a dropped authentication consumer: the CLI
uses `std::process::exit`, which can terminate a still-owned blocking helper.
Register pending helper ownership before worker dispatch and wait for all
started helpers at normal process shutdown. Check cancellation before every
unstarted helper; do not replay an unknown-effect command. Hard termination
retains its unknown-effect boundary. [Executed config-value evidence](../evidence/model-config-values.md)
records the observed premature exit, repaired actual Host and remaining full
registry/auth/Bun/Linux and dependency-acceptance obligations.


### Native tokenizer metadata and Host policy

The fixed catalog has eight tokenizer families; the native encoder crate has
ten encodings, including Cl100kBase/O200kBase. These are separate contracts.
Resolve family from catalog identity/requestModelId and carry it on Model;
provider/API names are insufficient evidence. Strict or explicitly accurate
unknown content uses O200kBase. Other unknown estimates remain approximate.
Host captures environment policy and injects it into immutable Core counters;
Core does not read product configuration or environment variables. Prepared
message-text observations exclude schemas, images, wire framing and billing.

Pinned xutf1.5.0 uses Unicode17 NFC and Unicode16 category/script data. A stable
replacement must preserve this mixed version contract. Claude content can have
more tokens than UTF-8 bytes, including the two-byte/three-token ξ case; a raw
byte shortcut cannot prove a strict budget fit. Fresh counts avoid stale mutable
message caches until an identity/version contract exists. See the [source,
executed evidence and required gaps](../evidence/native-tokenizer.md); full
tokenizer/cache/platform and unknown-accurate Core summary parity stay open.

### Shared authentication leases and exact refresh

Registry discovery and supported reference-Host requests share one Host
AuthStorage. Ordinary request environment defaults live in a private Session
resolver context after stored login and before static keys; installing them as
global config overrides would suppress login and leak one route's policy into
another. Explicit authored authentication retains priority.

Refresh and feedback use the failed private bearer and durable row identity.
A peer's newer bearer on the same row can be adopted; a row ID alone is not a
bearer replay cycle. Pre-output definite rejection is required for request
replay, and post-dispatch unknown refresh outcomes stay fenced. Classify native
OAuth failures using the first 500 UTF-16 units of private raw response text,
without storing that text in diagnostics or disabled causes. A missing refresh
field detected before dispatch is transient in the shared Codex adapter; it
must not erase post-dispatch InvalidResponse/unknown-outcome evidence.

These are bounded implemented boundaries, not full AuthStorage parity. The
[source and executed AuthStorage evidence](../evidence/auth-storage.md) retains
usage-only refresh, full callers, actual account and platform
requirements. Root/independent review is required before durable acceptance.

### Transient response notification and usage ingestion

The model port exposes transient provider-response facts through an awaited
callback. Reference Host storage, Registry and logical Session attribution
remain outside Core. Native SessionStats attributes headers to the current
Session account at callback time, independently of the request's authentication
source; it does not capture a credential lease. Resolve the current Registry's
first truthy provider base URL at that time. Compose internal ingestion before
the selected external callback, preserving per-call/default precedence.

Fixed ordinary OpenAI Completions and Responses notify only after successful
POST dispatch. A callback failure is local evidence and cannot authorize
credential refresh or model replay. Their body first-item budgets start after
the callback; Responses disarms its request watchdog before notification,
while Completions still settles callback work before reporting an elapsed
request deadline. The exact Completions body-disconnect timing and precedence
of a latched timeout over a callback rejection remain open.
Fixed ordinary Codex SSE has no generic response-notification producer;
retaining a callback option must not silently add one.

Local header ingestion preserves prior reports and logical expiry rather than
claiming a fresh full probe. Store override/delegate/aggregate hooks and the
existing report-invalidation stale-retention difference remain required work.
See [bounded source and executed header evidence](../evidence/usage-headers.md).
