# Investigated cases — 2026-10-14

## HWN-023: capacity telemetry must follow authenticated Goal slices

2026-10-16. Independent Codex UI review traced a fresh Goal slice through
`Runner::next`, transport and `wire.task_id`. A first UI draft accepted only
the browser's initial `localRunID` while `localBusy` remained true. A controlled
DOM reproduction proved that after the first slice ended, a later 900/1000-byte
receipt left the card showing the old 200/1000-byte settled observation.

The correction needs a separate capacity observer authenticated by the current
Hub Goal mirror (device, Session, Goal lease, writer epoch and current Run),
without granting task controls to arbitrary telemetry. Events ahead of that
mirror must remain unreported; loosening the task ID gate would mix sessions.
Per-Run tests, independent re-review and browser evidence are recorded in this
release's delivery record. The raw history and compression algorithm are not
the cause and require no policy change.

## 0.2.0 verification traps — 2026-10-16

An initial4.81MB candidate still imported vcruntime140 and the dynamic CRT.
An older Cargo static fingerprint was insufficient to identify the emitted EXE.
The formal build.ps1 output uses explicit static CRT (cb676ce8,4,922,880bytes);
actual imports and final-image process suites are the acceptance boundary.

Two stale fixtures failed without proving product regressions: cloud snapshot
tests compared a maintained hot tail against the full original transcript,
and Goal/Job tests parsed a superseded system-prompt marker. Reading the validated
segmented journal and the labelled runtime carrier restores the original strong
assertions. The original failed results remain in the version evidence.

Real Goal scripts also exposed a post-effect AST validator that rejected normal
Python str/list expressions. It was not an OS security boundary. A fixed approved
script and AST comparison now make the small live task deterministic without
claiming arbitrary autonomous coding quality. See the [version evidence](../evidence/version-0.2.0-20261016.md).

## HWN-020: capability validation belongs before a pending-effect fence

2026-10-15. Independent source review found a new fallible capacity check after
pending_run had been durably saved. A too-small declared window could return
before the normal finalizer, leaving a recoverable input error looking like an
unfinished effectful Run. No provider/tool had run. Capacity/input checks and
their bounded event admission now precede that checkpoint. Actual4096-window
rejection, native pending_run=None and recovery at1M in the same Session prove
the boundary without resetting native history. See
[current evidence](../evidence/model-capabilities-20261015.md).

## HWN-019: busy wire follow-ups must inherit the active execution authority

2026-10-15. The legacy workbench sends a follow-up as another agent-loop frame,
whereas the first implementation inherited plan/goal only for agent-follow.
A busy Plan could therefore queue an execute frame; a first Goal could lose its
Run-derived goal ID. Independent source review found this reachable branch.
Preparing both forms from the active frame fixes mode/Session/goal/criteria
inheritance while retaining each controller request ID. A real Hub/Node fixture
waits for the queued receipt before releasing the first provider request, then
verifies exact read schemas and unchanged goal identity/criteria. This proves
the specific wire boundary; it does not make allowlists an OS sandbox. See
[current evidence](../evidence/linked-context-modes-20261015.md).

## HWN-001: native tool history failed after a successful first Run

First controlled-model task wrote/read a real file, but the next task failed
typed session decoding. The pinned rpi Content enum and its inner structs both
serialize an identical `type` discriminator. Direct typed roundtrip was not
equivalent to the JSON shape emitted by that source revision. Canonicalizing
through serde_json::Value before save/read preserves typed tool/text history,
including initial files with identical duplicate tags. The actual checkpoint
and restart test plus real Host continuation provide separate evidence layers.
No failed resume was replaced with a fresh conversation.

## HWN-002: an error packet could end an unrelated active operation

Wire `out.done` describes an outbound response, not necessarily settlement of
the active job. A malformed command with the same ID could produce an error out
while the original process was still running. Host-internal `Packet.settled_id`
now owns settlement; wire errors do not inherit that authority. The real Host
test injects same-ID bad frames and observes the original job continuing.
This is independent of the completed-ID replay fence.

## HWN-003: healthy cloud Hub with an empty model catalog

Legacy local Hub fixtures passed, while the actual deployed HandWave returned
empty `/api/models` and HTTP503 on `/v1/models`. Current source routes models
through shared Gateway; legacy `upstreams.conf` is only a migration hint.
Read-only runtime inspection found neither gateway.json nor CLI overrides nor
the Gateway token environment. The existing Gateway's catalog was healthy.
Thus legacy node/Harness success does not prove the current cloud model chain.
Production configuration/restart and actual public-Hub task acceptance remain
separate gates. Keep credentials out of source, packaging and diagnostics.

