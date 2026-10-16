# HandWave 0.2.5 coordinated security deployment

2026-10-16. This publication follow-up supersedes the pre-deployment boundary in
[local hardening evidence](security-hardening-0.2.5-20261016.md). Scope is the
independent HandWave Host, Hub and Ray; root ARA Core/OMP delivery is separate.

## Sources and published artifacts

Rust runtime: `59d8b766358a5cdb7471b88baf73bb72be0e25da`; source package:
`d3a6bf2a76f97c5655afc5b0bdcf6551a831fc08` (README-only follow-up). All 31
runtime/Cargo inputs match the Linux verification image. Go security source:
`2de49cac59139cdf43129c57e6025448af887d68`; publication defaults:
`fd182737defa012143cc64e017371f57538224ff`. Builds use clean exact exports and
the versioned, hash-verified stable Web archive; unrelated auth/Web WIP is excluded.
Final adapted app remains `c30133ba09e36710d997272e925df079b536b983ca85cccd86bf32302b3f9e2f`.

| Artifact | SHA256 |
| --- | --- |
| Installed Windows Node, 5,164,544 bytes | `8b01e4384e7e39aec7149ff9b75dbbac3ab79eff80478ece871402e2f60d439e` |
| Running Linux Hub, 21,209,250 bytes | `3e9f5d80601729d64cfef2436c066bcb1a74a014ad8a814fba442254a7360745` |
| Running Linux Ray, 50,331,810 bytes | `413ab911fbba1dcaec04666294288f20c57630d83dad7c6de2e540be26cb5606` |
| `handwave-node-windows-8b01e438.zip`, 3,657,023 bytes | `319bfc8d00c440de948e22dc8f3f1beb5a0cfacfa8fb69f3bd8a6d5f9252eca4` |
| `handwave-node-source-634560d930572dcb.tar.gz`, 504,449 bytes | `634560d930572dcb3191b8835ae78cf00d8176e05ecf8124c4598b51005a6ed1` |

Both public artifacts are under `https://118.25.103.226/ray/releases/`.
`/ray/install.sh` now pins the new source URL and SHA. The immutable installer
script itself remains `fe4b10bee6a73659f6c5db78e624aa794bec0e0b57f91619d8be13c4f694094c`.
Twenty historical immutable artifacts remain byte-identical and reachable;
22 total public downloads were checked. Packages contain no live node.conf,
private seed, state or enrollment credentials. Ray tests (4.057s), vet and diff
checks passed on the exact publication overlay.

## Coordinated transition and preservation

Evidence root: `D:/WorkSpace/tmp/handwave-deploy-025-20261016/`. Machine-specific
paths, private backups and raw receipts remain local / on the deployment host.

- `deploy-receipt.json`: exact Hub/Ray process image hashes verified after atomic
  replacement and restart. All 49 freshly enumerated authentication, config,
  history and platform files remained identical at the binary-switch boundary.
  OMP edge/gateway, RainFlow and Arena PID/image/source checks remained unchanged.
- `installed-stage.json` / `installed-receipt.json`: no live Run/Job, unchanged
  state path set, pending-run and child-process checks preceded stopping the
  exact old Node. Private ACL-protected backup, handle wait and same-directory
  replacement preserved 66 other existing state files. The sole configuration
  change was HTTP to HTTPS for the existing Hub. Identity seed/display FP remain
  unchanged. No automatic tool replay occurred.
- `reapproval-receipt.json`: the actual signed RAYWORK (`U75VP37OFSBBK`) was
  admitted via public verified WSS, then reapproved through an authenticated
  controller with a complete 32-byte public-key pin. The previous grant was still
  live. Its expiry changed from 2026-10-17 08:45:04.381546838+08:00 to
  08:44:04.315193558+08:00, conservatively preserving the remaining authorization
  rather than granting another 24 hours. Aliases, denials and unrelated approval
  state were preserved; this device's old controller owner intentionally changed
  to the server-derived authority.
  A later acceptance-cleanup UI action temporarily renewed this grant; the
  correction and final expiry are explicitly recorded below. This initial
  checkpoint does not prove the expiry never changed afterward.
