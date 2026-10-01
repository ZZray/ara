# Delivery roadmap

The [terminal recovery and configured promotion checkpoint](evidence/terminal-recovery-promotion.md)
adds safe nonempty Length summaries, native empty-terminal continuation and
same-Agent configured route preparation with atomic journal metadata. Modules
68/0/0 and Windows 1,519/0/20 pass one stable 137.539s gate. The preferred CAS
task/reopen passes in 11.573s after a controlled empty Stop. Independent review
and Root artifact audit pass. Split-turn, remaining terminal/Host callers,
methods/rescue and recorded Provider contracts remain open. P0/V1 accepted,
P1–P6 open, RPC 27/42 and the null full marker retain their status.

The [native recovery/fold checkpoint](evidence/native-recovery-fold.md) adds
typed Responses context-recovery evidence, raw-reserve summary output budgets
and bounded message-window folding. Modules 53/0/0 and Windows backend
1,512/0/20 pass a single stable gate (338.253s, including 203s all-target
recompilation). Preferred CAS task/fold/continuation/reopen takes 29.558s,
with two actual summary calls and one complete-source commit. Independent
POST and Root artifact audit pass; all parent surfaces, P1–P6, RPC 27/42 and full marker null
retain their status. Remaining terminal recovery, promotion and other methods
are grouped next; input/tokenizer/settings/receipt limitations stay recorded.

The [soft overflow checkpoint](evidence/soft-overflow-recovery.md) connects
configured capacity, failed-turn rollback and cancellable same-route soft
maintenance. Modules 18/0/0 and one final Windows backend 1,508/0/20 pass;
full command 287.203s (149s all-target recompilation). Final preferred CAS
summary/new concurrent Bash/continuation/reopen task takes 31.523s and is
independently audited. Full native summary budgets, Responses recovery evidence,
terminal stops, promotion, other methods and rescue remain mandatory. P0/V1
accepted, P1–P6 open, RPC 27/42 and the full marker null are unchanged.

This is a new implementation, not a branch migration. No old ARA source, database, or Git ancestry is imported by this bootstrap. The fixed OMP source and the previous ARA behavior may be inspected in separate checkouts as evidence.

| Gate | Deliverable | Required proof |
| --- | --- | --- |
| P0 — baseline | Exact OMP checkout, complete behavior inventory, Rust crate/host boundaries, per-item source mapping | Pinned SHA and license verified; each intended OMP surface has an owner, test strategy, and explicit status. |
| P1 — OMP Rust Core | Messages, context, Session journal, streaming events, tool loop, cancellation, recovery, compaction, skills and knowledge loading | Same-input OMP/Rust behavior comparison, real Rust process tests, persistence/restart and negative paths. |
| P2 — OMP host and protocols | CLI/RPC, native tools, MCP/LSP where applicable, OpenAI-compatible Chat/Responses and Anthropic Messages, budgets and permissions | Actual host → Gateway → Provider → tool → journal path with controlled upstream, plus bounded real-model task trials. |
| P3 — fixed OMP parity | Complete required behavior inventory at the locked commit reproduced in Rust; necessary platform adaptations preserve observable behavior | Every required feature implemented, exercised and independently audited; no required feature remains open or is replaced by an intentional-difference label. No entire gate accepted from a sample or a test count alone. |
| P4 — ARA capabilities | [Agent evolution](knowledge/agent-evolution.md): sourced references and revisable memory, planning/decision evidence, reviewed feedback, background Jobs/timers, task review, Omni input | Real task artifacts, cross-Run continuity and correction, permission/budget/lease/cancel gates, explicit unsupported-modality errors. |
| P5 — product integrations | Host APIs for AI HandWave, Lantern/Paseo, Lumen and later products | Real product UI/API workflows, distinct product identity/data, shared Core package, error feedback and resumed history. |
| P6 — release | Reproducible builds, migration/rollback where needed, security and license review | Final acceptance matrix, isolated rehearsal, authorized deployment, observed runtime behavior. |

