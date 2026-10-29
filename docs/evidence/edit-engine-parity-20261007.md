## Current acceptance — 2026-10-11

**CRATE-PI-EDIT is accepted for the fixed pi-edit/pi-diff library surface.**
Fixed OMP `596f2da7101178214aa27a753529d15e6b7ad91d`; original286 IDs,
source locations and assertions remain unchanged. All286 pass on actual
nonroot Linux/ext4. Independent Codex `/root/read_complete_scope` final POST
APPROVE; Root independently verifies all1750 current E source pins, raw outputs,
original-ID association, final audit and the supplemental executable-bit gate.
The older Windows-only pending conclusion below is historical.

| Current entry | Result | Seconds |
| --- | --- | ---: |
| `cargo test --offline --locked -p ara-diff -p ara-edit --no-fail-fast` on the existing native snapshot | Original286/286 PASS; raw287 tests including one existing auto-repair group; zero failed/ignored/missing; two doc-test entries with0 cases | 90.590 total / 90.365 Cargo |
| Existing actual tools ELF: `upstream_file_reencoding_moves_and_partial_failure_report --exact --nocapture --test-threads=1` | 1/1 PASS, real `cfg(unix)` moved script retains0o755; lossy bytes, replacement and partial failure assertions also pass; no Cargo/model call | 0.084 |

Cargo's build report is1m13s. UID/EUID1000; workspace, output, Cargo cache,
Rustup and target are ext4 inside the D-backed WSL2 distribution. Original
`B-c2c3e78f87` directory-symlink behavior is actually executed and PASS.
The same1750 source pins are checked before/after execution and at acceptance.
All14 vendor test executables have ELF magic and exact hashes. The supplemental
tools ELF is reused from the already verified Unix22 build, not recompiled.

The prior169-file fixed-source audit remains applicable:166/167 vendor inputs
are byte-identical; `lib.rs` adds only the separately accepted auto-repair module
and formatting around the original body. Original169 vendor inputs used by the
accepted EDIT92 real CAS edit/readback/same-Session restart trial are identical
to current source. This is explicit historical-source reuse on its original
binary, not a new v21 model trial or a full Host re-execution.

Artifact root: `D:/workspace/env/cache/ara-port-candidates/native-edit286-v1-20261011`.

| Receipt | SHA256 |
| --- | --- |
| ROOT-ACCEPTANCE-v1.json | `20291a2d62c15c33f47af44dd6a7f760bc52139ca25ebb539148fe8e0981741d` |
| CRATE-PI-EDIT-INDEPENDENT-B-POST-v1.md | `51e2dc98644790c9ff5f97c1bd0f5e358a920efc3aaca23b4c4809150d1e3491` |
| CRATE-PI-EDIT-INDEPENDENT-B-POST-v1.json | `31116756f68f26e54aba25846504cef598dbfba37add521485b8e5d12cb3c1b5` |
| EXEC-result.json | `45cf3fae4a9a83cd2695b986f66c74336deba037b6e2d1be49fec910ec490d7b` |
| EXEC-cargo.log | `01e063513c2d0ac8705a57007e47cd8ad7300e4e74c89295c2aba4da4c85a8fe` |
| source-pins.json | `eaca8e671224bc5393b62250cfb20adf0e0391b6f0f0276cda54e9be110374a8` |
| result.json (supplemental tools entry) | `626524922a3b8988b0322f43fe5ce5b8e6d8f1d1bbd35fc985a15243b0486ebd` |

Formal inventory becomes6/111 accepted,0 tested,23 implementing,82 open.
Only this library surface advances. Windows recovery, NFC/walker and Bazel
adaptations retain the prior source-audit boundaries. No new PRE is claimed.
Workspace tests/Clippy/deny and AST corpus remain release/separate-surface gates;
full WRITE/TUI/MISC/Extension and products stay open. The fixed whole-upstream
marker remains `ported_through_commit=null`. Git index stays unchanged.

# Fixed OMP edit-engine contract restoration — 2026-10-07

## Scope and result

Fixed OMP: `596f2da7101178214aa27a753529d15e6b7ad91d`.
The independent audit compared all 169 pi-edit/pi-diff files and mapped all
286 inventory behaviors. Two ARA contract additions were removed: rejecting
lossy UTF-8 persistence and rejecting hashline moves onto an existing file.
The original patch/apply_patch destination guard is retained.

**Current result: Windows module execution passed. Full CRATE-PI-EDIT acceptance
is still pending; its inventory status remains `tested`.** One upstream
`#[cfg(unix)]` symlink behavior has not run on this platform. This receipt does
not accept CA-TOOL-EDIT, P1, production Session, or complete OMP.

## Delivery map

