# CTX-01b: context-file discovery

Point / requirement / exclusions:
Port OMP's capability registry and context-file capability as the Core crate `ara-discovery`:
- AGENTS.md, CLAUDE.md, GEMINI.md and Copilot instructions from native, Claude, `.agent(s)`, Codex, Gemini, OpenCode and GitHub locations;
- the standalone AGENTS.md/CLAUDE.md walker;
- `@` import expansion;
- containment dedupe;
- `loadProjectContextFiles`.

Excluded:
- Other capabilities (rules, MCP, hooks, tools, slash commands, settings, extensions). They come with A6.
- Plugin providers.
- Settings persistence for provider switches. The host owns it through `ProviderPolicy`.

Upstream SHA and source/test location:
`596f2da7101178214aa27a753529d15e6b7ad91d`.
- Sources:
  - `packages/coding-agent/src/capability/{index,types,fs,context-file}.ts`
  - `discovery/{builtin,claude,agents,codex,gemini,opencode,github,agents-md,claude-md,at-imports,helpers}.ts` (context-file parts)
  - `system-prompt.ts` (`loadProjectContextFiles`, `dedupeContainedContextFiles`)
- Tests: 36 behavior IDs, named per test in `crates/ara-discovery/tests/upstream.rs`:
  - `discovery/{agents-md,claude-md,at-imports,context-file-dedup,disabled-extensions}.test.ts`
  - the context-file cases of `github-copilot.test.ts`
  - `capability/fs-special-files.test.ts`
  - `system-prompt-context-dedup.test.ts`

Rust entry:
- `Discovery::{load_context_files, load_project_context_files}`
- `context_file_capability`, `load_standalone_context_files`, `Expander`, `dedupe_contained_context_files`, `FsCache`

Environment and sanitized commands:
- `python scripts/verify_backend.py` (with `CARGO_INCREMENTAL=0` after a disk-space cleanup): PASS, 601 tests, 1 upstream-ignored.
- `cargo test -p ara-discovery --test upstream`: 20 test functions covering 36 behaviors.
- `cargo test -p ara-discovery --test project`: 10 end-to-end scenarios and review regressions.
- `cargo deny check`: ok.

Expected vs actual normal result:
- Standalone walker boundaries:
  - Nested repo under home: workspace file loads, home's does not.
  - No repo under home: cwd, intermediate and home all load.
  - Repo above home: home loads.
  - Repo outside home: the repo root is the limit.
  - Hidden owner directories are skipped.
  - Repo root equal to home: home's file is project context.
- Precedence:
  - `.claude/CLAUDE.md` shadows a standalone CLAUDE.md at the same depth.
  - Standalone AGENTS.md wins the depth tie against CLAUDE.md.
  - An empty file claims no depth.
- `disabledExtensions` hides context files unless `include_disabled` is set.
- Copilot: user file via `COPILOT_HOME`, project file, custom-instruction-dir AGENTS.md.
- `@` imports:
  - relative to the importing file, `~/`, nested, depth cap 5, cycles, missing files;
  - fenced and inline code, emails and SSH URLs, trailing punctuation.
- Containment dedupe: exact, contiguous, depth-authoritative, fence-aware.
- A monorepo scenario checks all of the above together:
  - native user file first, then packages AGENTS.md with its import expanded, then cwd CLAUDE.md;
  - the repo AGENTS.md, which the cwd file imports, is dropped;
  - provider provenance is kept.
- Foreign `~/` config is opt-in, `.agents` is not; `CLAUDE_CONFIG_DIR` relocates Claude.
- Other checks: the nearest non-empty `.ara/` up to the repo root, cache invalidation, Bun-equivalent decoding (BOM dropped, U+FFFD).

Actual: as expected.

Expected vs actual failure/recovery result:
- A FIFO named CLAUDE.md reads as missing instead of blocking.
- `@` imports of a FIFO, a directory or a symlink loop keep their literal token.
- Symlinked context files are followed.
- A provider error becomes a `[Provider] Failed to load: …` warning.
- Validation warnings come out last-first, as upstream.

Actual: as expected.

Intentional differences:
1. Hosts own locations and switches.
   - `HostDirs` holds: native `~/.ara/agent` and `.ara/` (display name `ARA`, upstream `.omp`), `CLAUDE_CONFIG_DIR`, `COPILOT_HOME`, custom instruction dirs, and the WSL home.
   - `ProviderPolicy` holds the enabled and disabled providers.
   - Upstream uses process globals for both.
2. Each `Discovery` owns an `FsCache`; upstream has one process-global cache.
3. WSL: no `wslpath` or `cmd.exe` probe. `USERPROFILE` is mapped to `/mnt/<drive>`, and an absolute profile is normalized. A host with a custom automount root sets `extra_user_homes`.
4. `ProjectContextFile` also carries its `SourceMeta` (provider, level), as provenance.
5. Optional host limits, off by default for parity:
   - `FsCache::with_max_file_bytes`;
   - `Discovery::import_policy`, which filters which paths `@` imports may inline.
   - Reads open non-blocking and check the file type on the opened handle, which closes the check-then-open FIFO race.

Independent review:
The review followed `ara-git-review` + `ara-rust-core-review`, as a forked read-only agent working on a `git archive` export.
- Verdict: no blocking parity defect.
- F1 (Medium, parity): unbounded `@` import reads, with no host limit. Fixed by difference 5, with tests `host_limits_on_reads_and_imports` and `imports_of_special_targets_keep_their_token`.
- F2 (Low): WSL probe skip and non-normalized absolute profile path. Fixed and documented (difference 3), test `absolute_wsl_profile_is_normalized`.
- F3 (Low): validation-warning order. Fixed, test `validation_warnings_follow_upstream_order`.
- Test gaps the reviewer listed (import edge cases, special targets, registry validation order) are now covered.

Real-model route:
Context files reach a model through the system prompt in CTX-01d, which carries the trial.

Decision: tested. Accepted with CTX-01d's real-model trial.
