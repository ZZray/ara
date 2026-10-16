# 0.2.5 security hardening — 2026-10-16

## Scope

User authorized one repair round, commit and push, without production deployment.
Baselines: Node `1aac442e8daf62c24b00073f02020badd9cf8038` (implementation
`bd90b470`), Go Hub/Ray `efc8c90648a840e5ecc20dbec0e3ee024a377e90`
(implementation `16da1d5`). Go changes were prepared in a detached clean checkout
because the original checkout contains unrelated authentication/Web WIP.
Shared ARA Core, fixed OMP marker and other platform device protocols are excluded.

This follows the [read-only audit](security-audit-20261016.md). That report
describes the vulnerable published 0.2.4 version, not a repaired production state.

## Implemented contracts

- Node defaults missing URL schemes to HTTPS and rejects public HTTP/WS before
  dialing or attaching credentials. Only explicitly specified literal loopback
  IP HTTP/WS endpoints are supported for development. Those WebSocket and model
  HTTP connections bypass all environment/configured proxies, so a proxy cannot
  reinterpret loopback as its own computer. Public WSS continues to use normal
  certificate verification through a configured proxy. Model HTTP redirects are
  disabled; Go native WS also disables redirects rather than forwarding a bearer
  to a downgraded endpoint.
- Ray rejects HandWave plaintext machine requests before forwarding, without a
  credential-bearing redirect. Existing RainFlow and Arena machine contracts are
  unchanged. The supported Hub topology remains private loopback behind Ray, or
  a separately configured verified TLS ingress.
- Every node connection receives a random 32-byte, 30-second challenge. The
  Ed25519 proof signs a fixed domain, nonce, expiry, fingerprint and canonical
  full public key. It is consumed only by that socket before any registration,
  eviction or pairing. Old unsigned clients cannot downgrade the protocol.
- Explicit approval pins the complete proven public key and persists it before
  pairing. Missing/mismatched pins cannot obtain pairing/model grants. Persistence
  failure rolls approval back. Existing seed files and display FP are preserved.
  The existing Go node uses the same proof, keeping its actual protocol tests.
- All shared-root administrators have one server-derived, domain-separated stable
  controller ID. Browser `hello.fp` and CLI `SelfFP` no longer establish owner
  identity. The server sends `controller-identity` before other controller events;
  the existing UI updates its label. This remains a shared administrator system.
- Ray requires one exact HTTPS Origin for browser WS; Hub independently checks
  Origin scheme, host, port and shape. A missing Origin requires an independently
  verified native bearer, not a browser-cookie fallback. Ray removes a validated
  browser Origin only on its authenticated private HTTP hop to Hub.
- Browser sessions cannot export the permanent TOTP QR. Existing binding/code
  login, initial private provisioning and explicit server-local CLI export remain.
- Shared `websafe.AdaptApp` fixes notification text with DOM/textContent and
  completes HTML attribute escaping. It protects both direct Hub app.js and the
  final Ray-adapted script. Exact original/repaired call sites are supported;
  duplication or unknown drift fails closed. The upstream stable archive and
  unrelated dirty Web tree are preserved; layout and code content do not change.

## Upgrade boundaries

Both Hub/Ray and Node must be upgraded together. Node 0.2.5 deliberately refuses
an old Hub without the challenge protocol. Existing public HTTP node.conf values
must be changed to a verified HTTPS URL; there is no automatic secret/config
rewrite and no certificate-validation bypass.

Old approval records have no complete public key and require explicit reapproval.
Their expiry is not silently renewed. A new server-assigned controller owner does
not silently take ownership of old Goals or rewrite old local Session directories.
Files and audit history are retained; old-owner Goals are not automatically
resumable. Root-token rotation also changes the shared controller principal.

No live auth state, key, authorization, configuration, service or installed Node
was changed during this round. Previously published downloads remain 0.2.4 until
a separate coordinated deployment. No automatic credential rotation was performed.

## Verification record

The delivered Windows EXE is 5,164,544 bytes, SHA256
`8b01e4384e7e39aec7149ff9b75dbbac3ab79eff80478ece871402e2f60d439e`.
Hub SHA256 is `70e895af913ad063317772bc7ceb3f31c11269511769aac610f42facab4389c1`;
Ray SHA256 is `290a65f9b5a62b62c6d7692af600f09391e40a7d18f50e14889264dc598d5488`.
Evidence directory: `D:/WorkSpace/tmp/handwave-security-20261016/`.

