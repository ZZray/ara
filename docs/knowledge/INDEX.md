# ARA project knowledge

This index is the entry point for durable decisions in the new Rust repository. Read only the pages relevant to the task. The repository begins with a documented target, not an accepted Agent implementation.

- [Core ownership and host boundary](architecture.md): portable Agent Core, host services, RPC transport ownership, native Session replacement/retry, complete model routes and private request authentication, user Bash targets and retained writers, tool snapshots and connection URI admission, identity, state, and UI integration.
- [Fixed upstream baseline](upstream.md): OMP commit meanings, parity evidence, and later incremental sync.
- [ARA naming](naming.md): `ara-*` crates, binaries and `ARA_*` variables for ported and vendored code; where upstream names stay; the rename step in each sync.
- [References, memory, and context](context.md): durable user inputs, source provenance, revisions, retrieval, native RPC compaction sources, raw entry groups/model fragments, checked local rewrites/usage anchors, Session artifacts, handoff document/cache identity/cancellation and Skill source configuration.
- [Evidence-guided problem solving](problem-solving.md): ARA's default prompt method, replacement semantics, and the boundary between model guidance and host enforcement.
- [ARA Agent evolution](agent-evolution.md): task completion, feedback-driven improvement, bounded autonomy, Omni input, and cross-product reuse after OMP parity.
- [Product integration](integrations.md): AI HandWave, Lantern/Paseo, Lumen, and future hosts.
- [Verification boundary](verification.md): deterministic tests, real tasks, CAS/OpenRouter trials, audit, and acceptance.

Use [the roadmap](../roadmap.md) for planned work, [the feature ledger](../upstream/feature-ledger.md) for per-item implementation state, and `docs/evidence/` for actual test receipts. Do not copy transient progress or machine-specific secrets into this knowledge base.