## HWN-004: modern directory and short-grant compatibility

Independent review identified three contract pitfalls: directory reads could
consume time outside the model deadline, nil/empty protocol arrays needed the
legacy Chat meaning, and positive directory output bounds must constrain actual
request tokens. The Host now has one total deadline, normalized protocol lists,
and min(directory cap, smart cap, 8192). Focused Host evidence must inspect the
actual HTTP request, rather than only Rust fields or a successful model reply.

## HWN-005: smart restart expectation versus the Hub authorization lifetime

The first smart fixture omitted the modern route decision's profile_hash; the
Hub correctly refused it. After supplying the complete declared fixture, smart
tool effects and same-binding follow-up passed, but restart was denied. Source
counterevidence is modern `model_binding.go:308-326` and `hub.go:928-944`:
device disconnect intentionally removes both Run grants and Session pins.
Changing Node persistence cannot restore server authority. The test now proves
visible rejection, no resolve/model replay and byte-identical saved history
after disconnect. This clarifies a mistaken test expectation; it does not claim
smart reconnect acceptance. Default/explicit model restart continuation remains
separately exercised. Improving smart reauthorization requires an explicit
server/session contract beyond this compact Host delivery.

## HWN-006: public enrollment worked while granted model requests returned401

After authorized Gateway wiring, the real public node enrolled and paired, but
its granted directory GET returned401 before any model/tool effect. A public
health200 was too weak to identify the service. The front forwards non-arena
paths to OMP Edge; Edge deliberately sends root `/v1`, `/api`, `/assets` and
`/healthz` to Gateway and sends `/ws` to HandWave. The same URL origin therefore
split the Node's WS and model HTTP across different authorization systems.
The investigation preserves those existing shared routes and creates a distinct
HandWave path only after scoped source/streaming/UI-route review. Prefixing Node
requests alone does not fix Web assets, API calls or login redirects. Actual
public and browser evidence remain required; private SSH success is insufficient.

The scoped Edge ingress was independently reviewed and deployed with protected
other-service/config/auth hashes unchanged. Actual public Node WS/model HTTP
then passes compile/run/log/native-continuation in25.156s; public-b receipts
are recorded in delivery evidence. A real browser creates and reloads a
workspace and pairs the standalone Node through the same prefix.

## HWN-007: local credential helper and Microsoft Store Python redirection

A helper wrote an authorized workbench credential under LocalAppData, then the
native ACL operation reported file-not-found. Store Python's AppData redirection
made its apparent path differ from the actual Windows path. The helper removed
the file it created, preserving no unprotected credential. Native PowerShell/.NET
created the real private directory with explicit current-user/SYSTEM ACL first,
then created the file exclusively. Credential values were never tool output,
source, package contents or model requests. This is a machine-specific helper
trap, not a new Agent runtime claim; durable environment promotion is deferred.

## HWN-008: successful native task left the Web Agent card running

The actual browser in pressed 本机 mode writes Rust, compiles/runs an EXE and
shows its verified log. The composer becomes idle, but the Agent dock still
shows a running 本机 card and a second completed Agent card. Source evidence:
the deployed web startLocalRun creates agent_id=local; agentIdentity then
prefers agent_id over task_id. Rust wire events previously supply only task_id,
so their terminal event updates the second card. The task's real effects do not
prove correct progress settlement.

Final40 transport source SHA40f6bafd864a396f9a0e05d3d2ff500803c56676ba811ce6fb230b00e6b339e9
sets only agent-loop events to the existing local identity (`agent_name=本机`,
`agent_role=operator`), preserving distinct Run task_id values and agent-task identity.
Idle early rejection emits agent_end before ordinary out with no authority to
settle another job. Admitted grant denial/selection failure/timeout/pending
cancel emits one failed/canceled local end before its settling out. Same-ID
bad packets preserve the original active run and cannot emit its agent_end.

Source review found a related terminal-queue window: a late grant could start
after timeout had queued out, or duplicate retry/queued steer could reopen the
local card after its end. A shared ending marker is set before terminal events
are queued; late grants are ignored and old-run retry/steer progress is
suppressed. Legitimate new follow-up requests still queue under the existing
behavior. The actual-process fault fixture delays terminal collection beyond
the12s grant deadline, then delivers a valid grant, and separately cancels a
pending grant before delivering it. Both paths retain zero model/tool effects
and one correctly identified terminal end. This proves those bounded paths,
not a general concurrency ordering guarantee.

