# TOOLS-02 — `ara-tools` grep and glob

Point / requirement / exclusions:
Port the OMP `grep` and `glob` tools for filesystem targets, with their path grammar, ignore rules, output format, limits, notices and error texts, running against real directory trees.

Excluded (open):
- Internal URLs (`skill://`, `memory://` globs, `artifact://` …) and archive members as targets. An unregistered `scheme://` entry is an ordinary path, as upstream does when no protocol handler claims it.
- Hashline display mode (`## file#TAG`, `*N:line`, snapshot recording). It ships with the hashline edit tool (TOOLS-03); ARA uses upstream's plain mode (`*N|line`).
- SSH approval tiers, `ast-grep`, streamed glob updates, custom remote glob operations, and the TUI renderers.

Upstream SHA and source/test location:
`596f2da7101178214aa27a753529d15e6b7ad91d`.
- Tools: `packages/coding-agent/src/tools/{grep,glob,path-utils,match-line-format,grouped-file-output,list-limit,output-meta,file-recorder}.ts`, `session/streaming-output.ts` (`truncateHead`), `packages/utils/src/path-tree.ts`.
- Native engine: `crates/pi-natives/src/{grep,glob,glob_util}.rs` and the ignore-state traversal in `crates/pi-walker/src/lib.rs` (MIT; see `THIRD_PARTY_NOTICES.md`).
- Behavior tests: `test/tools/{grep-path-lists,multi-grep-path,multi-path-missing,glob-validate-paths,glob,grouped-file-output}.test.ts` (CA-TOOL-SEARCH surface, 133 cases). Cases ported to Rust are listed by ID in the ledger rows.

Delivered commit or exact worktree snapshot:
The `dev` commit that adds this record. `verify_backend.py` passes on it.

Rust entry and host chain exercised:
- `GrepTool`/`GlobTool::execute` against real temp trees (repositories, `.jj` repositories, FIFOs, symlinked roots, 5 MB files, 2100-file trees).
- `engine::grep` directly for the windowed-stop assertions.
- The `ara` binary, driven by a scripted fake upstream through `glob` → `grep` → `bash`, fixing a seeded bug (`ara-cli` e2e `search_tools_find_and_fix_a_seeded_bug`).

Environment and sanitized commands:
- `cargo test -p ara-tools`: 17 unit tests plus 30 integration tests (`search` 12, `search_upstream` 7, `global_ignore` 1, `tools` 10). The three search binaries were run 3 times each and passed every time (0.23 s, 0.02 s, 0.01 s).
- `cargo test -p ara-cli --test e2e search_tools`: 1 passed.
- `python scripts/verify_backend.py`: PASS (fmt, Clippy `-D warnings`, 144 tests, doc tests).
- `cargo deny check`: advisories, bans, licenses and sources ok. Added dependencies: `grep-regex`, `grep-searcher`, `grep-matcher`, `grep-pcre2`, `ignore`, `globset` (MIT/Unlicense), `regex`, and `pcre2-sys` (bundled PCRE2, BSD-3-Clause).

Expected vs actual normal result:

grep:
- A directory search prints grouped output: `# README.md` / `*1|alpha docs`, then `# src/`, `## lib.rs` with ` 1|` context, `*2|` match, ` 3|`/` 4|` after-context, and `## deep/`, `### mod.rs`. Context is 1 line before and 3 after.
- A single-file search prints no headers and marks gaps with `...`.
- 25 matching files show the first 20, then `Showing files 1-20 of 25. Use skip=20 for the next page, or narrow paths/pattern.`
  - `skip=20` returns files 21–25.
  - `skip=30` returns `No more results (25 files total; skip=30 is past the end)`.
- A hot file is trimmed to 20 matches in a directory scope (`perFileLimitReached: 20`). A single file keeps up to 200.
- Path forms: `src; tests` lists, JSON-array strings, `[]`, quoted paths, paths with spaces, absolute in-cwd paths shown cwd-relative, bracketed literal paths (`apps/[id]`), globs in paths, and `file:3-4` range filters that also drop context outside the range. A literal file named `notes:1-2` wins over the selector reading.
- Explicit files stay exact (`alpha.txt; beta.txt`). An explicit `.git/config` target is searched next to `.`. A file overlapping a directory target is deduped. Two unrelated trees (`/tmp`, `/var/tmp`) are searched as separate targets in well under 5 s.
- Stray parentheses and lookahead still search: `fetchProvider(` via the escape retry; `foo(?=bar)` and backreferences via PCRE2. `${platform}` keeps `$` as an anchor, as upstream does.
- A 600-char line is cut to 509 bytes plus `...`, with the notice `[Some lines truncated to 512 chars]`.
- `first\nsecond` matches across lines. A multi-line pattern that cannot match a newline still merges adjacent matching lines into one match.
- Windowed search: 2100 one-match files plus a 5 MB file stop after four 512-file windows (2047 files searched, 2000 matches, `limit_reached`), and the deferred large file is never searched. Under budget, the large file's results follow the normal ones: `z.txt` before `a_big.log`. The tool label reads `of 2000+`.

