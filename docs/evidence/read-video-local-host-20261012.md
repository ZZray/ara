# READ video, Local08 and shared-browser correction — 2026-10-12

Current status at 10:03 Asia/Taipei: Video25, Local08, shared browser and
normal CLI video have bounded runtime PASS with independent audit. Actual
Host UNKNOWN passed after the test-only compile fix; independent EXEC approved.
Linux ordinary CLI and Unix09 runtime passed independent EXEC audit.
Complete READ acceptance is pending, including attachment protocol wiring.
Formal inventory remains 7/111 accepted; accepted delta0.
Fixed OMP commit596f2da7101178214aa27a753529d15e6b7ad91d;
ported_through_commit=null. All candidate and runtime directories named below
are under D:/workspace/env/cache/ara-port-candidates/.

## Frozen source and independent review

Root adopted21 changed/new Rust paths only, with42 combined delivered pins;
Git index89d34e26be97c90a14680d44ace937e6b1ae5a86e8ee7b46dd6b90718a922475
was unchanged. Delivery read-video-compose-root-v2/
DELIVERED-READ-VIDEO-LOCAL-HOST-ROOT-v1.json SHA
1cfb7421d250e49d166f5ef0242e0a95a83798fb70a577c16d5134387c7a51ab.
Adoption verifies all source/before hashes and independent reviews before
writing the allowlist. No staging, commit, push or deployment occurs.

- Video Core A-v5:22 originals plus directory regression1 and controlled
  settlement regressions2; independent P SOURCE POST APPROVE4502fb5a.
- Video Host P-v4 and formal Native binding P-v2: independent A SOURCE POST
  APPROVE539df542 /91a11ee4. Unix accepts an absolute media temp root;
  Windows retains the reviewed D-only constraint. Normal CLI/RPC share the
  retained media runtime and close it after Session job settlement.
- Local08 B-v1: real Journal/SessionArtifacts/HostEditSession owners bound to
  local resolver and OS path, original six-byte ZIP and three assertions.
  Root SOURCE APPROVE44b1c7d. Complete LocalProtocol/Unix09 remain OPEN.
- Chrome OS HANDLE gate B-v1: Root SOURCE PRE APPROVE638e31cf. Bind the
  original approved Chrome image and process HANDLE before A exits; require
  it alive after A exits and B reads, with stable daemon id/pid; require the
  original HANDLE signaled after B exits. Previous output-only PASS is
  preserved separately as REVISE_SHARED_BROWSER_PRESERVATION.
- Private package B-v9:2555 pinned files, two private glue changes only.
  Windows no-console startup uses a short bootstrap to launch the retained
  broker with detached:true/windowsHide:true/unref. Chrome still uses the
  original detached:false launch and is stopped by the broker. Root SOURCE
  PRE APPROVE6ca03ddb; manifestbee90ce2. Codec reuse is bounded and reviewed;
  no fresh v9 codec execution is claimed.

Independent A final composition/driver SOURCE POST APPROVE, findings[]:
read-video-compose-root-v2/
READ-VIDEO-COMPOSITION-DRIVER-INDEPENDENT-A-POST-v2.json SHA
eb73d862b51c43ffe269d281f0b6d6697287d936c21840979c854bc14f9e7bf8.
The independent merge recomputation exits0; final native test bytes are the
merged prefix plus exact Video, Chrome and Local08 suffixes. Original native
image test body is unchanged. Review does not claim typecheck or execution.

## Native lifetime diagnosis supporting the correction

Root actually ran the bounded Bun1.4.0 parent/child experiment, then read the
raw receipt. Original detached:false+unref: parent natural exit0, child HANDLE
258→0→0, heartbeat stops11. Only detached:true+unref changes: original child
HANDLE258→258→258, heartbeat11→13→22; cooperative exit finally signals0.
Both child processes are settled. Diagnosis receipt SHA
e4fc9db60b25df1de7fd3168d1892d9818f3493619453c8c12b9f88a30a0a3ab;
independent B audit217de21e. This confirms the launch-option intervention in
that native experiment, not Bun internal Job ownership or complete Chrome
behavior. The browser path does not invoke the ARA LSP Job helper.

