# CA-TOOL-WRITE Conflict93 execution — 2026-10-10

## Requirement and current decision

Fixed OMP: `596f2da7101178214aa27a753529d15e6b7ad91d` (v18.1.8).
This batch implements and executes the original conflict-detect **56** and
conflict-integration **37** contracts in Rust, retaining every original ID,
path, line, title and assertion. The [93-row executed mapping](write-conflict-20261010-behaviors.tsv)
has SHA256 `40a82697b769ce469cfdfa46bf81bc21347fcec3cd934413e6753b307e163329`.

Current decision: **bounded original Conflict93 execution/evidence APPROVE**.
This is **93/204 original WRITE test behaviors exercised in this batch**, not a
complete WRITE surface. Formal inventory stays **2/111 accepted, 1 tested,
26 implementing, 82 open**; `ported_through_commit=null`. Existing EDIT acceptance
refers to its recorded snapshot; changed shared paths receive the related
regressions below. No commit, push, deployment or product installation occurred.

## Delivered source and scope

Local packet: `D:/workspace/env/cache/ara-port-candidates/extension-production-batch-20261009`.
Eight scoped paths were applied to the actual `E:/repos/ara-github` worktree:
`ara-tools` conflict_detect/lib/read/write, two original conflict test targets,
the existing EDIT test's fixed prefix expectations, and the existing module
runner's two WRITE groups. Original before files and unrelated dirty/index
state were preserved. The implementation uses shared per-Session conflict
history, original numbered/scoped READ and bounded full-file scanning, token
expansion, stale-region validation, CRLF splice and per-file bulk semantics.
Conflict writes reuse the existing fallback and Native entered-effect owner,
and bypass ACP/LSP formatting as the fixed upstream requires.

Final delivery manifest:
`write204-source-audit-v1/conflict93/application/delivery-manifest-v3.json`,
SHA256 `f27a597787a67d998e03044b8e57031052ff673ae9161d30e602ef6609a8fbf0`.
Actual executable SHA256:
`50ee1582a6f0cdfc3df04b2484d25e3889c7b61bc678eb94b6ddbaef390dcb41`.
Build, module, controlled fault and CAS trials all use that delivered source
and binary, with unchanged source pins. Target/cache/tool/temp stay on D;
Cargo jobs=2, stable Rust/Cargo 1.97.1. No E target migration was made.

## Concentrated execution and retained failure

The existing `scripts/verify_edit_module.py` runs one Cargo build followed by
the selected executable groups, with exact test-name/count, hash, source and
timeout checks. Command:

```text
cargo test --locked -p ara-tools -p ara-cli --all-features --lib --test conflict_detect --test conflict_integration --test tools --test edit --test e2e --no-run --message-format=json
```

First attempt `write-conflict-live-v1/build-result.json`: **FAIL**, Cargo exit
101 after **31.311 s**, 409 reused artifacts. Two new ToolError calls incorrectly
assumed Display. The scoped correction uses the existing `.0` message in two
production and seven assertion sites; the public type, inputs and assertions
stay unchanged. This failed attempt is retained rather than relabeled PASS.

Final `write-conflict-live-v2/build-result.json`: **PASS**, exit 0,
**370.089 s**, 409 reused/12 compiled artifacts, zero errors. Its `warnings=168`
field counts recorded non-error compiler diagnostics; this is not a strict
Clippy pass or a claim that the workspace has no warnings.

| Executed group | Passed | Failed/ignored | Seconds |
| --- | ---: | ---: | ---: |
| Original conflict core | 56 | 0/0 | 0.100 |
| Original conflict READ/WRITE integration | 37 | 0/0 | 0.140 |
| Existing basic WRITE | 1 | 0/0 | 0.072 |
| Existing EDIT/shared prefix family | 14 | 0/0 | 1.417 |
| Existing Native AST/Edit builtin graph families | 4 | 0/0 | 0.209 |
| Actual retained REPL process | 1 | 0/0 | 10.202 |

