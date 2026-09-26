# AGT-COMPACTIONc: strict Session source snapshot (WIP)

Fixed OMP source: `596f2da7101178214aa27a753529d15e6b7ad91d`, `packages/agent/src/session/session-manager.ts` path traversal and `packages/agent/src/compaction/compaction.ts` context construction. ARA keeps the existing tolerant `SessionJournal::branch()` and `build_context()` behavior. The new `compaction_source_snapshot()` is a read-only gate for a future compaction writer: it returns decoded messages with their actual journal entry IDs, session ID, and leaf ID from the loaded journal.

The snapshot refuses a lazy or unrepaired journal, a journal loaded with invalid UTF-8, duplicate or empty entry IDs, a missing, cyclic, or forward parent, invalid raw `parentId`, undecodable message, or an entry type that can change model-visible context but has no ARA projection. It skips only known non-context `model_change` and `label` entries. It takes the current branch in root-to-leaf order and does not write, repair, select a cut, or invoke a model. The returned leaf is only an in-memory observation; a later writer must lock, re-read, and compare the durable leaf before it commits a summary. A structurally valid journal is source evidence, not cryptographic authentication of its contents. If old write behavior has already replaced invalid UTF-8 and the file is freshly reopened, the original bytes cannot be recovered by this API.

Windows checks on this candidate:

| Check | Result |
| --- | --- |
| `cargo test -p ara-session --test compaction_source` | 7 passed on Windows. Covers reopened journal and source IDs, branch selection, duplicate IDs, missing/forward/cyclic parents, invalid raw parent and empty ID, undecodable messages, lossy UTF-8, unported context entries, and no disk change or backup on rejection. A separate Unix-only case exercises reading immediately after successful rewrite; it has not run locally. |
| `cargo clippy -p ara-session --all-targets --all-features -- -D warnings` | Passed. |
| `cargo fmt -p ara-session` | Passed. |
| `cargo test -p ara-session` | On the final candidate, 7 new tests passed; 10 existing `journal` tests passed and 3 existing rewrite tests failed with Windows `Io(PermissionDenied, code 5)` at `journal.rs:106,168,240`. This read-only slice did not change rewrite logic. The full Session suite is not green on this machine. |

An independent Codex reviewer checked the fixed OMP traversal and the code/test diff. It found that historical `malformed_records` would wrongly reject a successfully rewritten journal and that `open()` could silently replace invalid UTF-8 before strict reading. Both findings were fixed; the reviewer reran the seven focused tests, strict Clippy and diff check and found no remaining P0/P1 issue. It did not review the documentation or run the Unix-only rewrite regression.

Open: source snapshot consumption, safe cut, Session write locking and durable leaf recheck, compaction entry persistence, restart projection, manual/automatic host flow, real long-context trial, and full backend delivery gate. Neither AGT-COMPACTION nor A3 is accepted.
