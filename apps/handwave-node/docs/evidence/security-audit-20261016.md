# 0.2.4 read-only security audit — 2026-10-16

## Scope and conclusion

User asked whether privilege escalation or hijacking is possible. This audit
does not authorize a production change. No application code, live configuration,
identity, grant or service was modified. Existing unrelated worktree changes
were preserved. Findings remain **unfixed** in this audit.

Target: Node implementation `bd90b470fc89cc82e089d9c22eb1824a9340622f`,
Go Hub/Ray implementation `16da1d5adc4e09fff595872e9d1c3d9ab90328c2`.
Go dirty authentication/Web sources were excluded. The maintained Web archive
and final Ray adapter were inspected instead. This Host audit is separate from
shared ARA Core/OMP acceptance.

The release has concrete identity and Web injection defects. Functional PTY
acceptance does not establish security acceptance. Current evidence supports
supervised trusted-owner use, not exposure to mutually untrusted users. There
is no evidence here of an actual compromise or OS administrator escalation.

## Findings and distinguishing evidence

| Finding | Preconditions and impact | Evidence / limits |
| --- | --- | --- |
| P1: machine connection permits plaintext | A network observer or intermediary on an unencrypted segment can observe/reuse bearer credentials or alter traffic. The installed Node configuration specifies HTTP; this is independent of browser HTTPS. | Node `config.rs:117,187-193`, `proxy.rs:117-122`; pinned Ray `main.go:94-111,191-209` exempts machine requests from HTTPS redirect. Effective environment/launch/config were checked without printing secrets. No token capture or traffic interception was performed. |
| P1: node identity can be impersonated | An admitted node with the shared ingress token and a known approved fingerprint can replace that node, inherit pairing and receive subsequent tasks/model grants. This is task/identity hijacking, not proof of execution on the displaced real computer. | Pinned Hub `hub.go:483-499`, `model_grant.go:116-137`. Isolated real-WebSocket fixture confirmed replacement, eligibility and inherited pairing. `identity.key` currently produces a label; the handshake does not prove private-key possession. |
| P1: unapproved hostname reaches an HTML sink | An admitted, still unapproved node controls its hostname. An open controller automatically renders the pending-device notification, allowing an event-bearing HTML element. Script execution could use the current same-origin management session. | Hub `hub.go:487,883`; stable `app.js:99-108,2271-2276`. Final adapted app retains the same toast function. Isolated actual-function execution and HTML parsing confirmed an injected event attribute; no browser event or production payload was executed. |
| P1: model code blocks reach HTML attributes without complete escaping | Model output, potentially influenced by retrieved untrusted text, can inject event attributes into a code block or Apply button. The demonstrated paths require interaction with the resulting element. | Stable `index.html:366,369`, `render.js:5-6,24-26`, `app.js:564-565,1130`. The later global `escHtml` replaces the quote-escaping renderer function. Final adapted app preserves it. Two isolated fixtures confirmed event attributes, without executing them in a browser. |

The target application response chain does not set a HandWave CSP; the presence
of a production outer-proxy CSP was not checked. A CSP may mitigate event
execution but does not remove unsafe HTML construction.

## Additional trust boundaries

- Shared QR is an intentional shared-administrator design, not separate user
  identities. Pinned Ray `main.go:309-310` maps valid platform sessions to the
  same Hub authority. Hub `hub.go:555` trusts controller-supplied FP and
  `long_tasks.go:527-537` uses it as Goal owner. A second isolated fixture
  confirmed access to an existing Goal after claiming its controller FP.
- Pinned Ray `web.go:167-188` lets an authenticated session retrieve the shared
  provisioning QR. A stolen same-origin session can therefore potentially
  become persistent login access. The QR itself and its seed were not read.
- Hub `hub.go:352-355` disables browser WebSocket Origin checking. Strict cookies
  mitigate ordinary cross-site requests, but do not replace exact Origin checks
  against a compromised same-site service on another HTTPS port. This scenario
  was not exploited or reproduced against production.
