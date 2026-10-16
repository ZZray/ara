# HandWave 0.2.3 reconnection, failure reporting and recovery

Target: standalone Host from Rust base `e8b0567`, maintained Go base `314d76e`.
Root Rust Core/OMP work, dirty Go authentication/Web work, routing, grants and
unrelated services are excluded. Delivery requires the source, executable,
review and production receipts below, not the presence of this page.

## Observable requirements

- Lost and half-open Node/Hub/browser connections become visibly disconnected
  and reconnect with bounded retry; a failed handshake cannot reset backoff.
- Shutdown and stalled writes do not leave the connection actor waiting forever.
- Pending/partial replies and drafts remain attributable to the originating
  controller, device and Session; restoring text is explicit and does not replay.
- Interrupting controller-dependent tool work releases waits and prevents the
  next serial tool or model continuation from submitting an unknown effect again.
- Missing SSE completion is interruption, never successful completion. Explicit
  unauthorized responses provide a normal login path and preserve input.
- Scope, lease/budget, native history, effect fences and connection-local queue
  authority remain intact. Returning to a Goal observes actual receipts.

## Source findings and bounded changes

`connection.rs` centralizes bounded connection I/O, matching heartbeat and
backoff policy. `transport.rs` reuses the same actor and cleanup path; no second
scheduler or durable replay queue is added. An old timer measured the entire
session attempt, including its twenty-second failed handshake, as healthy uptime.
The new reset requires actual established uptime and a confirmed Pong.

Maintained Go Hub heartbeat closes its exact WebSocket on Ping error. The fixed
nhooyr v1.8.17 implementation only returns on a Pong-wait deadline after a
successful Flush; it does not close the main Read in that path. Existing
authentication WIP is excluded from this hunk and from the clean build.

Ray recovery and its UI/stream/draft integration are separately reviewed before
freezing. The original defects include no browser reconnect, stuck foreground
busy state, lost queue editing text, SSE EOF without done reported as success,
and dependent tool waits that could otherwise resume after disconnect.

## Executed evidence so far

Evidence directory: `D:/WorkSpace/tmp/handwave-recovery-20261016/`.
Final v0.2.3 unit suite134/134 PASS; fmt, warnings-denied Clippy and cached-offline
cargo-deny PASS. Initial release compilation succeeded, but Windows PowerShell
lacked Get-FileHash during packaging. Its failed log remains; the same source
and cached image packaged successfully with the installed pwsh runtime.

Final static-CRT EXE SHA256
`e56c526595c1a8a12ef0b0a2e9d66158e8398ced8d777d207ffbf867801d3332`.
`verify_reconnect.py` passes six real-process loopback faults: missing Pong,
wrong Pong, two healthy heartbeat exchanges, two real handshake timeouts with
increasing delays, abrupt EOF and unauthorized --once nonzero exit. All cases
make zero model HTTP calls. Both half-open cases reconnect at about31.5–31.8s;
healthy remains on one connection. These are controlled network faults.

`TestHubHeartbeatTimeoutClosesReadAndCleansController` runs the production
45s Ping plus15s deadline over a real local WebSocket. The client never reads
or responds to Ping; the actual Hub Read and controller cleanup complete after
60s. Clean-source test PASS; original dirty auth/Web is not part of that build.

## Final process, package and production evidence

The final EXE is4,961,792bytes. Its static-CRT ZIP is
`handwave-node-windows-e56c5265.zip`, SHA256
`aa3ce7e2b39756fdca13a622d3be894db0cd6a31caab417f6fa36a1a13316d3b`.
The619-file Linux runtime source archive is
`handwave-node-source-584331e1db15c5e6.tar.gz`, SHA256
`584331e1db15c5e6e1548ad7973554e45e9431d433f073242da4865592ef4d14`.
All26 runtime source files match the tested source. Later closeout documentation
does not rewrite this immutable download or its checksum. The unchanged Linux
installer is `install-linux-fe4b10bee6a73659.sh`.

