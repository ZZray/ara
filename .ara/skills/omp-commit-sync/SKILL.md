---
name: omp-commit-sync
description: Compare a fixed Oh My Pi commit with the ARA Rust implementation or a later exact upstream SHA, maintaining parity markers and source-backed evidence.
---

# Sync a fixed OMP commit

1. Read `upstream/omp.lock.json`, `docs/upstream-sync.md`, and `docs/upstream/feature-ledger.md`. Verify the exact target SHA and tag in a separate OMP checkout; never import its `.git` history.
2. For the initial port, inventory the complete fixed commit from upstream source and tests. For an update, compare the old `ported_through_commit` with a chosen new SHA using `git diff --name-status` and source/test hunks. Record added, changed, removed, and unchanged behavior in `docs/upstream/changes.md`.
3. For each bounded behavior, map source location and externally observable semantics to Rust owner, protocol/host adapter, intentional difference with reason, normal and failure/cancellation tests, and actual output. Add the row before claiming a percentage.
4. Implement without silently dropping OMP behavior. Execute direct Rust process/host tests and source-backed fixtures; selected integration points also need the bounded real-model task described in `docs/acceptance.md`.
5. Request independent audit of the tested diff. Advance `reviewed_upstream_commit` after source review. Advance `ported_through_commit` only when the complete selected inventory through that SHA has passed execution and audit. Leave partial work visible with the marker unchanged.
6. Retain MIT notices for copied upstream material and review any additional third-party dependencies separately.