## Single selected build and execution gates

Driver read-video-run-root-v1/run_batch.py SHA
1a2b8830d67abf92fe2098609a9e5e0ee31c0201f8bb437984f0db14702ba7e7
reuses scripts/verify_edit_module.py. Root printed and checked the plan:

```
cargo test --locked --offline -p ara-tools -p ara-cli
  --test read_video_utils_original --test read_video_original
  --test read_local_video_original --test read_video_settlement_faults
  --test read_host_native --no-run --message-format=json
```

Actual command is one argv invocation without the displayed line breaks.
D target/cache/temp, two jobs, serial selected tests. Native groups add
--include-ignored --exact.28 selected checks are video25, Local08,
normal CLI video and corrective browser. Full workspace/all-features and
unchanged IMAGE39/archive/cache/Unix groups are not added to this batch.
Actual runtime output: read-video-runtime-root-v1. Build/test timings,
artifacts and independent EXEC decision will be appended after execution.

## Remaining boundaries

Actual Host UNKNOWN/late settlement, Unix09, live late cancellation,
settings/full-resolution/model/env/inspect publication, attachments/history,
real-model UNKNOWN receipt, TUI, product and distribution/license gates
remain OPEN. Core controlled settlement tests do not prove actual Host
process ownership. CAS historical7 plus authorized5 reservation stays held
UNKNOWN; no replay/release and0newCAS in this batch. WSL already installed
and used on D; restartRequired=false, no additional administrator request.

## Actual batch v1: bounded27 PASS, native-video assertion FAIL

Root ran read-video-runtime-root-v1 on the frozen42-pin delivery. Build
PASS440.273s,502reused/10compiled,103warnings/0errors; test processes50.256s,
module509.800s, source_unchanged=true. Actual seven groups give27PASS/1FAIL,
zeroignored: utils14 including directory regression, READvideo7,
localvideo2, controlled settlement2, Local08 and corrective browser pass.
Normal CLI video fails after its frame/sheet pixel checks and isError=true
corrupt receipt, before its finish/EOF cleanup assertion.

Build receipt SHAa4b73ea3e3271963c07b0f3f876fdcc31bf208a805d89677fff80bb58424d96c;
module SHA7438e29e93b4eb46ff03386a8ea42e648951ad62aeec8f1b86363e75887f84f5.
Root actual pin/raw/image check ROOT-CHECK-v1.json SHA
7f8f19769986676a4d56dd631c2faa7492b0be67851ccb5b8f36254ad27db4a4.
The original native test executable is copied with its verified hash before
the next test-only incremental build. Ordinary production CLI SHA
dc1f51eae315063459aa32ca677f047bca8426cc5be45162147756c55a07932b,
confirmed by raw Cargo profile.test=false and the Host receipt.

### Shared browser correction is actually exercised

Browser group PASS41.349s. Original Chrome PID56232, daemon
a54eedc6-24ee-4899-966f-6c14ef8c6328 and scopea64d233431f4a08d remain
identical before A closes, after A exits and after B reads page2.
Bound original Chrome HANDLE states258→258→258→0. Last client exit to
Chrome signaled5932ms, within10000ms. Both normal CLI clients exit0;
the original Chrome executable is the approved installed Chrome image.
Root personally views the real blue/red1024x576 pages, frame320x240 and
sheet960x480, verifies all byte hashes, actual decoded pixel lengths and
center colors. Fresh scoped CIM receipt at01:26:26Z has0own processes.
This closes the earlier stable-browser finding within this Windows Host
batch; it does not accept complete READ or Unix broker behavior.

### Source-backed test-only correction, not a changed product expectation