- Node `local_tools.rs:27-34,53-70` deliberately allows absolute file paths and
  shell execution under the OS account. `--cwd` is a path base, not a sandbox.
  Plan removes write/execute tools but still permits account-readable files.
  Execute/Goal have no general per-dangerous-action approval boundary. Prompt
  instructions treating tool results as untrusted are useful but cannot provide
  OS isolation against prompt injection. No automatic OS elevation was found.
- PTY input is not itself a chat message, but terminal echo/application output
  can enter Job logs and later model context. Do not infer secret isolation
  merely from a dedicated input control.

## Verified existing protections

Platform cookies use HttpOnly, Secure and SameSite=Strict; login has attempt and
OTP replay limits. The application does not receive the platform login cookie.
Console `job_console.go:45-60` checks the live two-way pairing, sockets, device,
controller, Session and Goal owner; input/signal/resize additionally require an
active Goal, unexpired lease and AllowJobs. Control fields are filtered.
Node lease/nonce/receipt fences reject stale input and avoid replaying unknown
effects. These checks depend on upstream identity being trustworthy.

The terminal opens no new public listener. Pipe output uses textContent; PTY
uses the pinned xterm parser. No direct HTML/clipboard execution path was found
in the added terminal. OSC8 HTTP/HTTPS links remain possible with user click,
confirmation and cleared opener; absence of a link addon does not mean no links.

## Executed checks and retained evidence

Independent Codex reviewers: `/root/security_hub_boundary` (Hub/Ray) and
`/root/security_web_boundary` (maintained Web/terminal). Coordinator independently
read the critical source, fixture code and outputs and reran both fixtures.

- `go test ./apps/handwave -run '^TestSecurityAudit' -count=1 -v -timeout 20s`:
  **2/2 PASS**, final coordinator run 0.399 s test package time. These tests
  intentionally confirm vulnerable behavior, not a security pass.
- `python -X utf8 security_web_fixture.py`: **3/3 injection attribute fixtures
  confirmed**, archive manifest and final function equality asserted.
- Reviewer's `TestMaintainedReleaseRecoveryAdapter`: PASS; final adapted app
  SHA256 `adaa24782ec01567d46c4f73a955651aa246e06405f1e37d997cfbdf51dac28b`.
- Initial Hub fixture attempts had missing real socket/info prerequisites;
  corrected fixtures passed. Those setup failures are not product crashes.
- Temporary evidence: `D:/WorkSpace/tmp/handwave-terminal-20261016/`, files
  `security-web-fixture.json`, `security_web_fixture.py`,
  `security-adapted-app.js`, and clean-export
  `go-source/apps/handwave/security_boundary_audit_test.go`. No secrets included.

Not covered: exhaustive penetration testing, other platform service APIs,
production browser exploit execution, active MITM, dependency vulnerability
inventory or proof of absence of historical compromise.

Documentation review: both independent reviewers checked their respective
sections; the console lease wording was narrowed to input/signal/resize.
Scoped link checks and tracked diff whitespace checks passed. The repository-wide
bootstrap scan was stopped without a result after its unbounded directory walk;
it is not recorded as passed and does not affect the executed security fixtures.

## Recommended remediation, not implemented

1. Require verified HTTPS/WSS for public HandWave machine connections; retain
   separately contracted legacy RainFlow device paths and internal loopback.
2. Replace text HTML sinks with textContent/DOM APIs, fix attribute escaping and
   add an application-compatible CSP as defense in depth.
3. Bind approved devices to public keys and verify a fresh, scoped server
   challenge before registering or replacing sockets. A shared ingress token
   alone must not establish device identity.
4. Bind controller ownership to the server-authenticated principal, enforce exact
   browser Origin, and define shared-admin versus separate-user semantics.
   Reconsider routine export of the long-lived shared QR.
5. Run nodes with least OS privilege; define sensitive-action approval and
   enforceable filesystem/network/process boundaries appropriate to deployment.

After repairing ingress, assess credential rotation; plaintext history means
credential secrecy cannot be guaranteed. Changes require targeted negative
tests and actual secure-connection/normal-user regression before release.