Final actual-image regressions PASS: reconnect6, grant-terminal8,
native-session4, calendar3, Host23 and queue5 groups. Host and queue use
controlled providers; they are not model-intelligence benchmarks. The initial
Host invocation omitted modern-Hub mode and made no model requests. The first
queue attempt passed application assertions but hit a Python daemon traceback
teardown fatal; `verify_host.py` now suppresses only expected reset/aborted/
broken-pipe handler traceback. Other exceptions and application assertions
remain. Both failed attempts and the successful corrected fresh runs remain.

Independent Codex `/root/schedule_arch_audit` approved the bounded Rust/Hub
software point; `/root/context_memory_ui` approved front-end V4, the narrow
integration deltas and the later toast/status synchronization. Root inspected
the meaningful hunks and primary executable receipts. The final frontend has
38 recovery and23 machine-target independent targeted checks; unchanged
context21/queue25 checks retain their pinned V4 receipts.

The maintained Go export excludes dirty authentication/stable-Web code. Its
initial tree is `da0b0110f15998564ffd4c514b7122f058033c84`; Hub differs from
314d76e only by exact failed-Ping close/comment lines. Actual embedded stable
app SHA is32bcc87d..., distinct from old7ae318fe test fixture. The new full
stable-adapter test exports the rewritten app for actual `node --check`.
Hub/memory/Ray Go tests, vet and Windows/Linux builds PASS. The two-file final
toast delta has a separate clean export, Ray tests/vet/app parse/Linux build.

Production Node0.2.3 is installed with the original identity/config/54 state
files unchanged before new user-task execution. Hub Linux SHA is
`578f291298390af67422c150bab8d6a4fd616c1e32a81e7e27c73956d59718b5`.
Final Ray Linux SHA is
`1b7b10a37706adbd383ae881513db610cded021d024022fcbe0dd058bf33b8e3`.
Deployment receipts verify30 protected files, other services' PID/image hashes,
18 immutable download bodies and normal login/application gate. No device
grant is extended and no queued task is replayed.

The authenticated production browser survived the necessary Hub restart with
its unsent RECOVERY023 draft still present. The first UI pass found that an
already-reconnected peer left a stale offline/grant warning. Source confirmed
node-hello never refreshed that toast and early offline status permanently
consumed the observation attempt. The two-file correction waits for later
status under the same valid original grant and updates only visible connection
notices after verified peer/scope binding; auth/stream errors remain intact.
The old production leaf fails the new regression before the corrected leaf
passes. Final browser readback is recorded at closeout below.

Actual production Run `AYK37QMWOM` used `deepseek-v4.1-flash` through
`openai-completions`, called `run_script` once with Python
`print("RECOVERY023_OK")`, and returned the same stdout/final marker. The
native terminal receipt, exact one-tool result and final response were checked;
Auto was restored. Positive cacheRead values are upstream-adapter observations,
not billing reconciliation or a savings percentage.

No physical network, power-loss, saturated OS send-buffer, latest-source
Linux/macOS/ARM or all-provider reliability claim is made from these tests.
Full-repository bootstrap traversal is outside this standalone delivery;
scoped documentation links are checked at closeout.

## Final authenticated UI readback

On the final Ray image, the Node was stopped only after native preflight found
zero pending Runs/active Jobs and settled Goals. The same Hub image was
restarted with protected inputs unchanged; the Node was then restarted under
the original parameters and identity. The browser showed its retained unsent
`RECOVERY023_FINAL` draft, then both `已连接 · RAYWORK` and
`设备已重新接入，可发送新任务。此前操作结果仍待核对；不会自动重发。`.
No new Run was admitted, and all55 post-task native state files remained
unchanged. `live-recovery-final.json`, `.ax.txt`, `.png` and the separate
Hub-probe receipt preserve this observation. The test draft was cleared and
the recovery notice dismissed afterwards; Auto remained selected.

The delayed-device controlled cases are38/38 in the final recovery suite.
The real sequence also temporarily displayed the correct manual verification
path when the original peer had not been verified in the selected Session;
it did not grant new access or pretend that unknown ownership was verified.
The production trial does not establish interrupted active-tool recovery under
every timing; controlled Host/queue/stream faults retain that bounded evidence.
