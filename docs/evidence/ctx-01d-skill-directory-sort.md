# CTX-01d: fixed OMP Skill directory ordering (WIP)

## Requirement and boundary

Fixed OMP `596f2da7101178214aa27a753529d15e6b7ad91d`,
`packages/coding-agent/src/internal-urls/filesystem-resource.ts:16-20`, sorts
directory entries first, then compares names with JavaScript `localeCompare`.
The directory text is an immutable `skill://` resource. Its line order can
change what `read skill://name/path:raw:N-N` returns. Before this change,
ARA sorted those names by lowercase ordinal value with a raw-name tie break.

This repair changes only the `skill://` directory branch in
`ara-tools::read::ReadTool::execute`. Ordinary local directory listing,
internal URL resolution, file reads and other tool behavior are outside the
change. Rust's stable sort retains filesystem enumeration order when the
collator returns `Equal`, as OMP's JavaScript stable sort does.

## Fixed-source Bun oracles

`scripts/ctx_skill_sort_oracle.mjs` verifies the SHA-256 of the exact source
file before importing and calling `buildDirectoryResource`. Source SHA-256:
`a20c01f819dac9d33f14ba588a76d814193f7ec0d64aee60089d27e5a5322625`.
The script runs under Bun **1.4.0**, the version in the fixed OMP root
`package.json`. The `sampledRawBefore` and `sampledRawAfter` arrays are
separate `readdir` calls; `content` is the authoritative OMP function output.
`projectedRows` maps that content to one-based rows, but does not invoke
OMP's ReadTool selector renderer.

| Environment | Resolved `Intl.Collator` locale | Portable listing SHA-256 | Distinguishing row |
| --- | --- | --- | --- |
| Windows x64, ICU 78.3, `LANG=C.UTF-8` | `zh-CN` | `0c4de035d4a59ea45af401eecde41aad74b32236bca01ed5c98dc2455ac7a563` | row 7 `中.txt` |
| Linux x64 runner, ICU 78.3, `LANG=C.UTF-8` | `en-US` | `4e15baa43d909a11254b91f565b8ce0b89bb8c430c625f4c279b022d79d2c67e` | row 7 `a_1.txt` |
| Linux x64 runner, generated `zh_CN.UTF-8`, `LANG` and `LC_ALL` set | `en-US` | same as Linux default | Bun did not adopt the environment locale |

The punctuation fixture `a {`, `b`, `c`, `}` sorts to `}`, `a {`, `b`, `c`
in both observed environments (content SHA-256
`5712936b8a00e973675cb4d03c7370a240c709a35e52de16adc8b76adfada405`).
The Linux-only case-collision fixture sorts `a.txt`, `A.txt`, `ä.txt`,
`Ä.txt` (content SHA-256
`e978b1414272698fb6e4b161feaabe778d27d690ea8177ac568ca286280d7766`).