glob:
- Results are newest first by mtime and grouped: `c.ts` / `# src/` / `a.ts` / `## nested/` / `b.ts`.
- `**/tests` yields `src/tests/`, and `src/*.ts` does not recurse.
- Plain file and directory targets work; `;`/`,` lists work; quoted and space-containing paths work; paths outside cwd stay absolute (`# /tmp/…/` / `outside.txt`).
- `limit=2` adds `[2 results limit reached. Use limit=4 for more]`.
- `hidden:false` hides dotfiles, `gitignore:false` shows `.env.local`, and `node_modules` is listed only when the pattern names it.

Ignore semantics, grep and glob alike:
- `.git` is pruned. Nested `.gitignore`, `.ignore`, `.git/info/exclude` and the global gitignore are applied, the last only inside a repository (`global_ignore` test sets `XDG_CONFIG_HOME`).
- Anchored parent rules (`/pkg/dist/`, `pkg/gen.txt`) apply when the walk root is a subdirectory.
- `.ignore` `!keep.log` beats `.gitignore` `*.log` from any root.
- A parent rule that would hide the walk root itself is dropped for that walk.
- `.jj` marks a repository root.
- FIFOs are never listed.
- A symlinked root walks the real directory under its repository's rules.

Actual: as expected.

Expected vs actual failure/cancel/recovery result:
- Upstream error texts, all asserted verbatim:
  - `Pattern must not be empty`
  - `Skip must be a non-negative number`
  - `Path not found: nope` and `Path not found: x, y`
  - `Line-range selector requires a single file: src:1-2 is a directory`
  - `…not a glob: src/*.rs:1-2`
  - the `only line-range selectors` message for `:raw`
  - `Line selector 0 is invalid…`
  - `Cannot search external URL: https://…`
  - `Searching from root directory '/' is not allowed` (for `/` and `//`)
  - `Limit must be a positive number`
  - `Path is not a directory: <abs>`
  - `No files found matching pattern`
  - `No matches found`
- A missing list entry is skipped with `Skipped missing paths: …`, and `details.missingPaths` is set.
- A PCRE2 match-limit failure on a named file returns `Search failed: …`, not "no matches".
- `notes:٣` (a non-ASCII digit) is a path, not a selector.
- A 5 MB file named explicitly returns matches from its first 4 MB and the upstream note. Outside cwd the note uses a `../` path.
- Zero budget gives `Grep timed out after 0s; narrow paths or pattern, or scope with `glob` first`, and `Glob timed out after 0s before finding any matches — the scan is incomplete, NOT proof of absence. …`.
- A pre-cancelled token gives `Grep was aborted`. A dropped call future cancels the blocking scan through a drop guard (`run_blocking`).

Actual: as expected.

Streaming observation before completion:
Not applicable: upstream grep and glob return one final result.

Real-model route:
Not run. `openrouter.ai` and `api.agnes-ai.cn` still return proxy 403 from this environment, so the point stays below accepted.

Independent review and semantic findings:
The Claude Code `code-review` skill (forked) ran two rounds.

Round 1 had 15 findings. The reviewer reproduced items 1–4, 7–9, 11 and 12 against the prior tree. The fixes:
- Anchored parent rules, global gitignore outside a repository, parent precedence and `.jj` roots: the `ignore::WalkBuilder` walker was replaced with a port of pi-walker's `IgnoreState` chain (`walk.rs`).
- A searcher error on a named file now returns `Search failed`.
- Selector grammars accept ASCII digits only.
- `details` shape: `files: []` is kept and an empty `missingPaths` is omitted.
- The oversized-file note uses a relative path.
- A drop guard stops abandoned scans.
- Directory grep streams 512-file windows and stops at the budget instead of walking first.
- One searcher and read buffer are reused per call, and sizes are checked before reading.
- `THIRD_PARTY_NOTICES.md` now covers `ara-tools`.
- Glob targets run concurrently, so a slow root no longer starves the others.

Round 2 had 14 findings. The fixes:
- `non_matching_bytes` is no longer forwarded, because it changed multi-line results. The candidate-line and shortest-match fast paths are forwarded instead.
- FIFOs, sockets and devices are skipped.
- The walk root is canonicalized.
- PCRE2 JIT is off on macOS by default and overridable with `ARA_PCRE2_JIT`.
- Glob scans run on at most 8 threads, with an inline fallback.
- Windowed-search tests were added.
- `details.truncation` is set.
- Metadata is fetched only for glob matches, and fewer copies are made.
- The blocking helper is shared.
- Only missing, permission-denied and non-regular files count as skipped large files.

Intentional differences:
- A symlinked grep root is walked. Upstream grep lstats its root and reports no matches there; ARA follows upstream glob instead.
- Windows are searched sequentially, with the same result order.
- Output uses plain display mode until the hashline edit tool lands.

Unrun/blocked checks and impact:
- No real-model trial (network policy).
- Windows is not supported.
- A single 4 MB file cannot be interrupted mid-search (as upstream: cancellation is checked between files).
- Performance was only checked through the 2100-file and 5 MB fixtures.

Decision: tested. Not accepted (real-model trial pending).
