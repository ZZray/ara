# HandWave0.2.4 native terminal release

Requirement: complete native PTY/ConPTY before scoped commit, push and deployment.
No new product features are admitted. This compact Host point is independent of
Shared Core/OMP parity. Rust base6e43dba, Go basea2500346af569857fe47777c825c5332acb2e360.

## Contract

`terminal_process.rs` owns native Windows ConPTY/Job Object and Unix PTY/session;
`terminal_emulator.rs` uses pinned vt1000.16.2 to answer live DSR5/6/private6,
primary/secondary DA and ESC Z. This is not a claim of every xterm extension.
`managed_jobs.rs` integrates one merged artifact, bounded input/control journal,
original Goal lease, cancellation, automatic protocol replies on the same writer
and supervised input/output failures. `job_control.rs`/transport expose no new
spawn authority to the browser. Existing noninteractive hashes remain valid.

Ray vendors official npm xterm6.0.0 and addon-fit0.11.0 with MIT notices. It starts
read-only, replays without protocol writes, then explicitly attaches user input.
A fixed-version CoreService adapter distinguishes keyboard/IME/paste from parser
responses; missing support disables attachment.80ms batches are ACK-serialized;
visible attached output uses bounded150ms polling and immediate ACK reads.

Authorization/configuration/identity/other services, model routing, avatars,
Shared Core and OMP markers are preserved. Secret input is not persisted by the
direct control channel, but PTY ECHO or child output can expose it in Job logs.
Stable IDs and metadata-only recovery prevent unknown input replay.

## Evidence status

Current evidence root: `D:/WorkSpace/tmp/handwave-terminal-20261016`.
Native harness: `D:/WorkSpace/tmp/handwave-terminal-native-tests`.
The earlier pipe-only evidence remains in interactive-0.2.4-20261016.md and cannot
prove this native implementation. Final check receipts and publication status
will be appended after execution. Until then this point is not marked accepted.

## Boundaries

Windows ConPTY and WSL Linux are the target runtime platforms. macOS, ARM/musl,
every third-party CLI/login flow, real physical-phone keyboard and forced-power
loss are not claimed. No desktop observation/click API is added. Child setsid
escape on Unix is outside the owned terminal session; tool allowlists are not
an OS sandbox. Worker loss preserves unknown effects and never replays a child.


## Final software acceptance (before publication)

Independent Codex `/root/interactive_review` inspected the plan and final scoped
Rust/Go/xterm diff, independently ran console8pipe+8actual-vendor tests, then
accepted this bounded local software point from the executed receipts. Publication
remains a separate gate below. No Claude/Bridge reviewer was used.