| Delivered-code check | Result and evidence |
| --- | --- |
| Standalone `cargo fmt --all -- --check`; clippy all-targets/all-features with `-D warnings` | PASS; clippy 1m59s. Binary-only workspace has no library doctest target. |
| `cargo test --locked --workspace --all-targets` | 160/160 PASS, 0 ignored, tests 7.03s; `rust-tests.log`. Build including cache-lock/resource waits 9m58s. |
| `cargo deny --manifest-path apps/handwave-node/Cargo.toml check` | Advisories, bans, licenses and sources PASS under the existing Host policy; duplicate-version/unmatched-license warnings retained in `cargo-deny.log`. No policy relaxation or dependency upgrade. |
| Formal standalone static-CRT Windows release build | PASS, 9m31s including waits; `rust-release.log`. Immutable `handwave-node-final.exe` used below. |
| Full Go Hub/Ray/websafe tests, actual Rust TLS opt-in enabled | All three packages PASS, 89.036/3.408/0.052s; `go-delivered-tests.log`. |
| Real Node TLS and HTTP CONNECT proxy | 6/6 PASS: trusted direct/proxy, untrusted direct/proxy, hostname mismatch and redirect refusal; `actual-node-tls.log`, 12.86s test / 13.930s package. Certificate failures reach the TLS server and send no HTTP bearer. Redirect reaches no plaintext destination. |
| Actual Node/Hub identity and restart | 4/4 PASS, no model calls; `identity-final-v2/result.json`. Unsigned impersonation leaves the real paired Node intact; Node/Hub restarts preserve seed, FP, owner and complete pin. |
| Complete Host regression with proxy and catalog variants | 27/27 PASS, 114.911s, 23 controlled model / 19 catalog requests, no fixture errors; `host-final-v2/result.json`. Includes actual file/compile effects, continuation/restart, cancellation trees, unknown/replay fences, explicit catalog Sessions and proxy receipts. Only the download tool reaches the proxy; Hub WS/model traffic does not. |
| Actual native terminal regression | 2/2 PASS; `terminal-final/result.json`. Real TTY, headless cursor reply, merged output, resize/idempotency, same child after reconnect, scoped Ctrl+C/EOF, exit0 and no model polling. Controlled provider, not cloud intelligence. |
| Safe Web output | Go direct-Hub/final-Ray fixtures and executable JS regression PASS. Actual Chrome isolated DOM reports marker0 and unsafeElements=false; `browser-dom-result.json`. Final adapted app SHA256 `c30133ba09e36710d997272e925df079b536b983ca85cccd86bf32302b3f9e2f`. |

Independent Codex `/root/interactive_review` reviewed the plan, final scoped
source and actual artifact/results. `/root/security_web_boundary` independently
cross-reviewed Root's TLS/Origin/owner/QR implementation and ran eight focused
tests. Four reported boundary findings were resolved before acceptance. The main
coordinator inspected the source and receipts and observed the actual isolated
Chrome DOM result; no production browser payload was used.

Earlier full-Go failures were retained: the heartbeat fixture lacked a native
bearer; smart fixtures used a display FP without the matching private key; catalog
fixtures assumed a new self-reported owner after reconnect. Fixtures were updated
to real authenticated identity, preserving cancellation, fresh Session, protocol
and actual tool-effect assertions. Production checks were not loosened.

Independent review also found a loopback/proxy escape, duplicate-Origin ambiguity,
cookie fallback in the originless exception, and Go WS redirect downgrade. These
were corrected before delivery; the executed checks above cover those repairs.

Other retained verification failures are explicit: the first TLS fixture used a
CA certificate as an end entity, correctly refused by Rust; the second checked
an enum name while the actual hostname diagnostic was human-readable. The final
fixture uses a distinct CA/non-CA leaf and asserts the actual hostname rejection.
Initial identity/Host restart fixtures discarded the one-shot paired event while
waiting for status; Node logs proved pairing had already occurred. Reading paired
before node-hello fixed the fixture without reapproving or relaxing product gates.
Original `identity-final`, `host-final`, and first/second TLS logs remain retained.

Only test-fixture and documentation changes followed the product source freeze;
the tested Node/Hub/Ray artifact hashes above did not change. Scoped whitespace,
Python syntax and documentation link checks passed. A repository-wide bootstrap
walk is not used as Agent acceptance. No new Linux/macOS runtime, real cloud
provider or production activation is claimed for 0.2.5.

## Remaining limits

Publication follow-up: [coordinated production deployment](security-deployment-0.2.5-20261016.md)
records the subsequently authorized cloud/installed-node/download activation and
new Linux evidence. The earlier non-deployment statements above describe this
local hardening checkpoint, not the later publication status.

This is a bounded security repair, not an exhaustive penetration test. The shared
QR still represents one administrator, without personal accounts or per-person
revocation. Agent shell/filesystem tools retain OS-account permissions; cwd is
not a sandbox and prompting is not a hard defense against prompt injection.
PTY echo/application output can still enter logs/model context. Global application
CSP, OS isolation, sensitive-action approval and actual cloud
activation must be assessed separately; do not claim these are implemented here.
