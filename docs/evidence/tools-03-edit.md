# TOOLS-03 — `edit` tool, vendored edit engine, hashline anchors

Point / requirement / exclusions:
Port the OMP `edit` tool with its default hashline mode and the other four modes. Port the hashline display that `read`, `grep` and `write` feed into it. Use the vendored upstream Rust engine (`pi-edit`, with `pi-diff` and `pi-ast`).

Excluded (open):
- Streamed argument previews (`openArgStream`).
- LSP write-through (format on write, diagnostics) and the auto-repair model pass; the warning form of its note is kept.
- Plan mode, `local://` and `vault://` targets.
- ACP bridges, blackbox recording, approval tiers.
- `read` bracket context and structural summaries.
- Per-model mode selection (`resolveEditMode` model classes).

Upstream SHA and source/test location:
`596f2da7101178214aa27a753529d15e6b7ad91d`.
- Tool: `packages/coding-agent/src/edit/{index,schemas}.ts`, `utils/edit-mode.ts`, `utils/file-display-mode.ts`, `tools/{read,read-format,grep,write,hashline-format}.ts`, `crates/pi-natives/src/edit.rs` (`EditSession`, `EditStore`).
- Engine: `crates/pi-edit`, `crates/pi-diff`, `crates/pi-ast`, vendored at `crates/vendor/` (provenance and local modifications in `crates/vendor/README.md`).
- Behaviors:
  - CRATE-PI-EDIT: all 286 (pi-edit and pi-diff Rust tests).
  - CRATE-PI-OTHER: the 83 pi-ast tests.
  - Each behavior ID maps to its executed test in [vendored-behaviors.tsv](vendored-behaviors.tsv).
  - CA-TOOL-EDIT: TypeScript tests covering the renderer, ACP, blackbox and auto-repair (92 cases). These remain open.

Delivered commit or exact worktree snapshot:
The `dev` commit that adds this record. `verify_backend.py` passes on it.

Rust entry and host chain exercised:
- `EditTool`, `ReadTool`, `GrepTool` and `WriteTool` against real temp files: scripts with modes, notebooks, Latin-1 files, 60 KB lines, and a regular file blocking a directory path.
- The vendored crates' own suites.
- The `ara` binary driven by a scripted fake upstream:
  - `glob` → `grep` (tag `450E`) → hashline `edit` → `read` (new tag),
  - fixing a seeded bug (`ara-cli` e2e `search_and_hashline_edit_fix_a_seeded_bug`).

Environment and sanitized commands:
- `python scripts/verify_backend.py`: PASS (521 tests, 1 upstream-ignored). It runs fmt on ARA-owned packages (vendored crates keep upstream formatting), Clippy `-D warnings` on everything including vendored code, and all tests and doc tests.
- `python scripts/vendored_behaviors.py --corpus <omp checkout>`: `369 vendored behaviors: ignored-upstream 1, ok 368`.
  - The one upstream-ignored test is `pruned_walk_matches_unpruned_on_full_repo_corpus`. It was run separately:
    - command: `PI_AST_CORPUS_ROOT=<omp> cargo test --release -p pi-ast pruned_walk_matches_unpruned_on_full_repo_corpus -- --ignored`
    - result: 5287 files, 42296 comparisons, passed in 93 s.
  - Without a corpus, the sample sweep skips itself. That is how CI runs it, and the script reports it as `ok-no-corpus`.
- `cargo test -p ara-tools --test edit`: 9 passed. `cargo test -p ara-cli --test e2e`: 14 passed.
- `cargo deny check`: ok. New: tree-sitter plus the 57 upstream grammars, `ast-grep-core`, `phf`, `parking_lot`, `unicode-normalization` and `xxhash-rust`. BSL-1.0 is allowed for `xxhash-rust`, with the reason recorded in `deny.toml`.
- Grammar versions match OMP's `Cargo.lock`: `tree-sitter-cmake` is pinned to 0.7.4 and `tree-sitter-language` to 0.1.7.

Expected vs actual normal result:

replace mode:
- `print(msg)` → `print(msg.upper())` returns `[a.py]` followed by the numbered compact preview. Details: `op: update`, `firstChangedLine: 3`, and a diff containing `-3|    print(msg)`.
- `replace_all` works, and so does fuzzy whitespace matching (`alpha beta` against `alpha  beta`).
- Plain reads (no anchors).

hashline mode (the default):
- `read` gives `[a.py#TAG]` followed by `1:…` rows.
- `grep` gives `[a.py#TAG]` with ` 1:`/`*2:` rows (single file), or `## b.py#TAG` (grouped).
- `PUT 3.=3:` returns `[a.py#NEWTAG]` followed by the numbered file.
- A grep tag anchors `PUT >1:`.
- `PUT 1*:` resolves the Rust function through tree-sitter (`→ resolved lines 1-4 (4 lines)`).
- `MV lib/greet.py` returns `Moved to lib/greet.py`; `REM` returns `Deleted lib/greet.py`.
- `CUT 1.=1 @v` and `PUT >1 @v` move a line between files (two per-file results).

patch and apply_patch modes:
- An update hunk, and a create with parent directories.
- `*** Update File` with `*** Move to` plus `*** Delete File`.