The new Host-only test expected `ffprobe failed`. Fixed upstream
utils/video.ts:251 and read-local-image.test.ts:27 require
`Could not probe video`; current Rust read_video.rs:320 returns that
same source-backed text with the basename. Actual failure isError=true,
`Could not probe video 'corrupt.mp4': ... moov atom not found ...`.
Root prepares exactly one assertion replacement with that prefix/basename;
production code and original25 tests are unchanged. Independent narrow
SOURCE review and focused normal-video rerun subsequently passed as v3 below.
The v1 failure stays recorded; accepted delta0.

## Focused normal video v3 and build-cache identity

Independent B approved the test-only error-prefix correction. The abandoned
single-package v2 selection changed Cargo dependency artifact identity and
started unnecessary compilation. Root stopped only that owned batch after
95.458s; v2 ran zero tests. Its cancellation receipt remains preserved.

v3 retained the original seven-group build union and executed only native video:
build132.521s,510reused/2compiled; tests5.034s,total155.032s,1PASS/0FAIL/0ignored.
The two compiled outputs are native test and ordinary CLI. Production CLI SHA
00764454c916ad636586ed5df720645c606b7ec3c3e36e9380695caf90ca4990;
native test SHA9a62a7a15cec193c3b8856015018fc8087f2d63d9b7558925d4ce42bb5a348ef.
Real frame/sheet pixels, corrupt-file isError, three agent_end events, one
shutdown, EOF exit0 and empty media temp pass. B EXEC APPROVE receipt SHA
70f8ceb09e0783296a46068b88f4b2d13d5bb184c235f11b77fc2540f0949d96.
The prior27 green checks are reused through the reviewed source equivalence
bridge, not claimed as freshly run on this changed CLI binary.

Local08 observes nine real filesystem resources and actual bound Session owners.
No assistant append/save materialized the lazy journal/artifact directories;
this group does not prove persistence or restart behavior.

## Actual Host UNKNOWN v1: compile failure retained

Source-only delivery09338ec572ffeb6f57273d765c2ba82289a924b117c467d819891b29840c0e39
adds a real OS pipe-writer duplication group and an explicitly invoked ignored
child helper; production source is unchanged. Driver/compose PRE and SOURCE
POST were approved. Root verified42 pins and started the same union build.
read-video-host-fault-runtime-root-v1 build exits101 after71.028s, zero tests:
E0716 at read_host_native.rs:1152, a temporary joined path is borrowed across
tokio::select. The bounded fix binds the path before constructing the future;
new narrow source review and incremental execution are pending.

Unix09 source archive is already frozen:1192files/19Cargo manifests/51.45MB,
freeze receipt49038270708a9af5b9a87343401b47ca951de4fdb07250f92570e20c64f6b476.
Existing nonroot WSL user ara-verify and D-backed ext4/cache are verified.
Linux ordinary CLI build and actual Native READ symlink runtime remain pending.

## Actual Host UNKNOWN v2: bounded runtime PASS

P candidate changes only the local PathBuf lifetime. Independent B SOURCE POST
APPROVE receipt6bb4710e836b9666fd31d037120e8a89ea9f2696bc858f941db2c55e901989b8.
Root recomputed the exact single-hunk replacement, adopted01647b82 test bytes,
verified42 delivered pins and unchanged index. Delivery manifest SHA
ccdd834504e7cd8322b43c0bbf5f8fafa93619ba67bf65058d88f7beabf1cc2e.
Initial E0716 failure above remains historical; production code is unchanged.

read-video-host-fault-runtime-root-v2: build91.042s,511reused/1compiled,
103warnings/0errors; test19.734s,total118.477s,1PASS/0FAIL/0ignored.
Only the native test compiles; ordinary CLI is fresh with unchanged00764454 SHA.
Actual parent argv selects the exact UNKNOWN group; explicitly ignored child
helper is launched only through the production HostMediaPort real process.
The D helper copy and test artifact share SHA
f39a47a35ebf6db00ef1ce265d34bdee7ad3aac0becfd8687b7cac7ef749c0cc.

