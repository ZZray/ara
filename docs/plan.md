# ARA execution plan

**Draft v1, 2026-09-25.** This will be revised after the user's reference documents (`ara-doc-ref`) are added to the repository. It is a plan, not an implementation claim. Status lives in the [feature ledger](upstream/feature-ledger.md) and in `docs/evidence/`. Gates follow the [roadmap](roadmap.md) and [acceptance rules](acceptance.md).

## Goals

1. **G1: port OMP as the ARA Core.** Port the fixed OMP commit's Agent behavior to Rust surface by surface, with source-backed evidence. The [inventory](upstream/inventory.md) is the denominator.
2. **G2: build a next-generation task Agent on that Core.** The direction is in [agent evolution](knowledge/agent-evolution.md): move from answering to finishing tasks, with memory, reasoning, autonomous planning, decisions, execution, feedback, and unified Omni perception for text, images, audio, video, documents and sensors, in Chinese and English. This lays the ground for understanding the physical world.
3. **G3: validate against real models.** Every integration point runs a bounded real task through the actual host chain. Trials have used OpenRouter (`openrouter/free`), Agnes AI (`agnes-2.5-flash`, `agnes-2.5-pro`) and B.AI (`deepseek-v4.1-flash`) on OpenAI-compatible routes. Credentials come only from the environment; each model ID is verified against the live catalogue before a trial.

## Current priority: reproduce OMP before customization

**Latest Provider ordering, user decision 2026-10-01:** deliver OpenAI-compatible
protocols/custom configuration, then OpenAI account login for daily use. Other
Provider compatibility is explicitly deferred and recorded in the
[provider plan](provider-plan.md), including full cipher and advanced Codex
transport parity. This changes Provider ordering; deferred work is not accepted
or erased, and the independent Rust ARA/Core/P0–P6 goal remains intact.
Use module tests and one shared final gate, reuse original OMP test inputs,
and prefer the current local Manager → OMP management → CAS
`deepseek-v4.1-flash` route for bounded real tasks.

**User decision, 2026-09-30:** finish the fixed OMP behavior port first, then
customize ARA. Track A and P1-P3 now take priority over new Track B/P4 features,
product-specific additions and further V1 dogfood work. V1 is already accepted.
Existing completed user-requested additions remain recorded; unrelated WIP is
preserved without being included in parity acceptance.

**User clarification, 2026-09-30:** reproduce the fixed OMP completely; do not
omit features. Bounded checkpoints are progress records, not permission to
drop remaining methods, commands, failure paths or inventory surfaces. Every
remaining source-backed behavior is mandatory before full parity acceptance.
An unsupported response, a scheduled row or an intentional-difference label
cannot substitute for implementing a requested OMP feature. Necessary Rust or
platform adaptations must preserve its observable behavior. New customization
starts after that complete reproduction and its executable audit.

Work in coherent module batches rather than repeatedly accepting tiny fragments:

1. Continue the production RPC/host module against its complete fixed-source
   command and lifecycle map. Batch related command discovery, native queries
   and Session actions where they reuse one owner. Registered Skill metadata
   uses the existing snapshot; it does not imply a complete six-source catalogue.
2. Complete remaining Core/Session long-task behavior and provider/protocol
   parity using their existing implementations, mapped fixtures and open rows.
   Independent files may be implemented in parallel; ownership stays explicit.
3. Complete extensibility and remaining fixed-source surfaces, then classify and
   audit the full inventory for P3 before starting ARA customization.

Each implementation point runs its applicable focused checks. A completed module
batch shares one final full backend/dependency gate, relevant bounded task and
independent audit on the same delivered snapshot; its included points stay WIP
until those gates pass. Reuse unchanged source and artifact evidence, and rerun
checks to resolve an actual remaining risk or a required gate. Existing reviewed
plans need a new design review only when their scope or architecture changes.

Report module coverage and actual remaining differences separately from formal
acceptance, together with measured implementation/gate time and the next batch.
Do not use narrow fixture counts as an OMP completion percentage. No new
completion-date estimate is asserted without measured module throughput.

## Where we are

OpenAI daily Host checkpoint (2026-10-01): custom models.yml Chat/Responses
configuration now reaches tools and original-Session restart. Device
login/logout, private SQLite refresh/settlement and dedicated Codex SSE pass
the controlled real-process workflow, including `/new` attribution and hidden
reasoning replay. Windows module 22/0/0 and full backend 1,491/0/20,
inventory/deny/build pass in 134.536 seconds; configured CAS artifact/restart
passes in 7.032 seconds with answer-free prompts and tools disabled for recall. [Daily evidence](evidence/openai-daily.md) and
[usage](openai-daily.md) define the supported slice. Actual OpenAI authorization
and an account-model task still require live acceptance. Complete AuthStorage,
registry projection, Codex RPC/WS/Lite and other providers stay open/deferred;
P0/V1 accepted, P1–P6 open, RPC 27/42 and the parity marker null remain unchanged.

