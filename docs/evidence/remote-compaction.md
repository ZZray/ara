# Remote compaction — bounded WIP

Base `2c63205939729d8e92d563f82eacd590327301d9`, branch `dev`; fixed OMP
`596f2da7101178214aa27a753529d15e6b7ad91d`. This checkpoint ports selected
native V1/V2 and generic remote contracts, checked same-Session publication
and RPC/REPL selection. Full remote/Core/Host/phase acceptance remains open.

## Source and implementation mapping

| Fixed source under `packages/` | Rust owner and exercised scope |
| --- | --- |
| `agent/src/compaction/openai.ts` | `ara-ai::remote_compaction`: native full historical input, configured endpoint/model, V1 JSON, generic tool-free JSON, usage/error receipts, checked replacement and trailing-output trimming |
| `agent/src/compaction/compaction-v2-streaming.ts` | `remote_compaction_v2`: SSE terminal validation, exactly one compaction item, bounded transient retries, retained real User messages, approximate UTF-16 budget and V2 preserve data |
| `agent/src/compaction/compaction.ts` | `ara-agent::remote` and `ara-cli::remote_compaction`: V2 then V1, earlier non-auth error classification, cancellation without fallback, generic existing summary pipeline plus independent short summary |
| `coding-agent/src/config/{model-patch,model-registry}.ts` | `daily_model_config`: fieldwise remote settings merge, modelOverrides > authored model > bundled model > provider; generic wire model override retains original route authentication |
| `coding-agent/src/session/session-maintenance.ts` | `ara-session::remote`, RPC maintenance and REPL: strict source/context/route checks, phase-aware atomic publication, same-Session replay, readable route fallback, threshold and incomplete recovery |
| `agent/test/remote-compaction.test.ts` | Four wire and four Session families plus three preparation families reuse the fixed input classes; actual Host runner separately exercises the real process chain |

The short-summary prompt is copied from the fixed upstream. Attribution is
retained by module source notices and `THIRD_PARTY_NOTICES.md`. The upstream
receipt manifest hashes ten exact extracted files. Neither the lock nor the
full parity marker advances.

## Ownership and observable contracts

Native compaction consumes the complete raw history available to its route.
Its durable compaction entry stores provider replay through a separate
`providerReplayThroughEntryId`; prefix `sourceEntryIds` continue to describe
the structural cut. Repeat compaction sends the checked previous replacement
plus only the subsequent raw tail, without duplicating previously consumed
recent messages. Original journal fields, images and tool receipts remain raw
source evidence. Local prune/shake cannot rewrite native-covered originals or
claim those originals as newly saved model tokens.

The checked native carrier is model-only history, never an invented Assistant
receipt. An empty Stop carrier with the exact active route, checked native
payload, compaction item and no pending tool calls may continue without a
new User message. Ordinary completed Assistant messages and unresolved tool
effects retain the existing continuation guard. Structural cuts recognize an
earlier native summary while only readable text summaries are inserted as
ordinary User text.

Disabled, foreign or image-incompatible native replay finds an earlier readable
summary or expands real raw source history. Healthy lazy/empty Sessions allow
ordinary first-prompt adoption; the empty guard still validates the strict raw
journal and rejects malformed/invalid/duplicate history rather than silently
starting a new conversation. The existing grouped preparation family covers
healthy-empty versus damaged-empty and checks that damaged bytes stay unchanged.
Local reducers use the same strict empty/lazy preflight and return no native
protection boundary when no durable carrier exists. This preserves their
ordinary fresh-Session no-op behavior without bypassing journal validation.

Generic remote uses the existing readable summary serializer and window/split
pipeline. It then performs an independent short-summary call, capped at
`min(512, floor(0.2 * reserveTokens))`. Kept-tail serialization and short-output
budget are checked before a billable history call. Generic output stores
`method=remote`, short summary and native file details, without opaque preserve
data. RPC/public attempt artifacts expose only timing, usage, status and failure
presence, excluding arbitrary upstream error text and credentials.