Root personally reads the OS receipt: child59464/creation134362434001965495,
two real stdout/stderr writer duplicates,128KiB written to each pipe, one spawn
and normal exit. Original process HANDLE is signaled at UNKNOWN; pending pipes
produce typed Unknown at3015ms. Cleanup5014ms and Hostclose10011ms also return
Unknown while sentinel bytes remain. After duplicate release, real EOF settles
late close in76ms; repeat close succeeds and only then workspace is deleted.
One execute,zero command replays,finally settled,total19339ms.
Root final scoped CIM receipt01:51:26.901851Z has0owned processes.
ROOT-CHECK-v2.json SHA
673b22906a357f7c1163103872fdeefdc64ac9fe3778450ecdf3fbbd2006a08c.

Independent B EXEC APPROVE receipt SHA
90740bca14b3972a24d07e5a0fe7867c42525c9f4b2796aa410a0b67537b07d0.
Root personally checks this receipt against the raw runtime and source pins.
This group proves actual Windows
HostPort process/pipe ownership, not normalCLI UNKNOWN propagation, termination
helper/Job failure, Unix behavior, complete READ, real models or product delivery.
Linux single normalCLI build has now started as ara-verify on D-backed ext4;
fixed frozen production source is unchanged by the subsequent test-only edits.

## Unix local original09: actual normal CLI and persistent Session PASS

Root executed the frozen reviewed driver as ara-verify(uid1000) in the existing
ARA-Write-Unix WSL2. Workspace/output/target are native ext4 backed by the D VHD.
One offline/locked normalbin build completes197.358s,406reused/94compiled,
exit0/source_unchanged=true. Cargo reports the exact snapshot main.rs target,
profile.test=false/debug0, no features, ELF7f454c46. Actual ordinary binary SHA
d9f33ace6357acce6771d9631cae3e38b9607e6a066826be8fc5fce63eff63ae;
build receipt1cba89859cdec92576517ff8889ca97f30365a3a15df78ce64819a3acce5b2ee
at /opt/ara-env/logs/read-local-unix09-build-v1/build-receipt.json.
This is the frozen production source; Windows native test suffix/fix are not
compiled by this Linux ordinarybin target. No historical tools test ELF is used.

Actual runtime /opt/ara-env/logs/read-local-unix09-runtime-v1/unix09-result.json
SHA3f70e18ef692f1caa240ddc2d33edb6c4db2248e60f72294900797b6ec1a3c33:
PASS_AWAITING_INDEPENDENT_AUDIT,1.180s,normalEOFexit0,zeroCAS.
Root reads get_state sessionFile/sessionId and the actual physical journal;
Session01a12dd1-da3d-72e1-9936-c5ac0429fce7 is identical in its header.
Normal local://control.txt returns NATIVE_UNIX09_CONTROL_20261012.
Real local/linked points to runtime outside, lstatmode41471; fixed70byte PNG
SHA43be52f97ed308911037af5b5cc41f02117f6e22cc86f42c9fcb3387af06de49.
Native local://linked/secret.png returns exact `local:// URL escapes local root`,
isError=true,noimage; RPC and journal agree. Two actual agent_end events,
one session_shutdown,four localhost OpenAI requests,media-temp cleanup pass.
Journal SHA302deabb36a7eb2bbc430d5e9b6fd2e22c8a420554d893f0d8a89d47407e5d50.

Independent B actual EXEC APPROVE receipt SHA
1497dde734bd391cef5bd8b47392117f4d4848abf5bbb6fe6d2a8bda0c13bd54.
The auditor checks all1192 frozen inputs and compares them with current E;
only the Windows native integration test differs, and the index is unchanged.
Root personally reads the pinned review and raw journal/runtime observations.
This is fixed local original09
and Native Unix containment evidence, not real-model, full READ or release
acceptance. Administrator MSIexit0/signatureValid/restartRequired=false and
D distro/VHD registration receipts were personally refreshed before execution;
no new installation/UAC/reboot was requested or performed.
