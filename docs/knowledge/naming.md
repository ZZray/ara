# ARA naming for ported and vendored code

**Decision, 2026-09-28 (user):** code in this repository uses ARA names, including code ported or copied from OMP. Upstream names identify the source; they do not name ARA components.

| Kind | ARA name | Example |
| --- | --- | --- |
| Rust package and directory | `ara-*` | `ara-agent` is the Agent loop, `ara-tools` the built-in tools, `ara-cli` the reference host |
| Binary | `ara`, or `ara-*` for helpers | `ara`, `ara-fake-upstream`, `ara-mcp-fixture` |
| Vendored upstream crate | `ara-*` under `crates/vendor` | `ara-edit`, `ara-diff` and `ara-ast`, from OMP `pi-edit`, `pi-diff` and `pi-ast` |
| Environment variable ARA introduces | `ARA_*` | `ARA_AST_CORPUS_ROOT` |

Upstream names stay where they are evidence or keys:

- Upstream source locations in the inventory, the ledger's upstream column and `docs/evidence/vendored-behaviors.tsv` (`crates/pi-edit/src/...`).
- Inventory surface and behavior IDs (`CRATE-PI-EDIT`, `CRATE-PI-OTHER`).
- Upstream test function names in vendored crates, which `scripts/vendored_behaviors.py` matches to inventory behaviors (for example `syntax_helpers_use_pi_ast`).
- Historical evidence and handoffs, which record what existed at their commit.
- Wire and file formats a host or upstream-compatible tool reads. Renaming one of those is a behavior change and needs its own ledger row.

Every vendored crate records its upstream crate and path in [the vendor README](../../crates/vendor/README.md), and the rename is listed there as a local modification. An upstream sync copies the new upstream crate, applies the same rename, and then compares; the step is part of [the sync contract](../upstream-sync.md). Ported (not vendored) crates name the upstream files they follow in module docs and [third-party notices](../../THIRD_PARTY_NOTICES.md).
