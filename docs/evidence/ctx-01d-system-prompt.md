# CTX-01d: system prompt assembly, skill:// and CLI integration

## 2026-09-26 follow-up: product candidate `c9f9e24` remains WIP

The F1–F9 fixes in `b920852` and the follow-up in `c9f9e24` are implemented, but CTX-01d is not accepted. The follow-up fixes a same-day F4 reminder loss found in re-review, tests both provider wire roles for the developer fallback, narrows the emitted `skill://` guidance to available tools, and makes the F8 filename error check compile on Windows. `Cargo.lock` uses `find-msvc-tools` 0.1.14 because 0.1.13 breaks `cc` compilation on Windows; the descending context-file sort is written with stable `sort_by_key` to satisfy current strict Clippy. The prompt template differences are recorded in `crates/ara-context/prompts/README.md`.

Executed checks on the `c9f9e24` product code, plus the subsequent trial-harness check:

| Check | Result |
| --- | --- |
| Rust 1.94.1, `cargo fmt -p ara-context -p ara-discovery -- --check` | Exit 0. |
| Rust 1.94.1, `CARGO_INCREMENTAL=0 cargo test -p ara-context --test upstream` | Exit 0; 21 passed, including same-day rewrite/shrink, both developer wire roles, and read-only/read-plus-bash prompt guidance. |
| `cargo deny check` | Exit 0; advisories, bans, licenses and sources accepted by policy (six duplicate-dependency warnings and one missing license-field warning). |
| `python3 scripts/omp_inventory.py check` and `python3 scripts/verify_bootstrap.py` | Exit 0 for both. |
| `bash -n scripts/real_model_trial_ctx.sh` and synthetic checker fixtures (`7df9631` harness) | Syntax exit 0. Complete raw and numbered skill receipts exit 0, including when the model-writable skill file is later emptied; a nonzero Run exit, a missing paired result, or a result echoing only one skill sentence exits 1. No real model was called. |
| Rust 1.94.1, `cargo test -p ara-cli --test skill_protocol` on Windows | Exit 0; 3 passed. This is library handoff coverage, not CLI e2e coverage. |
| Rust 1.94.1, `cargo test -p ara-cli --test e2e` on Windows | **Not passed.** Test compilation uses Unix-only `libc::kill` and `SIGKILL`. |
| Rust 1.94.1, `cargo test -p ara-tools --test skill_urls` on Windows | **Not passed; 7/10 passed.** Two test expectations use `/s/demo/a` where the Windows path is `/s/demo\\a`; the Bash execution case cannot find `bash` in the process PATH. The corresponding Unix run is still required. |
| `CARGO_INCREMENTAL=0 python3 scripts/verify_backend.py` on Windows | **Not passed.** Rust 1.94.1 reaches `cargo test --workspace --all-targets --all-features`, then Unix-only discovery fixtures fail to compile (`std::os::unix` in `tests/{upstream,skills,project}.rs`). The script was started before the last prompt-test edit, so it is not a completed check of the final snapshot. |
| Rust 1.94.1, `cargo test -p ara-discovery --lib` on Windows | Exit 101: `paths::tests::resolve_and_depth` assumes `/a/c`, while Windows resolves `D:\\a\\c`; three other unit tests pass. |
| Rust 1.98.1, `cargo clippy --workspace --lib --bins --all-features -- -D warnings` on Windows | Exit 101 on existing Unix-oriented `ara-tools` Windows warnings (`bash.rs` unused `pid`/`GroupGuard`, `write.rs` unused `mut`, `bash.rs` `unnecessary_lazy_evaluations`). No full Clippy pass is claimed. |

Independent Codex subagent re-reviewed `f74d261..b920852` with `ara-git-review` and `ara-rust-core-review`, found the same-day F4 loss and the missing wire-role check, then re-reviewed the follow-up diff after both were fixed. Its final verdict found no remaining actionable product-code issue; it did not run tests. The same reviewer examined the corrected trial checker and found no remaining high-confidence issue. The full Linux backend verification on the final commit, two fresh real-model task trials and their artifact review remain mandatory. The prior trials below were on the earlier code and do not satisfy this gate. The interactive push was stopped while Git Credential Manager awaited authentication; remote `dev` remained `08c082f` at the final check. Neither `ARA_API_KEY` nor `OPENROUTER_API_KEY` is available in this task environment. No key was written to the repository.

Read-only live catalogue probes on 2026-09-26: unauthenticated `GET https://openrouter.ai/api/v1/models` listed `openrouter/free`; unauthenticated `GET https://api.agnes-ai.cn/v1/models` returned HTTP 401. These do not establish route authorization or task success. The authenticated model IDs and OpenAI-compatible protocol must be checked immediately before each trial.

