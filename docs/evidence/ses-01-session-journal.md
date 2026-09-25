# SES-01 — `ara-session` JSONL journal

Point / requirement / exclusions:
Port the OMP v3 session file:
- title slot, header, and `id`/`parentId` entries
- lazy materialization
- per-entry append
- malformed-record skip plus rewrite before the next append
- corrupt-header rejection
- branch reconstruction
- `model_change` entries

Also add ARA recovery for interrupted tool calls.

Excluded (open): compaction and branch-summary context, labels, custom-entry semantics, v1/v2 migrations, alternate storages, listing/search, fork/move, titles, blobs.

Upstream SHA and source/test location:
`596f2da7101178214aa27a753529d15e6b7ad91d`. Sources: `packages/coding-agent/src/session/{session-manager,session-entries,session-title-slot,session-loader,session-migrations}.ts`.

Behavior tests: `test/session-manager-immediate-persist.test.ts`, covering:
- B-74f01cedc8: immediate visibility
- B-af6346b2a8: malformed tail rewrite
- B-c1d3d6809c: corrupt header untouched
- B-405741c2e3: pre-assistant session not written

And `test/session-manager-on-disk-8860.test.ts`:
- B-0e65df2ed1: lazy session not on disk

Delivered commit or exact worktree snapshot:
The commit adding this file on `dev`, with parent `959c435`.

Rust entry and host chain exercised:
The `ara_session::SessionJournal` API against real files in temp directories. The CLI host chain is covered by CLI-01.

Environment and sanitized commands:
- `cargo test -p ara-session`: 13/13 pass.
- `python scripts/verify_backend.py`: PASS.
- `cargo deny check`: ok.

Expected vs actual normal result:
- Lazy session: no file until the first assistant message.
- The first line is exactly 256 bytes.
- Header is `version: 3`; entries chain `parentId`; ids are 8 hex characters.
- Each later append is on disk when the call returns.
- Reopen restores the branch messages, the model, and the leaf; the next entry's parent is the old leaf.
- Foreign `label` entries survive a rewrite verbatim and are not model context.
- Duplicate ids resolve to the last line; both lines are kept.

Actual: as expected.

Expected vs actual failure/recovery result:
- Torn tail: 1 malformed record is reported and the file is not touched on open. The next append does an atomic rewrite, and the backup is byte-identical to the torn original.
- A corrupt header, and an unsupported v1 header, are rejected with the file bytes unchanged.
- A missing middle record stops the branch walk; the session resumes with the suffix.
- Crash between a tool call and its result: `recover_interrupted_tool_calls` pairs `c2` with an `interrupted_unknown_effect` error result, journals it, and is idempotent. Nothing is executed.
- A tool result that ARA cannot decode still counts as answered, so there is no duplicate.
- An unpaired call followed by a user message is reported in `unpaired_earlier` and not appended out of place.
- A failed append (the path replaced by a directory) returns `Err`. Memory is rolled back, and the next append rewrites fully. No temp files leak.

Actual: as expected.

Real-model route:
Not applicable to this storage point.

Independent review and semantic findings:
The Claude Code `code-review` skill (forked, high) reported 10 findings. All were fixed and re-tested:
- v1 files were silently erased → versions are now rejected.
- A missing parent was a hard error → the walk now stops there.
- No directory fsync after rename → added.
- The backup was not synced before rename → synced.
- Recovery used decoded messages → it now uses raw entries.
- The in-memory append survived a failed persist → rolled back.
- Title `source`/`updatedAt` were lost → preserved.
- Recovery could append non-adjacent results → now reports them instead.
- Duplicate ids: first line kept → last line kept.
- The temp file name collided and leaked → unique name, cleaned up.

Unrun/blocked checks and impact:
- Power-loss durability is argued from the fsync sequence but has not been exercised on real hardware.
- Directory fsync is a no-op on non-Unix platforms.
- OMP v1/v2 sessions cannot be opened until migrations are ported.

Decision: tested. Not accepted (host chain and restart through the CLI are pending in CLI-01).