Next: close the actual account trial when device authorization is available;
continue remaining Core/Session and RPC reproduction in coherent modules.
Other Provider compatibility follows the recorded daily-use priorities.

Fixed model discovery/factory/manager checkpoint (2026-10-01, tested WIP):
692 original-source discovery cases, 3,388 protobuf cases, 58 CA helper cases,
actual HTTP/TLS/H2/process tests and independent review pass on the final
snapshot. The one-command module/full run takes 198.755 seconds: module
25/0/0, Windows full backend 1,469/0/20, inventory/deny/build PASS.
An actual CAS file task and same-Session resume also pass. See
[discovery evidence](evidence/model-discovery.md). Main registry/auth is still
open; next deliver the custom OpenAI configuration and account-login daily
path, with other Provider compatibility in the deferred register.

Fixed model variants/lossless checkpoint (2026-10-01, tested WIP): all eight
collapse exports and their mutable aliases/templates/reference identity, native
UTF-16/undefined/non-finite policy construction and native RegExp pass their
retained source oracles and Windows full gate; see
[variant evidence](evidence/model-variants.md). Continue with complete discovery
transport/normalization/model-manager, overrides and registry, then full
authentication and execution/journal integration. P0/V1 remain accepted,
P1–P6 open, RPC 27/42 and the full parity marker null.

Fixed model policy/construction checkpoint (2026-10-01, tested WIP): native compiled
rule/taxonomy resolution, complete compat/thinking/build, host identity/reference/
metrics and declarative behavior/auth accessors pass 46,136 original-source
comparisons, 270,556 behavior pairs and the Windows full gate; see
[policy evidence](evidence/model-policy.md). The variant-list dependency is
recorded above; discovery transports/normalization/manager, registry/authentication and live
metadata/journal adoption remain the next required integration. No phase or
RPC acceptance advances from this Host dependency.

Native models configuration/cache checkpoint (2026-10-01, tested WIP): fixed
schema/validation, Host-owned config file and SQLite model cache have executable
coverage; see [config/cache evidence](evidence/model-config-cache.md). Continue
directly with complete model building/compat and native registry/auth/discovery
integration. This prerequisite batch adds no accepted RPC command or phase and
retains every outstanding fixed-source behavior.

P0 inventory is done. P1 slice 1 is tested with a controlled upstream:

| Point | Crate | What it covers |
| --- | --- | --- |
| AI-01 | `ara-ai` | Message model and Chat Completions adapter |
| AGT-01 | `ara-agent` | Agent loop |
| SES-01 | `ara-session` | Session journal |
| TOOLS-01 | `ara-tools` | `read` / `write` / `bash` |
| TOOLS-02 (A2) | `ara-tools`, `ara-walk` | `grep` / `glob` with the pi-walker ignore chain |
| TOOLS-03 (A2) | `ara-tools`, `crates/vendor` | `edit` (all five modes, hashline default) with hashline `read`/`grep`/`write` |
| CLI-01 | `ara-cli` | Print host |

