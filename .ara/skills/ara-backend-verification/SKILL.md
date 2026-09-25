---
name: ara-backend-verification
description: Execute and record strict Rust backend tests for one ARA delivery point, including real host effects and relevant failure paths.
---

# Verify a backend delivery point

Use after implementation and before [point delivery audit](../point-delivery-audit/SKILL.md). Name the observable behavior, delivered commit/worktree snapshot, exact entry point, and mandatory evidence. Run tests on that snapshot; re-run affected checks after any fix.

1. When a Rust workspace exists, run applicable `cargo fmt --all -- --check`, `cargo clippy --workspace --all-targets --all-features -- -D warnings`, and `cargo test --workspace --all-targets`. If feature combinations are mutually exclusive, run the supported matrix and record why `--all-features` is invalid. Include documentation tests and relevant target/platform checks where behavior requires them. A missing manifest is `not runnable`, never a pass.
2. Exercise the actual Rust binary/host path with a controlled fake upstream. Verify request/stream/tool cycles, state journal, IDs, budgets, outputs, and file/process effects. Inject a relevant malformed/partial response, denial, cancellation, timeout, restart, or crash window; inspect recovery and unknown effects. A mocked internal function does not prove the host chain.
3. For streaming/background changes, observe a client event while the Run is still active. For concurrency, use a repeatable fault or interleaving test and inspect the resulting state; a single happy-path run is insufficient.
4. Where dependencies exist, run `cargo deny check` against a reviewed policy and lockfile to examine advisories, licenses, bans, and sources. Record unavailable policy/tool or findings. Do not invent a passing dependency audit before `Cargo.toml`, `Cargo.lock`, and policy exist.
5. For selected integration points, run a bounded real task through a currently verified CAS or OpenRouter route. Set maximum calls, turns, tokens, and time before execution. Check model ID/protocol, actual artifact, tool decisions/results, continuation, usage, latency, and final Task/Run states; redact credentials. `HTTP 200` and a plausible answer are insufficient.
6. Write commands, exit codes, expected/actual results, artifact paths/hashes when safe, failed or unrun checks, and evidence limits in `docs/evidence/<point-id>.md`. Keep the point open when mandatory evidence is missing. Do not relax test expectations to make a failure pass.
