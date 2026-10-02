# Snapcompact/frame rescue handoff — bounded WIP

Base `c6d3ce9a6b996662299fc51b7132ef708dd0760f`, branch `dev`, fixed OMP
`596f2da7101178214aa27a753529d15e6b7ad91d`. Read
[evidence](../evidence/snapcompact.md) and CTX-SNAP-01 in the
[ledger](../upstream/feature-ledger.md) before continuing. This point remains
implementing: dependency and actual image-task gates fail. No full parent,
phase or parity marker acceptance. RPC stays 27/42.

## Reuse verified work

Receipts/source snapshots: `C:\Temp\ara-snap-batch`.

- `final-manifest.json`: 71 scoped repository hashes, 65 fixed source hashes,
  patch and binary. Excludes existing vendor/EOL WIP and `.codebase-memory`.
- `module-20261002T123358Z`: module phase 219/0/0. The whole invocation later
  fails Clippy and remains FAIL; relevant diagnostics are repaired.
- `gate-20261002T124901Z`: fmt/Clippy pass, tests stop at a newly added
  out-of-scope Codex image success assertion. Only that new test branch/comment
  is removed; existing Codex rejection tests and generic image tests stay.
- `completion-20261002T125814Z`: composite Windows tests 1,616/0/20 and
  fmt/Clippy/doc/inventory pass; whole invocation FAIL at cargo deny. Reuses
  the complete unchanged Agent crate (128), runs AI and remaining crates
  (1,488), excludes the previous failed partial AI counts. Source unchanged.
- Separate final CLI build passes in 0.30s; `build-receipt.json`.
- Final actual controlled Host passes all five families:
  `final-host/run-20261002T130705Z/receipt.json`, unchanged source/binary.
  Independent POST `final-review.json` approves saving WIP, with point
  acceptance changes requested. SHA256 `6ce8fe9c8800a20d40e51a910b9e9651a54e57d85896451d037edee8b842fd73`.
- Actual CAS original image task: `live/20261002T124920.027464Z`, three calls,
  20.335s, exact amounts but Rill→Fill label error; strict artifact FAIL.
- Second isolated image task: `live/20261002T125318.174197Z`, four calls,
  49.658s, repeated write, MAPLE→MABE and incorrect amounts; strict task FAIL.
  Both have no active requests/relay errors. No real reopen passes. Root
  inspects correct native PNGs; source font/assets remain fixed. No third
  trial, expectation relaxation or renderer workaround is authorized by a
  successful HTTP response. Do not blindly replay these tasks.

## Required next work and progress

The 111 full surfaces remain 17 implementing/94 open; none is fully closed.
The ledger now has 57 bounded points after adding CTX-SNAP-01. Prior accepted
subsets remain valid in their scope; their count does not measure complete
OMP parity. P0/V1 accepted; P1–P6 open; full marker null.

Next implementation work: remaining native registry/auth callers and the
recorded Core/Host contracts. Retain actual image readability/reopen, Codex
image transport/account trials and dependency policy as open requirements.
Cargo deny failure is RUSTSEC-2026-0192, unmaintained ttf-parser via fixed
fontdue; no safe upgrade, policy unchanged. Preserve all other gaps listed in
evidence. No reliable remaining whole-project ETA is established.

Batch begins around 11:46 UTC; runtime verification and independent review
complete around 13:13 UTC, about 87min; Git closure is separate. The original 120–210min estimate applies only
to this module. Use original module families and a stable batch gate; do not
repeat completed unchanged crates or live tasks to fill waiting time.

Root owns Cargo/network/private configuration/Git mutations. Independent
reviewer reads frozen scoped source/receipts only. Existing 57 vendor/EOL
files and `.codebase-memory` must remain outside scoped commits. No deployment,
history rewrite or memory update is part of this checkpoint.
