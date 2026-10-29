# Ordinary local Native WRITE acknowledgement — 2026-10-08

**WIP. Execution-v5 SOURCE approved; original Native group PASS and unchanged
tools group PASS reused by exact source/artifact identity. Bounded real WRITE
task/restart PASS. Final independent POST PASS; complete WRITE remains OPEN.**

## Requirement and scope

Fixed OMP `596f2da7101178214aa27a753529d15e6b7ad91d`,
`packages/coding-agent/src/tools/write.ts:402–411,1285–1296`:
guards → synchronous original Writing callback return → filesystem effect.
Ignore a callback's return; do not adopt its returned Promise. An original
throw must prevent mkdir/write and retain its original realm/Run carrier.

Keep the shared UpdateFn, other actions, original Native values, final grants,
scope/parent, effect owner/FIFO/join and started-write uncertainty. This batch
adds a Write-specific acknowledgement port and pre-effect NotDispatched.
Worker data is native output/scalars/oneshot only; graph values stay with Host.

Five files:

- `crates/ara-tools/src/write.rs`: shared action/progress adapter and specific
  pre-effect refusal; original post-write path unchanged.
- `crates/ara-cli/src/actual_tools.rs`: Write bridge, synchronous scoped Call
  and full original failure retention; Write-only early cancellation rule.
- `crates/ara-cli/src/actual_tools/async_effect.rs`: existing FIFO request/ack,
  cancel-first select and no duplicate callback failure retention.
- `crates/ara-cli/src/native_observations/host_await.rs`: original coarse
  production NativeRun group observes nine modes and closed admission.
- `crates/ara-tools/tests/tools.rs`: original WRITE group observes success,
  refusal, dropped reply and cancel-plus-success before any directory/file.

## Candidate, delivery and review

Packets share `D:/workspace/env/cache/ara-port-candidates/`:

- Author `write-native-progress-batch-20261008`, Codex
  `/root/edit_host_integration`; scope SHA256
  `b7b93f9904fa86e13241d82d875cfd927b54ad1a0a2c3f7210343fcc50b5a8bd`.
- Independent PRE reviewer `/root/finish_edit_renderer` approved the five-path
  scope and required cancel/ack priority plus Write-only early NotDispatched.
- Candidate v2 SHA256
  `07c41949c84ed3ad5cef89fb76daf3a13a98318885b8e2b3ff28903481de7816`.
  v1 stays frozen; v2 only inspects the controlled original Error.message in
  one fixture instead of its generic handle Display. Production bytes unchanged.
- Delivered packet `write-native-progress-execution-v2-20261008`; manifest
  SHA256 `6b8965e88b5e0e9f42841b3c0ba43c6d9cde795b89737f4f2ff0dfef5413cc5f`.

Root directly read the five deltas, fixed source and relevant owner/type APIs;
validated all five candidate/baseline pins and six dependencies before copying.
Scoped fmt exit 0 / **0.517 s**; only those five paths changed. Full before pins
and five before/delivered snapshots remain in the packet. Index is unchanged:
`89d34e26be97c90a14680d44ace937e6b1ae5a86e8ee7b46dd6b90718a922475`.
Dirty `dev`, HEAD `20aa0e2ef9872f633ba2671376cdf778477e7115`; unrelated WIP and
the `/new` EditStore clone remain. No commit/push/deploy/reset/stash occurred.

## Concentrated execution plan

One command, same D Cargo/Rustup/target, jobs=2, profile/features unchanged:

`cargo test --locked -p ara-tools -p ara-cli --all-features --test tools --bin ara --test e2e --no-run --message-format=json`

Execute only the two original coarse groups:

1. `tools::write_creates_dirs_counts_utf16_and_marks_shebang`.
2. `ara::tests::journal_failure_cancels_the_run`, including host_await.

The existing e2e target is compiled to force a current production CLI binary
through CARGO_BIN_EXE_ara; no extra execution group or new test entry is added.
Prepared runner SHA256
`03c5d2cb2c681bfc1c3635ff280c46849b46fd34471d7077f5395f1fc4e6f305`.
SOURCE review runs alongside the affected build; Rust stays frozen.