Counterevidence to declaring the UI fixed from final events alone: deployed
recordAgentEvent assigned every event.status to its Agent card. Once all native
events use local, tool_end completed/failed would prematurely settle the main
card while the model continued. The independently reviewed Ray front adapter
at `ray-front-handwave-adapted-app.js:1023`, SHA
`d40975e7c2dbd700e6dd60bb858ecf38e693b5f4248d3a9d20636b9d4cf9f3a0`,
only excludes local tool_start/tool_end/retry/steer from main-card status
assignment. It keeps the original tool/progress status in events and timeline;
actual Agent lifecycle events continue to own the main card. Other Agent and
agent-task identities retain their existing treatment. Sending tool events to
invented child Agent identities would instead create unwanted extra cards.

Final Windows EXE09a5c339 and Linux binary801810fc each pass25 modern actual-Hub
checks,4 Smart checks and7 controlled grant/failure checks; final-source unit
tests are12/12 Windows and11/11 Linux. Source/build pins, raw failures and
measured timing belong in the delivery evidence. Independent Codex source
review plus `D:/WorkSpace/tmp/ray-front-independent-native-review.json` (17
attempts) and `ray-front-independent-fault-review.json` (7 attempts) record
actual-packet replay through the reviewed adapted reducer, all PASS. DOM
rendering is stubbed. The candidate receipt marks the adapter tested but not
deployed at that observation; replay does not prove the live browser has it.

Case status remains **open for real HTTPS/browser acceptance** by the main
agent: final40 intermediate running and completed/failed/canceled cards,
actual cancel effects, reload and native continuation still need live evidence.
Historical f4e8edae HTTP/cloud success cannot accept these changed artifacts.
Kernel send-buffer saturation and successful-end/duplicate collision timing
were not deterministically forced; the reviewer additionally records an
atomic ending check-to-enqueue interleaving as a potential unproven gap.
Linux rustfmt is unavailable, Linux Clippy/real cloud are NOT RUN, and its
glibc2.39/WSL2 result does not accept older Linux, musl, macOS or ARM.

## HWN-009: authentication complaint and old root login ingress

User reports invalid token/OTP binding and wants daily OTP-only login. Current
source already supports30-day trusted-browser proofs (remember checked by
default), followed by6-digit OTP. Safe live inspection finds valid TOTP storage,
one current unexpired/unlocked browser proof, matching fingerprint, unchanged
Hub/root/TOTP hashes, and two successful browser authorizations. Thus no live
evidence currently justifies resetting the enrolled secret. The old GET/login
success still redirects to root workbench, where /api goes to Gateway. The
minimum ingress repair redirects only GET/login to the HandWave namespace,
preserving query and leaving POST/auth/API contracts intact. First enrollment
failure consumes its one-time preauth and generates a new QR candidate; the
new QR must be scanned again. Existing enrolled TOTP verify failures only
refresh preauth and do not change the enrolled secret. User's exact failing
step and any required re-enrollment remain separate from route repair.

The user subsequently clarified that the requested reset is one shared Ray
platform QR for every new browser/user across all hosted Web services, rather
than only a HandWave login change. The earlier root-login diagnosis remains
historical evidence; it is not the final product scope. Ray's independent
browser ingress reuses existing application credentials server-side, while Web
users supply only the shared authenticator code. Old HandWave TOTP and device
enrollment credentials remain separate authorities. The user explicitly retains
all existing ESP32/NUC device contracts. At the 2026-10-14 candidate checkpoint,
the platform QR is privately provisioned but unbound; actual holder binding and
all-service browser acceptance are pending. This case must not be closed solely
from the candidate's 22 test groups.

Actual first-holder acceptance found another boundary: the user initially
reported scanning/binding, but content-free server metadata still showed
bound=false and no active sessions. The later form submission appeared to hang;
server attempts/counter remained zero, and a browser reload returned the explicit
expired-page error. Thus the observed failure is before OTP validation, not
evidence that the new QR secret is wrong. The five-minute one-use browser proof
and authenticator enrollment are separate lifetimes. The returned form has a new
challenge, so retry with the current code without resetting or rescanning the QR.
At this checkpoint actual successful holder submission remains unverified.

2026-10-15 follow-up: the narrow Ray login-page script now refreshes a one-use
proof immediately before a single native submission, preserving OTP, TTL and
replay/rate limits. Fetch failure restores the form; no POST is silently replayed.
The holder's actual successful browser submission and safe bound/Session/counter
metadata now agree. The existing shared QR was retained. All four authenticated
service entry pages were observed with that browser. This closes the observed
expired-form/login recovery case within those boundaries, rather than claiming
all-service business acceptance. Source/build/deployment and independent review
are linked in [GUI live evidence](../evidence/gui-live-20261015.md).

