# CTX-01c: frontmatter and skills discovery

Point / requirement / exclusions:
Port OMP's Markdown YAML frontmatter parser and skill discovery into `ara-discovery`:
- `parseFrontmatter` with its repair and line fallback;
- the skill capability: `SKILL.md` scanning and the native, Claude, `.agents`, Codex, OpenCode and GitHub providers;
- `loadSkills` with its settings: source toggles, custom directories, include/ignore globs, collisions, symlink dedupe, deterministic order.

Excluded:
- `/skill:<name>` invocation (`parseSkillInvocation`, `buildSkillPromptMessage`, 14 behaviors of `skills.test.ts`). It is a host feature of the interactive and RPC modes (upstream print mode has none) and needs session custom messages, so it moves to CTX-01e.
- Plugin skills (Claude, Agent and OMP plugins) and managed (auto-learn) skills. They belong to extensibility (A6) and memory work.
- `loadFilesFromDir` recursion (B-7fc1a5a9f2, B-9d38cdda49). It serves rules and other capabilities (A6).

Upstream SHA and source/test location:
`596f2da7101178214aa27a753529d15e6b7ad91d`.
- Sources:
  - `packages/utils/src/frontmatter.ts`
  - `packages/coding-agent/src/capability/skill.ts`
  - `discovery/{helpers,builtin,claude,agents,codex,opencode,github}.ts` (skill parts)
  - `extensibility/skills.ts` (`loadSkills`, `loadSkillsFromDir`)
- Tests: 47 behavior IDs, named per test in `crates/ara-discovery/tests/{skills,frontmatter}.rs`.
  - `skills.test.ts`: 35 of 49. The other 14 are `parseSkillInvocation` (CTX-01e).
  - `discovery/helpers.test.ts`: 9 of 11 (the `parseFrontmatter` cases). The other 2 are `loadFilesFromDir` recursion.
  - `utils/test/frontmatter.test.ts`: 3 of 3.
- Fixtures `tests/fixtures/skills*` are copied from upstream `test/fixtures/skills*` (MIT, Stencil Labs).

Delivered commit:
The commit that carries this receipt on `dev`. The code was first pushed as WIP `0c6b893`.

Rust entry:
- `ara_discovery::frontmatter::{parse_frontmatter, parse_yaml}`
- `ara_discovery::skills::{skill_capability, scan_skills_from_dir, load_skills_from_dir, bun_glob_match, expand_tilde}`
- `Discovery::load_skills`

Environment and sanitized commands:
- `CARGO_INCREMENTAL=0 python3 scripts/verify_backend.py`: PASS (fmt, clippy `-D warnings`, 634 tests, 1 upstream-ignored, doc tests).
- `cargo test -p ara-discovery --test skills`: 12 tests.
- `cargo test -p ara-discovery --test frontmatter`: 8 tests.
- `cargo deny check`: advisories, bans, licenses and sources ok. Zlib is allowed for `foldhash` via `yaml-rust2`, with the reason recorded in `deny.toml`.

Expected vs actual normal result:
- Frontmatter:
  - YAML 1.2 core schema: null, bool and number forms; `yes` and dates stay strings.
  - Anchors, aliases and `<<` merge; the last value wins for a repeated key; integer-like keys come first.
  - Colon-space description repair without a warning.
  - CRLF, HTML-comment and tab repair.
  - Per-line fallback that re-parses each value.
- Skills:
  - The fixture root loads with upstream names, descriptions, sources and order.
  - Invalid names, missing descriptions and long names give upstream warnings.
  - Custom directories load only when builtins are disabled.
  - Claude user skills load without a project dir.
  - `.agents` toggles and the third-party gate.
  - Include and ignore globs; `enabled: false`; `disableModelInvocation` hides a skill.
  - `~` custom directories; collision fixtures share a name with one winner and a warning.
  - Provider precedence; symlinked duplicates dedupe; a custom directory overrides a default-provider skill.

