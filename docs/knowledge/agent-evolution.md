# ARA's Agent evolution direction

**Product direction, 2026-09-25; planned, not implemented or accepted.** OMP at the [fixed commit](../upstream-sync.md) is the behavioral starting point. ARA's purpose is a shared Agent Core that helps people **finish tasks over time**, not only answer one prompt. After source-backed OMP parity, ARA can add its own capabilities without silently reducing upstream behavior. Claude Code and Codex are design references to study and test, not claims of compatibility or permission to copy their private implementations.

## Principles

1. **Continuity with provenance.** Keep the user's original inputs, references, corrections, and tool receipts retrievable across turns, summaries, and Runs. Summaries are replaceable views; source revisions are the evidence. Retrieval must show which version was used, respect access/revocation, and say when relevant context was omitted. See [references and context](context.md).
2. **Reason toward a verifiable outcome.** An Agent may propose a plan, revise it after observations, choose tools, and explain externally reviewable decisions. A plan is not success. A Task is complete only after its requested artifact/effect and failure conditions are checked and reviewed; a Run result is evidence, not automatic acceptance.
3. **Act with bounded authority.** The host grants capabilities, budgets, deadlines, and approval routes. The Core records intent, action, receipt, cancellation, and uncertain effects. Background Jobs, timers, polling, and notifications belong to a durable host scheduler; active work needs lease renewal until it ends. An expired lease or a successful HTTP response cannot manufacture completion or permission to replay a tool.
4. **Improve from observed feedback.** Corrections, rejected outputs, successful task patterns, and review findings can become candidate knowledge, preferences, tests, or Skills with source and scope. Promotion to durable shared behavior needs validation and the appropriate user/project review. Do not let model speculation, retrieved text, or an unreviewed run rewrite Core rules, permissions, or accepted knowledge.
5. **Typed Omni perception and expression.** Treat text, images, audio, video, and future sensor or physical-world observations as typed, sourced inputs with host-managed artifacts. Support Chinese and English inputs without losing provenance. A modality is accepted only when its parser, model path, permissions, and real task behavior have been exercised; unsupported forms return an explicit limitation. Unified representation should make cross-modal references usable, not imply every modality already works.
6. **One Core, multiple products.** AI HandWave, Lantern/Paseo, Lumen, and later projects bind the same Core through native/RPC host interfaces. Each host owns identity, data, authorization, lifecycle, and UI. A frontend shows real events and receipts; it does not duplicate the Agent engine. See [integration boundaries](integrations.md).

## Evolution loop

```text
User goal and sourced references
  → Task plan and bounded authority
  → model/tool action with live events
  → artifact and state verification
  → explicit review and feedback
  → sourced memory, test, or Skill candidate
  → validated promotion for later Tasks
```

Each arrow is an observable contract. A candidate may be rejected, revised, or kept local to one Session. The system must retain the original evidence and the decision to promote or reject it. This is a product design loop, not autonomous model retraining or a claim that current code implements self-improvement.

## Acceptance examples for later slices

- A user supplies a reference and then corrects it. After context compaction and a new Run, the Agent uses the current revision, cites its source, and can retrieve the original; revocation prevents reuse.
- A multi-step task survives a failed tool or interrupted Run. The Agent reports the uncertain effect, resumes the same Session safely, produces the requested artifact, and leaves Task acceptance to review.
- A scheduled Job wakes, polls an external result, renews its active lease, and sends one justified notification. Cancellation or expiry cannot silently rerun an uncertain external action.
- A text-plus-image task preserves both input types and their source links. An unsupported audio/video/sensor path fails explicitly rather than pretending to understand it.
- The same Core serves separate HandWave, Lantern, and Lumen hosts without sharing a user's private Session or granting cross-product authority.

These examples require executed host and product tests under [acceptance rules](../acceptance.md). The first implementation remains the fixed OMP inventory and Rust parity; ARA-specific evolution is a later, separately reviewed gate in [the roadmap](../roadmap.md).