| File | Change |
| --- | --- |
| `crates/vendor/ara-edit/src/files.rs` | Restore the original lossy decode/persist contract. |
| `crates/vendor/ara-edit/src/modes/hashline/patcher.rs` | Restore original MV overwrite/self-move behavior; retain Windows recovery adaptation. |
| `crates/ara-tools/tests/edit.rs` | Reuse the original two test entrances to check actual U+FFFD bytes, overwrite/source deletion, suffix/REM ordering and partial failure. |
| `scripts/vendored_behaviors.py` | Save the same Cargo output, propagate nonzero Cargo exit, and label the one upstream Unix test not applicable on Windows. |

No test entrance was added. Existing vendor fixtures and tool tests were reused.
The exact four-path v2 pins have SHA256
`fe0247c2c969c4ebace199921e23f1cf827272ba28e03d079f38d87b0c135b88`.
All gates used the same unchanged 897-file source map, SHA256
`e8229988a5c7cbcc69ba6863d9fb089e40228f426d7454ec71e2ffb3cbd090a2`.
Git index bytes and 1,211 other baseline file bytes were preserved during
application and execution; subsequent changes are scoped documentation only.

## Executed gates

Environment: Windows, existing stable Rust and vendor nightly rustfmt;
`CARGO_TARGET_DIR=D:/Temp/ara-build/ara-github-target-20261005`.

| Command / entrance | Actual result | Seconds |
| --- | --- | ---: |
| stable `rustfmt --check`, changed tool test | Exit 0 | 0.193 |
| nightly `rustfmt --check`, two changed vendor files | Exit 0 | 0.201 |
| `python -X utf8 scripts/vendored_behaviors.py --out <artifact>/vendored-behaviors.tsv` | Cargo and verifier exit 0; original grouped `cargo test --locked -p ara-diff -p ara-ast -p ara-edit --no-fail-fast`, including doc-tests: 368 passed, 0 failed, 1 upstream ignored | 185.214 |
| `cargo test --locked -p ara-tools --test edit` | Exit 0, 10/10 passed | 100.767 |

Final gates total 286.375 seconds; reported test bodies total 2.13 seconds.
The initial Temp formatting checks used an incorrect vendor config path and
then stable rustfmt against nightly settings; these were corrected before
application. Final nightly formatting passed. No Rust compile/test attempt
failed in this batch.

The same vendor output maps CRATE-PI-EDIT to **285 actual `ok` rows and one
`not-applicable-windows` row**. The additional Windows verbatim-drive recovery
test actually passed. The other 83 pi-ast rows map to 81 `ok`, one
`ok-no-corpus`, and one upstream ignored; this run supplied no OMP corpus and
does not establish full pi-ast parity.

The tool module checks real temporary-file bytes/effects, retained patch
behavior, writer partial failure and ordering. The Unix executable-bit block
was not executed. Six warnings from existing RPC/AI/Agent WIP appeared in the
tool build; no strict workspace Clippy pass is claimed.

The verifier failure check reused the completed real Cargo output with a
controlled injected Cargo exit 101. Inventory rows still mapped successfully,
but its in-process `main()` returned 101. This checks failure propagation without
a second Cargo run or separate verifier process; it is not an actual Cargo test
failure or real-model evidence.

## Audit and artifacts

Root directly checked fixed source, all four hunks, delivered hashes, complete
result mapping, actual logs and preservation. Independent Codex
`/root/native_realm_pre` performed the 169-file audit and four-path SOURCE PRE;
PRE SHA256 `9491749c083eca17c88445e921ba63a3cdd16d78777ac3e280480bd5d5c99261`.
Independent POST passed for this Windows module repair, with no blocking finding.
POST SHA256 `894de37b03ffee851073416b15f89dc0c61951b60657e1e46dd2e41b83d0e868`;
report: `C:/Temp/ara-model-fallback-batch/root-edit-module-close-independent-post-v1/POST.md`.
The reviewer checked all four target paths and 893 other source-map hashes at
review time; the 1,211-file preservation statement comes from Root's execution
receipt. Subsequent documentation changes include the vendor README in the
execution map; all compiled Rust and verifier source remains unchanged.
This receipt must not be used as full acceptance.

Retained local artifacts:
`C:/Temp/ara-model-fallback-batch/root-edit-module-close-v1` contains v2 pins/diff,
source map, individual gate JSON/logs, complete Cargo log/TSV and controlled
failure output. `execution.json` SHA256 is
`3d76dd3387e3bf6f905a634785a737ce3cb536026fc029424dff7b7fb059844a`.

Full acceptance still requires applicable Unix and final batch gates. This
batch did not repeat workspace-wide tests/Clippy/deny or real-model integration.
No commit, push, installation, deployment, durable knowledge or memory promotion
occurred. Global complete count and full upstream marker are unchanged.
