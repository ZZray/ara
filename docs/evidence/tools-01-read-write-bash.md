# TOOLS-01 — `ara-tools` read, write, bash

Point / requirement / exclusions:
Port the default behavior of the OMP `read`, `write` and `bash` tools with real file and process effects.

Excluded (open):
- `read`: hashline headers (tied to the edit tool), structural summaries, multi-range selectors, archives, SQLite, PDF, notebooks, URLs, internal URIs.
- `write`: archive and SQLite targets, auto-generated guard, LSP and ACP bridges.
- `bash`: persistent shell, PTY, async/auto-background, artifact spill, interceptors, Windows.

Upstream SHA and source/test location:
`596f2da7101178214aa27a753529d15e6b7ad91d`.
- Source: `packages/coding-agent/src/tools/{read,read-format,write,bash}.ts`, `exec/{bash-executor,non-interactive-env}.ts`, `session/streaming-output.ts`.
- Behavior tests: `test/tools/*`, `test/read-*`, `test/write-*`, `test/bash-*` (CA-TOOL-READ/WRITE/BASH surfaces; 596 upstream cases in total, most tied to unported features).

Delivered commit or exact worktree snapshot:
The commit adding this file on `dev`, with parent `cf21722`.

Rust entry and host chain exercised:
`AgentTool::execute` on `ReadTool`, `WriteTool` and `BashTool` against real temp directories, files, FIFOs, symlinks and `bash` processes. The loop and CLI chain are covered in CLI-01.

Environment and sanitized commands:
- `cargo test -p ara-tools`: 6 unit + 10 integration tests; 3 repeated runs, each passing in 4.0 s.
- `python scripts/verify_backend.py`: PASS.
- `cargo deny check`: ok (added `base64`, `libc`; MIT/Apache).

Expected vs actual normal result:

read:
- Full file, `:2-3` with the continuation notice `[2 more lines in file. Use :4 to continue]`, `:-2`, and `:4+5`.
- `N|text` numbers when enabled; `:raw` drops them.
- 5000-line file: capped at 3000 lines with `[2000 more lines in file. Use :3001 to continue]`.
- Wide file: byte cap applied.
- Directory listing, with a `/` suffix on directories.
- Image files return image content.
- `~` expansion and `.`/`..` normalization.

write:
- Parent directories are created.
- The result reads `Successfully wrote 7 bytes to deep/nested/x.txt` for `héllo😀` (UTF-16 count, as upstream).
- A new shebang file is made `chmod +x` and the upstream notice is added; an existing file keeps its mode.

bash:
- stdout and stderr are merged in order through one pipe (`a`, `b`, `c`).
- `env` and `cwd` are applied, together with the non-interactive environment (`PAGER=cat`, `GIT_TERMINAL_PROMPT=0`).
- Empty output returns `(no output)`.
- 20000 lines of output are tail-truncated with `[Showing lines …-20000 of 20000]`.
- A 30 MB output is bounded in memory and in the result.

Actual: as expected.

Expected vs actual failure/cancel/recovery result:

read:
- `File not found: missing.txt`.
- Beyond-EOF guidance.
- `Invalid selector ':0' on 'one.txt'`.
- A dangling symlink named `log:1-2` stays a literal path.
- A FIFO fails with `not a regular file` instead of hanging.
- A binary file gets the sniff message; a stray invalid byte beyond the first 8 KB decodes lossily instead.
- A 200 KB single line returns a 50 KB preview plus a notice.

write:
- A directory target returns an error.

bash:
- `exit 3` gives an error result with `Command exited with code 3` and `exitCode: 3`.
- `kill -9 $$` gives code 137.
- A 1 s timeout stops the call in under 2.8 s and returns `[Command timed out after 1 seconds]` with `timedOut: true`. The background child in the process group is killed; its marker file never appears.
- A normal exit also kills a lingering background child. The call returns in under 1.5 s and the child's marker never appears.
- Dropping the call future kills the group through the drop guard.
- Cancelling after the live update containing `tick 1` returns `Err("tick 1…[Command aborted]")` in under 4 s.

Actual: as expected.

Streaming observation before completion:
`bash_cancel_aborts_and_streams_updates` sees the `tick 1` partial update while the process is still running, and only then cancels.

Real-model route:
Not applicable to this tool point. It is exercised through the host chain in CLI-01.

Independent review and semantic findings:
The Claude Code `code-review` skill (forked, high) reported 10 findings. All were fixed and re-tested:
- Unbounded output buffer: now a bounded tail sink with exact totals.
- Background children holding pipes and leaked pump tasks: the group is killed at every end, with a single reader thread.
- Dropped future left the group alive: fixed with a drop guard.
- Oversized first line returned whole: now a bounded preview.
- Whole-file UTF-8 check: now an 8 KB sniff plus lossy decode.
- Unbounded read and uncancellable FIFO: reads are streamed with cancel checks, and non-regular files are rejected.
- stdout/stderr reordering: fixed with a single pipe.
- Signal exit threw an error: now 128+n.
- UTF-8 split after an invalid byte: fixed with an incremental decoder.
- No `~` expansion or normalization, `exists()` probe, and generic errors for invalid selectors: fixed with `~`, lexical normalization, an lstat probe, and `Invalid selector` errors.

Unrun/blocked checks and impact:
- Windows is not supported: bash and group kill are Unix-only.
- The memory bound on huge output is checked by a unit test and a 30 MB run, not profiled.
- Tools are not a sandbox; permission gating is the host's job (`before_tool_call`).

Decision: tested. Not accepted (host chain, real-model trial pending).