Windows output is at `C:\Temp\ara-ctx-skill-sort-oracle\windows.json`
(file SHA-256
`bc953cd173177f2c2e400987a8e8f063e2ddeceaecd81757becfe18295fbb72a`).
The Linux job [36594567194](https://github.com/ZZray/ara/actions/runs/36594567194)
passed and uploaded both JSON files. Local copies are in
`C:\Temp\ara-ctx-skill-sort-oracle\linux-braces`; the default file's SHA-256
is `f9b4d0b3f4dcc57c4570daa5a04ed42a2c359330caeccc43927f76d6ca31f49c`
and the Chinese-environment file's is
`3be0aebfdc8dd42016d25c5fc97580d7ff5cf5a3a1d9581c7c7ab2bca08a1601`.

## Rust implementation and checks

`icu_collator = 2.3.1` with compiled data and `icu_locale_core = 2.3.0` are
pinned in `Cargo.toml` and `Cargo.lock`. On Windows, `sys-locale = 0.3.2`
reported `zh-CN`, matching Bun. On Linux, the observed Bun default is
`en-US` even after `LANG`/`LC_ALL` change, so that locale is selected there.
No ordinal tie break follows a collator equality. A separate scratch probe
using the pinned ICU4X versions matched both complete portable listings and
the Linux case-collision listing. Its code and build are at
`C:\Temp\ara-icu-probe` and are not part of the delivered implementation.

`cargo fmt -p ara-tools -- --check` passed. The `skill_urls` suite passed
17/17 on Windows, including the full listing and actual `:raw:7-7` result.
The existing disjoint-range test now compares selected lines with the same
directory's raw listing, so its range check is independent of host locale.
A controlled mutation restoring the old comparator made the new oracle test
fail on the first three directory
names; the original source bytes were restored and the test then passed.
`cargo clippy -p ara-tools --all-targets --all-features -- -D warnings`
passed. `cargo deny check` exited 0 with existing duplicate-dependency and
missing-license-field warnings; the new `icu_collator` license is
`Unicode-3.0`, already allowed by `deny.toml`. The actual CLI process test
`skill_directory_locale_order_reaches_cli_and_journal` passed 1/1 on Windows:
its `read` tool result selected a fixture row, entered the Session journal
and reached the next model request. The CLI test checks the exact name on
Linux; on Windows it checks that one of the two fixture names is propagated.

`python scripts/verify_backend.py` passed on the `34a2360` code snapshot on
2026-09-30 (exit 0): owned formatting, strict workspace Clippy, target and
doc tests; 1,044 passed, 0 failed, 1 ignored across 74 suites; CLI e2e
80/80 and `skill_urls` 17/17. The full output is
`C:\Temp\ara-ctx-skill-sort-oracle\backend-gate-linux-index-fix.log`.
`cargo deny check` passed before the final test-only adjustments, with the
same lockfile.
An independent Codex subagent reviewed the final code, dependency and test
diff. It identified locale assumptions in three tests; after correction it
found no actionable production defect. CI run
[36596683551](https://github.com/ZZray/ara/actions/runs/36596683551) on
`acf79e9` failed only in the new Linux oracle test: NFC/NFD names occupy
zero-based indices 13-14 on Linux, but the test used Windows indices 14-15.
WIP `34a2360` corrected that test slice; an independent reviewer matched it
to both Bun outputs. [Linux CI 36597728861](https://github.com/ZZray/ara/actions/runs/36597728861)
passed on `34a2360`: the oracle test and all 17 `skill_urls` tests passed,
and `verify_backend.py` ended with PASS. Other
Windows locales receive generic tool-path assertions, but lack a full
fixed-OMP listing oracle. CTX-01d remains
**WIP**. The oracles cover Windows `zh-CN` and Linux `en-US`; macOS, other
Windows locales and exhaustive Unicode collation remain unverified.
Binary-size change was not measured. Do not advance the full OMP marker,
P1 or P3 from these fixtures.

## Real-model tasks on the delivered code

Before the tasks, authenticated B.AI `GET /v1/models` returned the exact
`deepseek-v4.1-flash` ID among 58 models. Both tasks used the real `ara`
binary built from `acf79e9` production code, OpenAI-compatible Chat
Completions, and a short-lived environment mapping of `BAI_API_KEY` to
`ARA_API_KEY`. The later `34a2360` change touches only a test assertion.
No credential was printed or saved in the task receipts.

1. `scripts/real_model_trial_ctx.sh` in a fresh Git repository at
   `C:\Temp\ara-ctx-acf79e9-bai` passed in 16 seconds and five model calls
   (limit 12 calls, 300 seconds, 2,048 output tokens per call). All eight
   tool starts have matching ends. The model read `skill://release-notes`,
   wrote `mathx.py` with the `AGENTS.md` owner line, verified
   `double(21) == 42`, and wrote the prescribed `RELEASE_NOTES.md`. The
   checker exited 0. Artifact SHA-256 values are
   `6a541b409f91acc8095732300f2454c87cc633b1a4f4bdee7f203eb2dafe3e14`
   and `9d38aa2ba47c8e847d1eaaa028edbfef232e81a92b7c2eea7d4e67203c05a196`;
   `events.jsonl` is
   `b3033e09f323dcdb018b47548732cfdb6d1b3dc6a27506079f9ee79f91cd24c8`.
   Reported usage: 6,594 input, 928 output and 23,168 cache-read tokens;
   cost unknown.
2. The sorting task in `C:\Temp\ara-ctx-acf79e9-sort-bai` asked the model
   to read `skill://choice/assets:raw:1-1` and write `SELECTED.md` with
   that filename. The tool returned `a_1.txt` and the write receipt records
   `a_1.txt\n`. The file contains exactly those eight bytes (SHA-256
   `68022fb8419aeab4e126f7242ea934067af05b439f0bcf628c82ec7897da35eb`).
   The first Run was limited to five calls, 120 seconds and 512 output
   tokens per call. It hit the call cap while the model double-checked the
   file, so it exited 1 despite the correct artifact. The same Session
   (`01a0edf8-c20f-70a9-8c05-16abece6ec1b`) resumed with a limit of two
   calls, 60 seconds and 512 output tokens per call, then ended with `stop`,
   exit 0 and no further tool call. Across both Runs, six model calls and
   six paired tool starts/ends are recorded; the model duration fields sum
   to 15.944 seconds. Reported usage: 6,213 input, 653 output and 27,904
   cache-read tokens; cost unknown. The original and resumed event logs
   have SHA-256 values
   `2f264c63d7a71572d7a18825707f53bbdd679bcec62bb0438c4646b75408eff6`
   and `3235d9243b7e9e6fb07e5a617f01be6d3e3a3f1d90e598352d9537e7fc9911b8`;
   the Session journal is
   `67599e64cdb65e9f69c71704c596846bf9ebad1cb79152616ef2480612e54deb`.
   An exact-key scan of the event logs and journal found no credential.

These tasks demonstrate actual tool and Session effects. The first sorting
Run remains a call-limit failure in the record; its artifact was checked
independently, and the same Session finished on resume.

## Point-delivery audit, 2026-09-30

Target: `34a2360` on `dev`, with production code in `acf79e9` and the
Linux-only oracle-index correction in `34a2360`. The fixed OMP requirement
is directory-first, locale-aware stable sorting of immutable `skill://`
directory text and its selected lines. The change does not authorize a new
sort order for ordinary paths. The changed code/dependency/test files and
their direct requirement mapping are listed above; no unrelated product
state, host or provider path changed.

The normal path is evidenced by fixed-source Bun oracles, Windows and Linux
backend gates, an actual CLI-to-Session-to-model controlled-upstream test,
and two bounded B.AI tasks with checked artifacts. Relevant failure paths
include the existing `skill_urls` invalid URL, traversal and read-only
write tests, plus the real-model first-Run call-limit receipt and native
resume without replaying a write. `cargo deny check` passed with recorded
warnings. The independent Codex reviewer found no actionable product-code
defect after re-review of the final tests and the Linux correction.

**Verdict: implementing (WIP), not accepted.** The full CTX-01d point still
lacks direct fixed-OMP listing comparisons on macOS and other Windows
locales, as well as broader Unicode collation cases. The current oracles
prove the two observed default locales and the tested fixtures; they do
not accept the entire OMP surface, P1 or P3. Keep the feature ledger and
upstream marker unchanged until those boundaries receive evidence or a
reviewed intentional-difference decision.

## Cross-platform same-host follow-up, 2026-09-30

WIP `97769f4` extended the fixed-source Bun 1.4.0 oracle with
`portable-no-equivalent` and `unicode-expanded` fixtures. Its
[oracle workflow 36600719734](https://github.com/ZZray/ara/actions/runs/36600719734)
passed on Linux x64, macOS arm64 and Windows x64. The two fixtures returned
`status: ok` on all three platforms. Their en-US full-content SHA-256 values
were respectively
`0d2e24e7ef80d1c4e037e20ffb7cf5cf051e00b4d1e45a89bc7298b9e003b135`
and
`86cf1cfc803d476a1ac29fe338b0ec088880b6005c6e92bd229474fcc1b158f9`.
The local Windows zh-CN Bun run produced
`d72613f29c0aa8958d35d9944aa393c32eccb32c40bb8610bc0a13794f1c9640`
and
`f612f822e333b7e62b682b61504d6aa1ebc4d5f2d84d8f1b11786afe18d7f311`.
On macOS, the original `portable` NFC/NFD names and `case-collision` fixture
were merged by the filesystem and recorded as `filesystem-collapsed`; the two
new fixtures were not merged. Local copies of the three runner JSON files
are under `C:\Temp\ara-ctx-skill-sort-oracle\cross-platform-97769f4`.

WIP `8885af1` added a native Rust test that reads the Bun JSON generated in
the **same CI job**, checks the fixed commit/source hash, Bun version,
platform and resolved locale, constructs the two non-collapsing fixtures,
compares each complete `skill://` directory text, and checks its selected
`:raw:7-7` first line against Bun's row projection. It also extends the
existing Windows `portable` assertion to en-US while keeping the zh-CN and
Linux en-US expectations. The selected-line check does not compare the
entire OMP selector renderer or continuation notice.

| Check on `8885af1` | Actual result |
| --- | --- |
| Local Windows zh-CN, `ARA_CTX_SKILL_ORACLE_JSON=C:\Temp\ara-ctx-skill-sort-oracle\windows-expanded.json`, exact new test | Exit 0, 1/1; the original fixed-OMP test also passed 1/1. |
| Deliberately pass the Windows en-US runner JSON to that zh-CN test | Exit 101 at the resolved-locale assertion (`en-US` versus `zh-CN`), showing a mismatched oracle fails before content comparison. |
| `python scripts/verify_backend.py` | Exit 0; formatting, strict workspace Clippy, target and doc tests passed, 1,045 passed across 74 suites including `skill_urls` 18/18. The same-host test in this full gate had no JSON environment variable; the explicit local and CI invocations above and below provide its comparison evidence. Log: `C:\Temp\ara-ctx-skill-sort-oracle\backend-gate-same-host-oracle.log`, SHA-256 `8fd1179d2ebad7afb80cd6b44fbc997fb39bbe8f5f5404af647746de5e08df27`. |
| `cargo deny check` | Online advisory refresh failed at the RustSec Git TLS handshake. `cargo deny --offline --locked check` exited 0 using the cached advisory database, with existing duplicate-dependency and missing-license-field warnings. This is not a fresh advisory check. |
| [same-host workflow 36607978940](https://github.com/ZZray/ara/actions/runs/36607978940) | Completed success on `8885af1`; macOS job `109541803214`, Linux job `109541803346`, Windows job `109541803464` each ran the exact native Rust test against its job's Bun JSON: 1 passed, 0 failed. Each workflow uploaded its Bun JSON before running Rust. |
| [repository checks 36607978943](https://github.com/ZZray/ara/actions/runs/36607978943) | Completed success on `8885af1`; the Linux `verify` job passed bootstrap, fixed inventory and `verify_backend.py`. |

An independent Codex subagent reviewed both changed files against `97769f4`
under `ara-git-review` and found no actionable defect. It did not execute
the cross-platform jobs. The same-host comparison covers the observed
Windows zh-CN/en-US, Linux en-US and macOS en-US cases and the named fixtures;
it cannot prove every locale or Unicode string. The `8885af1` change is
test/workflow only; production code remains from `acf79e9`. CTX-01d remains
WIP; the point-level re-audit is recorded below.

## CTX-01d point re-audit on `8885af1`

An independent Codex point auditor checked the full CTX-01d ledger row after
the three-platform run. **Verdict: changes requested.** The comparator
repair has direct evidence for the named fixtures and observed locales, but
`read.rs` then used `while let Ok(Some(entry))` for a skill directory. An
enumeration error could end the loop and return a successful partial
resource; a `file_type` error could classify an entry as a file. Fixed OMP's
`fs.readdir` propagates the enumeration error. The auditor also noted that
the JSON oracle projects selected rows rather than invoking OMP's complete
ReadTool selector renderer.

Other recorded boundaries still require separate decisions: plugin-contained
skills are open with CTX-01c rather than proven by this directory fixture;
ARA's 256 MiB post-window scan bound may affect very large immutable skill
resources; and untested locales and Unicode strings remain outside the
observed matrix. `/skill:` invocation and other internal URL schemes belong
to separate rows. No CTX-01d, P1 or P3 acceptance follows from this run.

## Skill directory enumeration error repair (WIP)

On the worktree following `8885af1`, `crates/ara-tools/src/read.rs` now
propagates `next_entry` and `file_type` errors for an internal `skill://`
directory before sorting or returning text. The ordinary local-directory
fallback is unchanged. This follows fixed OMP
`filesystem-resource.ts:10-39`, whose `fs.readdir` must succeed before a
resource is returned.

The local Windows checks on this worktree passed: `cargo test --locked -p
ara-tools --test skill_urls` 18/18, the exact same-host zh-CN oracle test
with `ARA_CTX_SKILL_ORACLE_JSON` 1/1, the ordinary-directory
`read_directories_binaries_images` test 1/1, and
`python scripts/verify_backend.py` 1,045 passed across 74 suites (exit 0).
The full gate log is
`C:\Temp\ara-ctx-skill-sort-oracle\backend-gate-listing-errors.log`, SHA-256
`8540367a7f317176613636037801db811fe7b3d49d5df7d6b18175ce3ff666ff`.
An independent Codex diff reviewer found no actionable regression and traced
`ToolError` through the Agent loop to an `isError=true` tool result and
journal receipt. Its review was read-only and did not run tests.

A portable ordinary temporary directory does not reliably induce a
post-open `next_entry` or `file_type` fault. The new error branches have
source-level review but no injected-fault execution receipt; a controlled
faulting filesystem or syscall injection would be needed for that check.
Keep CTX-01d implementing (WIP) while the remaining point-level boundaries
above are resolved. This repair does not advance P1 or P3.

The repair and this evidence were committed as WIP `e96dc5a` and pushed to
`dev`. On that exact SHA, [repository checks 36613800339](https://github.com/ZZray/ara/actions/runs/36613800339)
passed bootstrap, inventory and the Linux backend gate. The
[fixed OMP oracle workflow 36613800340](https://github.com/ZZray/ara/actions/runs/36613800340)
passed its native same-host Rust comparison steps on macOS, Linux and Windows.
These green runs verify the delivered success paths; they do not inject the
post-open directory I/O fault or close the remaining CTX-01d boundaries.

## Skill resource scan-budget repair (WIP), 2026-09-30

Requirement: a fully loaded immutable `skill://` resource must count all
its lines for tail, single-range and multi-range reads. Fixed OMP
`596f2da7101178214aa27a753529d15e6b7ad91d`,
`tools/read.ts:2368-2374`, passes the complete resource to the in-memory
renderer with Skill result limits disabled. `tools/read-format.ts:314-327`,
`339-341` and `529-531` calculate the complete line count before selecting
or rendering. No 256 MiB post-window scan budget applies to this path.

The tested worktree follows `c341f291`. Its only production change is
`crates/ara-tools/src/read.rs`: the three scan-budget exits now apply only
to ordinary files, with the existing cancellation checks retained. A
private const-generic wrapper lets tests use an 8-byte budget through the
same implementation. Ordinary-file budgets, resource discovery/loading,
directory ordering, providers and product state are unchanged. Plugin
containment and complete selector-renderer parity remain separate open
boundaries.

| Check | Actual result |
| --- | --- |
| `cargo test --locked -p ara-tools --lib skill_resource` | Exit 0, 3/3. Checks tail, complete single/multi line totals, ordinary-file budget behavior and cancellation after more than 4,096 lines. |
| Restore the three prior resource-budget exits temporarily | Exit 101, all three new tests fail. `read.rs` was restored byte-for-byte by SHA-256. Script/log: `C:\Temp\ara-ctx-resource-scan\mutation.py` and `mutation.log`; log SHA-256 `b66dd51ae9ac54759486f735966ce0acf92dca86f9e8cf1be751b1b0d6443b89`. |
| `python scripts/ctx_skill_resource_scan_trial.py --binary E:\repos\ara-github\target\debug\ara.exe --output C:\Temp\ara-ctx-resource-scan` | Exit 0 on the final script. A real CLI process reads a 269,484,288-byte Skill asset (65,778 lines) against a controlled local OpenAI Chat SSE upstream. Tail `:raw:-2`, head `:raw:1-1` and multi `:raw:1-1,3-3` all return exact expected text to the next model request and to three successful Session tool receipts; every receipt records `totalLines=65778`. |
| `python scripts/verify_backend.py` | Exit 0; owned formatting, strict workspace all-target/all-feature Clippy, target and doc tests pass: 1,048 passed, 0 failed, 1 ignored across 74 suites. Log `C:\Temp\ara-ctx-resource-scan\backend-gate.log`, SHA-256 `dbb93b1a0c1d1e197c714f2d4e60a7bcfe9abd5fbbbec8f5ff23a894a3bbc05a`. |
| `cargo deny --offline --locked check` | Exit 0; advisories, bans, licenses and sources pass with existing warnings. Cached advisory database only; the earlier online RustSec refresh failed its TLS handshake. Log `C:\Temp\ara-ctx-resource-scan\cargo-deny-offline.log`, SHA-256 `9c186629d4ae76c1b555db3f36f90d0f3a46e7afad161b543e895218581c6679`. |

The final controlled trial artifacts are under
`C:\Temp\ara-ctx-resource-scan\host-ffy345v8`: `events.jsonl`,
`stderr.txt`, `requests.json`, `summary.json`, and the Session journal
`sessions\2026-09-29T21-12-57-219Z_01a0ef03-7883-7304-878b-76b160d5ee0d.jsonl`.
It used four logical model calls/four HTTP requests, three tool receipts,
0.929 seconds, a 120-second Run limit, a 150-second process limit and
256 output tokens per call. The upstream is controlled, not a real model;
usage remains unknown. Only the generated large asset was deleted after
the run. Timeout handling also preserves captured output and requests
before re-raising failure; that branch was reviewed but not fault-executed.

Final hashes: `read.rs`
`ce283ab4ce473ec55bbc0c3c218f400c99692a541d6a3e31ab7e5bfdeb259494`,
trial script
`11c58ba3bfb830580f8f9269ebd6ca48b7c43892d906c81eb30e0783fd3fbb44`,
tested debug binary
`a20e48a8b9b26dd09e50b14a1878f1cf8cbaafa4d46885c718d720512c2607f8`,
fixture
`84faf46391e315f39a7556678a821da0484bbc4740d93ec52b38c8cc3e90e92c`,
and final summary
`108ef4acb339d5e6c42bc7d394a7e06d5ecc6d3a9e38674e6075d95c95fbd188`.

Independent Codex subagent `resource_scan_plan` approved the narrow plan.
`resource_scan_diff_review` reviewed all seven `read.rs` hunks against
`c341f291` using the fixed OMP source, found no actionable defect and
independently passed the new resource tests (3/3), Skill read entrance
tests (4/4), ordinary-file scan-budget test (1/1) and `git diff --check`.
Its script review found no false-positive PASS path; the observed timeout
evidence gap was repaired, the final script was rerun as above, and its
independent re-review passed. Independent Codex point auditor
`ctx_point_audit` checked both final file hashes and the actual artifacts
and found no new production defect: this evidence supports WIP delivery,
not full point acceptance.

This cancels an already-set token while scanning more than 4,096 lines;
it does not measure mid-scan cancellation latency. The large-file trial
exercises the actual 256 MiB boundary for all three selector paths, but
its expected strings are source-backed assertions rather than a direct
invocation of the full OMP selector renderer. No new real-model task ran
on this repair. **CTX-01d stays implementing (WIP); P1/P3 do not advance.**