- Existing common QR and browser session survived. Reloading the real IAB
  displayed connected RAYWORK and the server-assigned controller identity.
  Old-owner Goals were not migrated or resumed automatically.
- `public-check.json`: default certificate validation succeeds; login200,
  unauthenticated app303, plaintext machine URL308, hostile Origin403, QR401;
  installer pins and both new public artifact hashes match. No TLS bypass. The
  real already-authenticated browser separately visited /ray/qr in a temporary
  tab and displayed only the private-server-export refusal; no QR/seed appeared
  (`authenticated-qr-blocked.png`). Public308 is the existing outer OMP ingress
  redirect, not a claim of public426.

Rollback is coordinated across Hub/Ray and the stopped Node/config; binary
rollback never restores all user data. Before reapproval, the original grant
remains untouched. A rollback after reapproval would additionally require a
compare-and-restore of only this device's exact old approval, preserving its
original absolute expiry and refusing concurrent changes. No rollback was needed.

## Linux software verification and explicit limits

`linux-verify/linux-verification-summary.json` records actual GNU Linux behavior.
The Node image is a debug verification build, SHA256
`306e29c294422ec9c73de6c4c33aa3e6192588e6d0024d40181caa81e4df8cac`; it is not a
new optimized Linux binary download. Source installation remains the public path.

- Signed identity / unsigned replacement rejection / Node and Hub restarts:4/4.
- TLS trusted/untrusted direct and CONNECT proxy, wrong host, redirect refusal,
  Origin/controller/cryptographic negatives:PASS; Go tests23.393s.
- Actual native PTY chain2/2 (26.973s), terminal workers6/6 (8.484s):TTY, live
  headless cursor response, resize, same-child reconnect, scoped input/control,
  EOF and cleanup; controlled upstream, not real-model intelligence.
- Real separate pipe worker stayed running after12s (beyond10s startup grace),
  accepted exactly64KiB, rejected overflow, accepted zero-byte EOF and produced
  an independently read exact65536-byte artifact. No model polling.

Full Linux unit suite remains **157/159**, not all green. The unreadable-file
fixture assumes a non-root process; the exact image passes it as nobody with
TMPDIR=/tmp. The input-quota unit fixture directly calls execute_worker without
holding worker.lock, so status correctly marks it interrupted after startup
grace. The production worker takes that lock and the actual after-grace process
test above passes. These fixture defects remain open; no product behavior or test
assertion was relaxed for deployment. Windows160/160 and prior actual ConPTY
acceptance remain recorded separately in the local hardening evidence.

## Production task acceptance

The first real browser Goal `63ZQB7EMK4` used Auto and reached its120s Run deadline
before any tool call. Afterward pending_run was cleared, Goal was failed, no new
Job receipt existed and no Node child remained. It was not replayed or reported
as successful. Journal correlation confirms model=auto and120.050s lifecycle;
request-level Gateway logs are absent, so the selected provider / exact upstream
failure is unknown. A recent DeepSeek health probe passed; it does not explain
that particular Auto timeout. No routing change was made for this deployment.

The retained intermediate experiments also do not count as success:

- B (`SBDU665YOI`): explicit DeepSeek returned in7.169s and correctly declined
  job_start because the operator enabled background continuation but omitted the
  separately revealed allow-jobs checkbox. It was stopped, not accepted.