**Priority milestone, user decision 2026-09-28: daily-driver v1.** Before the remaining parity slices, make `ara` usable for daily command-line work in place of OMP: a line-based interactive session with streaming output, Ctrl+C that cancels the current turn, Session resume, the existing tools and basic compaction, on OpenAI-compatible Chat Completions routes (OpenRouter and compatible endpoints). An OpenAI Codex account login (Responses with OAuth) is a later option; Anthropic account login is out of scope. V1 is a host milestone, not a gate: it draws points from P1/P2 surfaces, and each point keeps its own ledger row, evidence and audit. Scope and exit criteria are in the [execution plan](plan.md#priority-daily-driver-v1).

**Current priority, user decision 2026-09-30:** finish fixed OMP behavior parity
before adding ARA customization. V1 is already accepted. Work in module batches,
run per-point focused checks, and share final snapshot gates across the completed
batch. New P4/Track B and product-specific additions wait for P3. Preserve prior
completed work and unrelated WIP. See the [current execution priority](plan.md#current-priority-reproduce-omp-before-customization).

**User clarification, 2026-09-30:** complete reproduction is required, with no
feature omission. Open/partial rows are work still to finish. Earlier V1
exclusions apply to that early host milestone only; they do not remove fixed
OMP features from the full reproduction. P3 must close the required inventory
before customization starts.

**Provider ordering update, user decision 2026-10-01:** first deliver custom
OpenAI-compatible protocol/configuration, then OpenAI account login for daily
use. Remaining Provider compatibility is explicitly recorded and deferred;
see the [provider plan](provider-plan.md). Reuse fixed OMP test inputs, verify
by module and use a single final batch gate. The preferred live trial is the
current local Manager → OMP management → CAS `deepseek-v4.1-flash` route.
Deferred provider contracts are not accepted parity. Core/Session/RPC and
P0–P6 requirements retain their existing ownership and acceptance boundaries.

The [OpenAI daily checkpoint](evidence/openai-daily.md) now verifies configured
Chat/Responses CLI routes and synthetic device/Codex SSE lifecycle with one
shared Windows gate (module 22/0/0; backend 1,491/0/20; 134.536 seconds), plus
a configured answer-free CAS artifact/restart task (7.032 seconds). Actual OpenAI account
authorization and a subscription-model task remain open. [Usage](openai-daily.md)
and the provider register describe the usable slice and required later contracts.

The [ThinkingLoop recovery checkpoint](evidence/thinking-loop-recovery.md)
adds fixed stream detection, native switches/notices and original-Session
continuation. Windows modules 24/0/0 and backend 1,499/0/20 pass with one
114.925-second shared gate; actual CAS task/restart takes 11.338 seconds across
Runs. Final source/binary/artifacts are independently audited. Other stream
guards, overflow/interrupted recovery and native special caps remain required.
This adds behavior to existing RPC variants; RPC stays 27/42, P1–P6 stay open
and the full parity marker stays null.

The [tool-call-loop checkpoint](evidence/tool-call-loop-guard.md) adds fixed
cross-turn detection, native Host switches and durable notices before steering.
Modules 20/0/0 and Windows backend 1,503/0/20 pass one 194.995-second shared gate;
preferred CAS file task/original-Session recall takes 9.011 seconds across Runs.
Lossless UTF-16 model-text projection, advisor/other Hosts and remaining guards
stay mandatory open work. This checkpoint accepts no full parent surface.

The ordered slices and next-generation design are in the [execution plan](plan.md). The first implementing AI works on **P0 then P1**. Do not implement ARA-specific enhancements by weakening an OMP behavior.

Each point moves through `planned → implementing → tested → audited → accepted`. A local commit can record an unfinished step but does not advance acceptance. See [acceptance](acceptance.md).

The [native Session retry checkpoint](evidence/rpc-session-retry.md) records
`set_auto_retry`/`abort_retry`, same-route recovery and sourced native metadata
on `8d20531`. Windows 1,316/0/1 and an independently audited real write/reopen
task pass; platform/audit details are in the evidence. RPC has **27/42 bounded
implementations**. Complete Retry, CA-RPC and P1–P6 remain open. Finish all
remaining overflow/thinking/interrupted, credential/OAuth/usage, model/fallback/settings
and remaining inventory contracts before ARA customization; none is omitted.
