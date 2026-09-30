# RPC-01: fixed RPC transport

Status: bounded transport accepted on `69de6716fa8ff2f09112f105acdbbdad7172ec0e`
by the final independent audit below. Full CA-RPC/A5 remains open.

## Requirement and scope

Source: OMP `596f2da7101178214aa27a753529d15e6b7ad91d` (v18.1.8).

| Fixed source | Rust owner | Observable behavior |
| --- | --- | --- |
| `packages/coding-agent/src/modes/rpc/rpc-frame.ts` | `ara-rpc::frame` | v1 ceiling/shrink/overflow, terminal prefix snapshots/reset, v2 lazy physical chunks and output decoder |
| `packages/coding-agent/src/modes/rpc/rpc-input.ts`; `packages/utils/src/stream.ts::readLines` | `ara-rpc::input` | LF splitting, replacement UTF-8, JS trim, all JSON values, malformed-line recovery, final nonempty EOF line |
| `packages/coding-agent/src/modes/rpc/rpc-mode.ts:785-811` | `ara-rpc::writer` | One logical frame owns output until physical lines complete, awaited writes apply backpressure |
| ECMAScript JSON value/string semantics required by those sources | `ara-rpc::json` | UTF-16 surrogate preservation, f64 parsing/formatting, numeric-index key ordering, duplicate-key replacement, immutable deep snapshots |

Preserve: print/REPL, HostSink, Agent/free loop, Session, providers and product
state. Change: only the new host-neutral transport crate, its tests/oracles and
verification/documentation. Defer: production `--rpc`, command dispatch,
Agent/Session binding, queue leases/durability, side channels, live-model RPC
tasks and product integration. PKG-WIRE is a separate collaboration protocol.

Upstream behavior IDs for the frame/malformed-input test files:
`B-49d6ce0c84`, `B-0d628ae8da`, `B-5df32e15d3`, `B-24f76d4319`,
`B-b3efd1d2a1`, `B-5da0b79952`, `B-ac4f81d831`, `B-ce50baef91`,
`B-08ce55515a`, `B-81892e609f`, `B-5f0174e2bf`, `B-20cbe033f0`,
`B-9700cff33f`, `B-fd5f115daf`, `B-6726bd8687`, `B-6102d769a9`,
`B-916d59a3ac`. The original tests execute unchanged; Rust comparisons and
source-scenario tests exercise their JSON transport semantics. JavaScript
accessors/undefined are host-language values: Rust transport accepts inert
`WireValue` trees, with omitted fields represented by absent properties.

## Limits and explicit differences

- Output physical ceiling: 1 MiB including newline. Logical v2 ceiling:
  64 MiB excluding newline. Chunk payload: 256 KiB.
- These are output limits. Fixed stdin and host queues have no such cap; this
  crate adds no inbound limit and makes no bound on total memory. Lazy chunks
  still retain logical bytes; snapshots, values, reassembly and future host
  backlog also occupy memory.
- The decoder reassembles server output. Server stdin parses direct ordinary
  JSON lines, including values which later command dispatch may reject.
- Rust JSON syntax diagnostics retain concrete reason/offset and the input
  `Failed to parse command: ` prefix. They differ from Bun's engine-specific
  `SyntaxError.message`, also for completed malformed output JSON. Recovery,
  error priority and pending-state effects must match; other codec error
  messages are compared exactly.
- Rust mutable ownership serializes output. Cancellation or write failure
  poisons the writer so a later logical frame cannot follow a partial sequence.
  Completion means AsyncWrite accepted and flushed all bytes, not peer receipt.
- The test-only `pipe_probe` explicitly awaits output flush before exit. Fixed
  `rpc-mode.ts` EOF drains commands/background tasks but does not await its
  stdout queue before `process.exit`. This probe does not establish production
  EOF delivery. Rust owning one stdin reader also does not reproduce Bun's
  singleton lock against other native stdin readers.
- A transport-only slice needs no model call. Later host acceptance requires
  controlled Agent/Session task chains and a bounded actual model task.

## Verification procedure

`scripts/rpc_transport_oracle.py` exports exact Git blobs, source hashes,
unchanged upstream tests and the MIT license, then runs Bun 1.4.0.
`scripts/rpc_transport_oracle.mjs` imports unchanged modules through aliases
which export actual fixed utilities. Its records are reference outcomes,
including expected errors; they are not passing Rust tests by themselves.