The final40 GUI gate from HWN-008 also now has actual compile/read-log,
System-query/analysis, checksum-gated official download, intermediate running,
canceled card, real child-process exit and no-replay same-chat continuation.
The sentinel remains absent after the original delayed effect time. These are
final40 effects; they do not retroactively accept another binary's GUI.

## HWN-010: cloud coordinator versus direct native execution

The unpressed 本机 button uses the existing cloud coordinator. A real readonly
System-log task succeeds through run_node_task, but a later compile objective
fails there with "provider returned incomplete or non-object tool arguments"
before any new Node admission/effect. No root cause for that cloud provider
failure is claimed. Pressing 本机 sends agent-loop directly to the compact
Rust Host; the same bounded compile objective succeeds and persists native
history. This is an execution-mode and acceptance boundary, not a repair or
acceptance of the entire old cloud coordinator/provider surface.

## HWN-011: small-agent context requires source and projection separation

2026-10-14, current compact Host worktree; final execution evidence is recorded
in `../evidence/context-memory-20261014.md`. This case is separate from Shared
Core/OMP acceptance and the still-open Ray browser login.

The original node persisted raw messages but supplied all of them to every
provider call. Its fixed rpi transform hook changes only the model-facing copy,
has no Result channel, and default conversion drops Custom messages. Therefore
a derived summary needs an explicitly labelled provider-facing carrier plus a
pre-HTTP failure gate. A no-error hook must never turn compaction failure into an
empty-context request.

Two implementation-review findings changed the first slice. A retention rule
could retain the sole large old turn and refuse to compact it; allowing a cut
after that complete old turn preserves the entire current request. An attempted
generic candidate-text replacement could alter serialized role/tool identifiers
when a candidate contained a word such as `assistant`; it was removed. The
product contract is candidate/adopted state separation, not erasing ordinary
discussion of a candidate from every historical string. Protocol identifiers
must remain untouched.

Revoking only the independent reference still leaves its text in old assistant
or tool paraphrases. The first slice therefore invalidates the entire prior
working view at a complete native boundary. Raw history remains audit data,
and current active references can be retrieved again. This trades unrelated
older working context for a simple, inspectable withdrawal boundary. It does
not replace the Session or replay tools.

Context-maintenance failure is distinct from a failed native checkpoint. Once
the loop has settled, the raw transcript validates and native persistence has
not failed, a failed summary can clear the pending-run fence and permit a later
attempt. Unknown tool effects or failed native persistence retain that fence.

The initial controlled-process scripts also exposed test assumptions: a new
context sidecar is not a second native checkpoint; project-rule sentinels must
not also occur in the user prompt; and a deliberately canceled provider handler
must finish before checking its error receipt. Expected connection-reset errors
are identified only for that cancellation scenario, not ignored globally.

The suspected oversized tail-row misnumbering was independently excluded:
the existing loop drains older rows before skipping an oversized row, leaving
only a continuous suffix. No speculative tool patch was made for that hypothesis.

The first real summary trial exposed a provider compatibility boundary that
controlled text-only output did not cover. The fixed route returned Thinking
alongside ordinary Text despite ThinkingOff. A narrow admission fix retains
strict stop/type checks, ignores Thinking for summary content and bounds both
streams. A subsequent real result confirmed `stop` and `[thinking,text]` but
failed strict JSON parsing. Neither old failure preserved summary text, so
fence versus schema errors cannot be inferred. The selected fix requests JSON
mode only for a cloned summary model and adds a schema example; it does not
extract arbitrary JSON or silently retry. Safe parse-shape diagnostics and an
actual wire assertion distinguish output-format failures from content-type
failures without storing model reasoning bodies.

A content-free shape probe reproduced numeric source IDs; the prompt now
shows the entire candidate schema and requires exact string IDs. A subsequent
trial stopped at the output limit with Thinking only. Pinned rpi's Off omits
reasoning effort rather than sending DeepSeek's disable switch. The verified
model receives that switch only for summary requests. Another bounded probe
returned valid typed candidates plus unused root metadata. Independent review
accepted ignoring extra root fields while keeping required known fields and
candidate objects strict. This avoids treating an implementation preference as
a product requirement: unused metadata is never persisted or given authority.
Neither numeric IDs nor unknown sources are repaired by inference.

The final release trial also caught an acceptance-prompt mismatch: after
correctly recalling and writing the marker, the model read back its newly
written file. The prompt prohibited rereading original inputs, while the test
forbade all additional tools. The original FAIL and exact correct artifact were
preserved; the prompt was clarified to forbid output verification too, without
weakening assertions or changing the binary. A fresh same-release trial passes
all source, summary, adoption and one-write recall checks. A test mismatch is
not recorded as a product memory defect.