Total: **6/6 groups, 113/113 tests**, zero failed/ignored. Test execution
**12.140 s**; final module **387.158 s** including build and receipt checks.
The original conflict groups cover malformed/scoped/unknown and mixed directives,
unchanged rejected files/history, stale regions, repeated
registration, out-of-order resolution, tokens, rendering, prefix restoration
and CRLF. A caught leading-BOM TextDecoder difference is corrected and checked
inside the existing original window case. No extra original behavior IDs were
invented. Other original WRITE families are not accepted by these tests.
Bulk per-file mixed success and a splice failure in one file were source-reviewed,
but were not separately exercised in this batch; these execution checks remain
open for complete WRITE closure.

## Actual Native failure boundary

`write-conflict-live-v2/native-fault/receipt.json`: **PASS**, **2.603 s**, same
binary/source. A controlled local OpenAI fixture drives actual Native READ →
malformed mixed per-id bulk WRITE → registered-side READ. The WRITE carries an
error receipt, file bytes stay unchanged at every request, and the subsequent
`conflict://2/theirs` READ succeeds, proving registered history survives this
failure through the real Host. Four bounded fake calls, three tools, no external
API. This is deterministic process evidence, not a real-model trial. A stopped
Run carrying the tool error does not itself accept a Task.

## Real CAS and same-Session restart

Live ARA Manager local OMP management → CAS catalogue verified;
`deepseek-v4.1-flash`, `openai-completions`. Same binary/source, bounds 6+2 calls,
120 s per Run and 2,048 output tokens. Receipt:
`write-conflict-live-v2/real-cas-receipts/20261010T155244Z/receipt.json`,
SHA256 `63d6bbdba6ce30c30ba6bc0acd7ea20ebadf69e0daa917d0c2358b60c0cf742f`.

- READ discovery → exactly one `write(conflict://*, @theirs)` → READ readback:
  **7.145 s**, four model calls/three tools. No ordinary WRITE occurred.
- New process, same Session, zero tools: correctly recalls the discarded
  ours-side random suffix from prior conversation; **2.219 s**, one call.
- Session `01a12684-46ac-719e-8038-b7358b8c3f28`: two user/five assistant/three
  toolResult messages persisted. No replacement conversation was created.
- Actual file has both selected sides and intact surrounding lines with CRLF;
  artifact SHA256 `08a8451232c17e47c6416572f337a95c32dfd893a192411228c557aa2538b87e`.
- Actual usage objects are retained; missing fields remain unknown. Private
  credential scan across ten output/profile/journal files finds zero leaks.

This does not claim cross-process conflict-history persistence, external
LSP/ACP installation, account login, unrelated Host faults or product release.

## Independent source review and remaining gate

Independent Codex reviewer `/root/compact_combined_dependency_post` approves
the final scoped source, original 93 bindings and runner; BOM/text differences
were resolved. Report `conflict93/review/POST.md`, SHA256
`5b3389dfb43cc5089de591ef67c4fa7bab769c67d505c9a1635928466b0b5027`.
The subsequent compiler-only correction has independent APPROVE in
`conflict93/review/COMPILE-FIX.md`, SHA256
`a59dc27db8ceb1ce362bdf3fef2d4ae06859a671407958274f1521a313bed2b0`.
These source reviews did not execute the runtime gates. Final independent
execution/evidence audit is **APPROVE** for the original Conflict93 batch, using
ara-git-review, ara-rust-core-review and point-delivery-audit criteria. Report
`conflict93/review/EXEC-AUDIT.md`, SHA256
`96f525f7989d75f7bf368f05ff0f869637707bc04beaa393cefaa8ca1c2399ef`.
The reviewer independently checked all 1,610 current build pins, executable
hashes, 93 original associations, six raw test outputs, Native failure receipts,
CAS tool/journal ordering, CRLF bytes and same-Session recall. No complete
surface or product gate is accepted by this bounded approval.

Before this batch, all 204 original WRITE IDs were source-audited at
63 implemented_unverified / 5 partial / 136 missing, independently approved;
this historical mapping is retained in `write204-source-audit-v1`.
Complete WRITE still requires generic xdev22, streaming preview15, cache
collapse marker8, exact remaining scalar contracts, archive/image/default URI
gaps, Unix kernel/shebang behavior and any other fixed source behavior, including
unported SQLite routing. Passing the 204 test associations alone is not proof
that every fixed source path is ported. Full workspace lint/tests, full
Extension and P1–P6/product/release gates were not run or accepted here.