## Original execution-v2 failure and current execution-v3

Independent Codex `/root/finish_edit_renderer` approved all five delivered
paths and format deltas; SOURCE frozen report SHA256
`74e0457934035ca3035fb41bc843bdbe50eca2e6da3faff60239fc6397b6397a`.
Its build passes: **358.227 s, 409 reused / 7 compiled, 69 warnings / 0 errors**,
source_unchanged=true. The two original groups fail **0/1 each**, in **0.072 s**
and **2.420 s**; module total **363.488 s**. Both actual failures are the new
fixture expected path separator: Path::new on a slash-containing literal
retains that spelling, while existing ToolContext::resolve normalizes path
components and display preserves native separators. Actual `nested\\file.txt`
and `awaited-success\\file.txt` mismatch the fixture's slash paths on Windows.
Existing caught callback/panic negative controls are separate from these new
failures. Original module receipt SHA256:
`67a84f2218d573d16a30785a6f5b8446a6251b77a11788e77f64a0374d534859`.

Current packet: `write-native-progress-execution-v3-20261008`. Root directly
read resolve/display/normalize, then changed just the two fixture expectations
to construct relative paths with Path(parent).join(file). Exact Writing text,
details, ordering, original throw and no-effect assertions remain. Three
production paths remain byte-identical; all five before/delivered snapshots
and whole-tree before pins are preserved. Scoped fmt **0.449 s**; index and
unrelated source unchanged. Delivered manifest SHA256:
`ab4434774eee12ec812c9de90ab73cbade5c80ed0167bfff90a0747355bcd943`.
The original reviewed runner and CAS harness are byte-identical. The original
two failed groups were executed on this repaired source; no broad gate added.

Execution-v3 build passes **64.731 s**, but both groups fail **0/1 each**
(0.072/2.409 s): displayed path is now correct, while fixture target strings
still retain raw slash spelling in resolvedPath. Root rechecked the complete
input/target/display/detail flow. Execution-v4 changes only those two fixture
target constructors to native components; three production paths stay identical.
Manifest `db11f65ea6d6f802a8327e2064888a6f7e60dc4e63dd212b4069ed6f23219038`,
fmt **0.210 s**. Its build passes **16.696 s**; tools passes **1/1, 0.074 s**,
Native fails **0/1, 2.418 s**. The next exposed bad fixture condition uses a
`// Code generated` marker in file.txt; the existing detector intentionally has
no comment style for .txt and recognizes .ts. Root read the detector and
existing passing guard.ts test. No production guard was changed.

## Execution-v5 grouped and current-binary task evidence

Execution-v5 changes only the Native generated-guard fixture target/raw input
to guard.ts. It retains zero callback, original Error.message, unchanged file,
all other eight modes and exact identity/no-effect assertions. Five pins,
one fixture delta, four other paths unchanged; three production paths unchanged.
Fmt **0.224 s**, index/unrelated source unchanged; manifest SHA256
`99b61b9d57fa0ae54b923dbbb03a300f75466ef729749e55f54dbf92d5305b9b`.
Independent SOURCE approval frozen report:
`299610abd88f83ad2b5cb21ad578f9506588c7c2b965248f390e2d922fae3f44`.

Current build passes **19.815 s, 415 reused / 1 compiled, 69 warnings /
0 errors**, source_unchanged=true. Only the failed original Native group
re-executes: **1/1 PASS, 9.063 s**, module total **30.434 s**. Its raw Native
WRITE receipt confirms nine modes: success has completed receipt; generated
and path guards have zero updates; pre-cancel has no update; throw,
infrastructure, panic, callback cancel and callback revoke each retain the
original callback and have not_dispatched receipt. Released admission is closed
with a known no-effect receipt. This is not a pending-channel-close trial.

