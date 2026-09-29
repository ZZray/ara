# V1-COMPACT: basic compaction from the A3 pieces (WIP)

## Scope

Single-level soft compaction for daily-driver v1 on OpenAI-compatible Chat
(`--api openai-completions`). A threshold from an explicit host estimate
(`--compact-threshold`, default 32,000 estimated tokens) plus a manual
`/compact` REPL command. The summary is persisted in the Session journal with
source entry IDs; raw history stays in the journal and resume reads the
projected context. A second compaction on an already-compacted session is
refused: chained summaries need the still-open A3 persistence decision, so V1
keeps one level. This is a reference-host subset, not full A3 parity.

## Code

- `crates/ara-session`: `append_compaction` (kind `compaction`, `method:
  soft`, `summary`, `firstKeptEntryId`, `sourceEntryIds`, `tokensBefore`) and
  `model_context` (latest valid projection; summary sent as a lower-trust
  user message, kept/later messages raw; falls back to `build_context`).
- `crates/ara-cli`: `--compact-threshold`, `--compact-keep-tokens`,
  `/compact` in the REPL, auto-compact after each completed turn (REPL and
  print), startup/resume context via `model_context`.
- Reuses A3 WIP unchanged: `compaction_source_snapshot`,
  `select_whole_turn_cut`, `summarize_sources` (1024 output tokens, 120 s
  deadline), strict projection validation.

## Real-model trial (2026-09-28, `openrouter/free`, OpenAI-compatible)

1. Clean session, two completed turns (fix `add.py` `a-b` to `a+b`; read
   `notes.txt`, maintainer Li). Both exit 0.
2. Third turn with `--compact-threshold 100 --compact-keep-tokens 60`:
   `ara: compacted 555 estimated tokens down to 343; summary persisted with
   source IDs`, exit 0. Journal holds the `compaction` entry with the fix and
   the maintainer in its summary text.
3. Fourth turn (`--continue`, auto-compact off): maintainer/fix question
   answered from the summary-only prefix (thinking receipt carries both
   facts; the free model returned no text block, so print showed nothing).
   Exit 0, same session file.

## Boundary observations

- A session whose tail holds an interrupted/unknown-effect tool call yields
  no valid cut (the A3 unknown-effect rule); the host reports nothing on
  auto and keeps the session. A budget-aborted first turn poisoned the first
  trial session this way; a clean session compacted fine.
- `default_session_dir` now also replaces `?` (verbatim `\\?\` prefix made
  the default dir illegal under Windows; found during this work).

Checks on the final code: workspace Clippy for the touched crates with
`-D warnings`, `ara-session --all-targets` suites green, `cargo fmt`
applied. Full backend gate and V1-TRIAL hardening remain open; formal counts
unchanged.
