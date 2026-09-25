---
name: project-knowledge-maintenance
description: Maintain durable ARA Rust architecture decisions, source-backed constraints, and the indexed project knowledge base.
---

# Maintain project knowledge

1. Read `docs/knowledge/INDEX.md`, then only relevant pages and the current code or tested evidence. Use the page that already owns a topic; create a new page only for a distinct durable decision.
2. Separate a requirement, design target, implemented behavior, and accepted behavior. Give an evidence source and date. Never turn another worker's report, an old ARA claim, or an unrun test into acceptance.
3. Put reusable project facts, non-obvious boundaries, root causes, and verification traps in `docs/knowledge/`. Put transient status in `docs/evidence/` or a handoff. Put machine-specific routes and secrets outside Git. Keep contributor rules in `AGENTS.md` and repeatable procedures in a Skill.
4. Update the index link in the same change. Check local links, scoped diff, and `git diff --check`; run `python scripts/verify_bootstrap.py` if the documentation structure or links changed.
5. State what was verified and what remains only a plan. Documentation validation is not Agent behavior testing.
