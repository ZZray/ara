# CTX-SNAP-01 — native image archive and frame rescue

2026-10-02, base `c6d3ce9a6b996662299fc51b7132ef708dd0760f`, branch `dev`.
Fixed OMP `596f2da7101178214aa27a753529d15e6b7ad91d` is unchanged.
**Implementing / tested bounded WIP. Mandatory real-model acceptance failed;
this point, its parent surfaces and P1–P6 are not accepted.**

## Observable requirement and scope

Reproduce the fixed local text-to-image archive, frame-only token accounting,
native Session publication/reopen and smaller-budget archive rescue. The
reference Host exposes explicit REPL and RPC compaction plus configured
threshold and incomplete-turn rescue. Preserve genuine raw entry identity,
retained boundaries, opaque metadata and explicit no-progress outcomes.

| Fixed source | Rust owner and observable slice |
| --- | --- |
| `packages/snapcompact/src/{index,snapcompact}.ts`, prompts and original tests | Neutral `ara-snapcompact`: shapes, archive validation, serialization, Unicode, layout and rendering preparation; grouped native test families. |
| `crates/pi-natives/src/snapcompact.rs`, fonts and licenses | `ara-snapcompact::native`: fixed renderer and font assets; NAPI boundary replaced with an ordinary Rust API. |
| `packages/agent/src/tokenizer.ts`, `test/snapcompact-frames.test.ts` | `ara-agent::tokenizer`: derived archive frames add 5024 tokens each; ordinary User image accounting retains its prior semantics. |
| `packages/agent/src/compaction/{messages,utils}.ts`, archive-context prompt | Checked `ara-session::snapcompact` projection/publication, source ownership and archive-to-soft-to-soft continuity. |
| Session blob store, loader, persistence and image families | Host-owned blob directory; publish bytes before references, reopen images, old truncated-frame healing and retained warning references. |
| `agent-session-snapcompact-budget.test.ts`, frame-dead-end family and maintenance callers | `ara-cli::{snapcompact,rpc_host_snapcompact,rpc_host_reduction,rpc_host}`: frame sizing, automatic rescue, terminal ownership, safe result/event metadata and one canonical warning. |
| Fixed provider image encoders and model input metadata | Generic Chat/Responses retain explicit detail. Authored custom model `input` reaches Host and promotion metadata; absent image capability is not inferred. |

Source copies are under `C:\Temp\ara-snap-preplan\upstream`; the final
`C:\Temp\ara-snap-batch\final-manifest.json` records all 65 source hashes,
including the four later source additions. The baseline object store remains
separate from ARA Git history. The scoped manifest records 71 repository
source/test/runner paths, the binary and patch hashes. Existing vendor/EOL WIP,
`.codebase-memory`, private configuration and generated files are excluded.

## Boundaries and intentional adaptations

- Native renderer working-memory estimate is capped at 512 MiB per frame,
  with at most two concurrent frame workers. Extreme upstream allocations can
  be rejected explicitly; this is a resource adaptation, not full parity.
- `ImageContent::compaction_frame` is runtime-only and is derived from a
  validated archive. Explicit `detail` persists and reaches generic wire;
  public RPC/event results remove archive text/frame payload while preserving
  unrelated `preserveData` fields.
- Blob references use content-addressed canonical files. A canonical directory
  or non-regular file fails before reference publication. Missing/malformed
  references remain present with a warning, as in the fixed loader. Existing
  regular canonical files are reused without an added content-integrity audit.
- Only verified archived images with known source ownership can survive a
  soft-summary chain. The original `archiveSourceEntryId` remains checked on
  later soft summaries. New raw-image soft summaries are still unsupported;
  unknown legacy archive sources remain unknown rather than being invented.
- Rescue sizing always uses the configured threshold recovery band. Threshold
  success still requires at most floor(threshold × 0.8); overflow/incomplete
  retry uses ordinary usable-window fit. Unknown windows retain hard caps.
- No-progress threshold maintenance emits its end event before one canonical
  warning. An unpublished rescue does not change old archive warnings. A new
  inadequate archive receives the actual latest warning/badge and safe result.
  Warning-publication failures stop the Host rather than continuing fallback.
- Renderer cancellation joins the CPU worker before returning and never
  publishes an abandoned result later. Full concurrent manual RPC and all
  replacement/speculation owners remain required later work.

## Executed module and workspace receipts

Commands use the project's module runner:

```text
python -X utf8 scripts/verify_snapcompact.py
python -X utf8 scripts/verify_snapcompact.py --module core
python -X utf8 scripts/verify_snapcompact.py --full
```

