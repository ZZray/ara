# Windows context-discovery depth and test portability (WIP)

Scope: make local Windows verification of the existing context-discovery behavior meaningful without requiring Ubuntu. The only product change is `ara-discovery::paths::calculate_depth`, which now counts host path components instead of splitting on `/`. `Discovery::context` supplies normalized paths; the root and drive components cancel when comparing a cwd with one of its ancestors. Fixed OMP `packages/coding-agent/src/discovery/helpers.ts:616` uses `path.sep` for the corresponding depth calculation. The project-root and context precedence tests exercise the result on a real Windows path.

The test-only changes retain FIFO and symlink scenarios on Unix, WSL scenarios on Linux, and common skill precedence on Windows. The vendored `pi-edit` change only gates an import used by a Unix-only test. No tool process behavior, Session write flow, or CLI resume path changed.

Verification on the candidate worktree based on `b37906a` (Windows):

| Command | Result |
| --- | --- |
| `cargo test -p ara-discovery` | 4 library + 8 frontmatter + 8 project + 11 skills + 19 upstream tests passed; doc tests passed. |
| `cargo clippy -p ara-discovery --all-targets --all-features -- -D warnings` | Passed. |
| `cargo fmt -p ara-discovery -- --check` | Passed. |
| `cargo test -p pi-edit --test replace_parity` | 11 tests passed. |
| `cargo clippy --workspace --all-targets --all-features -- -D warnings` | Still failed in existing Windows `ara-tools` warnings (`bash.rs` unused process-group values and `write.rs` unused `mut`); the previous Discovery Unix-API and vendored import errors were cleared. |

Independent Codex review of all five changed files against `b37906a` applied `ara-git-review` and `ara-rust-core-review` and found no blocking defect. The reviewer also ran both crates' strict Clippy, the focused tests, and `git diff --check`; all passed. Unix-only tests and the full backend verifier have not passed on this candidate. CTX-01d and overall roadmap acceptance remain open.