Hosts own private authentication and persistence. The native wire worker is
awaited through cancellation and request settlement; its child token cannot
cancel the parent on successful completion. RPC checks Session, route/model,
generation, messages, raw snapshot and stop before commit. Unpublished incomplete
recovery restores its owned failed Assistant. Publication errors fail-stop;
cancel never authorizes fallback or another primary request.

## Reproduction and execution receipts

Receipts: `C:\Temp\ara-remote-batch`; deterministic module receipts use the
platform temporary directory's `ara-remote-batch` sibling. Private configuration
and the bounded live runner stay outside Git.

```text
$env:CARGO_BUILD_JOBS='4'
$env:RUST_TEST_THREADS='1'
python -X utf8 scripts/verify_remote.py --module all --full
python -X utf8 scripts/verify_remote_host.py --binary E:/repos/ara-github/target/debug/ara.exe
```

Select `wire|session|prepare|config|route|host|repl|account-wire|account-cli` for
module checks. `--full` shares one workspace fmt/Clippy/all-target/doc gate,
inventory, dependency policy and binary build. Ordinary configuration/prompt
details receive source review, without additional micro-tests.

Final runtime snapshot: 34 scoped source/test/runner paths in `manifest.json`,
SHA-256 `95b64612d95723cc9691c07f210954563e789804cce72b6b132952425a0b7bce`.
Final binary SHA-256
`e1fde352062fca4a2e077ab2b6fd9ae531a197ee5ce5718df78412e81af44d59`.

The nine module families pass **150/0/0** on `module-20261002T111811Z`,
78.932s execution + 11.696s compilation under the serial environment. After
two stale Session unknown-method test inputs are corrected, unchanged module
implementation/inputs are reused. `completion-20261002T113007Z/receipt.json`
records the **composite** backend gate: **1,578/0/20**, current fmt/Clippy/doc,
inventory/deny/build pass, source unchanged during completion. It reuses 87
fully passed targets (1,358/0/20) from the retained failed full invocation,
reruns the entire Session plus unexecuted testkit/tools/walk targets (220/0/0),
and excludes all duplicated partial Session counts. Completion takes 36.332s;
the preceding full backend attempt takes 274.748s and remains FAIL. This is
not a successful single full run.

Final `host/run-20261002T113121Z/receipt.json` passes **5/5** families in
12.720s on the final binary: native full/repeat/reopen; disabled/foreign
readable fallback; generic two calls; threshold/incomplete continuation;
Abort/EOF. Four cancellations settle in 3–16ms, without publication, fallback
or extra primary request. Source and binary remain unchanged throughout.

### Real task

`live/20261002T110230.980375Z/receipt.json`: PASS, 18.106s, three processes,
six actual CAS calls: generic summary and short summary two, write/read/final
three, tool-disabled same-Session reopen one. Bounds are eight actual calls,
three processes, 90 seconds per process and 2048 output tokens per call. No
automatic task replay occurs.

The preferred Manager gateway catalogue probe is unavailable. Its private
Manager-configured CAS route's live catalogue confirms `deepseek-v4.1-flash`
using OpenAI Completions. The real credential exists only in the relay; the
Rust child receives a loopback dummy credential. Request outcomes are settled,
with no active request or relay failure at stop.

Only the imported settled raw read receipt contains the invoice facts; the
original source file does not exist and the later user task supplies no facts.
Actual tools execute exactly one `write totals.json`, then one `read totals.json`
with successful receipts. The file is
`{"adapter":87.5,"case":33.0,"grand_total":120.5,"shipment_label":"Cedar-6814"}`.
All five original raw entries remain exact; a new leading native title slot is added.
The no-tools original-Session reopen correctly recalls all four values, uses
no tools and leaves the JSON unchanged. Actual upstream JSON/SSE usage totals
32,014 prompt + 1,226 completion = 33,240 tokens across six calls; absent buckets
remain unknown. This trial proves generic remote, not native/account Internet.