`C:\Temp\ara-snap-batch\module-20261002T123358Z\receipt.json` records nine
module families **219 passed / 0 failed / 0 ignored**: native renderer 18,
original-source families 8, Session 4, Session migration/projection 19, frame
count 4, image HTTP 50, budget/rescue 5, RPC compaction 17 and REPL 94.
Compile takes 8.764s; module execution takes 103.497s. This whole invocation
later fails at backend Clippy and remains FAIL; module-phase success is not a
successful full invocation. The collapsible-if diagnostics are fixed.

`gate-20261002T124901Z` passes current formatting and Clippy but stops at the
AI lib: 264 passed / 1 failed. A newly added positive Codex image assertion
crossed the existing deferred text-only transport scope. Fixed OMP supports
Codex images; ARA does not yet. Only the new unsupported positive branch and
incorrect comment are removed. Generic Responses/Chat detail assertions and
the existing Codex image rejection family remain. Runtime is unchanged by
this test correction; the failed receipt is retained.

Final `completion-20261002T125814Z/receipt.json` records composite Windows
tests **1,616 passed / 0 failed / 20 ignored**, source unchanged. It reuses
only the complete unchanged Agent crate (128/0/0) and reruns AI plus all
previously unexecuted crates (1,488/0/20). Current formatting, Clippy, doc tests
and inventory pass. The whole completion invocation remains **FAIL** because
the dependency policy fails; it does not run its final build step.
Partial counts from the failed AI lib are excluded. This is composite
verification, not a successful single full invocation. Windows uses
process-local `CARGO_BUILD_JOBS=4` and `RUST_TEST_THREADS=1` with Git Bash.

Completion takes 360.753s: suffix test command 342.187s, including 1m41s
compilation; Clippy 9.511s and doc tests 3.899s. Earlier passed original
module families and live tasks are not repeated. A separate actual command
`cargo build -p ara-cli --all-features --locked --bin ara` succeeds in 0.30s;
Root records it in `build-receipt.json`.

`cargo deny check` fails on **RUSTSEC-2026-0192**: unmaintained `ttf-parser`
0.25.1, via the fixed renderer's `fontdue` 0.9.4. Its advisory reports no safe
upgrade. Bans, licenses and sources pass; existing duplicate/license warnings
remain visible. `deny.toml` is unchanged and no advisory is ignored. Resolving
this source-preservation/dependency-policy contract remains mandatory before
acceptance; a scoped WIP commit cannot make the failed gate pass.

## Actual controlled Rust Host

```text
python -X utf8 scripts/verify_snapcompact_host.py --binary target/debug/ara.exe --family all
```

The supplied actual binary uses loopback upstream fixtures, never private or
live credentials. Separate receipts retain these observed families:

- `host/run-20261002T124343Z`: manual RPC, actual PNG/blob, same-Session reopen
  image wire/detail and explicit REPL mode pass; a later automatic fixture in
  this whole invocation fails, so the whole receipt remains FAIL.
- `host/run-20261002T124530Z`: threshold normal plus just-written archive
  rescue produces two archives/four final frames; stale 16-frame archive
  becomes a text-only archive while preserving IDs and unrelated metadata.
- `host-failed/run-20261002T124555Z`: failed Assistant ownership and retry,
  two controlled requests, one appended rescue and durable continuation.
- `host-deadends/run-20261002T124555Z`: minimum, text-only and oversized
  retained-prefix cases each emit one canonical warning, make zero model
  calls and do not alter the old archive warning or add a barrier.
- `host-insufficient/run-20261002T124807Z`: unknown-window rescue publishes
  smaller frames but still lacks headroom. Lower tiers run, end-event safe
  result retains opaque metadata, latest badge matches the notice, zero calls.

The last earlier receipt directly binds final runtime source except the
test-only Codex fixture correction; the other receipts precede the final
safe-result/warning patch. The final actual combined command passes all five
families on the frozen source/binary:
`final-host/run-20261002T130705Z/receipt.json`, 15.003s, source unchanged, exit 0.
These affected Host families are rerun because the final result/warning patch
changes their runtime contracts; no unchanged Internet task is repeated.

Earlier Host failures are retained: custom model `input` was missing from Host
metadata and is now passed through; one usage-anchor fixture stayed below the
configured threshold and now supplies genuinely unknown usage; one stale
archive fixture lacked a settled raw prefix and now supplies that prefix.
The original assertions are not weakened. Typed archive numerical equality
uses its native f64 meaning rather than JSON integer-vs-float representation.

## Real CAS tasks — FAIL, no task acceptance