Accepted on 2026-09-25 for the behaviors exercised in the [real-model trials](evidence/real-model-a1-a2-20260925.md) (Agnes `agnes-2.5-flash`, OpenRouter `openrouter/free`). Context points (A2 CTX-01) carry their own [real-model trial](evidence/ctx-01d-system-prompt.md) on both routes: an `AGENTS.md` rule and a skill's format must be followed.
- Accepted: CTX-01a (`ara-prompt`), CTX-01b (`ara-discovery` context files) and CTX-01c (frontmatter and skills).
- CTX-01d is accepted for the explicitly bounded system-prompt/reminder and non-plugin Skill host behaviors on `d05dab0`; see the [final fault evidence and independent point audit](evidence/ctx-01d-skill-directory-fault.md). Evidence includes the observed Windows zh-CN/en-US, macOS en-US and Linux en-US sorting fixtures; the 257 MiB complete resource scan; 22 fixed-source representative Skill renderer comparisons; actual Linux post-open enumeration/type EIO receipts with 100 rehashed uploaded files; a bounded B.AI task with exact artifact and Session receipts; and final Linux backend gate 1,051 passed, 0 failed, 1 ignored. Other locales, generic selectors (TOOLS-01a), plugin/managed Skills (CTX-01c) and `/skill:` (CTX-01e) retain their open boundaries. The native addon build provenance and earlier unconfirmed REPL-warning failure stay explicit in the evidence. P1/P3 and the full OMP marker do not advance. Historical source and repair receipts remain in the [sort evidence](evidence/ctx-01d-skill-directory-sort.md), [renderer evidence](evidence/ctx-01d-skill-selector-renderer.md) and [handoff](handoffs/2026-09-30.md).
- CTX-01e (`/skill:` invocation) remains implementing as a whole. Its bounded line REPL and specialized Session slice is accepted on `862911a` by independent Codex audit: unchanged fixed-source parser/builder comparison (35 cases), custom persistence/projection/compaction/restart, controlled failures/cancellation and actual Linux FIFO exit, Windows full gate 1,070/0/1, Linux 1,073/0/1 (e2e 90/90), final delivered dependency policy and final-binary B.AI task with exact artifacts (6 calls, 4 receipts). The subsequent owned RPC Skill point is accepted on `97567f0` (see [RPC Skill evidence](evidence/rpc-host-skills.md)). Full TUI, line-host images, autoload/generic custom families, plugin containment and remaining RPC contracts stay open; no full CTX-01e, P1/P3 or parity-marker acceptance. [Evidence and final audit](evidence/ctx-01e-skill-invocation.md#final-independent-point-audit-2026-09-30).
- A4 MODEL-01a (WIP): opt-in `--api proxy-auto` selects the exact model's Anthropic or OpenAI Chat endpoint from a dual-protocol proxy before Session creation. Controlled CLI tests cover a tool effect, Session receipts and fail-closed discovery; model capability metadata, source-bound resume, the full Windows gate and a bounded live task remain open. [Evidence](evidence/model-01a-proxy-protocol.md).
- A3 preparation (WIP): the isolated AGT-TOKENIZERa text/message estimate is under implementation; [evidence](evidence/agt-tokenizer-estimate.md). The initial byte-based budget probe was removed after pinned Claude fixtures disproved its claimed token upper bound. The tokenizer point remains WIP; A3 compaction/long-task acceptance is separate from the bounded CTX-01d decision.
- A3 AGT-TOKENIZERb (WIP): a Claude-only content counter with pinned vocabulary and fixture evidence builds on stable Windows Rust; [evidence](evidence/agt-tokenizer-claude.md). Model-family selection, actual provider-request sizing and compaction remain separate open work.
- A3 AGT-TOKENIZERc (WIP): the model carries an optional Claude tokenizer family, selected from a conservative canonical ID subset or explicit host override; Agent text-fragment counts use it. The loop and provider-request budget remain open; [evidence](evidence/agt-tokenizer-model-selection.md).
- A3 AGT-TOKENIZERd (WIP): the OpenAI-compatible adapter can report selected-family counts for prepared request message text, using the same JSON value it sends. Tool payloads, images, framing and proxy transformations remain unknown; no hard budget or compaction decision uses this report. The CLI exposes an opt-in diagnostic; [evidence](evidence/agt-tokenizer-request-text.md).
- A3 AGT-COMPACTIONa/b/c/d/e/f (WIP): source-tagged input, complete-turn validation, a bounded one-shot summary call, strict read-only Session source snapshots and projection, conservative whole-turn cut candidates and a provisional recent-message target selection exist. Panic receipts are now excluded from summary cuts and projection; cancelled tool errors with empty details still lack an effect classification. These pieces do not prove provider context fit, persist a summary or resume from it; [input evidence](evidence/agt-compaction-input.md), [call evidence](evidence/agt-compaction-call.md), [source evidence](evidence/agt-compaction-session-source.md), [cut candidates](evidence/agt-compaction-cut-candidates.md), [projection evidence](evidence/agt-compaction-projection.md), [selection evidence](evidence/agt-compaction-cut-selection.md), and [unknown-effect handoff](evidence/agt-compaction-unknown-effects.md).
- A3 AI-RETRYa (WIP): OpenAI Chat streams retry known-empty stops and typed transient failures before output reaches the Agent, including pre-Start failures. A private broad account-cap classifier suppresses unsafe outer replay for terminal HTTP and in-band errors. The `image_end` event now commits an attempt, but no current adapter emits output images; [evidence](evidence/ai-retry-stream.md). An optional receipt now preserves outer-attempt usage and timing through the host journal, with unknown buckets explicit. The inner HTTP cap is unchanged. Billing totals, provider image output, real-model evidence and full acceptance remain open.
- AI-01c Chat object-argument extension (WIP): MiniMax-compatible streamed object fragments now merge recursively, preserve cumulative and incremental strings/arrays, filter unsafe keys and emit one complete JSON delta before tool end. Controlled HTTP and real CLI write/journal tests pass. The full Windows gate and bounded real-model task remain open; [evidence](evidence/ai-01c-openai-object-arguments.md).
- AI-01b Chat Mistral/Devstral history profile (WIP): an explicit CLI option enables nine-character paired tool IDs, result names, assistant bridges after tool results, and same-source Thinking as text. Provider and real CLI restart/tool-effect tests pass; automatic route detection, cross-provider Thinking demotion, the full Windows gate and a bounded real Mistral task remain open; [evidence](evidence/ai-01b-mistral-chat-history.md).
- AI-01e Responses explicit assistant `dt:false` full-history snapshot consumption is WIP in the provider. A controlled HTTP request proves earlier history replacement, following-message retention and unmatched-call repair; the host does not yet produce such a snapshot. User-message/compaction snapshots, remaining provider parity, a bounded real-model task and the full gate remain open; [evidence](evidence/ai-01e-responses-full-snapshot.md).
- AI-01e Responses `previous_response_id` chaining is WIP behind an explicit provider option and CLI `--responses-stateful` flag. Controlled HTTP and real CLI tests cover strict-prefix deltas, stored response IDs, tool continuation, restart full-history fallback, ZDR, cancellation and concurrent branches. CLI remains stateless by default; code-only stale-ID rejection, real-model evidence and the full gate remain open; [evidence](evidence/ai-01e-responses-chaining.md).
- AI-01e Responses tool wire postprocessing is WIP: nullable scalar unions, bare enum types and same-description constant unions are normalized after draft upgrade. Provider HTTP and Agent tool-effect tests pass; property-comment key remapping is deferred because it would diverge from execution validation, and the full gate remains open; [evidence](evidence/ai-01e-responses-wire-postprocess.md).
- AI-01e Responses failure details are WIP: direct and nested error codes, incomplete and status reasons, and stream error frames now reach the user and Session journal. Controlled HTTP and real CLI tests pass; the pre-output retry interaction is tested. The Windows Bash gate and bounded real-model task remain open; [evidence](evidence/ai-01e-responses-failure-details.md).
- A4 AI-ANTHROPICa strict tools are WIP: canonical official requests select eligible strict schemas, classified pre-stream rejection falls back to non-strict, and an optional per-session endpoint/model state prevents repeating the rejected strict request. An Anthropic-only CLI opt-in proves the controlled custom-route fallback, one edit effect and Session journal across two prompts in one process. Official HTTPS, bounded real-model work, broader protocol parity and the full Windows gate remain open; [evidence](evidence/ai-anthropic-strict-tools.md).
- A4 AI-ANTHROPICa cross-provider tool history ID conversion is WIP: Anthropic requests now use legal, unique tool-use IDs and pair Responses composite call/result IDs without changing raw history. Controlled provider HTTP, Agent and real CLI restart tests pass; real-model acceptance and the full Windows gate remain open; [evidence](evidence/ai-anthropic-tool-id-history.md).
- A3 AGT-AGENTa (WIP): `ara-agent::Agent` now owns run state and in-memory steering/follow-up queues, with OMP's default one-at-a-time mode, optional all-mode, queue inspection/retraction and idle continuation; [evidence](evidence/agt-agent-queue.md). Queue cancellation rollback, durable enqueue receipts, production Host binding and mid-tool steering remain open.
- TOOLS-01a multi-range `read` extension (WIP): comma-separated spans sort/merge and return exact selected lines, skipped-range notices and edit provenance through the CLI tool/Session path. Buffered local UTF-8 reads and immutable `skill://` resources now add non-raw AST/lexical block boundaries. Resource text and notices retain their uncapped semantics. A bounded real-model task and the full Windows backend gate remain open; [evidence](evidence/tools-01a-multi-range-read.md).
- A2 TOOLS-02d `ast_grep` (WIP): the fixed OMP default-disabled structural search is available only through explicit CLI tool selection. Local and loaded `skill://` scopes, AST captures, global paging, parse diagnostics, cancellation and hashline source receipts have focused and real CLI fake-upstream evidence. Other internal URL transports, TUI rendering, the no-skip backend gate and a bounded live task remain open; [evidence](evidence/tools-02d-ast-grep.md).

## Priority: daily-driver v1

**User decision, 2026-09-28.** The user wants to use `ara` for daily command-line work in place of OMP as early as possible. V1 comes before the remaining Track A parity work; parity points it does not need (for example Anthropic-specific A4 work) are parked, not dropped.

**Status, 2026-09-29:** the five V1 points are accepted on `b179087` with their [audit](evidence/v1-audit.md). V1 is a host milestone; the remaining Track A parity work and P1/P3 gates continue separately.

**Routes.** V1 targets OpenAI-compatible Chat Completions (`--api openai-completions`): OpenRouter and compatible endpoints, with credentials from environment variables. An OpenAI Codex account login (Responses API with OAuth) is a later option after v1. Anthropic account login is out of scope; the existing Anthropic and proxy WIP stays as is.

**Already present** (print host, WIP or accepted per the ledger): one-shot print mode with streamed text, `--continue`/`--resume` on the JSONL Session journal, Ctrl+C abort of a print run, the six default tools plus opt-in `ast_grep`, context files and skills, MCP stdio tools, and pre-output retry.

| Point | Scope | Observable exit criteria |
| --- | --- | --- |
| V1-REPL | `ara` without a prompt on a terminal starts a line-based session: read a line, run a turn with streamed text and tool progress, repeat in one Session. Minimal commands: `/help`, `/new`, `/exit`. Not a port of OMP's TUI; the ledger records it as a reference-host subset. | A multi-turn edit-and-test task in one process with every turn in one journal; EOF and `/exit` leave a valid Session |
| V1-CANCEL | Ctrl+C during a turn aborts that turn, keeps the process and returns to the prompt; Ctrl+C at an idle prompt exits. Tool effects in flight are recorded as unknown, never replayed. | Interrupting a running `bash` tool returns to the prompt, the journal shows the abort, and the next turn works |
| V1-RESUME | `--continue` and `--resume <path>` enter the same REPL on the recovered Session. A failed resume is reported and never replaced by a new conversation. | Exit, restart with `--continue`, and the model uses the earlier turn's result |
| V1-COMPACT | Basic compaction from the A3 AGT-COMPACTION pieces: a threshold from the model's context window (or an explicit host value) and a manual `/compact`; the summary is persisted in the Session with source IDs, and resume uses it. Not full A3 parity. | A long session compacts, keeps working, and resumes after restart from the persisted summary |
| V1-TRIAL | Bounded real-model daily task through the REPL on an OpenAI-compatible route | Evidence per [acceptance](acceptance.md): artifact check, receipts, usage with unknowns kept, latency |

V1 is complete. Its initial pre-implementation estimate is retired; remaining
OMP work follows the current module execution priority above.

## Track A: OMP parity (G1)

Each slice is a set of bounded points. Every point goes through implement → fake-upstream tests → independent review → evidence → push. Every slice ends with a real-model task through `ara`.

| Slice | Surfaces | Observable exit criteria | Real-model task |
| --- | --- | --- | --- |
| A2 Coding tools and context | CA-TOOL-EDIT (replace + hashline), CA-TOOL-SEARCH (grep/glob), CA-SYSPROMPT, CA-DISCOVERY (AGENTS.md/CLAUDE.md context files), CA-EXT-SKILLS | Upstream edit/search result formats and error paths. The system prompt carries tools, context files and skills with provenance. | Fix a seeded bug in a small repo and pass its test |
| A3 Long tasks | AGT-TOKENIZER, AGT-COMPACTION, CA-COMPACTION-HOST, AGT-APPEND-CTX, AI-RETRY (replay-safe), CA-TURN-RECOVERY, CA-QUEUE, AGT-AGENT | Compaction keeps source IDs and resumes after restart. Retries never replay committed output or unknown effects. | A multi-step task that exceeds the context budget |
| A4 Providers | AI-ANTHROPIC, AI-OPENAI-RESPONSES, AI-REGISTRY, CA-MODEL-REGISTRY/PKG-CATALOG (subset), AI-STREAM-GUARDS, AI-DIALECT (subset) | Wire fixtures per protocol, cross-provider history replay, model discovery | The same task on two protocols |
| A5 Host protocols | CA-RPC, PKG-WIRE, CA-SDK, CA-CONFIG, CA-MODEL-CONTROLS | A JSON-lines RPC host drives the Core with live events, cancellation and resume | An RPC-driven task observed live |
| A6 Extensibility | CA-MCP, CA-EXT-HOOKS, CA-CAPABILITY, CA-TOOL-INTERACT, CA-ASYNC-JOBS, CA-SUBAGENT | MCP stdio/HTTP tools; hooks and approvals; background jobs; subagents | A task using an MCP tool and a subagent |
| A7 Hardening | CA-SECURITY (secret redaction), AGT-TELEMETRY, CA-LSP, CA-ACP | Secrets never enter model context or logs; spans are recorded | A task with seeded secrets |
| A8 Classification | service/host-ui surfaces (TUI, web, stats, voice …) | Each is ported, marked intentional-difference with a reason and a test, or scheduled | — |

`ported_through_commit` advances only when every selected row passes execution and audit.

### Current execution: RPC transport, then production host

**Decision, 2026-09-30:** after the bounded CTX-01e REPL/Skill Session
acceptance, port fixed `rpc-frame.ts` and `rpc-input.ts` completely as the
host-neutral `ara-rpc` transport point [RPC-01](evidence/rpc-01-transport.md).
The bounded JSON transport was accepted on `69de671` after Windows/Linux
execution and independent artifact audit; its documented runtime differences remain.
Preserve existing print/REPL, Agent/free-loop, Session and provider behavior.
Private-to-transport JS-compatible JSON semantics are necessary for UTF-16,
f64 and own-property ordering; Core string types remain unchanged.

Then bind the production RPC command host to existing Agent/Session ownership.
That next point must explicitly cover scheduling, side channels, cancellation,
native resume, command schema, stdin ownership, shutdown delivery and real
Core-driven task evidence. Transport tests do not accept CA-RPC or A5.
PKG-WIRE collaboration protocol and durable queue/lease work retain their own
owners and acceptance gates. The fixed OMP marker remains unchanged.

**Production Host investigation, 2026-09-30:** three independent Codex views
and cross-review produced the [full 42-command Host plan](rpc-host-plan.md).
The owned per-Run configuration prerequisite is implemented and Windows tested
([evidence](evidence/rpc-host-run-config.md)); it preserves a single Agent and
its queues, but does not implement mid-run live settings or any production RPC
command. The plan records additional Session/compaction/Skill interfaces and
nonblocking input/output ownership. CA-RPC/A5 and the full marker stay open.

**Production Host entry, 2026-09-30 (WIP):** the real `--mode rpc` process now
binds one persistent Agent after existing route/tool/Session initialization.
The first twelve commands cover protocol, prompt/direct images, canonical
queues/modes, live queries and joined abort/replacement. An independent stdout
actor preserves control responsiveness, while the Run sink journals completed
messages and exposes full partial events. Runtime Session adoption and owned
RPC Skill provenance were open at this entry checkpoint. The subsequent native
Session and [owned Skill point](evidence/rpc-host-skills.md) implement those
bounded paths; remaining unsupported contracts return explicit errors.
The [entry evidence](evidence/rpc-host-entry.md) distinguishes executed tests
from remaining whole-surface acceptance. Formal gates remain P0 only.

## Track B: next-generation Agent (G2), design v1

ARA features sit behind explicit Core interfaces and never weaken an OMP behavior.
Following the user's 2026-09-30 priority, new B points start after fixed OMP
parity is accepted at P3. This design remains reference material during Track A.

```text
                ┌──────────────────── Host (CLI, RPC, HandWave, Lantern, Lumen) ────────────────────┐
 user goal ───► │ Task ledger · identity/grants · scheduler/leases · artifact store · review UI     │
                └───────────────┬───────────────────────────────▲──────────────────────────────────┘
                                │ ports (typed, versioned)       │ events, receipts, decisions
                ┌───────────────▼───────────────────────────────┴──────────────────────────────────┐
                │ ARA Core                                                                          │
                │  Planner ──► Agent loop (OMP) ──► Tools ──► Receipts/effects                     │
                │     ▲             │                              │                                │
                │  Decisions   Context builder ◄── References/Memory (sourced, revisioned)         │
                │     ▲             │                              ▼                                │
                │  Verifier ◄── Omni content (text/image/audio/video/doc/sensor handles)           │
                │     │                                                                             │
                │  Feedback → candidate knowledge / tests / Skills (validated promotion)           │
                └───────────────────────────────────────────────────────────────────────────────────┘
```

| Point | Capability | Core design | Acceptance (from agent-evolution) |
| --- | --- | --- | --- |
| B1 | Task, Run and receipts | Typed `ProjectId/TaskId/SessionId/RunId/JobId` threaded through events. Each tool call gets an `EffectClass` (read, local-write, process, external, unknown) plus intent/args-hash/outcome receipts. Run outcome ≠ Task acceptance (`awaiting_review`). | An interrupted multi-step task reports the uncertain effect, resumes the same Session and leaves acceptance to review |
| B2 | Sourced references and revisable memory | A `ReferenceStore` port holding `{id, revision, supersedes, source, owner, scope, sensitivity, content-type, hash}`, with retrieval under a budget and citations. Compaction summaries record the source IDs/revisions they used. Revocation blocks reuse. | A reference is corrected, compaction and a new Run happen, and the Agent cites the current revision and can still fetch the original |
| B3 | Planning and decision records | A `plan` tool with steps, verification criteria and status, updated from observations. `DecisionRecord {question, options, choice, rationale, evidence refs}` is externally reviewable; no private chain of thought is stored. | Replanning after a failed step is visible in the journal |
| B4 | Verification and review | Verifiers are artifact checks, commands or tests declared with the Task. The Run ends `verified`/`unverified`, and a host reviewer accepts or rejects. | A task is not "done" until its artifact check passes |
| B5 | Feedback into knowledge | Corrections, rejections and successful patterns become *candidates* (knowledge, test or Skill) with source and scope. Promotion needs validation and review, and the decision is kept. | A candidate is promoted after its test passes; a rejected one never influences later runs |
| B6 | Background Jobs and timers | A host scheduler port provides `{schedule, lease, idempotency key, owner}`. Leases are renewed while active. Expiry or cancel never re-runs an uncertain external action. | Job wakes → polls → renews lease → one justified notification |
| B7 | Omni perception | Typed content parts for text, image, audio, video, document and sensor, backed by host artifact handles with hashes. Model capability negotiation comes from catalog `input` modalities. Unsupported modalities fail with an explicit error. Chinese and English text keep their provenance. Image goes first (the OMP path exists), then documents, then audio and video. | A text-plus-image task keeps both parts and their links; an audio input on a text-only model fails explicitly |
| B8 | One Core, many products | Versioned RPC/native host API (from A5). Products own identity, data and UI. | Two hosts share the Core without sharing Sessions or authority |

## Real-model validation (G3)

Before each trial:
- Query `/models` on the route.
- Record the exact model ID and protocol.
- Set bounds: at most 6 model calls, 180 s, and 1024 output tokens per call, unless the slice states otherwise.

Run `scripts/real_model_trial.sh` (extended per slice) through the `ara` binary. Record in the evidence file:
- the artifact check
- tool receipts
- usage, with unknown values kept as unknown
- latency

The initial trials used OpenRouter `openrouter/free` and Agnes `agnes-2.5-flash`. Current bounded V1/CTX trials also use B.AI `deepseek-v4.1-flash` when its live catalogue and route are available. A route failure keeps a point open when its required real-model evidence is still missing.

## Open inputs

- The user's reference documents (`ara-doc-ref`), to be pushed to the repository. The plan and Track B design will be revised against them.
- Network allowlist for `openrouter.ai` and `api.agnes-ai.cn`: both returned proxy 403 on 2026-09-25.


### RPC native Session continuation, 2026-09-30

[Session adoption evidence](evidence/rpc-host-session.md) records the bounded
new/switch slice, shared CLI setup/provider construction and exact fixed-source
semantics. The preceding entry commit's Linux gate is green (1,115/0/1, 86
suites, e2e 90/90). The module now has fourteen bounded command implementations
out of 42; full command parity and P1-P6 acceptance remain open. Different-cwd
RPC switches cancel, and saved role/model/settings plus runtime capability
reconciliation remain explicit gaps. Owned queued Skill provenance is now
accepted on `97567f0`; continue command discovery, native queries/actions and
remaining task-chain/side-channel contracts under the complete Host plan.


