# HandWave0.2.1 context telemetry and calendar integration

2026-10-16, base `01f6e756`. Only this compact standalone product changes;
unrelated root Rust Core/OMP work remains excluded. The runtime adds measured
working-projection and complete-request JSON byte observations. Compression
algorithm, model route, source history and adoption semantics remain unchanged.
The Go calendar reuses the existing native paused-Goal resume command; no
calendar engine or future-prompt authority is added to Rust.

Final Windows EXE:4,931,072bytes, SHA256
`626f4b669bcef542bdf0a9a83eb9b6dcf7597716ef2e9f7694bdddc7156844f9`.
Explicit static-CRT release build; actual PE imports checked, no dynamic CRT.
Cargo fmt/Clippy and118 unit tests PASS; frozen deny policy PASS (cached advisory
snapshot, existing dependency duplication warnings). Commands used the isolated
`apps/handwave-node` workspace and did not format/build the root migration WIP.

`tests/verify_context.py` on this exact EXE plus the clean maintained Windows
Hub passed15 actual-process groups in136.163s. The added assertions observe
nonzero ready/compressing receipts before real controlled summary HTTP, shared
request budget measurements, durable summary completion, original history,
restart and invalid/canceled/budget-exhausted summary failures. Connection-reset
stderr at idle test HTTP cleanup remains in the original output; assertions and
fixture result passed.

`tests/verify_schedule.py` passed in20.900s on the exact EXE and clean Hub
SHA`98f1b0d5726a732a8c7e34bdff25ccb3f562784a0e99dedc491e9d93632f8939`.
Four controlled model calls/two native slices, original root budgets; a closed
controller was followed by cloud tick resume, exclusive Python artifact
`sum42/count3`, then awaiting_review. Reconnected list reports resumed, not
accepted. Cancellation, wrong writer and expiry do not add model calls. Its
final paused state belongs to subsequent negative setup, not a failed task.
Fake-provider process evidence does not prove real-model intelligence.

Independent Codex `composer_goal_ui` reviewed the three Rust source files,
fixture and browser context/memory/calendar modules using ara-git-review and
ara-rust-core-review. It found a UI gate that lost autonomous Goal slice
telemetry. The scoped current-mirror observer fix passed independent re-review;
task controls retain their stricter existing ownership gate. Separate Codex
`context_memory_ui` reviewed composer/menu and closed an Escape-aborts-Run P2.
The primary agent inspected these source boundaries and actual result files.

Production Node was updated preserving identity, configuration and startup.
Normal authenticated browser checks cover measured context2.6/28.0KiB and
request15.4/56.0KiB, separate goal editing, original input preservation, memory
folding, Plan entry,390px light editor and1280px desktop without overflow.
The actual Auto trial FAILED at an Nvidia upstream quota limit16/16; no retry,
successful chat or real-model calendar acceptance is inferred. Its test Goal
was stopped, while the previous0.2.0 Goal remains awaiting_review. The calendar
production UI list was exercised; production creation/trigger was NOT RUN.

Evidence root: `D:/WorkSpace/tmp/handwave-ui-goals-20261016/`; context-process and
schedule-process result.json, go-checks.json, two independent POST reviews,
PE/import and package receipts, actual-browser screenshots and deployment
receipt. Package/runtime hashes and cloud preservation are also recorded in
the Go project's `docs/evidence/handwave-ui-goals-20261016.md`.

New source URL: `handwave-node-source-a256889b8dcf6b6d.tar.gz` (SHA256
`a256889b8dcf6b6df996ca79c22f88c815d9a490264c1c6722e8dc6c34065795`),
Windows ZIP `handwave-node-windows-626f4b66.zip` (SHA256
`7238af79d1c5d92ab068fee2a4e4938c7d37f72e297095eb21e31c7ba8a9cbd6`).
Installer unchanged; source manifest pins runtime files, and closeout docs are
later. This patch does not rerun Linux/ARM/macOS compilation, power-loss testing,
1M exact token capacity or provider billing/cache economics. Calendar supports
one resume of an existing paused leased Goal, not recurrent/new-prompt tasks.
