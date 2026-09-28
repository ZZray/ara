# Vendored OMP crates

These crates are copied from [Oh My Pi](https://github.com/can1357/oh-my-pi) at the pinned commit `596f2da7101178214aa27a753529d15e6b7ad91d` (`upstream/omp.lock.json`), under the MIT license (`THIRD_PARTY_NOTICES.md`, `ara-ast/LICENSE`). They follow ARA naming ([naming rule](../../docs/knowledge/naming.md)): each package and directory is renamed from `pi-*` to `ara-*`. Otherwise they keep upstream sources, tests, fixtures and formatting. Upstream formats them with a nightly rustfmt configuration, copied here as `rustfmt.toml`. `scripts/verify_backend.py` checks formatting only for ARA-owned packages, so re-syncing stays a plain diff against upstream once the rename is applied.

| Crate | Upstream crate and path | Used for |
| --- | --- | --- |
| `ara-diff` | `pi-diff`, `crates/pi-diff` | Line and structured diffs |
| `ara-ast` | `pi-ast`, `crates/pi-ast` | tree-sitter block ranges, parse checks and summaries (all upstream grammars) |
| `ara-edit` | `pi-edit`, `crates/pi-edit` | The edit engine behind the `edit` tool (all five modes), the snapshot store, and hashline formatting |

## Local modifications

Each code change is marked in the source with an `// ARA:` comment, except the mechanical rename in the first bullet.

- **Names (2026-09-28):** directories and packages `pi-diff`/`pi-ast`/`pi-edit` are `ara-diff`/`ara-ast`/`ara-edit`; every crate path (`pi_diff::`, `pi_ast::`, `pi_edit::`, including `use` items and doc comments) and the `-p pi-ast` command in `ara-ast`'s ignored full-corpus sweep follow. Upstream test function names are unchanged (for example `syntax_helpers_use_pi_ast`), because `scripts/vendored_behaviors.py` matches them to inventory behaviors by upstream path and name. To re-sync, copy the new upstream crate, apply the same rename, then diff (or reverse the rename and diff against upstream).
- **Manifests:** `authors`, `repository` and `[lints] workspace = true` were removed, because ARA has no workspace lint table. `publish = false` is inherited.
- **`ara-edit` suffix recovery:** `src/path_policy.rs` `recover_missing` walks with `ara-walk` (ARA's port of the pi-walker ignore-state walk) instead of `pi_walker::WalkRequest`. The options, the `**/<suffix>` glob, the two-entry limit and the 5 s budget are unchanged. `pi-walker` is not vendored.
- **`ara-edit` NFC:** `src/text.rs` uses `unicode-normalization` for NFC instead of `xutf`, which needs nightly `portable_simd`. ARA builds on stable.
- **`ara-edit` lossy decodes:** in `src/files.rs`, `FileRead.lossy` marks a file that is not valid UTF-8, and `FileRead::persist` refuses to re-encode it. This happens while staging, so nothing is written. Upstream writes U+FFFD over every undecodable byte.
- **`ara-edit` Windows tag recovery:** in `src/path_policy.rs`, `allow_tag_path_recovery` compares a verbatim drive path (`\\?\C:\…`, what `canonicalize` returns for the cwd) and a plain drive path (`C:\…`, what `canonical_key` stores for snapshots) in the plain form. Upstream compares the two spellings directly, so on Windows a canonicalized cwd rejected every recovery target. In `src/modes/hashline/patcher.rs`, `recover_target` drops the authored target's own snapshot by comparing its `canonical_key`, not its verbatim absolute path. Without this, a file deleted outside the edit tool would recover onto itself and report its absolute path, and a same-content nested copy would become a second, ambiguous candidate. Verbatim UNC and device paths, outside-cwd denial and internal-URL denial are unchanged; `canonical_key`'s textual prefix strip for `\\?\UNC\` keys is not addressed.
- **`ara-edit` hashline `MV`:** in `src/modes/hashline/patcher.rs`, `stage_patch` refuses a destination that already exists, with patch mode's wording (`Cannot move X to Y: destination already exists.`), unless the same payload `REM`s it earlier. Upstream overwrites the destination.
- **`ara-ast` corpus tests:** in `src/block.rs`, the repository-corpus tests use the repository root one level higher, or `ARA_AST_CORPUS_ROOT`. The sample sweep skips itself unless that root has OMP's `packages/` directory. Its evidence run points it at the pinned OMP checkout: `python scripts/vendored_behaviors.py --corpus <omp>`.
- **`BUILD.bazel` files:** omitted.
- **`Cargo.lock`:** pins `tree-sitter-cmake 0.7.4` and `tree-sitter-language 0.1.7`, the versions in OMP's lock, so parse trees match upstream.

## Evidence

`python scripts/vendored_behaviors.py` runs the three crates' test suites. It maps every inventory behavior of these crates (upstream surfaces CRATE-PI-EDIT and CRATE-PI-OTHER/pi-ast, keyed by upstream path) to its executed test, and writes `docs/evidence/vendored-behaviors.tsv`.