An accepted-trial follow-up observation remains nonblocking: the model's final
text incorrectly described a successful proposal as missing fields. Actual
tool arguments, completed receipt and persisted candidate contradict that
self-report. The working projection's masked prior proposal arguments are a
plausible explanation, not an isolated root-cause result. Preserve this distinction
for a later usability change rather than misclassifying the memory write as
failed or silently promoting a candidate.

The 256-message/16 MiB native limit and full-checkpoint rewrite cost remain.
Model input compaction does not solve storage growth. No measured model-speed
gain, arbitrary-length conversation, personal-account isolation or OS sandbox
is claimed by this slice.

## HWN-012: complete-task fixture boundaries and external download deadline

2026-10-15, enhanced Windows c9bf. A complete real-cloud task initially failed
because its PowerShell helper assumed Get-FileHash was discoverable. A read-only
subprocess comparison found inherited module-path discovery differs between the
parent and Windows PowerShell child; this is environment evidence, not a product
memory defect. .NET SHA256 removes this fixture dependency without changing the
actual environment, binary, expected official hash or hash-before-exec rule.
Rust source comparison permits only one trailing LF, retaining both hashes.

The independently reviewed fresh attempt then timed out at the overall
180-second limit after write_file and download_file start. There is no download
completion receipt or downstream artifact; the helper was never reached.
No specific network root cause is established. The tool's own 300-second cap
does not guarantee completion within a shorter Run budget. Cancellation cleanup
stops the temporary node, revokes approval, stops the tunnel and removes its
download temporary file. No third blind replay was performed.

Both failures remain failures. They do not erase the separate controlled final
binary compile/download/proxy matrices, real-cloud context task, or final40 GUI
compile/log/download/cancel/continuation. Acceptance must name the actual version
and evidence layer, and cannot combine their names into a fabricated single-Run
success. [Receipts and exact limits](../evidence/gui-live-20261015.md).
No comparable manual baseline was measured; no efficiency percentage is claimed.

## HWN-013: reproducible archive timestamps can hide a stale Cargo product

2026-10-15 cloud-memory source acceptance. The archive normalizes mtimes to zero.
An initial build reused a product target and returned the old a2059 image, without
the new cloud marker. Exit0 and a version string were insufficient evidence.
The original receipt remains INVALID_STALE_CACHE. Advancing only the task's
extracted Rust source mtimes forced recompilation, with source hashes unchanged.
Final source3ffb builds GNU x64 image70c8 with both cloud and explicit-follow
markers in 61.894s. Public installer uses a fresh target; ARM/musl remain untested.
This is an acceptance-cache error, not evidence of a runtime memory defect.

## HWN-014: explicit Web sessions require both native and UI ownership

2026-10-15. Real Run I7W3O7W2AQ failed the existing route fence because multiple
new Web conversations mapped to the same controller/cwd native history. Do not
delete history, weaken the fence or silently create a new session after failure.
An explicit validated workspace/session key now selects its own native path;
empty requests preserve the exact legacy path. Grant lookup and Run start use
the same key. Explicit follow rejects a different key and retains its supplied
ID so the browser can bind subsequent receipts, including duplicate requests.

Independent original-JS-function replay then proved A text/end could POST into
the currently selected B conversation. Native path isolation alone did not fix
that distinct defect. The browser now registers actual sent Run IDs with captured
owner/epoch, rejects late cross-owner text/terminals and does not let old terminal
frames clear a newer Run. Detached output stays in the node audit. Final EXE
native4/cloud5, original-function VM and actual recall/revoke pass; see
[final evidence](../evidence/cloud-memory-20261015.md).

The actual revoked-memory trial answered UNKNOWN with no old marker in raw
history but additionally called reference_read once against the instruction.
Memory invalidation and model obedience are separate judgments. Preserve this
failure rather than claiming a fully passed no-tools task or automatic learning.

## HWN-015: a successful content match can hide its own evidence

2026-10-15. search_files searched a longer line but returned its first1024 bytes.
A hit near the end therefore gave the model an excerpt without the requested
literal, encouraging unnecessary rereads. The fix maps the first match back to
original UTF-8 and clips near it, preserving byte offset and explicit truncation.
İ lowercase expansion and Greek contextual sigma are covered independently;
actual temporary-file search and real Hub/model receipt check the visible hit.
This does not expand the existing inspected-byte budget. Controlled host success
is separate from real-model task success; no manual baseline exists.
[Versioned evidence](../evidence/problem-tools-20261015.md).

## HWN-016: script initialization can invalidate valid PowerShell source

