# ARA execution plan

**Draft v1, 2026-09-25.** This will be revised after the user's reference documents (`ara-doc-ref`) are added to the repository. It is a plan, not an implementation claim. Status lives in the [feature ledger](upstream/feature-ledger.md) and in `docs/evidence/`. Gates follow the [roadmap](roadmap.md) and [acceptance rules](acceptance.md).

## Goals

1. **G1: port OMP as the ARA Core.** Port the fixed OMP commit's Agent behavior to Rust surface by surface, with source-backed evidence. The [inventory](upstream/inventory.md) is the denominator.
2. **G2: build a next-generation task Agent on that Core.** The direction is in [agent evolution](knowledge/agent-evolution.md): move from answering to finishing tasks, with memory, reasoning, autonomous planning, decisions, execution, feedback, and unified Omni perception for text, images, audio, video, documents and sensors, in Chinese and English. This lays the ground for understanding the physical world.
3. **G3: validate against real models.** Every integration point runs a bounded real task through the actual host chain. Configured routes are OpenRouter (`openrouter/free`) and Agnes AI (`agnes-2.5-flash`, `agnes-2.5-pro`), both OpenAI-compatible. Credentials come only from the environment; each model ID is verified against the live catalogue before a trial.

## Where we are

P0 inventory is done. P1 slice 1 is tested with a controlled upstream:

| Point | Crate | What it covers |
| --- | --- | --- |
| AI-01 | `ara-ai` | Message model and Chat Completions adapter |
| AGT-01 | `ara-agent` | Agent loop |
| SES-01 | `ara-session` | Session journal |
| TOOLS-01 | `ara-tools` | `read` / `write` / `bash` |
| TOOLS-02 (A2) | `ara-tools` | `grep` / `glob` with the pi-walker ignore chain |
| CLI-01 | `ara-cli` | Print host |

None is accepted: the real-model trial is still blocked by the environment's network policy.

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

Use OpenRouter `openrouter/free` as the primary route and Agnes `agnes-2.5-flash` as the second. A route failure keeps the point open.

## Open inputs

- The user's reference documents (`ara-doc-ref`), to be pushed to the repository. The plan and Track B design will be revised against them.
- Network allowlist for `openrouter.ai` and `api.agnes-ai.cn`: both returned proxy 403 on 2026-09-25.
