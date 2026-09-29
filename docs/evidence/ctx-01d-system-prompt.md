# CTX-01d: system prompt assembly, skill:// and CLI integration

## 2026-09-29 current-head B.AI trial and gate (WIP)

Target `dev` at `9390d76`, fixed OMP source `596f2da`. The historical
`c9f9e24`, `41cb9a7` and `ca2a076` fixes are ancestors of this target; the
Agnes/OpenRouter trials below used older code. On this target, Linux CI
36579715665 passed bootstrap, inventory and `verify_backend.py`. The local
backend gate passed 1,042 tests, 0 failed, 1 ignored (CLI e2e 79/79), strict
workspace Clippy, owned formatting and doc tests.

The current route was checked before the trial: authenticated B.AI
`GET /v1/models` returned 200 and listed the exact `deepseek-v4.1-flash` ID
among 58 models. `ara` used `openai-completions`; only the short-lived trial
process mapped `BAI_API_KEY` to the CLI's `ARA_API_KEY` for this custom route.
No key was printed or saved. `scripts/real_model_trial_ctx.sh` ran the same
owner-rule and release-notes-skill task in a fresh local Git repository under
`C:\Temp\ara-ctx-9390d76-bai`, bounded to 12 calls, 300 seconds and 2,048
output tokens per call. The CLI and checker exited 0 after 31 seconds and
six model calls. All eight tool start IDs have matching end receipts. The
model successfully read `skill://release-notes`, wrote `mathx.py` with the
`AGENTS.md` owner line and a working `double(21) == 42`, and wrote
`RELEASE_NOTES.md` with `## vNEXT` and a `- [ara]` entry. One Bash call
returned an error; the Run continued and finished with `agent_end`.

The provider reported 12,786 input, 848 output and 23,040 cache-read tokens
(36,674 total); billing cost was unavailable. The JSON events and Session
journal are local artifacts at that trial path. The journal records one user
message, six assistant messages and eight tool results for Session
`01a0ed82-780f-739e-ad3d-7f173e58756e`; it names the exact model and
protocol. SHA-256: `mathx.py`
`a0287c0f34415d0853fe56417aa19c1f9f3bcbd2f691df9d64584f5912d4f122`,
`RELEASE_NOTES.md`
`717f1fb6b765a4dc3636f5d22f86a6128d714ac196ff299eb074a1f05f11b7d7`,
`events.jsonl`
`265f0d892ac26a83d99cb92f9761473e474e80312663d3a0273ee4a6c93c1efa`.
An exact-key scan of the 26 files in the trial directory found no match.
The live HTTP request body was not captured; the current-head fake-upstream
CLI tests below check that tool receipts reach the next model request.

Focused checks on the same commit passed: `cargo test -p ara-context --test
upstream` 21/21; `cargo test -p ara-tools --test skill_urls` 16/16; CLI e2e
filter `skill_url` 2/2 and `discovered_system_md_and_prompt_flags` 1/1.
Those cover the system prompt, immutable skill search/write failure with no
file effect, traversal rejection and receipt propagation. `cargo deny check`
exited 0 with existing duplicate-dependency and missing-license-field
warnings. An independent Codex reviewer inspected current CTX-relevant code
since `ca2a076` and found no new actionable defect; it ran no tests or model
call. The older claim below that skill images, binary files, directory text
and range context remain unported was superseded by `bc1f5db`, `59cc41e`
and `2e58b97`, with separate tests and reviews.