Actual: as expected.

Expected vs actual failure/recovery result:
- Unrecoverable YAML falls back to per-line parsing with a `Failed to parse YAML frontmatter (…)` warning, which is surfaced through `load_skills` warnings.
- 30,000 nested `- ` markers in a 2 MB-stack thread give a parse error and the line fallback instead of a stack overflow; `parse_yaml` rejects depth 300.
- `!foo` include globs negate.

Actual: as expected.

Intentional differences:
1. WSL: B-394494e571, B-9ca7e04896 and B-82d9ed5857 test upstream's `cmd.exe`/`wslpath` probes. ARA spawns no process during discovery; the host passes the Windows profile through `HostDirs::with_env` (test `wsl_host_agents_skills`).
2. YAML comes from `yaml-rust2` events resolved with the core schema, not `Bun.YAML`. Differences found by the side-by-side review are documented in `frontmatter.rs`:
   - `.nan` becomes null and `±.inf` becomes `±f64::MAX` (JSON has no non-finite numbers; truthiness and non-string type are kept);
   - Bun's number quirks (`+.5`, signed hex, `1e`) follow the spec;
   - some block-scalar and complex-key edge cases may differ.
3. Nesting is capped at `MAX_YAML_DEPTH` (256); upstream relies on Bun's native parser.
4. Native locations are `~/.ara/agent/skills` and `.ara/skills` (upstream `.omp`), owned by `HostDirs`.

Independent review:
- Reviewer: a forked read-only general-purpose agent ("Review skills + frontmatter port") following `ara-git-review` + `ara-rust-core-review` on a scratch copy.
- Method: ran bun 1.3.11 (OMP pins 1.4.0) and Rust side by side on 213 YAML inputs, 64 frontmatter inputs and 18 glob patterns, and probed about 290 inputs for slicing panics.
- It confirmed parity for provider order and gating, directory walks, name and description rules, filter order, symlink dedupe, custom-directory override, warning order and sorting.

Findings and resolutions:

| Finding | Severity | Problem | Resolution | Test |
| --- | --- | --- | --- | --- |
| F1 | High | A deeply nested `SKILL.md` in a default-loaded project folder overflowed the stack and aborted the process | Iterative event reading plus the depth cap | `deep_yaml_nesting_does_not_overflow` |
| F2 | Medium | Skill-name globs did not follow Bun.Glob: `!foo` negation; `*` and `?` must not cross `/` | Fixed | `bun_glob_semantics` |
| F3 | Low | Unicode `\w` where JS uses ASCII | ASCII classes | `review_parity_details` |
| F4 | Low | `.nan`/`.inf` were stored as strings | Mapped as difference 2 | `frontmatter.rs` unit test `core_schema_resolution` |
| F5 | Low | Bun number and block-scalar quirks | Documented as difference 2 | — |
| F6 | Low | A `<<` key with a scalar value | Kept as an ordinary key, as Bun does | `review_parity_details` |
| F7 | Low | Integer-like key order | JS order | `review_parity_details` |
| F8 | Low | Warning source truncation | 63 UTF-16 units plus `…` | `review_parity_details` |
| F9 | Low | Skill paths not normalized | Normalized | `normalized_paths_and_frontmatter_warnings` |
| F10 | Low | Frontmatter warnings for skill files were dropped | Surfaced | `normalized_paths_and_frontmatter_warnings` |

Real-model route:
The CTX-01d trial exercises skills end to end (skill listed in the prompt, read through `skill://` or its path, format followed) on Agnes `agnes-2.5-flash` and OpenRouter `openrouter/free`. See [CTX-01d](ctx-01d-system-prompt.md).

Unrun checks:
- Bun 1.4.0 itself was not run, so a few Bun-quirk comparisons may shift.
- Windows paths are not exercised.

Decision: accepted, with the CTX-01d real-model trial.