Final Session-slice Windows evidence: RPC child tests **21/21**, full backend
**1,120/0/1 across 86 suites**, e2e **87/87**, **71.299 s**; dependency policies
pass (**7.623 s**). Three-Run actual A/new-B/switch-A task passes in **19.341 s**,
seven calls/four receipts and exact recall after source deletion. Original,
isolated and full-gate executable hashes match. Independent final audit is
recorded in the [Session evidence](evidence/rpc-host-session.md). This remains tested WIP; pushed-commit Linux
verification and complete RPC/model/settings/capability contracts stay open.

Exact `840d2da` Linux run `36674773925` subsequently exposed a terminal/owned
Run-settlement race (RPC 20/21). At that checkpoint the immediate priority was the
narrow RPC terminal correction and its final receipts; owned Skill implementation
waited for this gate to close. The source-backed three-view
Skill design is retained without claiming implementation or advancing a phase.

The repaired Windows snapshot and deterministic mutation now pass: backend
1,121/0/1 (86 suites), all-feature RPC 21/21, e2e 87/87; actual three-Run task
18.344 s with seven calls/four receipts and exact recall. Final independent
audit and accurate repaired-commit Linux receipt were the immediate gates;
retain the complete 42-command denominator and P1-P6 acceptance boundary.

Accurate correction `9491fa1` Linux repository run `36681353101` succeeds:
1,124/0/1 (86 suites), RPC 21/21, e2e 90/90 and the deterministic oracle; backend
325 seconds. The terminal correction is closed. The reviewed owned queued Skill
point is subsequently accepted on `97567f0`: Windows 1,148/0/1, Linux 1,151/0/1,
RPC 26/26 and an independently audited three-Run actual task. Continue the
remaining full Host plan. RPC remains 14/42 bounded
implementations, with full command parity and P1-P6 still open.

