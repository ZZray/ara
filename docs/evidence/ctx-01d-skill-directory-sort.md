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
