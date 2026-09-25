# Vendored OMP crates

These crates are copied from [Oh My Pi](https://github.com/can1357/oh-my-pi) at the pinned commit `596f2da7101178214aa27a753529d15e6b7ad91d` (`upstream/omp.lock.json`), under the MIT license (`THIRD_PARTY_NOTICES.md`, `pi-ast/LICENSE`). They keep their upstream package names, sources, tests, fixtures and formatting. Upstream formats them with a nightly rustfmt configuration, copied here as `rustfmt.toml`. `scripts/verify_backend.py` checks formatting only for ARA-owned packages, so re-syncing stays a plain diff against upstream.

| Crate | Upstream path | Used for |
| --- | --- | --- |
| `pi-diff` | `crates/pi-diff` | Line and structured diffs |
| `pi-ast` | `crates/pi-ast` | tree-sitter block ranges, parse checks and summaries (all upstream grammars) |
| `pi-edit` | `crates/pi-edit` | The edit engine behind the `edit` tool (all five modes), the snapshot store, and hashline formatting |

## Local modifications

Each code change is marked in the source with an `// ARA:` comment.

- **Manifests:** `authors`, `repository` and `[lints] workspace = true` were removed, because ARA has no workspace lint table. `publish = false` is inherited.
- **`pi-edit` suffix recovery:** `src/path_policy.rs` `recover_missing` walks with `ara-walk` (ARA's port of the pi-walker ignore-state walk) instead of `pi_walker::WalkRequest`. The options, the `**/<suffix>` glob, the two-entry limit and the 5 s budget are unchanged. `pi-walker` is not vendored.
- **`pi-edit` NFC:** `src/text.rs` uses `unicode-normalization` for NFC instead of `xutf`, which needs nightly `portable_simd`. ARA builds on stable.
- **`pi-edit` lossy decodes:** in `src/files.rs`, `FileRead.lossy` marks a file that is not valid UTF-8, and `FileRead::persist` refuses to re-encode it. This happens while staging, so nothing is written. Upstream writes U+FFFD over every undecodable byte.
- **`pi-edit` hashline `MV`:** in `src/modes/hashline/patcher.rs`, `stage_patch` refuses a destination that already exists, with patch mode's wording (`Cannot move X to Y: destination already exists.`), unless the same payload `REM`s it earlier. Upstream overwrites the destination.
- **`pi-ast` corpus tests:** in `src/block.rs`, the repository-corpus tests use the repository root one level higher, or `PI_AST_CORPUS_ROOT`. The sample sweep skips itself unless that root has OMP's `packages/` directory. Its evidence run points it at the pinned OMP checkout: `python scripts/vendored_behaviors.py --corpus <omp>`.
- **`BUILD.bazel` files:** omitted.
- **`Cargo.lock`:** pins `tree-sitter-cmake 0.7.4` and `tree-sitter-language 0.1.7`, the versions in OMP's lock, so parse trees match upstream.

## Evidence

`python scripts/vendored_behaviors.py` runs the three crates' test suites. It maps every inventory behavior of these crates (surfaces CRATE-PI-EDIT and CRATE-PI-OTHER/pi-ast) to its executed test, and writes `docs/evidence/vendored-behaviors.tsv`.
