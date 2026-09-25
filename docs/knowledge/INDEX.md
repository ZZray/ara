# ARA project knowledge

This index is the entry point for durable decisions in the new Rust repository. Read only the pages relevant to the task. The repository begins with a documented target, not an accepted Agent implementation.

- [Core ownership and host boundary](architecture.md): portable Agent Core, host services, identity, state, and UI integration.
- [Fixed upstream baseline](upstream.md): OMP commit meanings, parity evidence, and later incremental sync.
- [References, memory, and context](context.md): durable user inputs, source provenance, revisions, retrieval, and compaction.
- [Product integration](integrations.md): AI HandWave, Lantern/Paseo, Lumen, and future hosts.
- [Verification boundary](verification.md): deterministic tests, real tasks, CAS/OpenRouter trials, audit, and acceptance.

Use [the roadmap](../roadmap.md) for planned work, [the feature ledger](../upstream/feature-ledger.md) for per-item implementation state, and `docs/evidence/` for actual test receipts. Do not copy transient progress or machine-specific secrets into this knowledge base.