`crates/ara-rpc/tests/oracle.rs` recreates each recorded recipe and compares
physical bytes via length/SHA-256/prefix/suffix, logical values, errors and
state. `ARA_RPC_ORACLE` selects a freshly generated oracle; the committed
fixture supports offline full-workspace testing.

`scripts/rpc_transport_process.py --probe <built pipe_probe> --output <dir>`
exercises production input/writer functions through real child OS pipes. It
checks fragmented input, malformed-line recovery, Unicode/CRLF trim, EOF tail,
live v2 chunks before stdin EOF, paused stdout/backpressure, ordered resume and
bounded broken-pipe failure. Receipts bind executable/runner and line hashes.
Sampled OS memory is an observation, not a bound.

## Independent reviews and fixes so far

Three independent Codex design viewpoints (`ctx_point_audit`,
`oracle_diff_review`, `resource_scan_plan`) proposed transport before host and
cross-reviewed the ownership/resource boundaries. No Claude was invoked.

`resource_scan_plan` independently reproduced the unchanged 17 upstream tests,
the original 31 oracle records, JSON tests and the initial Rust comparator.
It required more shrink/snapshot/decoder-state coverage. Production review
found cancelled partial writes could accept a following frame and cancelled
input reads could discard a buffered prefix; both were repaired with
deterministic regression tests. The UTF-8 codec error wording was corrected
against the executable oracle. No-op compaction now borrows the original
frame and moves serialized bytes instead of cloning large nonterminal values.

## Frozen transport snapshot and Windows receipts

The 74-case fixed-source oracle was generated by the final scripts in
`C:\Temp\ara-rpc-transport-oracle\run-20260930T020342Z-83a6e5c3`:

- Unchanged Bun tests: 17 passed, 0 failed, exit 0.
- Oracle: 74 reference records; both original-test and oracle logs, source
  Git blobs/SHA-256, launcher/runner/Bun identities and exit codes are retained
  in `summary.json` and `source-manifest.json`.
- Oracle JSON SHA-256:
  `c4ba38776c6008e2522d6ae8c0f8fdf83cc2a2eb196680f99610a9c22179a9fd`.
- Source manifest SHA-256:
  `dcd7f8ef41e2c4854def861a8c03e5408d00498d9ea3b076be55ef528a93fddf`.
- Both are copied verbatim to `crates/ara-rpc/tests/fixtures/` for offline
  comparisons. Runtime manifests record environment identities, so Linux
  manifest hashes may differ while source blobs and compared case bytes agree.
  Scoped `.gitattributes` rules preserve receipt bytes, including the manifest's
  actual CRLF endings; carriage return is treated as its recorded line ending.
- Rust comparator: all 74 records matched in 24.37 s; `oracle_diff_review`
  independently reran it successfully in 24.23 s. Error class correspondence
  maps Bun's fatal UTF-8 TypeError to the exact Rust codec diagnostic; the
  Rust public error remains `RpcError`.
- JSON 9, input/writer 9 and frame scenario 4 tests passed; strict crate
  all-targets Clippy passed. The full workspace gate is recorded separately.
- Final Windows process run: 4 passed, 0 failed, exit 0 in
  `C:\Temp\ara-rpc-transport-validation\process-final\results.json`.
  Broken pipe now requires and retains the `BrokenPipe` diagnostic, not merely
  a nonzero exit. Both streamed and paused output were observed while the
  child was live; exact chunks and following echo arrived in order.
- Dependency audit on the delivered lockfile: `cargo deny check` passed
  advisories, bans, licenses and sources. Existing duplicate-version and
  upstream missing-license-field warnings remain visible. The only new
  external package is `ryu-js` 1.0.3, `Apache-2.0 OR BSL-1.0`; no policy change.
- Python compile, bootstrap links/marker and fixed inventory checks passed.

### Full Windows backend gate

`python scripts/verify_backend.py` on the frozen implementation passed fmt,
strict workspace all-targets/all-features Clippy, all-targets tests and doc
tests: **1,093 passed, 0 failed, 1 ignored, 85 suites**, e2e **87/87**, exit 0.
Log: `C:\Temp\ara-rpc-transport-validation\backend-windows.log`, SHA-256
`33b4728dfee288f5465efc694dd869f3cd5507c904f1fb28c8ba6e0baeef8680`.

The first full run failed the existing
`repl_resume_after_interrupted_tool_reports_unknown_effect_without_replay`
warning assertion (86/87 e2e). Its isolated unchanged test and the complete
unchanged rerun passed. The first failure is retained in
`backend-windows-first-failure.log`; its cause is not established, and no
REPL/Session code or test was altered for this point.