The subsequent [native query/action batch](evidence/rpc-native-queries.md) adds
registered Skill discovery, raw branch selectors, bounded statistics and Session
naming. The subsequent [native fork/paging checkpoint](evidence/rpc-native-fork-paging.md)
adds real persistent/memory Session forks and stable-snapshot message paging.
RPC at that checkpoint has nineteen bounded command implementations. This combined checkpoint
is tested/audited WIP; complete CA-RPC and P1-P6 remain open. Continue the remaining
Host execution/configuration contracts in coherent batches, sharing final gates
and bounded tasks across each batch. ARA customization waits for fixed OMP parity.

The [Host tool/URI batch](evidence/rpc-host-bridges.md) adds two bounded commands
and their actual bidirectional execution, per-model-call tool/prompt refresh,
instance content routing and connection shutdown. RPC now has **21/42 bounded
implementations**. Windows focused/full/dependency gates, a three-call real
task, independent audit and exact-code Linux `36715269879` pass on `c4165e4`.
Full xdev presentation, remaining RPC commands and
P1–P6 remain open. Continue fixed OMP modules before customization.

The [native user Bash batch](evidence/rpc-native-bash.md) adds `bash` and
`abort_bash`, independent input dispatch, typed process results, original
Session/branch receipts, safe streaming flush and native restart projection.
RPC now has **23/42 bounded implementations**. Windows full/dependency gates
and a two-Run artifact/restart task pass; exact-commit Linux and final audit
receipts are recorded in the point evidence. Complete RPC and P1–P6 stay open.
Next: OMP compaction maintenance (`compact`, `set_auto_compaction`), then
Session retry recovery (`set_auto_retry`, `abort_retry`). Share gates across
each coherent module and reuse the existing child/receipt fixtures.

