# ARA execution plan

**Draft v1, 2026-09-25.** This will be revised after the user's reference documents (`ara-doc-ref`) are added to the repository. It is a plan, not an implementation claim. Status lives in the [feature ledger](upstream/feature-ledger.md) and in `docs/evidence/`. Gates follow the [roadmap](roadmap.md) and [acceptance rules](acceptance.md).

## Goals

1. **G1: port OMP as the ARA Core.** Port the fixed OMP commit's Agent behavior to Rust surface by surface, with source-backed evidence. The [inventory](upstream/inventory.md) is the denominator.
2. **G2: build a next-generation task Agent on that Core.** The direction is in [agent evolution](knowledge/agent-evolution.md): move from answering to finishing tasks, with memory, reasoning, autonomous planning, decisions, execution, feedback, and unified Omni perception for text, images, audio, video, documents and sensors, in Chinese and English. This lays the ground for understanding the physical world.
3. **G3: validate against real models.** Every integration point runs a bounded real task through the actual host chain. Trials have used OpenRouter (`openrouter/free`), Agnes AI (`agnes-2.5-flash`, `agnes-2.5-pro`) and B.AI (`deepseek-v4.1-flash`) on OpenAI-compatible routes. Credentials come only from the environment; each model ID is verified against the live catalogue before a trial.

## Where we are

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
- CTX-01d remains implementing (WIP): system prompt, date/cwd reminder, and `skill://` tool routes have controlled host and real-model evidence. Fixed-OMP Bun oracles for Windows `zh-CN` and Linux `en-US`, a narrow ICU4X repair, Windows and Linux backend gates, independent review and two delivered-code B.AI tasks are recorded in the [sort evidence and point audit](evidence/ctx-01d-skill-directory-sort.md). WIP `acf79e9` failed Linux CI in the new oracle test's platform-specific index; WIP `34a2360` corrected it and Linux CI passed. Same-host Bun/Rust oracle CI 36607978940 now passes on Windows en-US, macOS en-US and Linux en-US with broader Unicode fixtures, alongside local Windows zh-CN evidence. The full CTX-01d point remains open: the point re-audit requested error propagation for skill directory enumeration and explicit decisions on remaining resource boundaries; the narrow error-path repair is WIP e96dc5a with green cross-platform and repository CI. Do not advance P1 or P3 from this slice. See the [CTX-01d evidence](evidence/ctx-01d-system-prompt.md) and [current handoff](handoffs/2026-09-30.md).
- Open: CTX-01e (`/skill:` invocation), which needs the interactive/RPC host and session custom messages.
- A4 MODEL-01a (WIP): opt-in `--api proxy-auto` selects the exact model's Anthropic or OpenAI Chat endpoint from a dual-protocol proxy before Session creation. Controlled CLI tests cover a tool effect, Session receipts and fail-closed discovery; model capability metadata, source-bound resume, the full Windows gate and a bounded live task remain open. [Evidence](evidence/model-01a-proxy-protocol.md).
- A3 preparation (WIP): the isolated AGT-TOKENIZERa text/message estimate is under implementation; [evidence](evidence/agt-tokenizer-estimate.md). The initial byte-based budget probe was removed after pinned Claude fixtures disproved its claimed token upper bound. CTX-01d still gates A2 acceptance, and A3 compaction/long-task behavior is not claimed.
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

**Estimate:** 4–8 working weeks, low confidence: REPL, cancel and resume 1–2 weeks; basic compaction 2–4 weeks (its persistence and resume are still open in A3); hardening and the real-model trial 1–2 weeks. A full OMP replacement (A3–A8 parity) remains 4–8+ months.

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

## Track B: next-generation Agent (G2), design v1

ARA features sit behind explicit Core interfaces and never weaken an OMP behavior. B points start once A3 is tested, because they need compaction and recovery. B1 foundations may start earlier as additive types.

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
