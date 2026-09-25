# ARA

ARA is a Rust Agent Core for completing tasks across personal AI products. Its first implementation target is a source-backed Rust port of [Oh My Pi](https://github.com/can1357/oh-my-pi) at the fixed commit `596f2da7101178214aa27a753529d15e6b7ad91d` (v18.1.8). After that baseline is tested, ARA adds its own memory, planning, background execution, and typed multimodal input.

The Core is shared code. AI HandWave, a customized [Paseo](https://github.com/getpaseo/paseo) frontend in [Lantern](https://github.com/ZZray/Lantern), Lumen, and later products connect through host/RPC interfaces. Each product keeps its own identity, data, permissions, and lifecycle.

**Current status:** repository bootstrap only. No Rust Agent runtime, OMP feature, product integration, or real-model trial is implemented or accepted in this repository. The earlier Go/Rust development tree is reference material, not this repository's history.

## Start here

1. Read [AGENTS.md](AGENTS.md) and [project knowledge](docs/knowledge/INDEX.md).
2. Read the [delivery roadmap](docs/roadmap.md), [OMP sync contract](docs/upstream-sync.md), and [acceptance rules](docs/acceptance.md).
3. Run `python scripts/verify_bootstrap.py` to check this documentation bootstrap. `python scripts/verify_backend.py` reports `NOT RUN` until a Rust backend exists, then runs baseline formatting, lint, and tests.
4. The implementing AI starts at the exact SHA in [upstream/omp.lock.json](upstream/omp.lock.json), builds a source-to-test inventory, then ports and verifies one complete behavior at a time.

The bootstrap verifier checks repository structure and links. It is **not** a product test. The backend CI check runs when Rust code exists; actual host effects, failure paths, dependency policy, and bounded real-model tasks still need point-specific evidence before delivery or acceptance.

## Scope

| Layer | Responsibility |
| --- | --- |
| Shared Rust Core | Message and Session semantics, context, tools, events, memory/reference provenance, planning, decisions, execution receipts, cancellation, and recovery contracts. |
| Rust host adapters | Product identity, storage, authorization, Gateway access, scheduling/leases, UI/RPC/MCP transport, and review. |
| Product frontends | AI HandWave and Lantern/Paseo present state and input through host APIs; they do not copy the Core or own the task ledger. |

Initial provider wire focus is OpenAI-compatible Chat Completions/Responses and Anthropic Messages. ARA Manager's OMP management offers a local CAS route for bounded real-model trials; OpenRouter free models may be used as an alternative after verifying the current model ID and protocol. Model access is configured outside Git; see [.env.example](.env.example) and [testing guidance](docs/acceptance.md).

See [provider trial setup](docs/testing-providers.md) for a bounded local CAS or OpenRouter check. Even a free-tier API key stays out of the repository.

## Repository map

- [`upstream/omp.lock.json`](upstream/omp.lock.json): immutable initial upstream marker and separate ported-through marker.
- [`docs/upstream-sync.md`](docs/upstream-sync.md): old-SHA to new-SHA incremental sync procedure.
- [`docs/roadmap.md`](docs/roadmap.md): implementation order and delivery boundaries.
- [`docs/acceptance.md`](docs/acceptance.md): per-point real test and audit requirements.
- [`docs/knowledge/INDEX.md`](docs/knowledge/INDEX.md): durable architecture and product knowledge, loaded on demand.
- [`.ara/skills/INDEX.md`](.ara/skills/INDEX.md): repeatable contributor workflows.
- [`docs/handoff-prompt.md`](docs/handoff-prompt.md): ready-to-use prompt for the first Rust implementation AI.
- [`docs/evidence/README.md`](docs/evidence/README.md): executed test receipt template for later delivery points.

## Skills and review status

The seven [project Skills](.ara/skills/INDEX.md) cover knowledge maintenance, fixed OMP sync, Git/Rust/Provider code review, backend execution, and per-point audit. `AGENTS.md` directs contributors to the applicable combination. They are contributor workflows, not an implemented Rust Skill loader. The previous ARA project's knowledge Skill and the installed local generic/OCR review workflows informed these focused versions; no Go-specific runtime instructions or reviewer binary were copied. An implementation point must record which review and tests actually ran. See [tooling evidence](docs/evidence/review-tooling-bootstrap-20260925.md).

The new Git root deliberately has no ancestry from the previous ARA repository. Upstream OMP is consulted in a separate checkout; imported upstream code must retain its license and attribution. See [third-party notices](THIRD_PARTY_NOTICES.md).