The live trial's binary is explicitly earlier SHA-256
`5620b9f8d4aacf751dc6146b5139277f441d77e85ca42e4501173e45ffde1f8d`.
Its directly pinned 26 scoped source paths are unchanged except the later
strict empty/lazy native reducer guard. Removing that unique four-line guard
reproduces the prior file hash exactly; the persisted, nonempty live Sessions
cannot take its new branch. Two later test-only unknown-method input updates
do not affect model/tool execution. Root and independent review therefore
reuse this settled task under explicit source equivalence; no Internet trial
is claimed on the final binary. Final guard behavior is exercised by the
original reducer families and the composite/final Host checks.

### Failed and interrupted receipts retained

- `module-20261002T103231Z`: native picture preparation failed and source changed
  during compilation; backend did not start. Text-only cut/commit restrictions
  were corrected for checked native replay, with text-summary rules preserved.
- `host/run-20261002T104036Z`: compactable-prefix expectation was invalid for
  keep=1000 and a very short retained turn; zero HTTP calls. The input now has
  a real retained User boundary above that budget; assertions are unchanged.
- `host/run-20261002T104458Z`, `104653Z`, `104802Z`: Windows native main-thread
  stack overflow before HTTP. Stage observations reach history/trim/body, with
  zero requests. Moving the owned native HTTP poll chain to an awaited worker
  passes the same actual Host families; no main-thread stack limit is increased.
- `host/run-20261002T104956Z`: three families pass; native incomplete publication
  then ordinary Assistant-role continuation refuses the model-only carrier.
  Checked carrier continuation and iterative structural-summary handling close
  this path in the existing recovery family.
- `module-20261002T105247Z`: REPL 93 pass/1 fail on a Windows main-thread stack
  overflow. Heap-pinning the top-level `run` future preserves the ordinary
  workflow and the original 94 REPL tests pass.
- `module-20261002T105411Z`: account workflow reaches the strict fresh-journal
  preflight and fails. Fresh/clear adoption and damaged-empty separation are
  corrected. Its existing scripted account fixture explicitly chooses soft
  summaries because it supplies no native remote endpoint; original wire,
  tool, Session, error and usage assertions remain intact.
- `module-20261002T105723Z`: all 150 module checks pass, backend stops at Clippy's
  large enum variant. Summary storage is boxed, matching the existing message
  variant, with mechanical helper updates. Existing binary fixture/lint issues
  are corrected without adding tests.
- `module-20261002T105947Z`: Root interrupts the next compile after remaining
  known all-target diagnostics become available; no full receipt is claimed.
- `module-20261002T110200Z`: backend linking stops with `LNK1180`, explicitly
  reporting insufficient disk space. Root removes only 560 regenerable PDBs
  in this project's `target/debug/deps`, all predating this batch; the cleanup
  receipt is `generated-pdb-cleanup.json`. Source, executable and task artifacts
  are retained. Subsequent Cargo work uses process-local `CARGO_BUILD_JOBS=4`.
- `module-20261002T110758Z`: all 150 module checks, fmt and Clippy pass;
  all-target compilation succeeds. Workspace execution then stops with
  353 pass / 2 fail in the unchanged `openai_http` fixture. The connection-refused
  fixture unexpectedly receives a successful response; the statusless retry
  fixture receives five requests rather than two. Its closed ephemeral port
  can be reused by concurrent fixtures, supporting a port-reuse hypothesis;
  request-to-port correlation was not captured, so this is not a proven root
  cause. The original whole module subsequently passes 50/0/0 with
  `cargo test -p ara-ai --test openai_http --all-features --locked -- --test-threads=1`
  in 35.18s of test execution. The final shared gate uses process-local
  `RUST_TEST_THREADS=1`, preserving original expectations and recording this
  environment restriction. No provider behavior or fixture assertion changes.