write in hashline mode:
- A `[w.py#ABCD]` path is unwrapped and `1:`/`2:` prefixes are stripped, with the upstream note.
- The result starts with `[w.py#TAG]`. That tag anchors the next edit.

notebooks:
- `read n.ipynb` shows editable cells.
- An edit using the header tag succeeds and changes the cell source.

Tool surface:
- The description and schema follow the mode (`input` for hashline, `path`/`old_string`/`new_string` for replace).
- Concurrency is exclusive.
- The tool order is `read, write, edit, bash, grep, glob`.
- `ToolContext::new` enables hashline display by default (upstream `hasEditTool ?? true`); without the edit tool, reads are plain.

Actual: as expected.

Expected vs actual failure/cancel/recovery result:
- Upstream texts:
  - `Could not find a close enough match in a.py.` with the closest match and the 95% threshold
  - `Found 2 occurrences in a.py:` … `Add more context lines to disambiguate.`
  - `File not found: zz.py`
  - `Edit rejected for a.py: file changed between read and edit.\nSection is bound to #OLD, but the current file hashes to #NEW.`, followed by the current rows (nothing written)
  - apply_patch on a missing file returns an error
- A Rust edit that breaks the parse is applied, with the engine's `Warnings:` block and the host note `Warning: lib.rs no longer parses after this edit. …`.
- Intentional data-safety differences. The engine checks them while staging, before any write:
  - `Refusing to edit l1.txt: the file is not valid UTF-8 text…`; the Latin-1 bytes stay intact. This includes a file reached by unique-suffix recovery (`legacy.txt` → `sub/legacy.txt`). A `REM` of such a file still works.
  - `Cannot move a.txt to b.txt: destination already exists.` (patch mode's wording); both files stay intact. `REM b.txt` earlier in the same payload frees the destination.
  - A mid-call failure returns `Cannot write blocker/p3.txt: File exists (os error 17)` plus `Already changed on disk by this edit before the failure (re-read before retrying): p1.txt`. Moves that copied without removing the source, and writes that may be partial, are listed the same way.
  - A moved `0755` script stays `0755`.
  - In hashline mode, `write` only strips `N:` prefixes when the numbers are consecutive, as read output is. `200: OK` / `404: Not Found` and `10:30 standup` are written verbatim.
- Hashline numbering equals the engine's: `a\rb\nc` reads as `1:a 2:b 3:c`, and `PUT 3.=3` changes `c`. A BOM file shows no BOM in row 1.
- A 60 KB first line gives the upstream hashline message plus `[2 more lines in file. Use :2 to continue]`.
- Invalid UTF-8 after the 8 KB sniff window gives display-only `1|` rows and no tag.
- A pre-cancelled token returns `Edit was aborted`.

Actual: as expected.

Streaming observation before completion:
Not applicable: argument previews are not ported.

Real-model route:
Not run. `openrouter.ai` and `api.agnes-ai.cn` still return proxy 403 from this environment.

Independent review and semantic findings:
The Claude Code `code-review` skill (forked) reported 15 findings. The reviewer reproduced 9 of them with probes. Fixes, each with a regression test:
- Non-UTF-8 targets, `MV` onto an existing file and dropped move permissions: the intentional differences above.
- Partial writes named in the error, and I/O errors that carry the path.
- Hashline-aware `write`.
- Tags minted from the same in-memory text that `read` displays (no second read).
- Notebooks read as editable cells.
- Oversized first line keeps the continuation notice.
- `ToolContext` display mode derived from the edit settings (upstream default on).
- Untaggable files get `N|` rows.
- Staging, parse checks and suffix recovery run on a blocking thread.
- `vendored_behaviors.py` no longer reports the self-skipping corpus sweep as a plain `ok`, and no longer makes a redundant run.
- The unused `ignore` dependency was removed.
- Stale docs, the ledger and `THIRD_PARTY_NOTICES.md` were corrected.

Round 2 (14 findings on the round-1 fixes). Fixes:
- The UTF-8 and move guards moved from an `inspect()` pre-flight into the vendored engine, so suffix recovery, tag rebinding and every header form are covered. Deletes are allowed, and patch/apply_patch keep their upstream rename wording.
- Hashline reads display the normalized text their tag hashes (lone CR, BOM).
- `write` tags hash the BOM-stripped text, or a notebook's cells.
- Every effect already on disk is reported, including across a panic.
- Moves report a copy whose source removal failed.
- Notebook `fileSize` is the on-disk size.
- `futures::executor::block_on` replaces the runtime-handle `block_on`.
- Undecodable files are not read twice.

Declined:
- Atomic temp-file writes for updates. Upstream writes in place, which keeps symlinks, hard links and ownership. The "did not change on disk" guard is ported but has no test, because a filesystem that ignores writes cannot be staged portably.

Unrun/blocked checks and impact:
- No real-model trial (network policy).
- Upstream TypeScript renderer, ACP and blackbox cases are open.
- Windows is not supported.

Decision: tested. Not accepted (real-model trial pending).

Decision: accepted for the behaviors exercised in [real-model trials 2026-09-25](real-model-a1-a2-20260925.md) (Agnes `agnes-2.5-flash`, OpenRouter `openrouter/free`); items listed there as not exercised stay open.