| Delivered-code check | Executed result |
| --- | --- |
| Standalone `cargo test --locked --workspace --all-targets` |157/157 PASS,0ignored,7.83s (`rust-tests-final.log`) |
| `cargo fmt -- --check`, all-target/all-feature clippy `-D warnings` |PASS (`fmt.log`, `clippy.log`); standalone workspace, binary-only/no library doctest target |
| cached `cargo deny --offline check` |advisories/bans/licenses/sources PASS; no fresh advisory database claim |
| Final Windows Node + actual Hub + Python terminal |2 groups PASS; real TTY on all3streams, initial no-browserCPR,103x37resize/idempotency, same child after reconnect, scoped denial, Ctrl+C/EOF and zero model polling |
| Final Windows same-EXE terminal workers |7groups PASS: PowerShell Read-Host, CMD set /p, timeout, lease, quota, input/log faults; faults interrupt promptly rather than waiting for the lease |
| Final pipe / detached Job regressions |4groups/9groups PASS (`release-pipe-final`, `release-jobs-regression`) |
| Actual `deepseek-v4.1-flash` / `openai-completions`, terminal=true |PASS on final image;5HTTP attempts,36.548s, actual sum42/count3file and Job-log reads, fresh granted slices, `awaiting_review`; supplier usage stays unknown |
| WSL Linux native / emulator / managed Jobs |6/6,6/6,12/12PASS; actual Linux Hub/Node terminal chain2groups and shell/failure worker6groups PASS |
| Linux source bridge |29runtime inputs match exactly: allsrc/*.rs plus Cargo manifest/lock; source tar4ffb36dd and tested ELF e2ad0711 linked by `linux-source-bridge.json` |
| Linux clippy |NOT RUN: existing toolchain lacks the component; Windows complete-workspace clippy is passing |
| Final clean Go export |full Hub/Ray tests/vet/rewritten-app parse and Win/Linux builds PASS; after final source-package pin update, Ray tests/vet/parse/build rerun and all17overlay hashes match actual source |
| Browser component |actual keyboard/Chinese input, explicit attach, dark and light375pxcontainer PASS; screenshots browser-terminal-dark.png / browser-terminal-light-narrow.png. Transport is simulated, separate from final live acceptance |

All final Windows `release-*` commands use immutable
`D:/WorkSpace/tmp/handwave-terminal-20261016/handwave-node-final.exe`:

```text
python -X utf8 apps/handwave-node/tests/verify_terminal.py --binary <immutable-exe> --hub-binary <evidence-root>/handwave-hub.exe --output <evidence-root>/release-terminal-final
python -X utf8 apps/handwave-node/tests/verify_terminal_worker.py --binary <immutable-exe> --output <evidence-root>/release-worker-final
python -X utf8 apps/handwave-node/tests/verify_interactive.py --binary <immutable-exe> --hub-binary <evidence-root>/handwave-hub.exe --output <evidence-root>/release-pipe-final
python -X utf8 apps/handwave-node/tests/verify_jobs.py --binary <immutable-exe> --output <evidence-root>/release-jobs-regression
python -X utf8 apps/handwave-node/tests/verify_long_goals_live.py --terminal --binary <immutable-exe> --hub-binary <evidence-root>/handwave-hub.exe --output <evidence-root>/release-live-model-final
```

Final EXE:5,151,744bytes, SHA256
`1396bffa848d7332b369281a13df7a1d4067967ab6dda5fa0063246249fc3e23`.
Public candidate ZIP:3,647,990bytes,560allowlisted files, SHA256
`d311f699dafa05caa66b00e9914bc21f6b6cab4f62973d528bd66817618800a1`.
Its EXE matches the immutable tested image. No VC runtime DLL dependency was
found in normal/delay imports. Required Windows version is10 1809+ or11.
Locked license collection covers275packages/532notice texts, with no missing
original text. Official xterm/Fit MIT notices are separately shipped in Ray.

Source candidate: `handwave-node-source-4ffb36ddd1c069f9.tar.gz`,499,364bytes,
SHA256 `4ffb36ddd1c069f951350e4b08a847e28918e28ca2a354d78c139284285d5fcb`.
All19previously published package files are preserved unchanged. The installer
script remains its existing immutable fe4b10be version; only its default source
URL/checksum and current Windows package are advanced.

## Retained intermediate failures

- The old resume-tool count assertion expected5 instead of7after terminal tools
  were introduced; corrected to the actual7with explicit resize/signal checks.
- The800-entry journal fixture did not cross256KiB;1000restored entries plus4
  actual dispatches verify the boundary and2048operation quota.
- The new service-fault fixture initially reused a running receipt and was
  correctly denied as duplicate dispatch. It now creates an initial receipt;
  product replay protection was not relaxed.
- Windows atomic pending-file publication denied a privacy scan open once;
  the fixture now retries only bounded publication errors and permits removal
  of transient pending files. Original privacy/content assertions remain.
- Packaging rebuilt the release image from4f40f960to1396bffa. All listed final
  process/model regressions were rerun on1396;4f40results remain intermediate.
- Initial Go export pins predated the last UI replay fixes. Final export pins
  all17overlays, isolates twoHub routing hunks and excludes unrelated dirty auth/
  legacyWeb. Final installer-pin-only updates received fresh Ray checks/build.

## Publication

Software point accepted. Scoped commit/push, deployed binaries, public download
verification and installed-device/browser receipts are pending at this snapshot.