2026-10-15. Independent Codex reviewer rust_tools_review reproduced a P2:
prefixing executable initialization before param()/using makes legal source fail.
The rejected design is preserved here as counterevidence to treating a simple
Write-Output fixture as full script acceptance. A separate bootstrap receives the
unchanged BOM-prefixed source path via argv and propagates errors/native exits.
Final Windows tests exercise param defaults, using namespace, native exit7,
Write-Error, timeout/cancel and cleanup. Python readiness-marked descendant tests
verify no delayed file effect after cancel. This is not universal interpreter
semantics acceptance or an OS sandbox.

The real DeepSeek repair trial also encountered one exact-edit failure because
its replacement used LF against a CRLF file. It inspected bytes, rewrote and
executed the repaired code successfully. Exact-match edit behavior remains
intentional; no fuzzy patch or silent line-ending rewrite was introduced. The
18.821-second task time has no comparison baseline; supplier token usage is
unknown. Receipts and remaining platform boundaries are in the same evidence.

## HWN-017: a proposed lesson must not manufacture its own evidence

2026-10-15, Node self-evolution slice. Existing user references prove received
input, not successful tool effects. Reusing only model-written lesson text as
proof of recovery would create a self-confirming loop. The selected design
resolves call/result pairs from the current native Run, not model-supplied status
or output, and retains the existing user adoption boundary. An Emitter append
is not durable when checkpoint save failed; the snapshot rejects fatal/context
failure and cancellation on both sides of the copy. Pending proposal calls are
excluded from completed-pair evidence rather than requiring premature Run closure.

Independent Codex reviewer evolution_pre_review also found the initial 8KiB
retrieval budget omitted its JSON envelope. The fix measures the whole response,
with escaped/Chinese boundary tests; candidate proposal arguments must be removed
from projections just like ordinary memory proposals. These safeguards preserve
reviewable provenance rather than claiming automatic intelligence improvement.
[Actual tests, immutable image and remaining limits](../evidence/evolution-20261015.md).


## HWN-018: opaque protocol IDs were not a usable model-facing reference

2026-10-15. The first real-model evolution task used its 10-call allowance after
repairing the report but before proposing a lesson. A separate one-fault LF
trial then repaired and verified the report, but guessed symbolic tool IDs in
repeated proposals. The strict Host correctly rejected missing native pairs;
no candidate was adopted or persisted. This is a tool reference usability gap,
not evidence that invented IDs should be accepted or that budgets should grow
until an experiment passes.

The repair exposes two exact current-Run receipt IDs in bounded provider-only
metadata, without output bodies, persistence or an extra model call. The model
can copy these references while the original native pair validation remains.
Independent review caught the added metadata potentially pushing an otherwise
valid context over its byte limit; optional omission preserves the original
projection, and a near-limit regression verifies byte-for-byte preservation.
Both earlier failures and the new image trial remain separate in
[the release evidence](../evidence/evolution-20261015.md). No causal or efficiency
percentage is inferred from a successful candidate.

## HWN-019: a canceled waiting Job must not resurrect its Goal

2026-10-16. The first actual Goal fault fixture confirmed that worker cancellation
alone was insufficient: a waiting Goal had no Active Run, and its late native
`waiting_job` observation overwrote the dispatch cancellation. The worker had a
real canceled receipt, yet the scheduler issued another model Run. Restart could
also restore the old native status over the canceled dispatch manifest.

The fix retains a durable cancellation fence across observation and recovery,
and records the idle native Goal as canceled when safe. Pending native effects
are still settled by their own emitter. An actual targeted retest verifies no
new provider call before or after Node restart and no delayed file effect.
The original failing fixture remains separate from that retest. Full final-image
acceptance is recorded with the long-task delivery evidence, not inferred from
the source correction alone.

## HWN-020: stopping a Run must settle authority even if output stalls

2026-10-16. Independent reviews found that taking Active during pause/cancel
made the later terminal packet miss its identity match. That skipped model
grant end, operation receipt completion and the cloud Goal mirror. A bounded
output channel can also prevent a terminal packet from being queued at all.

The Node now finalizes successfully stopped frames directly through the socket
and durable receipts, independently of late terminal delivery. Old terminal
packets cannot stop a newer Active Run. Cleanup timeout remains fatal even when
the second Job cancellation scan also fails; reconnecting in that state could
otherwise overlap an unsettled old task. Actual active-pause fixtures verify
child cleanup, ended grant, settled operation receipt and stable command result.
Combined disk failure plus cleanup timeout remains a separate fault boundary.

## HWN-021: idle cancellation needs its own cloud publication

