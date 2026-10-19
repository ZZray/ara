# HandWave Node 0.2.8 publication

2026-10-19. The user explicitly authorized commit, push and deployment of the
small reviewed Run-efficiency increment. No further feature was added. Full
ARA/OMP parity and autonomous new-Session model reuse remain separate and open.

## Sources and packages

- Rust implementation/version: `abf45378876f37a7e6a307f0987e0131702c1f44`, pushed
  to `ZZray/ara` `origin/dev`, confirmed by the remote ref. Exactly 17 paths.
- Ray installation publication: `c861050e66d4596d488edf184dacc1cec813242c`, pushed
  to its existing `origin/dev/rust`, confirmed by the remote ref. Exactly four
  paths: source-install code/test and two new immutable archives.
- Both final artifacts were built from their exact commit exports. The Ray
  export restores only the existing verified stable Web archive; all 788 Go
  source files and module files match the tested candidate. Its resulting
  binary is identical to that candidate. All 37 unrelated dirty Go files and
  all old 24 embedded public artifacts remain byte-identical. Root Rust/OMP WIP
  was excluded from every commit and build.

| Artifact | Bytes | SHA256 |
| --- | ---: | --- |
| Windows static-CRT standalone EXE | 5,355,520 | `2abf8e84ee4107219e798dcaa420a28fe357fef33945a8992c3f866dbdb53890` |
| [Windows ZIP](https://118.25.103.226/ray/releases/handwave-node-windows-2abf8e84.zip) | 3,742,919 | `5fcafb057e04245134b0cfe71b5957fafa1509a7eda24aa0e03e6b1b4b952e6c` |
| [Source archive](https://118.25.103.226/ray/releases/handwave-node-source-69d9cc3f2be22dfd.tar.gz) | 585,030 | `69d9cc3f2be22dfdf6c00a6e91e0e74f4bd64f2784d4d0ab0acbda6d4e90fcbe` |
| Running Linux Ray | 60,231,842 | `01efbb9dff4942182f1c7aaa583a7d0a8ab0892d195e75b0d87aa2fe7cc94166` |

The EXE is 5,632 bytes above published 0.2.7. No dependency was added.
`/ray/install.sh` pins the new source URL/SHA; the fixed installer script remains
`fe4b10bee6a73659f6c5db78e624aa794bec0e0b57f91619d8be13c4f694094c`.
All 654 source members and 560 ZIP members match the exact export or the pinned
EXE/checksum manifest. Compile-time Markdown, licenses and the committed launcher
are present; live configuration, identity, credentials and state are excluded.
All 26 old/new public artifact URLs were fetched and verified after deployment;
a separate Windows client verified new packages/default pin with normal TLS.

## Executed acceptance and deliberate evidence reuse

Evidence root: `D:/WorkSpace/tmp/handwave-deploy-028-20261019/`.
The versioned [publication receipt](receipts/stage-publication-20261019/publication.json)
and [independent Codex audit](receipts/stage-publication-20261019/review.md)
record scoped acceptance without publishing private raw state or configuration.

- Windows 0.2.8: fresh 203 tests PASS, zero failures, one explicitly ignored
  public-network test; fresh clippy PASS. Exact-export release build PASS.
  Final EXE actual Node/Hub environment Host 5/5 (36.9s) and existing Host
  regression 27/27 PASS, including artifact compilation, restart, cancellation
  and malformed stream paths.
- The earlier local candidate's context 15, Web 5 and modes 5 groups are
  **historical, source-bridged evidence**, not 25 fresh tests of this new EXE.
  `version-source-bridge.json` confirms all 40 runtime build files and 37 Python
  fixtures byte-identical; only Cargo package version metadata changed in two
  files. Those earlier checks and the original bounded model result remain in
  [the implementation evidence](run-efficiency-20261019.md).
- GNU Linux x86_64: actual new source archive built offline under existing
  non-root verification tooling; knowledge 14, skills 12 and budget 2 tests
  passed. Actual Linux Node + signed Hub + controlled provider Host 5/5 passed
  with 32 requests and no fixture errors. A fresh source directory reused the
  existing validated dependency target; no dependency-cache copy or installation
  was performed. Debug EXE: 184,983,344 bytes, SHA256
  `91da2cdcc0f35cb4732223312a0a32fff80e8c536a6097f08b08ff7f0ff9f5df`.
  This is source-build/Host evidence, not a small Linux release/LTO binary,
  all-platform test result or complete online system-service installation.
- Ray package tests, vet, stripped Linux build, avatar DOM and complete-script
  parsing passed. Stable JS bytes equal the previous published build; no
  unrelated authentication/readability/UI work was shipped.

No new paid-model trial, production Goal or browser visual trial ran for this
publication. The previous autonomous trial remains FAIL/reuse NOT_RUN, despite
its separately reviewed successful first-Session script/project-note effects.
No causal speed/token/cache saving is claimed. Root-wide documentation bootstrap
remains uncompleted as recorded in the implementation evidence; scoped release
links and diff checks are validated separately.

## Deployed state and retained installation failure

Only Ray restarted in the cloud. PID1291971 runs exactly the final Ray SHA above.
Fresh postchecks confirm all 59 authentication/configuration/history/lease files
and every other service/Arena process unchanged. Login/application/asset gates,
old downloads and existing QR identity remain. The forward and rollback helpers
wait for expected process-image readiness; no rollback was required this time.

The Windows installed Node now runs PID42836 with the exact new EXE. Device
fingerprint is unchanged; full inventory and all 126 original state/launcher
file hashes are unchanged at switch and startup, excluding the runtime lock.
The existing launcher still uses 12 calls/120 seconds. It connects to the public
HTTPS/WSS endpoint and is **waiting for normal controller approval**. No grant
was approved or renewed, and no saved task was replayed.

The first local preflight identity probe was refused by the running Node's
exclusive state lock. It failed before backup, stop or switch. Fresh inspection
confirmed the original PID/SHA, unchanged files and absence of partial stage
effects. The corrected order checks complete inventory/hash, stops the verified
quiescent process, obtains the old fingerprint after lock release, then stages
the new image and checks identity/state again. The original refusal and
reconciliation receipt are retained; this is not described as a failure-free
first attempt. User state was never restored from backup.

Rollback executables, private state snapshots and raw receipts remain outside Git. Before
rollback compare the current image to this release pin, then restore only the
intended executable/default package pin. Never restore whole user-state,
knowledge or authorization directories or extend an expiry as part of recovery.
The machine-specific backup locations are maintained in the local knowledge base.