The trial harness now exits nonzero when its checks fail. It requires a zero CLI exit code, 1–12 model calls within 300 seconds, a successful tool result paired by `toolCallId` that contains the release-notes skill body captured before the Run (raw or numbered read output), and the requested file artifacts. Its earlier printed `TRIAL PASS: False` was not an exit-status gate.

Open bounded differences: `grep`, `glob` and `write` do not resolve `skill://`; image, binary and directory skill reads do not match upstream's resource handling; range context lines for text skill reads remain with TOOLS-01. Quoted `skill://` tokens nested inside another shell quote stay literal as an intentional safety difference from upstream (F3). The prompt now directs the model to use filesystem paths with other file tools and advertises Bash expansion only when Bash is available.

Point / requirement / exclusions:
Build the Agent system prompt from discovered context the way OMP does, and let the model use skills:
- `ara-context`, the port of `buildSystemPrompt`:
  - upstream templates;
  - context files with dedupe;
  - `SYSTEM.md` and custom/append prompts;
  - the skills listing with its visibility rules;
  - personality presets and `PERSONALITY.md`;
  - environment and kernel identity;
  - model line;
  - active child repo context;
  - the compact tool inventory;
  - internal-URL guidance gated to the schemes the host resolves.
- The date/cwd reminder (`date-cwd-reminder.ts`), injected at request time through a new `LoopHooks::transform_provider_context` hook.
- `skill://` in the tools (`internal-urls/skill-protocol.ts`, `tools/bash-skill-urls.ts`):
  - `read` resolves it;
  - `bash` expands it in the command, env values and cwd.
- CLI host wiring (`ara-cli`): `Discovery` → context files, skills, and project-first `SYSTEM.md`/`APPEND_SYSTEM.md` discovery → `ToolContext::with_skills` → `build_system_prompt`; `--no-skills` and `--skills <globs>`. Explicit prompt flags override discovery and repeated flags keep the last value.

Excluded:
- `/skill:<name>` invocation. Upstream handles it only in the interactive, RPC and ACP hosts, not print mode; it moves to CTX-01e.
- The `namespace functions` tool catalog. ARA uses provider-native tool calls, so upstream's compact inventory mode applies.
- Rules, workspace tree, GPU probing, subagent settings, `xd://`.
- Internal URL schemes other than `skill://` (agent, artifact, memory, rule, local, attachment, history, mcp).
- Prompt refresh on model change.

Upstream SHA and source/test location:
`596f2da7101178214aa27a753529d15e6b7ad91d`.
- Sources:
  - `packages/coding-agent/src/system-prompt.ts`
  - `session/date-cwd-reminder.ts`, `utils/active-repo-context.ts`
  - `prompts/system/*` (copied with the MIT notice; ARA edits are listed in `crates/ara-context/prompts/README.md`)
  - `internal-urls/{skill-protocol,parse}.ts`, `tools/bash-skill-urls.ts`
  - `tools/read.ts` `#handleInternalUrl`, `read-format.ts` `buildInMemoryTextResult`, `utils/file-display-mode.ts`
- Tests: 58 behavior IDs, named per test.
  - `crates/ara-context/tests/upstream.rs` (34):
    - `system-prompt-dedup`: 9 of 10;
    - `-personality`: 4 of 4;
    - `-kernel`: 3 of 3;
    - `-model`: 2 of 6;
    - `-inventory`: 10 of 21;
    - `date-cwd-reminder`: 6 of 7.
  - `crates/ara-tools/tests/skill_urls.rs` (21 of 47 in `bash-skill-urls.test.ts`): all 13 `expandSkillUrls` cases and the 8 `skill://` quoting cases of `expandInternalUrls`. The other 26 need other schemes.
  - `crates/ara-cli/tests/skill_protocol.rs` (3 of 5 in `skill-protocol-customdirs.test.ts`): B-edc3d29fb5, B-4ebceb9b09, B-0b170f55e3.

Candidate product-code commit: `c9f9e24`; trial-harness commit: `7df9631` (both local WIP until push completes). Earlier WIP: `68d774d`, `0c6b893`, `b920852`.

Rust entry and host chain:
- `ara_context::{build_system_prompt, resolve_prompt_input, DateCwdReminder}`
- `ara_agent::LoopHooks::transform_provider_context`
- `ara_tools::internal_urls::{resolve_skill_url, resolve_skill_url_to_path, expand_skill_urls, expand_skill_urls_strict, validate_relative_path}`
- `ReadTool` and `BashTool` via `ToolContext::skills`
- `ara-cli` `main.rs`
- Exercised through the real `ara` binary by `tests/e2e.rs` and the real-model trial.

