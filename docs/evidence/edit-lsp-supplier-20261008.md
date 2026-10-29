# LSP supplier and concentrated Windows ownership repair — 2026-10-08

## Scope and identity

Fixed OMP `596f2da7101178214aa27a753529d15e6b7ad91d`; dirty ARA `dev`
HEAD `20aa0e2ef9872f633ba2671376cdf778477e7115`. Index SHA256 remains
`89d34e26be97c90a14680d44ace937e6b1ae5a86e8ee7b46dd6b90718a922475`.
Root owns live application, formatting, Cargo and process/model execution.
Independent author/reviewer candidates remain on D. No commit or push.

The 21-path LSP supplier batch supplies actual private/shared processes,
settings/factory, separate WRITE options and Session diagnostic ownership.
Source preparation took 3261.054 s. Initial build failed after 508.523 s;
a nested Native Result and conservative presence enumeration were repaired.
Execution-v2 built successfully in 616.992 s and ended FAIL13/15 after
833.612 s total, with unchanged source. Original failures remain preserved.

## Failure evidence and minimal repair

- `retained-repl` timed out after 180.056 s. An unchanged-binary diagnostic
  was bounded to 45 s (45.929 s including termination). Actual processes and
  frames showed the holder exited, both LS instances had received didClose,
  and the independent unused presence remained. The collector was waiting
  for EOF. Exact Rust 1.97.1 Windows process source (`8bab26f4f`, SHA256
  `d17a8a0628a2b537248e87c7b6b249ece77458510f4e4d09e605d86ede7f2289`)
  shows broad inherited-handle spawning and explicit stdio duplication.
  Main now clears only the inherit bit of original standard PIPE handles
  once, before child creation. It neither closes them nor restores flags;
  explicit `Stdio::inherit` still creates its own inheritable duplicates.
- `journal-failure` failed after 2.863 s in the new Native observation.
  Fixed OMP write.ts:1353-1361 and Rust write.rs:632-645 place diagnostics in
  details, while content contains the write acknowledgement. The observation
  now requires the actual Native `details.diagnostics.messages` sentinel and
  equality with the metadata carrier. Effect/file/child assertions remain.
- Other changes: one required Windows API feature and stage timing in the
  existing process group. All original assertions and collector joins remain.

## Delivered v3 and original selected groups

Packet: `D:/workspace/env/cache/ara-port-candidates/edit-lsp-supplier-execution-v3-20261008`.
Delivered manifest SHA256:
`cc8a2d7cfe4b01bc64e3da0f25aba6d8534bb4d0439ea5cd85ffba798e724cac`.
Four changed paths, 17 unchanged; all 21 paths and 13 direct dependencies
checked. Scoped fmt PASS0.612 s.

Executed existing `scripts/verify_edit_module.py --groups retained-repl
journal-failure new-session clear-compact` with that delivered manifest.
Original runner, features, profile, D target and jobs=2 retained.

| Step | Result | Seconds |
| --- | --- | ---: |
| Selected Cargo build | PASS, exit0 | 163.539 |
| retained-repl | PASS1/1 | 9.533 |
| journal-failure | PASS1/1 | 22.231 |
| new-session | PASS1/1 | 0.296 |
| clear-compact | PASS1/1 | 0.538 |
| Module total including build | PASS4/4, source unchanged | 199.968 |

The holder exit and both pipe EOF stages occur at 1.063 s; reuse, cancellation,
bidirectional IDs and cached initialization then pass. Final unused presence
exit/EOF occurs at 5.751 s and LS subtree disappearance at 8.765 s. Native
output contains the real fixture diagnostic in both details carriers. The
controlled child group also exercises batch/fresh bytes, late/stale results,
cancellation, pull error, child exit, bounded initialize and graceful shutdown.
The remaining 11 v2 green groups retain their original source/binary pins;
they are not relabeled as observations of the v3 binary.

## Actual gopls and CAS task

Existing gopls v0.23.0 readiness PASS5.699 s was reused. Actual ARA task uses
the physical `D:/WorkSpace` directory, Manager-configured CAS,
`deepseek-v4.1-flash`, `openai-completions`, retry disabled and bounds of
6/2 calls, 120 s/run and 2048 output tokens/call.

Actual task PASS14.658 s, 5 model calls and 4 tools: write invalid Go, read,
receive real compiler diagnostic, edit to integer45, read back. Artifact is
`var count int = 45; println(count + 3)` and gofmt diff is clean. Raw Host
`lsp_late_diagnostics` event194-195 carries compiler IncompatibleAssign before
edit event330; assistant prose is not used as diagnostic/timing evidence.
Second process PASS2.740 s, 1 call and no tools, correctly recalls45/3 in
the same Session `01a11c24-7320-7796-9fe7-b1a3ac8c32fc`.

The same task injects a task-owned non-directory presence path. Worker46004
survives unknown enumeration for4.141 s; removing the fault restores absence
and reaping by7.461 s total. The fault is removed and cleanup is registered
for Python failure. Credential configuration remains private outside the repo.
Per-call usage is preserved; total reported tokens24422, cost not asserted.

Receipt: packet `real-cas-receipts/20261008T153149Z/receipt.json`, SHA256
`574b7e2df21afa32d7c21a6fd5e9e17839bc831b7bb96f224eb6b7a9f7691ed6`.
Production binary SHA256:
`aa2a709b9e4e89860f9c5d15ff37d90cd2b992d1fc37cfe5aeb521d294c86a85`.
Procedure SHA256:
`b95e2f01b445bb26be86598c2afc6843096d75526ff6f680cc1939ff7c9725a5`.
Root inspected raw events, disk artifact, clean formatting, Session continuity,
usage, fault cleanup and source/binary identity.

## Independent review and remaining obligations

Codex `/root/finish_edit_renderer` used ara-git-review and ara-rust-core-review.
SOURCE-V3 bounded APPROVE SHA256
`68dcc1ebf88ba506915280186e0e568adf088a311c55bc5c0f862424b7720334`.
Final POST is bounded APPROVE, SHA256
`ab3b6ac674616d45d337db1c1855d797ae75658ba41576422427c655a4f69524`;
report at the repair candidate packet `POST-V3-FINAL.md`. It independently
checks delivered build/group artifacts, actual EOF stages, Native carriers,
raw real-model diagnostic ordering, artifact, same-Session recall and owned
presence-fault cleanup. This record does not accept a complete OMP parent.

Full LSP/edit/write, shared-state replay gaps, contention/replacement/long
linger, generic launch/profile, Settings/plugin, ACP/plan/vault/extension and
Host-authorized WorkspaceEdit remain owning obligations. WorkspaceEdit is
explicitly unsupported. Unix runtime was not exercised. Workspace/clippy/deny
release gates were not rerun for this concentrated repair. Formal total remains
0/111 complete; the next independent candidate is shared Fs scan cache.