- C: attempting a different job-authority binding in B's existing Session was
  rejected before tool execution with `session mode differs from saved binding`.
  The correction was a genuinely new Session, not replacing the saved binding.
  Independent final inspection found an empty Hub lease (`X3CFC226PM`, zero
  Runs/calls/commands, no Node Goal) reserved before that rejection. The operator
  used normal UI disconnect/bye to revoke this server-controller/device's
  residual test leases. The subsequent UI button labeled "接入" unexpectedly
  issued a default24h approve, temporarily renewing expiry to2026-10-17
  18:14:01.84330192+08:00. Independent review detected it before release
  acceptance. An authenticated, device-scoped setttl then retracted the expiry to
  **2026-10-17 08:43:53.918407799+08:00**, earlier than the initial08:44:04 grant
  and original08:45:04 expiry. It compared the exact observed renewal before
  changing it and verified that only expiry changed: full key, controller owner,
  aliases, denials and other records remained identical. Old-owner Goals were
  not migrated. The
  reservation/rejection cleanup edge remains a known product limitation; an
  empty live lease must not be mistaken for active side effects or erased with
  a broad data restore.
  `independent-final-receipt.json` retains the needs_correction result;
  `expiry-correction-receipt.json` records the actual retraction. UI reconnection
  and renewed approval must not be treated as equivalent operations.
- D: an actual native job emitted three True TTY values, but the operator opened
  its console after the90s input window. It timed out without input or echo.
  It was stopped and was not replayed or marked passed.

Final E is a separate explicitly authorized experiment with a300s process limit,
an already-open console, and a new stable op_id. The real browser selected
`deepseek-v4.1-flash` / `openai-completions`, invoked one job_start, attached to
the running ConPTY and typed the non-sensitive test string plus Return.

- Goal `I6NUV63UHI`; Job
  `22bc948d2e6ecfb2135eaf39bbe1cb39e5889e2c604754895b9d5892cd435c03`.
- Actual output: `SEC025E_TTY:[True, True, True]`, `SEC025E_READY`,
  `SEC025E_ECHO:SEC025E-PING`.
- Durable receipt: terminal/interactive true, succeeded, exit0, effects_unknown
  false;416 stdout bytes, SHA256
  `be2105c0c5b0e7fc38590ca173a8f5aef0b0b37c16373313d48b5c33db7dc82f`;
  zero stderr bytes. Worker34416 and child31516 no longer exist.
- The operator stopped the acceptance Goal after confirming output. Goal status
  is canceled, pending_run is null; the successful process receipt remains.
  This proves the terminal capability, not an automatically accepted Goal.
- `production-task-result.json`, sanitized `production-terminal-receipt.json`,
  exact `production-terminal-stdout.bin` and `production-terminal.png` retain
  independently inspectable evidence. No credentials were typed into the test.
- UI returned to a fresh Session with Auto restored, panels folded and the
  previously observed unsent draft preserved. The temporary QR tab was closed.

This bounded real-model/browser/Windows-terminal trial does not establish
universal task intelligence or reliability of every randomly selected Auto
upstream. Service health and controlled fixtures remain separate evidence layers.

## Final independent acceptance

Independent Codex `/root/interactive_review` reviewed the deployment plan, exact
source/package overlays, executed Linux results, production image/state checks,
browser evidence and terminal artifacts. Its original
`independent-final-receipt.json` required correction of the temporary UI renewal.
After the explicit expiry retraction, `independent-final-accepted.json` records
ACCEPT for this bounded deployed release: final expiry08:43:53.918407799 is within
the old grant, full key/owner are correct, all five new-controller test leases
are revoked, and the four old-owner records are structurally unchanged. TOTP,
Ray browser auth, other services and installed seed/image remain correct; no
pending Run, live Job or test worker/child remains. Both receipts stay in the
evidence root above; the failed observation was not replaced with a green one.

The coordinator inspected the final receipt, hashes, stdout and real browser
state. Documentation link and scoped whitespace checks passed. Final follow-up
commits contain documentation only and do not change the tested runtime inputs.

Shared QR still means one administrator. Shell tools retain OS-account authority;
there is no newly implemented OS sandbox or per-person isolation. This bounded
security repair is not an exhaustive penetration test or universal deployment
acceptance for all Linux distributions/macOS.