Prior evidence at `f74d261` (before F1–F9 fixes); these results do not verify the current candidate:
- `CARGO_INCREMENTAL=0 python3 scripts/verify_backend.py`: PASS (fmt, clippy `-D warnings`, 634 tests, 1 upstream-ignored, doc tests).
- `cargo test -p ara-context --test upstream`: 16 tests.
- `cargo test -p ara-tools --test skill_urls`: 6 tests.
- `cargo test -p ara-cli --test skill_protocol`: 3 tests.
- `cargo test -p ara-cli --test e2e`: 16 tests, including `context_files_and_skills_reach_the_model_and_skill_urls_resolve` and `skill_flags_filter_listing_and_resolution`.
- `cargo deny check`: ok.

Expected vs actual normal result:
- System prompt (ported tests):
  - Date and cwd stay out of it.
  - `SYSTEM.md` used as the custom prompt renders once; project wins over user.
  - Loaded prompt text is not resolved as a path.
  - A custom prompt suppresses the discovered `SYSTEM.md` but keeps the footer.
  - Explicit context entries dedupe by content; identical discovered context keeps the closest copy.
  - `PERSONALITY.md` overrides the preset.
  - Model line; kernel identity fallbacks.
  - Compact tool inventory; guidance for absent tools drops out.
  - Skills are listed only with `read` and not when hidden.
  - Internal URLs follow the host.
- Reminder:
  - It rides the first user turn and is never stored.
  - A changed date/cwd attaches to the next new user turn, or to a developer turn after the last message.
  - Earlier request bytes stay identical.
- `bash` (upstream cases):
  - Tokens become shell-escaped absolute paths: quoted, multiple, spaces, quotes, bare URL is the skill directory.
  - Namespaced `pkg:tool:suffix` uses the longest-prefix match.
  - URLs inside `$()` and backticks within double quotes expand; literals inside single quotes or escaped quotes stay.
- Tool level:
  - `bash` resolves `skill://` in the command, in an `env` value and in `cwd`.
  - `read skill://demo` is `SKILL.md`; `:-2`, `:2-3` and `:raw` selectors apply.
- Host level:
  - Skills loaded from custom directories resolve through `read`.
  - First-wins across custom directories, with a collision warning.
  - A custom directory overrides a Claude project skill.
- E2E (`ara` binary against the fake upstream):
  - AGENTS.md appears in `<repo-rules>` with its canonical path.
  - The skill is listed and `skill://` advertised; unported schemes are not.
  - The model's `read skill://greeting` returns the skill body; `skill://greeting/assets/extra.txt` returns the asset.

Actual: as expected.

Expected vs actual failure/recovery result:
- Unknown skills, `..` segments (plain, `%2E%2E`-encoded or starting with `..`), absolute paths and malformed escapes (`%ZZ`, `%FF`) are errors:
  - `read`: `Unknown skill: x\nAvailable: …`, `Path traversal (..) is not allowed in skill:// URLs`, `URI malformed`, `File not found: …`.
  - Strict `expandSkillUrls` errors the same way.
  - `bash` leaves the token as written, as upstream's `expandInternalUrls` does.
- The E2E traversal read returns `isError: true` and the run continues to a final answer.
- `--skills fare*` lists and resolves only `farewell`: `read skill://greeting` gets `Unknown skill: greeting\nAvailable: farewell`.
- `--no-skills` removes the listing, and nothing resolves (`Available: none`).
- A malformed `SKILL.md` is reported on stderr as `ara: skill warning (…): Failed to parse YAML frontmatter …`.

Actual: as expected.

Intentional differences:
1. `read skill://s/../x`: upstream's WHATWG URL parse collapses dot segments first, so it reads `s/x`. ARA keeps the path as written and rejects the `..` segment, as `bash` does. Both stay inside the skill directory.
2. Skill reads use the file-read path with upstream's display rules:
   - immutable, so no hashline tag and numbering only with `readLineNumbers`;
   - no line or byte truncation.
   The range context lines of upstream's in-memory renderer are not ported, the same open item as plain reads in TOOLS-01. That leaves B-20f58d354f and B-3deff3fcd6 (delimited multi-path reads) unported.
3. The reminder is applied through a Core hook (`transform_provider_context`) rather than inside the session, so any host can use it. The CLI passes the local date and cwd.
4. Prompt identity is host-supplied (`harnessName`, ARA edit 1).
5. Bash expansion leaves a quoted `skill://` token literal when the token starts inside another shell quote. Upstream expands that case and can turn the result into shell syntax. `quoted_tokens_nested_in_quotes_stay_literal` and the marker-file regression cover this safety difference.