**Decision: still WIP, not accepted.** The fixed source at
`packages/coding-agent/src/internal-urls/filesystem-resource.ts:16-20` was
rechecked at `596f2da`: it sorts directories first, then names with
`localeCompare`. ARA uses case-folded ordinal ordering
([resource review](ctx-01d-skill-read-resources.md#review-and-remaining-differences)).
That can change a selected line. No fixed-OMP Bun directory-order oracle was
run on this host. Keep this parity gap explicit until it is resolved or
assigned as a reviewed intentional difference; do not claim the whole OMP
directory-read surface, P1 or P3 from this task trial.

## 2026-09-27 skill URL search/write follow-up

Local WIP `41cb9a7` routes `skill://` through `grep` and `glob`, rejects it
before `write` has a file effect, and checks the path through the real CLI
with a controlled fake upstream. Follow-up WIP `ca2a076` updates the gated
system-prompt guidance. The [focused receipt](ctx-01d-skill-url-tools.md)
records source mapping, tests, independent review and the still-failing
Windows backend gate. The earlier live-model runs below used older code;
CTX-01d remains **changes requested**.

## 2026-09-26 Windows real-model retest on `c9f9e24`

The user supplied route credentials for this task. They were read without terminal echo into short-lived process environment variables, never written to the repository or trial directories. Authenticated `/models` calls returned HTTP 200 and listed the exact requested IDs: Agnes `agnes-2.5-flash` (11 listed models) and OpenRouter `openrouter/free` (458 listed models). The CLI binds both to `openai-completions`. The bounded task was run through the real Windows `ara` binary with Git Bash, Rust 1.94.1, at most 12 model calls, 300 seconds, and 2048 output tokens per call. `$TEMP/ara-ctx-trial-bin/python3` is a no-secret wrapper for the installed Python executable. Local Ubuntu was not used or required.

| Route and artifact directory | CLI / checker | Calls and time | Observed result |
| --- | --- | --- | --- |
| Agnes, `$TEMP/ara-ctx-agnes-8_dhjqj4` | CLI exit 0; original shell checker exit 1 on Windows default GBK decoding; the same checker with explicit UTF-8 decoding exits 0 on the existing events and artifacts | 5 calls, 10 s | Successful `read skill://release-notes`; Python owner header, `double(21) == 42`, and `## vNEXT` release-note format all pass. The model recovered from one read of the not-yet-created notes file. |
| OpenRouter, `$TEMP/ara-ctx-openrouter-pvclebo4` | CLI exit 0; checker exit 1 | 7 calls, 20 s | Skill read, Python owner header, and function pass, but the release notes start with `## v1.0.0` instead of the skill's exact `## vNEXT`. This is a real task failure, retained as evidence. |
| OpenRouter retry, `$TEMP/ara-ctx-openrouter-retry-y3mzoopj` | CLI exit 0; checker exit 0 | 7 calls, 37 s | Skill read, owner header, function and `## vNEXT` release-note format all pass with the same task and bounds. |

The trial harness now reads UTF-8 explicitly on Windows. `bash -n`, `git diff --check`, and replaying the corrected checker on the original Agnes artifacts passed; that replay did not make another model call. The two OpenRouter outcomes show model variability, not a changed acceptance rule. `openrouter/free` is the requested router ID; the underlying model selected for either call was not established. Reported per-call usage summed to Agnes 6,367 input / 307 output tokens (cost unavailable); OpenRouter first 34,862 / 627 and retry 30,677 / 977 (provider-reported cost 0.0 for each, not independently billed cost). All three runs have an `agent_end`, paired tool receipts, a persisted session journal, and real file artifacts. The requested artifacts' SHA-256 hashes are:

| Run | `mathx.py` | `RELEASE_NOTES.md` |
| --- | --- | --- |
| Agnes | `8ecbf284a0649b249898d0a9205d94d83ff2d9c30a4875dad385d4bdb44ae7f1` | `9909490e8d1bb9443d9af636be246259ed4d97a01f2df060cd77bdc120a5fe41` |
| OpenRouter first | `0925d4ef0506419ff9c2c3ae3190d8aa2594936cbf8a53e0e9023744b9858d88` | `b4b040785d90571359686567f4de0f2f68f06c0a969790f2d9d71f378d1ea2a1` |
| OpenRouter retry | `0925d4ef0506419ff9c2c3ae3190d8aa2594936cbf8a53e0e9023744b9858d88` | `a48b5e3bc20c438912de6b155bebc33ae3fc52f67c6e03bc8991d17398b554ab` |

A credential-pattern scan of all 26 files in each trial directory found no matching strings. CTX-01d remains **changes requested**: the first OpenRouter task failed, full backend verification on the delivered snapshot has not passed, and the local WIP commits are not pushed. The repository acceptance rules do not require Ubuntu on the developer machine; the existing Ubuntu CI is one possible full-verification environment after a push.

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

Independent Codex subagent re-reviewed `f74d261..b920852` with `ara-git-review` and `ara-rust-core-review`, found the same-day F4 loss and the missing wire-role check, then re-reviewed the follow-up diff after both were fixed. Its final verdict found no remaining actionable product-code issue; it did not run tests. The same reviewer examined the corrected trial checker and found no remaining high-confidence issue. At this earlier checkpoint, full backend verification and fresh model trials were pending; the later results are recorded above. The prior trials below were on the earlier code and do not satisfy this gate. The interactive push was stopped while Git Credential Manager awaited authentication; remote `dev` remained `08c082f` at that check. No key was written to the repository.

Read-only live catalogue probes on 2026-09-26: unauthenticated `GET https://openrouter.ai/api/v1/models` listed `openrouter/free`; unauthenticated `GET https://api.agnes-ai.cn/v1/models` returned HTTP 401. These do not establish route authorization or task success. The authenticated model IDs and OpenAI-compatible protocol must be checked immediately before each trial.

The trial harness now exits nonzero when its checks fail. It requires a zero CLI exit code, 1–12 model calls within 300 seconds, a successful tool result paired by `toolCallId` that contains the release-notes skill body captured before the Run (raw or numbered read output), and the requested file artifacts. Its earlier printed `TRIAL PASS: False` was not an exit-status gate.

At the earlier `c9f9e24` checkpoint, `grep`, `glob` and `write` did not resolve
`skill://`; the 2026-09-27 WIP above adds those routes. Still open: image,
binary and directory skill read parity, and range context lines for text
skill reads (TOOLS-01). Quoted `skill://` tokens nested inside another shell
quote stay literal as an intentional safety difference from upstream (F3).

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

## Current-head Windows focused recheck (2026-09-28, WIP)

On clean local `dev` at `4cbf171`, these checks ran through the current Rust
code and actual `ara` process against a controlled upstream:

| Command | Observed |
| --- | --- |
| `cargo test -p ara-context --test upstream --quiet` | Exit 0, 21/21 |
| `cargo test -p ara-tools --test skill_urls --quiet` with Git Bash on `PATH` | Exit 0, 16/16 |
| `cargo test -p ara-cli --test skill_protocol --quiet` | Exit 0, 3/3 |
| `cargo test -p ara-cli --test e2e skill_url_search_and_read_only_write_reach_cli --quiet` | Exit 0, 1/1 real-process fake-upstream path |
| `cargo test -p ara-cli --test e2e discovered_system_md_and_prompt_flags --quiet` | Exit 0, 1/1 real-process fake-upstream prompt path |

The separate unfiltered backend run on this same code passed owned formatting
and workspace Clippy, then failed the known Windows Bash deadline assertion
(CLI 50/51); see the [handoff](../handoffs/2026-09-28.md). The current shell
has no `ARA_API_KEY` or `OPENROUTER_API_KEY` environment variable; no live
route or current-code real-model task was verified in this recheck. The prior
Agnes/OpenRouter trials above remain earlier-snapshot evidence. CTX-01d stays
**changes requested** and formal progress is unchanged.
