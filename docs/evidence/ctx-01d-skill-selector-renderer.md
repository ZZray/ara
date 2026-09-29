# CTX-01d representative Skill renderer comparison (WIP)

## Requirement and scope

Compare the complete output of representative immutable `skill://` reads
with the actual fixed OMP in-memory renderer, rather than projecting rows
from a directory listing. The baseline remains
`596f2da7101178214aa27a753529d15e6b7ad91d` (v18.1.8).
`tools/read.ts:2368-2374` passes the resource to
`buildInMemorySelectorResult`; `tools/read-format.ts:314-327` resolves the
parsed selector using the full line count and dispatches single/multi
rendering.

The worktree follows `cc9047a`. No production file changes. The requirement
maps to three files: `scripts/ctx_skill_selector_oracle.py` exports exact
Git bytes and launches the oracle; its `.mjs` companion imports and calls
the fixed parser/renderer; `crates/ara-tools/tests/skill_urls.rs` executes
the same inputs through the current Rust `ReadTool` and compares complete
text, notices, `totalLines` and the absence of editable Skill snapshots.
The complete generic ReadTool selector surface remains TOOLS-01a.

The launcher exports 12 unchanged source modules plus upstream LICENSE,
checks each Git blob ID and SHA-256, and retains every failed run. The
runner verifies the bytes before and after execution. Eight separate
throw-only sentinels make unused relative imports resolvable; they do not
replace any of the 12 original modules. The 29 dead-import traps must
remain uncalled. The actual parser, line splitting, display mode, result
builder and native block-context implementation execute.

## Executed inputs and results

| Fixture | Representative comparisons |
| --- | --- |
| `plain.txt`, 12 lines ending in LF | Default and `3-4`, with line numbers off/on; raw full/range/tail/multi; non-raw tail/multi; single and multi out-of-bounds notices. |
| Empty asset | Non-raw empty text has zero addressable lines; raw has one empty segment. |
| Same text with CRLF | Raw exact second line and non-raw `3-4`, including CR preservation and complete notices. |
| ASCII directory, 2 directories and 10 files | Actual `buildDirectoryResource` followed by default/range/raw range/tail/multi rendering. |
| Existing six-line `blocks.ts` | Raw and non-raw disjoint ranges; the real native call returns block boundaries `[3,6]`. |

Command (Windows):

```powershell
python scripts/ctx_skill_selector_oracle.py --upstream C:/Temp/omp-596f2da --bun C:/Temp/ara-ctx-skill-sort-oracle/bun-windows/bun-windows-x64/bun.exe --native C:/Users/loveu/.omp/natives/18.1.8/pi_natives.win32-x64-baseline.node --output C:/Temp/ara-ctx-skill-selector-oracle
$env:ARA_CTX_SKILL_SELECTOR_ORACLE_JSON = 'C:\Temp\ara-ctx-skill-selector-oracle\run-20260929T230046Z-34e5ab93\oracle.json'
cargo test --locked -p ara-tools --test skill_urls --all-features skill_selectors_match_fixed_omp_renderer -- --exact --nocapture
Remove-Item Env:\ARA_CTX_SKILL_SELECTOR_ORACLE_JSON
```

Both commands exited 0. The actual Rust comparison ran **22 complete
cases**, not a no-input test pass. There were seven real native calls and
zero calls across all 29 traps. The native sidecar records request code
hashes/ranges and actual return values, including `[3,6]` for `blocks.ts`.
The Rust test fixes fixture identities, requires the representative
selector matrix and both numbering modes, and checks the fixed source
manifest before executing every case.

| Negative check | Actual result |
| --- | --- |
| Append incorrect text to the first expected result in a JSON copy | Rust exit 101 at the complete-text comparison. |
| Replace the first source SHA-256 in a JSON copy | Rust exit 101 at the fixed-source assertion. |
| Add a comment to exported `read-selector.ts` in an isolated oracle copy | Bun exit 1 before imports with source hash mismatch; stdout empty. |

No production or test source was changed by these mutations. Their
artifacts are `wrong-result-text.{json,log}`, `wrong-source-hash.{json,log}`,
`oracle-mutations.json` and `source-tamper-ob0a1xfq/negative-summary.json`
under `C:\Temp\ara-ctx-skill-selector-oracle`.

## Backend gate and independent review