- `module-20261002T111321Z`: all 150 modules pass and the former HTTP failures
  pass under the recorded serial environment. Backend reaches 662 pass / 1 fail /
  1 ignored before the original fresh-Session local-reducer family exposes an
  unconditional durable compaction projection. `native_reduction_boundary`
  receives the same strict raw-journal empty/lazy guard as route preparation,
  returning no protection boundary. The original two binary local-reducer
  families then pass in 0.06s; no assertions or new tests are added. Independent
  review identifies this late correction as `FINAL-REDUCER-R1`.
- `module-20261002T111811Z`: the final runtime source passes all 150 modules,
  fmt/Clippy and 87 complete workspace test targets (1,358/0/20). The invocation
  later stops at 1,395/1/20 because an old Session projection fixture still
  labels `remote` unsupported. The fixture uses `unrecognized` instead, keeping
  its original error and no-write assertions. A first four-crate suffix attempt
  also exposes the same stale input in the handoff family; its unknown-method
  input is updated while all stale/details/archive/provider assertions remain.
  `backend-suffix.log` retains this failed attempt. No runtime source changes
  follow this invocation. Final completion reuses the unchanged, fully passed
  crate prefix and reruns the entire Session plus unexecuted remaining crates,
  current fmt/Clippy/docs/inventory/deny/build. This is explicitly composite
  verification, not a successful single full invocation; duplicate partial
  Session counts are excluded.
- `host/run-20261002T110345Z`: timeout during the concurrent workspace rebuild.
  The first process completes both compact publications and shuts down after
  four controlled calls; the cold-reopen family exceeds its process budget.
  No stack error is observed. The failure stays recorded; the same bounded
  Host command is repeated after compiler load settles, without relaxing its
  expectations or time limit.

## Required later work

Full remote is not accepted. Required open contracts include 401/403-triggered
force refresh (ordinary expiry refresh exists), V2 reasoning High/off/clamp/
mapping policy, Azure, Lite/WS, attestation, custom/computer native items,
bigint/original image detail and the exact OpenAI tokenizer. Trailing-input
UTF-8 sizing is an estimate, not strict token-window proof. The readable generic
serializer still has existing source-count/image/raw-role limits. Full hooks/
transforms, settings inheritance, concurrent manual RPC, speculation/deferred
maintenance, actual V2 Host and overflow Host cases, exact remote publication
durability injection, native/account real tasks and Linux gates remain open.
REPL automatic handoff/snap/shake selection retains its separately recorded gap.

Provider priority remains custom OpenAI first, OpenAI account second; other
provider families are deferred required work. Snapcompact and frame rescue are
next Core modules. P0/V1 accepted; P1–P6 open; RPC 27/42; full marker null.

Independent Codex `/root/remote_review` final POST is
`approve_bounded_wip_checkpoint`, no open blocking findings, recorded in
`final-review.json` (SHA-256
`ee4e995846a51130141d64739f175ab65c3ac8690f2c1e7bd79a9533b17f6c0f`).
Earlier structural-cut, carrier, fresh-Session and `FINAL-REDUCER-R1` findings
are closed on the final frozen sources. The two unknown-method fixture
adaptations preserve their original error/no-write guards. The reviewer reads
source and raw execution receipts; it does not independently execute Cargo,
network or live model calls.

Root `root-audit.json` separately checks all 34 final repository hashes, ten
upstream hashes, final binary/Host/composite gate, actual paired tool receipts,
artifact/no-tools recall and five unchanged original raw entries. Windows
receipt path keys are normalized before comparison: Host directly binds 26
final scoped source/prompt paths; CAS directly binds 26 earlier scoped paths
under the documented runtime equivalence. The other evidence paths are
tests/runners checked against the frozen manifest. This approves the bounded
tested/audited WIP checkpoint, not full remote, a parent surface or a phase.
