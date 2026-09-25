# Delivery roadmap

This is a new implementation, not a branch migration. No old ARA source, database, or Git ancestry is imported by this bootstrap. The fixed OMP source and the previous ARA behavior may be inspected in separate checkouts as evidence.

| Gate | Deliverable | Required proof |
| --- | --- | --- |
| P0 — baseline | Exact OMP checkout, complete behavior inventory, Rust crate/host boundaries, per-item source mapping | Pinned SHA and license verified; each intended OMP surface has an owner, test strategy, and explicit status. |
| P1 — OMP Rust Core | Messages, context, Session journal, streaming events, tool loop, cancellation, recovery, compaction, skills and knowledge loading | Same-input OMP/Rust behavior comparison, real Rust process tests, persistence/restart and negative paths. |
| P2 — OMP host and protocols | CLI/RPC, native tools, MCP/LSP where applicable, OpenAI-compatible Chat/Responses and Anthropic Messages, budgets and permissions | Actual host → Gateway → Provider → tool → journal path with controlled upstream, plus bounded real-model task trials. |
| P3 — fixed OMP parity | Every selected OMP behavior at the locked commit classified as ported, intentionally different, or open | Per-item executable evidence and independent audit; no entire gate accepted from a sample or a test count alone. |
| P4 — ARA capabilities | Sourced references and revisable memory, planning/decision evidence, background Jobs/timers, task review, Omni input | Real task artifacts, cross-Run continuity, permission/budget/lease/cancel gates, explicit unsupported-modality errors. |
| P5 — product integrations | Host APIs for AI HandWave, Lantern/Paseo, Lumen and later products | Real product UI/API workflows, distinct product identity/data, shared Core package, error feedback and resumed history. |
| P6 — release | Reproducible builds, migration/rollback where needed, security and license review | Final acceptance matrix, isolated rehearsal, authorized deployment, observed runtime behavior. |

The first implementing AI works on **P0 then P1**. Do not implement ARA-specific enhancements by weakening an OMP behavior. Product hosts may be prototyped to prove the Core interface, but their acceptance is later and separate.

Each point moves through `planned → implementing → tested → audited → accepted`. A local commit can record an unfinished step but does not advance acceptance. See [acceptance](acceptance.md).