The first `python scripts/verify_backend.py` run failed the existing
`repl_resume_after_interrupted_tool_reports_unknown_effect_without_replay`
warning assertion (CLI e2e 79/80). The test and production paths were not
modified. Its focused default-feature recheck and an all-feature recheck
using the same Git Bash environment both passed 1/1. A second complete
gate on the unchanged code exited 0: owned formatting, strict workspace
Clippy, target and doc tests passed, **1,049 passed, 0 failed, 1 ignored
across 74 suites**, including CLI e2e 80/80. The first failure's cause is
unconfirmed; retaining these logs does not claim that its intermittent
failure has been fixed.

The full gate does not supply selector-oracle JSON and therefore only
compiles the new comparison test. The explicit 22-case invocation above
provides the actual comparison evidence. This test-only change modifies
no dependencies, lockfile or deny policy; the prior cached dependency
audit is not represented as a new or fresh audit.

Independent Codex `resource_scan_plan` reviewed the plan before
implementation. `oracle_diff_review` reviewed all three final files
against `cc9047a`, inspected the final oracle/source/native receipts and
the explicit Rust output, and found no actionable defect. It confirmed
that failed early runs did not write `oracle.json`, and the final runner
SHA matches the executed script.
Independent Codex `ctx_point_audit` separately inspected the exact
three-file scope, oracle/native receipts and positive/negative logs. It
found no actionable defect and approved WIP evidence delivery, retaining
the native provenance and first-gate-failure limits below.

## Receipts and limits

Final artifacts: `C:\Temp\ara-ctx-skill-selector-oracle\run-20260929T230046Z-34e5ab93`.

| Receipt | SHA-256 |
| --- | --- |
| `oracle.json` | `5fb44293c0180ddef11430f4fb6a9531d68672385d92b1ea170c960923800678` |
| `source-manifest.json` | `e6576775dd98c00409cc6281ba7acac2b118f4fb5b4e0ee1ebcde85ee0b7d8e8` |
| `rust-comparison-final.log` (parent output directory) | `b6d4e641e8fcde513742ad740bb8161cc3db3d88eba6833f8897532d59105839` |
| `backend-gate.log` (first, failed) | `b00e35ad656f0576d43e40f94eda8bb1a243cbd763c64c6ce23db5a8eebd19ef` |
| `backend-gate-recheck.log` (passed) | `8ace5bb4366c080da57807624dc9958adfd945265ecb6b8328802a0df94d9304` |

Final tested source hashes: Rust test file
`e02ae4a93f999d052de96fa074b9c95a77cdaad9c2aca172230d5d90ba7dcfb1`,
Python launcher `2e9357ea5667bca924164c9dba0c846ddad35d96562787aec1bd502c9d0f7637`,
MJS runner `c92dbcf230ca588eb503606ba1c90b285c643fb55d285a5aa18b210da57d5710`.

This run uses Windows zh-CN, Bun 1.4.0 and an already installed native
18.1.8 addon with SHA-256
`fd757d36c44b8fa4cb184adc979f39b6aedabf8341d5a5316bf36f3c3949aa20`.
The addon marker and actual functions were checked; its precise build
provenance is unverified. These representative inputs do not prove the
complete OMP ReadTool or all platforms/locales. This is local tool/renderer
evidence, not a new real-model or CLI-to-Session trial.

**CTX-01d remains WIP.** The representative renderer gap has direct
evidence; controlled post-open directory I/O fault receipts, final
point-snapshot checks and targeted real-model/point acceptance are still
pending. Plugin/managed Skill containment remains assigned to the open
CTX-01c follow-up; `/skill:` remains CTX-01e.

## Delivered snapshot

WIP `81d036e1903327f07fa034099871c780caeda729` was pushed to `dev`.
[Repository checks 36643619576](https://github.com/ZZray/ara/actions/runs/36643619576)
and the [three-platform directory oracle 36643619584](https://github.com/ZZray/ara/actions/runs/36643619584)
both completed successfully on that exact SHA. These workflows run the
Linux backend gate and the existing same-host directory comparison;
they do not supply the new selector-oracle JSON. The explicit Windows
22-case comparison above remains this slice's dynamic renderer evidence.
CI receipts: `C:\Temp\ara-ctx-skill-selector-oracle\ci-81d036e.json`,
SHA-256 `d6748db28f615204cfcbd59a12e75ac7eb14d863455a6bf39094aeb6162dfa9e`.
