# ARA project Skills

- [Project knowledge maintenance](project-knowledge-maintenance/SKILL.md): update durable project decisions and the knowledge index without claiming untested implementation.
- [Fixed OMP sync](omp-commit-sync/SKILL.md): inventory the pinned commit and later compare exact old/new SHAs with source, Rust, and test evidence.
- [Point delivery audit](point-delivery-audit/SKILL.md): run real behavior checks, inspect artifacts and failure paths, review the scoped diff, and record an acceptance decision.
- [Git change review](ara-git-review/SKILL.md): select the exact diff or revision, optionally use local OCR for deterministic scope, and report confirmed defects with coverage.
- [Rust Agent Core review](ara-rust-core-review/SKILL.md): inspect state, async execution, tool effects, persistence, permissions, and context provenance.
- [Provider wire review](ara-provider-review/SKILL.md): inspect OpenAI-compatible and Anthropic frames, streams, usage, errors, and tool correlation.
- [Backend verification](ara-backend-verification/SKILL.md): execute compiler/lint/tests, real host effects, relevant faults, and bounded model tasks before acceptance.

These are project workflows, not evidence that an Agent feature is implemented. `ara-git-review` is read-only; `ara-rust-core-review` and `ara-provider-review` add domain checks to its exact scope. `ara-backend-verification` runs applicable tests and feeds `point-delivery-audit`. Read only the matching Skills.