Real-model route:
`scripts/real_model_trial_ctx.sh`.
- Task: add `double(x)` in a new `mathx.py`, then record the change in the release notes.
- Setup: the rule "Every new Python file MUST start with `# owner: ara-trial`" exists only in `AGENTS.md`, and the release-notes format exists only in `.ara/skills/release-notes/SKILL.md`.
- Checks (artifacts, not answer text):
  - the model read the skill via `skill://` or its path;
  - `mathx.py` starts with the owner line;
  - `double(21) == 42`;
  - `RELEASE_NOTES.md` starts with `## vNEXT` and has a `- [ara] ` entry.
- Bounds: ≤ 12 model calls, ≤ 300 s, ≤ 2048 output tokens per call.
- Keys come from the environment only; `HOME` and `ARA_HOME` are isolated per run.

Observed output on the earlier `f74d261` code (before the current fixes):

| Route and model | Calls | Time | Tokens in / out | Skill read | Result |
| --- | --- | --- | --- | --- | --- |
| Agnes `https://api.agnes-ai.cn/v1`, `agnes-2.5-flash` | 6 | 57 s | 6,378 / 1,055 | By path, after a failed guess (`read work/release-notes` → `File not found`) | TRIAL PASS |
| OpenRouter `openrouter/free` | 5 | 26 s | 23,267 / 771 | `read skill://release-notes` | TRIAL PASS |

- Agnes: the owner line was followed, `double(21) == 42` held, and the notes had the skill format. The model ran its own `python -c` assertions.
- OpenRouter: a `glob RELEASE_NOTES.md` miss was recovered from; all checks held.
- Earlier round (before the bash expansion and final read changes):
  - Agnes: 6 calls, read `skill://release-notes`; PASS.
  - OpenRouter: 12 calls, with recovery from tool errors; PASS.
  - That round found that models also try `cat skill://…` in bash, which led to porting the bash expansion.
- Harness fix: tool results are now paired with their calls by `toolCallId`. Parallel calls can finish out of order.

Prior independent review of `68d774d^..f74d261` (blocking at that snapshot):
- Reviewer: a forked read-only general-purpose agent following `ara-git-review` + `ara-rust-core-review`.
- Target: `68d774d^..f74d261`, read from a `git archive` export. Upstream compared at the pinned SHA.
- Checks run by the reviewer:
  - scoped crate tests, all passing (`ara-context` 16, `ara-tools` `skill_urls` 6, `ara-cli` `skill_protocol` 3 and `e2e` 16, `ara-agent` 21);
  - scratch-only probes through `BashTool`, the `ara` binary and a byte-exact node emulation of upstream's regex.
- Verdict: **blocking**. Findings, with the required fixes, are tracked in [the 2026-09-26 handoff](../handoffs/2026-09-26.md):
  - F1 (High): the CLI never passes the discovered `SYSTEM.md`/`APPEND_SYSTEM.md` as the custom/append prompt, so they never reach the model. Upstream: `main.ts:1037-1105`.
  - F2 (High, safety): the unquoted token class in `internal_urls.rs` lacks the backslash exclusion (`\;` upstream). A `"` next to a `skill://` token then flips quote parity after expansion, and quoted text runs as a command.
  - F3 (Medium, safety, also upstream): a quoted `"skill://…"` token nested inside another quote is expanded and can inject shell syntax.
  - F4 (Medium): `DateCwdReminder` keys its state by index and count; upstream keys it by message identity. A rewritten or shrunk transcript gets stale content.
  - F5 (Medium, evidence): claims above that did not match the code (corrected here). Unrecorded differences:
    - `skill://` is not resolved by `grep`/`glob`/`write`;
    - `--append-system-prompt` is repeatable (upstream keeps the last value);
    - skill reads of images, binaries and directories differ;
    - `THIRD_PARTY_NOTICES.md` omits `ara-context`'s templates.
  - F6 (Low): `ToolContext::with_skills` mutates every clone.
  - F7 (Low): a relative `ARA_HOME` lists user skills that `read skill://` cannot open.
  - F8 (Low): unreadable prompt files and `PERSONALITY.md` fall back without a warning.
  - F9 (Low): extra workspace roots are skipped when context files are supplied.
- The "Host level" list above is library hand-off coverage (`skill_protocol.rs`), not CLI coverage: the CLI has no custom-directory setting.
- At `f74d261`, the developer-turn fallback of the reminder had no test; `b920852` added the append-only case and `c9f9e24` added both provider wire-role cases.

Unrun checks:
- `/skill:` invocation (CTX-01e).
- The other internal URL schemes.
- Model-change prompt refresh.
- Plugin-contained skills (`containRoot`).

Fixes for F1–F9, with regression tests, are in WIP commit `b920852`; the F4 follow-up and final review are in WIP `c9f9e24`. Full verification and the real-model re-runs are still pending (see the handoff).

Decision: changes requested. The independent re-review is complete; acceptance still needs full verification on the final code and both real-model trials with reviewed artifacts.