Dependency log SHA-256:
`e813cf701d30dd92e5b4e38ed979bd8802d8a30f9e9a513bcfae82f1d7ee6c36`.
Final process receipt SHA-256:
`344fcc2e7111add9e1a83f6be45a553f189708961f6554afe720b31ebbc7c766`.

## Delivered commit and Linux verification

Implementation commit: `69de6716fa8ff2f09112f105acdbbdad7172ec0e`.
Reviewed tree: `f587fbfccaf06615b429d6b4765e198009c56b72`.
All 28 scoped committed paths match the independently reviewed staged blobs;
no production code changed after Windows verification or Linux execution.

| Exact-head workflow | Run | Result |
| --- | --- | --- |
| Repository checks | [36659354675](https://github.com/ZZray/ara/actions/runs/36659354675) | fmt, strict Clippy, all-targets and docs: 1,096 passed, 0 failed, 1 ignored, 85 suites; CLI e2e 90/90 |
| Fixed RPC transport | [36659354678](https://github.com/ZZray/ara/actions/runs/36659354678) | 17 unchanged Bun tests, 74-case Rust comparison, all 23 crate test functions, live process 4/4 |
| CTX-01e invocation regression | [36659354633](https://github.com/ZZray/ara/actions/runs/36659354633) | success |
| CTX-01d directory faults regression | [36659354635](https://github.com/ZZray/ara/actions/runs/36659354635) | success |
| CTX-01d directory oracle regression | [36659354628](https://github.com/ZZray/ara/actions/runs/36659354628) | success |

Artifacts and extracted logs are retained under
`C:\Temp\ara-rpc-transport-validation\github`:

- Transport artifact ZIP SHA-256:
  `8a92949218126687c6a54db1b977dc0893d6c49bfb5d79309bebf6a995d67e13`.
- Linux process receipt SHA-256:
  `3fcf79a1a02269ed76bb2e40a27468db5ecaaca7da469a14121ef1e1e16dc999`.
- Full Linux backend step log SHA-256:
  `eade65634a4e5c6b90e74f782093f119a9f1d0f2d835a5c3cea34b8f6db6ab2e`.
- Full log ZIP SHA-256:
  `59fcd670255a443b1d701e916a7ea69c16ad8372d4408d55c66d0035bcd98147`.

The Linux process observed five chunks before stdin EOF and seventeen chunks
under paused stdout/backpressure, followed by the exact echo in order.
Broken output reported `BrokenPipe`, exit 1, with its stderr hash retained.
The executable hash is recorded by the runtime; the binary itself is not
retained in the artifact. The exact-head workflow builds and runs the same
recorded executable path.

## Final independent point audit, 2026-09-30

Reviewer: independent Codex `resource_scan_plan`, using `ara-git-review` and
the point-delivery boundary. Precommit scope covered all 28 paths, none
skipped, and independently reran fresh upstream/oracle, all crate targets,
strict Clippy and Windows process cases. Scope receipt:
`C:\Temp\ara-rpc-transport-final-review\scope.json`.

After push, the reviewer audited all 28 Linux artifact files and their ZIP
bytes, all 13 unchanged source/license exports against exact OMP Git blobs,
runner/launcher/process-script hashes against the delivered Git objects,
and every Linux physical line plus aggregate stdout length/hash in all four
process cases. Linux recipes/outcomes/limits equal the committed 74-case
fixture; only environment-linked manifest hashes differ. Exact checkout
revision and the whole Linux gate log/counts were independently checked.
The reviewed public upstream and synthetic artifacts had no credential
signature finding.

Final audit receipt:
`C:\Temp\ara-rpc-transport-final-review\linux-transport-audit.json`, SHA-256
`2ed4cb45b619861ce36f763fcabf9f396321b0650ebb6e295afac0af035ecd1b`.
Verdict: **RPC-01 bounded JSON transport accepted on `69de671`**, with no
remaining actionable finding. Object-valued `__proto__` shrink is covered by
source reasoning and the implementation's setter-equivalent omission, not a
separate executable differential case; that optional case is a non-blocking
coverage gap. The explicit diagnostics and runtime differences above remain.

This accepts no production RPC command host, Agent/Session binding, complete
CA-RPC/A5/P2, PKG-WIRE or full OMP marker. `ported_through_commit` remains null.
Next: independently plan production host ownership and command contracts,
then execute controlled Core/Session task chains and a bounded real RPC task.