Root independently validates the v4 tools group reuse: v4/v5 whole source pins
differ only in Native host_await fixture; tools sources and executable remain
identical. Original tools **1/1 PASS** and current executable both bind SHA256
`40067c2b22e9ca4eecb50cde11faa1e9f9e917361980d968518c3fdb5092a589`.
`combined-group-result.json` retains both original receipts and this reuse proof,
SHA256 `e4b22d529edcb74764ff50ce23968ffa81aca2beeb8b3fa9f18b9eebc003acb5`.
Current build and Native module receipts:
`bb92a9f8568579b78896ea2188e3e4d4d184fe41577ce3427b6f6354a69004fb`,
`a251342c321de77be7e97f269906067dab4eb12468993fc05fb5b3a36ffae9cb`.

After SOURCE and applicable grouped PASS, Root ran the prepared bounded CAS
harness once, using current production CLI SHA256
`b7691fda85ad79fbdb0f1b06e1292a49e7ba3cde2ca517b6e396380eda8054c4`.
Manager local route was unavailable (URLError); private configured CAS supplies
the live catalog's deepseek-v4.1-flash / openai-completions. Retry/model fallback
disabled, two Runs with 6/2 max calls, 120 s/Run and 2048 output tokens.

The actual task passes **6.602 s / 4 calls / 4 tools**: read CSV/summary,
write summary, read it back. Root independently checks the artifact and answer:
3×2.75 + 2×11.5 + 4×4.25 = **48.25**. Answer-free, tool-disabled restart passes
**1.755 s / 1 call / 0 tools**, recalling original quantities 3/4 and book
price 11.5. Both terminal stop answers and the single 11-message journal bind
Session `01a11b5a-5d77-73d9-8c6f-4a7f83b72bdf`. Source/binary remained unchanged.
CAS receipt `real-cas-receipts/20261008T115105Z/receipt.json` in execution-v5,
SHA256 `4d925e40ed68f10b4c7f09e603d69d6dcbb7c7dd6cc8e44dc6f212773a0234d6`.
Final independent same-batch POST passes. Codex `/root/finish_edit_renderer`
independently checks original v5 compiler output, Native nine-mode JSON and
closed admission, unchanged tools executable/source reuse, actual CAS writing
update before tool end, four successful tool results, artifact/readback/answer,
same-Session answer-free recall and 11-message journal. All five scoped
snapshot/build/CAS/current pins match; CAS before/after has zero source delta;
all five current physical build artifacts match their recorded hashes.
Report `write-native-progress-review-20261008/REPORT.md` SHA256:
`5a830d67e235465f0fdb51d4a3cf047f055bb0bc7e97179f7025cd9036eab468`.
Both original groups have applicable PASS evidence; tools is explicitly reused,
not reclassified as a new execution. All earlier failures remain separate.

After SOURCE approval and deterministic PASS, the prepared current-binary CAS
task uses `read,write` for an actual random CSV summary write/readback, then
tool-disabled answer-free same-Session recall. Exact `deepseek-v4.1-flash`,
OpenAI-completions, current private route/catalog verification, 6/2 call bounds,
120 s/Run, 2048 output tokens, retry/model fallback disabled. Harness SHA256
`2a1f08e64e06b782358b686b14b61e648571f78860543bbcae259c1629abeb70`.
It was executed once on the final execution-v5 binary as recorded above.
Final same-batch POST also passes, within the recorded bounded scope.

## Remaining requirements and historical stop

Complete WRITE, legacy Boundary's no-update entry, plan/bridge/LSP/SSH/archive/
SQLite/conflict/streaming and full special URI timing remain OPEN. Arbitrary
returned Graph Promise, Native pending-oneshot close and a newly instrumented
post-effect cancellation trial are precise remaining execution gaps. Direct
dropped-reply and Native released-admission fixtures do not replace those gaps.

Original `write-native-progress-execution-20261008` records a Root orchestration
error: validation failed before applying source, yet old-source Cargo started.
It was stopped after **37.363 s**, zero groups, and cannot be reused as candidate
evidence. All original logs and `baseline-run-cancelled.json` remain. The stop
also mistakenly closed GameViewer/WebView processes through parent PID reuse;
the user was informed. Candidate v2 and its execution are separate.