Root privately verifies the current Manager/CAS route and catalogue. The
Manager gateway is unavailable, so these trials use its private configured
direct CAS route, model `deepseek-v4.1-flash`, OpenAI Chat. Credentials stay
only in relay memory, outside child config/source/Git. The native local
`/compact snapcompact` makes zero model calls; test facts exist solely in the
imaged middle, absent from textual archive edges and initial text wire.

Runner: `C:\Temp\ara-snap-batch\run_live_snap.py`. Each task uses a fresh
isolated output/Session, at most three processes, 90s per process and 2048
output tokens. The first task has an eight-call cap and settles after three;
the second cap is reduced to the remaining five, preserving the combined
eight-call bound. Seven actual calls occur. Both attempts have settled: no active requests or relay
errors remain. No third task is run and no earlier task is blindly replayed.

| Receipt | Observed outcome |
| --- | --- |
| `live/20261002T124920.027464Z/receipt.json` | FAIL; 3 calls, 20.335s. Actual write/read succeeds and all amounts equal 66.25, 28.5, 94.75, but `Rill-Image-8476` is read as `Fill-Image-8476`. Strict artifact acceptance fails; no reopen is attempted. |
| `live/20261002T125318.174197Z/receipt.json` | FAIL; 4 calls, 49.658s. Model reads `MAPLE-9037` as `MABE-9637`, writes twice and produces incorrect amounts 1920, 975, 2895 rather than 59.5, 23.5, 83.0. The tool-order assertion fails; no reopen is attempted. |

Root inspects the original PNGs. The second image visibly contains the exact
label `MAPLE-9037` and CSV `ring,7,8.50; plate,2,11.75`; its PNG hash is
`5462a13049b1a8573e7a0e9aa174a295da65a8ca0b4a803fea19a75992d042b8`.
Six renderer font/license assets match the fixed source. This supports a
model image-reading/instruction-following failure under this trial, not a
successful end-to-end acceptance. No font, shape or expectation is changed
to make the trial pass. Image dispatch/blob success cannot substitute for
correct real-task artifacts and same-Session recall. Real acceptance stays open.

## Audit and mandatory remaining work

Independent Codex `/root/remote_review` final POST approves saving the
scoped WIP snapshot and gives **point acceptance: changes requested**. Point
status remains implementing. `final-review.json` SHA-256 is
`6ce8fe9c8800a20d40e51a910b9e9651a54e57d85896451d037edee8b842fd73`.
It reads exact source and retained receipts without independently running
Cargo/network or reading private configuration. Coverage: 71 code hashes,
65 fixed source files, 246 final Host pins, 242 composite pins and 27 reused
Agent pins, binary and both actual failed artifacts. Seven documentation
hashes are a separate pre-report snapshot; binding this POST does not rerun
the source review or change code. No open introduced-code blocker is reported;
the dependency and real-task acceptance failures remain open.

Root `root-audit.json` separately checks 71 repository hashes, all 65 files
against Git objects at the fixed SHA, final binary and actual Host/composite
receipts, exact tool starts/results and artifacts for both failed real tasks.
All 224 tracked vendor-file raw hashes are retained, including the 57 visible
vendor/EOL WIP entries. Neither those files nor `.codebase-memory` enter this
scoped snapshot. Final source/binary remain frozen; no real success on the
final binary is claimed.

Implementation begins around 11:46 UTC; runtime verification and independent
review complete around 13:13 UTC (about 87min). Documentation/Git closure is
separate. The 120–210min planning estimate applies to this batch only. One
composite suffix gate takes about 6min, including 1m41s compilation; both
settled real attempts together take 69.993s. Reuse of the fixed native code,
grouped original input families and unchanged completed crates limits repeated
work. No reliable full-project completion estimate follows from this module.

Required gaps: dependency policy resolution; actual successful image-task write/read/reopen; Codex account
image/snapcompact transport; full tokenizer and native settings percentages;
all automatic REPL selection; concurrent manual RPC/replacement/speculation;
new raw-image soft summaries; unknown-provenance archive-to-new-raw-cut;
global lossy persistence truncation/event deletion/signature deduplication;
Linux XDG layout and Linux/account real tasks; the older completed-soft
warning/order/badge contract. Rust strings cannot retain lone UTF-16
surrogates: a cut inside a supplementary scalar can substitute U+FFFD; BMP
ASCII/CJK follows the source. Existing regular blob contents are not rehashed
on reuse. These gaps remain required, not accepted intentional omissions.

P0/V1 accepted, P1–P6 open, RPC 27/42 and full parity marker null remain.
Per-point test counts and this checkpoint do not provide a total parity
percentage. Provider ordering remains custom OpenAI, OpenAI account, then
other explicitly deferred families. The next engineering work is the
remaining registry/auth callers and recorded Core/Host contracts; failed
vision acceptance remains an explicit open item.