2026-10-16. Independent review found a separate visibility gap after HWN-019:
an idle waiting-Job Goal could be durably canceled without any Active Run to
emit its terminal Goal mirror. The Hub therefore retained waiting_job even
though the worker and native Goal were canceled. The typed stop command already
published the result; the old agent-abort path did not.

The fix captures matching contracts before cancellation and, only when no Active
Run matches, publishes the native canceled Goal with the same Goal identity.
Accepted history and stale/non-canceled observations are skipped. The final
release-v4 actual-process regression checks increasing native revision, Hub
inspection of that exact canceled revision, worker tree cleanup and no Goal
resurrection after restart. Seven Goal fault cases pass in111.938s. This is a
state-publication correction, not a new authorization or recovery policy.

## HWN-022: a large-history fixture can fail before testing recovery

2026-10-16. Windows actual-Node stress trials crossed the former raw limits but
did not reach every recovery assertion: some hit an artificially short180/300s
fixture watchdog, while separate trials received intermittent Hub upstream502.
The original fixture default was900s; there was no user latency SLA. Raising a
functional execution budget does not turn the original timeout into a performance
pass, and reaching17MiB does not prove the unexecuted restart/revocation chain.

Isolated catalog, mixed GET/summary/agent HTTP and90actual-Node Run probes did not
reproduce the dial failure. The first diagnostic captured only a flattened error
chain because the transport formats the lower error with `%v`. A second isolated
diagnostic logs types/op/errno before flattening, without URLs, credentials or
bodies. No product retry or connection-policy change is justified by the current
evidence. Specific errno and cause remain UNKNOWN; diagnostics are not feature
acceptance. Full process evidence and any explicitly reviewed composed coverage
are recorded separately in the long-task delivery evidence.

## HWN-025: cancel before model grant had no frontend terminal

2026-10-16. During managed-queue regression, the actual `verify_grant_terminal.py`
fixture reached the model-grant request, canceled before grant admission, and
received only `model-grant-end`. No `agent_end`/`out.done` reached the controller;
the frontend could remain busy although execution had stopped. The exact0.2.1
installed image and the first0.2.2 candidate both reproduce the same30s timeout,
with zero model/tool HTTP effects. This is an existing publication defect, not
evidence of a replayed model call or a new queue execution failure.

Source: an admitted Active with `job=None` has no Agent worker to publish a
terminal when `stop` removes it. Cancellation must create one canceled terminal
only if no terminal was already claimed, then preserve the existing receipt and
model-grant settlement path. Late grants must still have no authority. Current
fix verification and retained baseline failures belong to the
[0.2.2 queue evidence](../evidence/message-queue-0.2.2-20261016.md).

Final executable regression passes eight groups in16.201s, including three
cancel-to-next-message trials and two actual early active snapshots. Exactly
one old terminal precedes the next grant. The authenticated production browser
also completed a real DeepSeek Run and its edited FIFO successor; that does
not replace the deterministic pending-grant cancellation evidence.

## HWN-026: queue presentation and effective objective must have one owner

2026-10-16. Independent source review of the new queue identified that the
legacy Frame can carry `task.objective`, which `objective(frame)` prefers over
`msg`. Editing only `msg` could acknowledge a new displayed queue text while
starting the old hidden objective. Queue admission now normalizes this effective
objective into the sole pending message and removes the redundant task payload.
This applies to queued agent-loop text; the independent agent-task contract and
large immediate-request behavior remain unchanged. A focused unit test covers
structured ingress, editing and later effective objective consistency. It does
not stand for full legacy task or OMP protocol parity.

## HWN-027: successful writes and elapsed attempts did not prove liveness

2026-10-16. Source audit of0.2.2 found no active Node Ping/Pong deadline, bounded
only the outgoing-channel write rather than every direct write, and reset the
retry delay after any session attempt lasting twenty seconds. A timed-out
twenty-second handshake therefore repeatedly returned to the shortest delay.
The maintained Hub's fixed nhooyr v1.8.17 Ping only returns on missing Pong;
its concurrent Read does not inherit that deadline after a successful Flush.

The scoped fix binds all Node writes/dial to deadlines and shutdown, requires a
matching timely Pong for liveness and healthy reset, and closes the failed Hub
connection to release its ordinary cleanup. Actual0.2.3 loopback faults prove
missing/wrong Pong reconnection, healthy survival and increasing handshake-fault
backoff; the real Hub test observes cleanup after its production60s heartbeat
window. Physical-network and send-buffer saturation are not established.
See [the evolving recovery evidence](../evidence/recovery-0.2.3-20261016.md).

## HWN-028: transport restoration and recovery notice diverged

