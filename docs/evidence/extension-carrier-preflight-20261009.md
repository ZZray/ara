# Actual Extension carrier preflight — 2026-10-09

Fixed OMP `596f2da7101178214aa27a753529d15e6b7ad91d`.
The existing practical/architecture/risk review and module freeze select one
Host-owned Bun 1.4.0 child for arbitrary JS/TS; Agent/Session/Run and authority
remain Rust-owned. This experiment verifies synchronous transport feasibility,
not complete loader, SDK, Core or product delivery. Formal totals stay 0/111.

One D package:
`D:/workspace/env/cache/ara-port-candidates/extension-p0-bun-sab-20261009`.
Author `/root/edit_host_integration` prepared the first executable package in
about eight minutes. Root owns actual execution. The original 45–60 minute
budget covered preparation, execution and one concrete repair; it was not
used to open repeated helper gates.

## Actual source and executable

Original manifest SHA256:
`a6c6eb5617bdfce13cab0689f4af55b295e4c95324696bc2632e8219ad447349`.
First standalone Rust compile PASS **3.312 s**, then run-1 FAIL **0.839 s**:
Windows error 10035 on the first bootstrap receive, before Hello/guest Go.
The nonblocking listener's accepted stream was not restored to blocking mode.
Root changes only `stream.set_nonblocking(false)` before existing I/O timeouts.
Original source, manifest, executable, compilation and FAIL outputs remain.
No matching Bun process remains from that attempt.

Final manifest SHA256:
`e3f29a916686d70e829d26f1a9d4510630e777d4bb84f2fe48987a068d5ed609`.
Standalone Rust compiler: `rustc 1.97.1 (8bab26f4f 2026-07-14)`, edition 2021.
Recompile PASS **0.842 s**; peer-v2 executable SHA256
`607651669904ffe2d8c3c259b4915670f8c98944da2411abc794c07448a040b5`.
Actual run-2 PASS **2.773 s** (command wall **2.880 s**), nine observations.
Receipt SHA256:
`fbceaeaa9d5127c008b9df1c26769b65feb5ef1e1b65099637783c12f9459297`.
Root `root-audit.json` checks unchanged eight source files and ordered frames.
No Cargo, npm, fake Rust peer or Node carrier was used.

```powershell
rustc --edition 2021 .\peer.rs -O -o .\peer-v2.exe
.\peer-v2.exe --bun 'D:\workspace\env\tools\bun-1.4.0\node_modules\bun\bin\bun.exe' --package 'D:\workspace\env\cache\ara-port-candidates\extension-p0-bun-sab-20261009' --artifacts '<fresh directory inside the package>'
```

## Observed process and negative controls

- Actual Bun main thread, transport worker and SAB communicate with a real
  std Rust synchronous peer over a separately framed authenticated loopback.
  Real TS enum and adjacent asset load; one module produces two fresh binds.
- JS getter → Rust → nested JS getter → Rust completes with separate inner
  and outer waiters. Competing Session Go executes after outer return.
- Original thrown Error keeps its handle and JS identity; a different Error
  is the negative control. Promise returns its handle at 385 ms and settles
  under the same owner at 467 ms, after event-loop recovery.
- Forged parent 999999999 is rejected before guest probe; probe count is zero,
  then an admitted probe increments it. A 1,200 ms missing Host reply produces
  typed Unsettled under the original operation; Session object identity survives.
- Normal child PID 68300 exits zero, worker/socket and both pipe readers join.
  A second real child PID 4624 enters a CPU loop after Go; a 500 ms missing
  terminal preserves original operation 13 as Unknown, zero replay, then
  kill/wait and pipe join finish. A process query found no matching Bun child.

## Scope and successor

P0 uses purpose-built fixture functions; it does not execute the fixed 2,897-line
legacy loader, full SDK exports or production ValueRuntime. It does not prove
production admission races, actual unknown file mutations, permissions,
four-domain lifecycle, native addons, product UI or real model behavior.
There is no new repository test target or complete-parent acceptance.

After this result, Root captured the current Vault-approved **1,606-file**
baseline once, index unchanged. Single complete candidate:
`D:/workspace/env/cache/ara-port-candidates/extension-production-batch-20261009`.
Baseline manifest SHA256:
`cde7f296dfafd366442bafda8228539dac64477e1556e210ab62594287403318`.
Three disjoint lanes prepare real loader/SDK, Rust transport/value/invocation,
and Session/permissions/registration. Root reserves shared production wiring
and the original coarse Host family. One concentrated freeze, independent
SOURCE review, affected build, original groups and POST follow whole-module
integration. Existing full parent denominators and milestone gates remain.
