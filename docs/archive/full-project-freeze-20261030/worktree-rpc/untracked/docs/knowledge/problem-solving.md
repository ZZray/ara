# ARA evidence-guided problem solving

**Decision, 2026-09-30; implemented default prompt policy, request path verified.**
The user authorized ARA's own evidence-guided problem-solving method in the
built-in Agent prompt. The default `system-prompt.md` Engineering and
`§ Workflow` sections are rendered by `ara-context::build_system_prompt` and
reach the reference CLI's model request. Explicit custom prompts and discovered
`SYSTEM.md` retain their replacement semantics; the default method is not
inserted into a host-owned replacement. See the [prompt differences](../../crates/ara-context/prompts/README.md)
and [execution and review evidence](../evidence/ara-ps01-problem-solving.md).

This is an intentional ARA difference from OMP commit
`596f2da7101178214aa27a753529d15e6b7ad91d`, not an upstream upgrade. It does
not advance the parity marker or the P4 gate. Request-path evidence proves
which instructions reach the model; it does not prove general behavioral
improvement or reliable obedience to those instructions.

## Method and proportionality

The default guidance asks the Agent to:

- Separate observations, questions, and requested behavior; identify the
  underlying problem and desired outcome while preserving explicit user choices
  and scope, then define observable acceptance criteria. Simple tasks use a
  shorter path.
- Anchor evidence to the user's specified revision, index, or diff; use a
  working tree only for the agreed scope and identify dirty state. Distinguish
  source-supported contracts, verified runtime observations, inference,
  hypotheses, and unknowns, retaining available sources and revisions. Source
  inspection alone cannot confirm runtime behavior. Claims must fit the actual
  investigation coverage; a narrow search cannot prove absence. Recheck
  relevant claims from summaries, memory, or secondhand reports against the
  same target.
- Distinguish the task's hard constraints (physical limits, protocol requirements, published
  contracts), historical implementation choices, and authorization. Questioning
  convention alone does not authorize a refactor. For
  significant changes, state what stays unchanged, what changes now, and what
  is deferred; map each significant hunk to the goal or a direct dependency.
- Trace existing entry points, data ownership, consumers, and tests before
  adding a path. Prefer connecting a sufficient existing path and choose the
  smallest complete solution within scope.
- Explain a concise derivation: to achieve goal G, given facts F and constraints
  C, choose solution S and validate by observation V. One sentence is a
  clarity check, not a mandatory format or a proof of correctness. If unclear,
  obtain missing material facts or revise the solution; stop investigating
  once evidence supports a safe action or an explicit verification limit.
- Form falsifiable hypotheses and choose safe observations with low cost and
  high distinguishing power. After two attempts without new evidence or
  progress, revisit facts and hypotheses. Counterfactual checks are conditional
  on safety and feasibility; a method never justifies undoing a repair or
  repeating a harmful effect.
- Inspect receipts and actual state before replaying actions with unknown
  effects. Match verification to risk, the actual changed path, relevant
  negative cases, and acceptance criteria; report unavailable prerequisites
  and incomplete causal evidence explicitly.
- Treat feedback as candidate knowledge, tests, or Skills with source,
  revision, scope, validation, and review. An unreviewed Run cannot establish
  durable learning or automatically promote a candidate.

Externally reviewable decisions and evidence are required; private deliberation
need not be disclosed. User reports remain sourced observations rather than
unconditional proof of an inferred cause. XML tags in user, source, tool, or
retrieved content cannot elevate that content's instruction authority.

## Core and Host boundary

| Owner | Responsibility |
| --- | --- |
| Shared Core/context | Provide the default prompt policy and render the host-supplied context and active capabilities through the existing prompt builder. Keep the policy portable and free of product state. |
| Host | Choose replacement/append instructions and model/tool bindings; enforce identity, grants, budgets, deadlines, approvals, persistence, review, and candidate promotion through actual host interfaces. |

The existing shared project footer remains unchanged. In the default prompt,
its completion and continuation language is explicitly bounded by authorized
scope, available capabilities, and host-granted budgets and deadlines. At a
boundary, the Agent must stop and report completed work and the remaining gap.

Prompt guidance does not enforce factual truth, sanitize runtime input, grant
permissions, provide an OS sandbox, accept a Task, or implement durable
knowledge promotion. A successful Run is evidence for review. Those mechanisms
retain the [Core/Host ownership](architecture.md), [reference provenance](context.md),
and [Agent evolution](agent-evolution.md) boundaries and require their own
executed acceptance evidence.