2026-10-16. Authenticated production acceptance of0.2.3 found a connected
RAYWORK header beside an old offline/grant-expiry notice. Accepted node-hello
cleared the observation flag but never refreshed that notice; the first Hub
status without the peer also permanently consumed the automatic observation
attempt. A later peer could restore legacy pairing without restoring the
recovery UI. This was not evidence that device authorization had expired.

The narrow two-file frontend fix refreshes only visible connection notices
after actual peer/current-scope binding. A temporarily missing or offline peer
can be checked on later status under the exact original controller and valid
grant. Explicit expiry/rejection remains manual; auth/stream notices and
dismissed prompts are not replaced. New controlled cases fail on the old leaf
and pass on the final leaf. Final production restart observes retained draft,
consistent connected header/notice,55 unchanged native files and no replay.
See [final recovery evidence](../evidence/recovery-0.2.3-20261016.md).

Preflight trap: this host's Microsoft Store Python cannot see the actual
AppData Node state through its redirected view. A zero-file scan was rejected
and retained; native PowerShell with hidden files reports54 real state files
before the live task. Safety preflight must require the known state directory
and a nonempty manifest rather than treating an empty traversal as quiescence.

## HWN-029: stale native Goal could reopen input after a canceled dispatch

2026-10-16. Independent review of the0.2.4 implementation found a crash window:
Runner persisted cancellation before Job markers/native suspension. If Node
crashed between those steps, the independent worker could still be alive and
native Goal could still say waiting_job. Exposing Execute input tools from that
native snapshot alone would bypass the durable canceled dispatch fence.

The rejected assumption was that matching native Goal/lease proved current
authority. Transport now supplies resume_job_lease only after Runner and native
Goal agree on the identical active original lease. No native-history fallback
can manufacture this Host authorization. A focused restart/cancellation test
keeps native waiting_job deliberately stale and proves input remains denied.
This deterministic fault test does not prove every physical crash/power-loss
interleaving. See [0.2.4 evidence](../evidence/interactive-0.2.4-20261016.md).

## HWN-030: returning to the same process did not identify the same log view

2026-10-16. The reviewer reproduced A->B->A selection while A's old cursor8192
request was in flight. Matching only job_id allowed the old suffix to replace a
new view at cursor0, skipping the prefix. A stable process identity therefore
was insufficient to identify the presentation generation.

The console now binds response admission to selection generation, requested
cursor and current cursor, and retires old log tickets on selection changes.
The reproduced controlled DOM case passes after repair. Real browser checks
exercise normal display/input/EOF; the exact ABA interleaving is a controlled
transport test, not a claim of physical-network coverage. Unknown input recovery
separately retains scoped operation IDs only, so reload cannot silently convert
an unconfirmed send into permission for a fresh input.


## 2026-10-16 Native terminal host boundaries

Scope: compact Host0.2.4; Windows ConPTY and WSL Linux PTY, independent of
root Core/OMP. Sources: terminal_process.rs, terminal_emulator.rs and
[executed terminal evidence](../evidence/terminal-0.2.4-20261016.md).

- A pipe-redirected Windows worker can leak its own redirected std handles into
  the ConPTY child even when a pseudoconsole attribute exists. The delivered
  spawn uses STARTF_USESTDHANDLES with INVALID_HANDLE_VALUE for all three handles;
  actual Python stdin/stdout/stderr isatty and CMD/Read-Host input establish this
  host shape, beyond merely successful CreatePseudoConsole.
- An interactive Unix shell can create other process groups inside its terminal
  session. Killing only the root group leaves such jobs alive. Linux cleanup
  enumerates groups belonging to the original session, with a one-shot guard;
  actual bash job control and descendant tests pass. Deliberate setsid escape
  remains outside this boundary, as does Unix worker SIGKILL cleanup.
- Browser replay must not answer old terminal queries. A persistent worker VT
  parser answers the declared live DSR/DA subset; fixed xterm input-source
  classification forwards only actual keyboard/IME/paste. Initial no-browser
  CPR, reconnect and attached-parser tests distinguish this from dropping every
  response or flushing old responses on attach.
- The old generic metadata writer had a256KiB ceiling. Terminal input receipts
  use a separate2MiB atomic journal with an executed1004-entry regression; a
  larger read allowance alone would not fix persistence admission.
- Completed test/build evidence is bound to image hashes. Rebuilding the same
  release directory during packaging can change the image; the final package
  is re-exercised and copied to an immutable evidence path before publication.

The independent reviewer requested each of these corrections or verification
boundaries. Actual output, negative cases and unverified platforms remain in the
release evidence; no unreviewed runtime experience is automatically adopted.
