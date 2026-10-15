# Linked context and execution modes — 2026-10-15

Scope: compact HandWave Rust Host plus Ray/Hub context integration. Shared ARA
Core/OMP work, authentication changes, device grants and other services are
outside this change. Source and isolated integration are verified; production
activation remains blocked by the deployed Hub's unidentified source baseline.

## Executed local evidence

- Node workspace fmt, clippy (all targets/features, locked, warnings denied) and
  Cargo tests pass; Windows 82 tests, zero failed/ignored.
- Static-CRT Windows release: 4,642,304 bytes, SHA256
  `c97b0450a531ad86036d48f81e765f775bd1cca5a6ce56e699f1384b2e6e61cb`.
- Actual release EXE plus actual Go Hub controlled-model modes fixture passes
  five scenario groups: exact read-only schemas/forged tools, no-model command
  rejection, busy legacy agent-loop mode/goal inheritance, real write/read goal
  artifact with restart/pause/resume/explicit accept, and real child cancellation.
- Actual authenticated Hub chooser/source APIs plus simulated Ray carrier and
  actual Node Plan/reference_read pass four additional groups: public markers
  and source hash/version retained, private fields omitted, no tool-permission
  increase, and current Session identity unchanged. Ray HTTP assembly is covered
  separately by Go tests, not claimed part of this process fixture. The first
  fixture attempt incorrectly hashed a noncanonical Windows cwd and remains
  failed; the final check independently verifies samefile and hashes the stored
  canonical path (including Windows extended prefix). No runtime code changed.
- Actual-process context regression passes 15 groups; experience regression
  passes five groups with 24 model requests and actual failure/recovery artifacts.
- Linux offline compile and actual unprivileged ara-verify test image pass 80
  tests, zero failed/ignored. Extracted and final Cargo/src hashes match. This is
  Linux test-image acceptance, not Linux release packaging or Hub/model testing.
- Go memorystore, Ray and Hub tests and vet pass both on the working tree and
  clean HEAD export plus the explicit feature allowlist. Original auth/UI WIP
  was excluded. Authenticated dispatcher integration checks 401/200 for both
  reference endpoints. Ray Linux image SHA256
  `c7c43776850d6398f1c42ee5cad2c0b31018822b2ad53492d2a9f9416af7766e`.
- All nine Ray JS suites pass, including links29, machine17 and memory28 cases.
  Whole pinned rewritten app parses. Actual browser preview checks @menu and
  Enter creating a reference chip without selecting another conversation,
  disabled mode options without Node capabilities, 375px dark/light and 1200px
  light with no horizontal overflow; main input computed border is 0px.
  The preview has synthetic data and no live Node connection. Physical mobile
  keyboard, real UI goal/experience flows and production rendering are not
  accepted by those observations.

Raw isolated receipts are under
`D:/WorkSpace/tmp/handwave-links-modes-20261015/`. Histories, credentials, local
state and private model payloads are not included in Git. Controlled-model
fixtures prove contracts and effects, not model intelligence or token savings.

## Independent review and retained findings

Codex context_modes_audit reviewed Go source after integration; context_ui_audit
reviewed Rust; context_cloud_audit reviewed UI. Main agent checked the mode gate,
goal commands/persistence and final executable receipts. Three integration issues
were found instead of accepting isolated passing suites:

1. Busy legacy agent-loop follow-ups originally bypassed mode inheritance. Fixed
   by preparing queued follow-ups from the active frame while preserving their
   controller request IDs; actual wire regression passes.
2. An outer session-reference validation error could occur after the inner
   transport marked context ready, and pinned startLocalRun caught the error and
   still sent a blank brief. Fixed with a separate verified-context dispatch gate;
   actual pinned composer regressions cover missing receipts and first binding.
3. A late older Run could replace a newer goal in the same selected Session.
   Fixed with a monotonic Run owner for goal/candidate cards; stale-Run regression
   passes. The legacy hello workspace-PATCH completion also captures its owner.

## Real-model evidence and retained failed trial

Current inventory verified deepseek-v4.1-flash/openai-completions. First bounded
trial passed the read-only Plan and completed the Goal artifact/progress, but
used exec instead of the verifier's required run_script, so it remains FAIL.
The prompt had not explicitly required that tool. The original receipts and
artifacts remain; no in-flight or unknown-effect operation was replayed.

A separate fresh fixture with explicit run_script instruction passes in26.117s:
Plan2 call reservations, Goal6, same native Session across restart, actual
Python-generated report sum42/count3, readback, goal_update and awaiting_review.
An explicit fixture `/goal accept <id>` makes acceptance durable without a model
call. No candidate was auto-adopted. Temporary approval revocation is confirmed
in both trials. The report SHA is
`a39a27e94d999fd287ecd1967b29436576f3f055c5ddfe00e2a381ce8a7403cc`.
Supplier token usage and actual downstream HTTP counts remain unknown;
reservations are not supplier calls. No token-saving percentage is claimed.

## Production boundary

Existing Hub image differs from current HEAD in Go version, dependencies and
four embedded assets. Raw/LF-normalized Git-object checks in ARA history,
ai-handwave and eight relevant worktrees found no matching source. Static asset
replacement cannot prove backend equivalence. Hub, Ray and installed RAYWORK
were left untouched; the old public downloads remain the old version. New
session APIs, connect-approved and browser mode/experience activation await a
verified production source upgrade. Do not describe the local package or clean
Ray build as deployed.

Final independent Codex context_ui_audit read the actual release image, raw
controller goal revisions2..5, native accepted state, zero-model review and
unadopted memories, Python source/report and Linux source hashes. It accepted
this bounded source/isolated integration delivery and explicitly withheld
production acceptance. UI implementation was independently reviewed by
context_cloud_audit; the implementer's evidence readback is not called an
independent UI code review.

Six changed/index pages have no missing relative links. Full root bootstrap
validation exceeded a30-second bound and is NOT COMPLETED. Dependency security
audit was not rerun; no dependency/lockfile change belongs to this slice.

Final package ZIP3,483,757 bytes, SHA256
`d411f7227345593c65fab1ae62880fdfe6cb01caedea43258314bc8c715317dc`;
ZIP EXE and README match the delivered files, with no live configuration/state.