The [RPC soft compaction batch](evidence/rpc-soft-compaction.md) adds serial
`compact`, focused and chained summaries with cumulative raw sources, native
public projection, preserved queues and provider replay reset. Persistent
`set_auto_compaction` now gates actual supported threshold passes. Windows
full/dependency execution and the actual summary/restart artifact task pass on
`599db7c`; exact-code Linux also passes (1,286/0/1, 97 suites). The point evidence
records the final artifact/audit boundary. RPC is now
**25/42 bounded implementations**, with full maintenance and whole CA-RPC open.
Remaining compaction methods, split-turn preparation, overflow recovery,
mid-turn/idle/incomplete triggers, model-capacity resolution and settings
integration are mandatory reproduction work, alongside the remaining RPC
commands. No checkpoint substitutes for that work or starts customization.

The [native Session retry checkpoint](evidence/rpc-session-retry.md) adds
`set_auto_retry`/`abort_retry`, actual same-route recovery, durable original
error metadata and phase-specific cancellation. Windows 1,316/0/1 and the
independently audited two-Run real write/reopen task pass on `8d20531`;
exact-code Linux and final audit details remain in its evidence. RPC has
**27/42 bounded implementations**, with complete Retry and P1–P6 still open.
Next finish R2 overflow/thinking/interrupted recovery and R3/R4 native
credential/model registry, configured chains and authorized route adoption.
The complete settings/fallback/usage/cooldown matrix and every inventory gap
remain mandatory; customization still waits for complete fixed OMP parity.

The [native model/auth foundation](evidence/model-auth-foundation.md) starts
R3/R4 with exact catalogue metadata, native schema-7 storage, fixed fallback
selectors and complete per-call route/authentication ownership. A busy original
Agent adopts model/provider/options atomically on its next call while retaining
the in-flight tool snapshot and queues. This is tested foundation WIP; RPC remains
27/42 bounded implementations. Next connect full models.yml/models.db discovery,
AuthStorage precedence/OAuth/usage/rotation and native model/role/thinking journal
adoption, then fixed fallback apply/served/revert and every remaining contract.
