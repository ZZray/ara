# Shared EDIT / WRITE Host batch — 2026-10-08

## Scope and current state

Fixed OMP: `596f2da7101178214aa27a753529d15e6b7ad91d`.
Delivered dirty worktree HEAD: `20aa0e2ef9872f633ba2671376cdf778477e7115`.
This batch connects WRITE to the existing shared EDIT path policy, ACP/LSP
consumer interfaces, mutation fallbacks, Session diagnostics and Native batch
effect owner. It does not claim to install production ACP/LSP services.

SOURCE v2 is independently approved. The original 15-group execution is
terminal **FAIL**: build PASS **842.180 s**, total **890.600 s**, test bodies
**35.042 s**. Fourteen groups pass; engine is 13 PASS / 1 FAIL. All 15 groups
execute on unchanged v2 source. Build reuses 409 targets and compiles 16,
with 82 warnings and zero compiler errors. The original failure is retained.
Complete EDIT/WRITE and the fixed OMP marker stay open; formal acceptance
increment is zero.

## Exact delivery

Execution packet:
`D:/workspace/env/cache/ara-port-candidates/edit-write-shared-host-execution-v2-20261008`.

- Candidate manifest SHA256:
  `10debf712cffc3b9bc664195a70243f38635419de9e93fd66418e0dfe5218812`.
- Delivered-source manifest SHA256:
  `06bab8e244ad00be470f140e8e3d7227a23a4e7f4ec04ef9205b9f9176fc2e23`.
- Unchanged Git index SHA256:
  `89d34e26be97c90a14680d44ace937e6b1ae5a86e8ee7b46dd6b90718a922475`.
- Root verified all 15 live before pins, 15 candidate after pins and seven
  unchanged baseline pins before copying. Original live bytes are retained in
  the execution packet's `before/`; no commit, push, reset or stash occurred.
- Scoped `rustfmt --edition 2024 --config skip_children=true` on the 15 exact
  paths: exit 0, **1.124 s**. Eight paths have formatting-only delivery deltas.

## Material SOURCE repairs and independent review

The preserved v1 review requests changes for inline diagnostics incorrectly
depending on a deferred queue, and a generated-file fixture using unsupported
`.txt` slash comments. v2 independently supplies the same Session reduction
ledger and changes only that fixture target to `.go`. Original assertions and
production generated-file guard remain intact.

Reviewer: independent Codex `/root/finish_edit_renderer`, using
`ara-git-review` and `ara-rust-core-review`.
Report: `D:/workspace/env/cache/ara-port-candidates/edit-write-shared-host-review-20261008/SOURCE-V2.md`.
SHA256: `84195da67c4d808e9ab2c6f89e8df77ad9d1a0730db124fee98a079aa6ad5f5e`.
Verdict: **SOURCE APPROVE**; 15 physical candidate/delivery/before mappings,
seven baseline pins, index and all eight formatting deltas checked.
SOURCE is not runtime acceptance.

## Reproducible original module gate

```powershell
python -X utf8 scripts/verify_edit_module.py --output 'D:\workspace\env\cache\ara-port-candidates\edit-write-shared-host-execution-v2-20261008\gate' --include-write --delivery-manifest 'D:\workspace\env\cache\ara-port-candidates\edit-write-shared-host-execution-v2-20261008\delivered-source-manifest.json' --manifest-sha256 06bab8e244ad00be470f140e8e3d7227a23a4e7f4ec04ef9205b9f9176fc2e23
```

The existing runner validates delivery before Cargo, derives artifacts from
the selected groups, preserves the original 14-group default, and adds WRITE
tools for 15 distinct groups. Native WRITE is already in `journal-failure` and
runs once. Cargo/Rustup/target stay on D, jobs=2, profile/features unchanged.
Module receipts, executable hashes and full source pins belong in `gate/`.

## Remaining original acceptance obligations

Original EDIT 92 mapping rows plus five inspection contracts still apply to
the same delivered snapshot. Production ACP transport, LSP client/server,
active plan/vault, extension initialization/permission/registration lifecycle,
scan-cache owner, Unix denial/symlink and the other full WRITE protocols remain
required. Controlled supplier observations do not prove those services are
installed. Full workspace/clippy/dependency gates remain milestone obligations.
Bounded CAS write/edit/readback and cross-process same-Session recall were
pending after v2; they pass on v3 as recorded below. Final POST follows the
focused repair execution; it does not accept a complete parent.

## Concentrated URI repair v3

Root and the independent reviewer inspected the original failure at
`tests/edit.rs:532`: `local://PLAN.md` returns `Unknown protocol: local://`.
The earlier unknown-scheme gate prevents native local/vault paths reaching
the later shared policy. Fixed `tools/write.ts:1117-1218` supports the local
sandbox; changing the fixture would conceal the production defect. The same
branch also validates selectors without passing its peeled target to the
policy, which could create a suffixed filename. Both are repaired together.

The repair changes only `ara-tools/src/write.rs` and observations in the
existing ACP coarse group. Original assertions, Host registered/read-only/
Removed precedence, unknown-scheme rejection, root/escape/plan checks and ACP
exclusion stay intact. New observations inspect real file bytes and absence
of suffix files, denied effects and ACP events; no new test entrance is added.

- Candidate packet: `D:/workspace/env/cache/ara-port-candidates/edit-write-shared-host-uri-repair-v3-20261008`.
- Candidate manifest SHA256: `b1e0314ae54a2065f5809552125bf9d2f1e265064a0fd73e14d03f3b0f99750f`.
- Execution packet: `D:/workspace/env/cache/ara-port-candidates/edit-write-shared-host-execution-v3-20261008`.
- Delivered manifest SHA256: `b981f6aabfdbf594a681926f4070a11f121f0f405cc4ebce5fe3dbfebae489a7`.
- Root checks all 15 existing delivered paths and the two candidate before/
  after pins; only two paths change. Scoped fmt PASS, **0.593 s**.
- Independent SOURCE repair report: `D:/workspace/env/cache/ara-port-candidates/edit-write-shared-host-review-20261008/SOURCE-REPAIR-V3.md`.
  SHA256 `dd42205ed373d2b90d387dc50da7a76d5a9c88898a5c033407d5c362c721f44d`,
  **APPROVE**. Its scope analysis finds four focused original groups sufficient
  for this repair. Unchanged registered URI branches need no additional run
  for this delta; `host_uris` was not run, and full URI acceptance is not claimed.

The selected build and original groups `engine`, `write-tools`,
`journal-failure`, `retained-repl` finish **PASS** on unchanged v3 source:
**17/17** cases (engine14 + three single coarse cases), no ignored cases.
Build **287.726 s**, test bodies **11.947 s**, total **305.482 s**; 409 targets
reused, eight compiled, 69 warnings, zero compiler errors. The Native coarse
receipt includes the nine original WRITE modes and closure/cancellation
boundaries. Its stderr contains intentional caught panic/failure sentinels;
the original assertions and final test result pass. The other v2 receipts
retain their actual old source/executable identity and are not v3 reruns.

## Actual bounded configured CAS task

Receipt: `D:/workspace/env/cache/ara-port-candidates/edit-write-shared-host-execution-v3-20261008/real-cas-receipts/20261008T134633Z/receipt.json`.
SHA256: `f88b68ac78af270dae60c9ec694ac3d94af3ab24c015063547b247b3d5d655d7`.
Production debug `ara.exe` SHA256:
`099959d32bbdaa1da1c93d8e40776a26e4551a68a0c2c6481e316ea7018df934`.

The currently enabled Manager configuration and live `/models` catalogue
confirm `deepseek-v4.1-flash`; protocol is `openai-completions`. Local Manager
OMP-management gateway lookup fails with `URLError`, so execution uses the
CAS endpoint configured in Manager. This does not prove the local gateway.
Keys are supplied through the environment, omitted from configuration files
and redacted before output persistence.

Two process runs pass with the same compiled source/binary: task **10.423 s**,
six model calls and six tools; restart **1.961 s**, one model call and zero
tools. Bounds: task6/recall2 calls, 120 s/run, 2048 tokens/call. The task writes
item amounts with total zero, reads it, edits the total and reads it again.
The actual file and last successful read both contain
`{"pen":16.5,"book":34.5,"cup":4.25,"grand_total":55.25}`. The requested
write-before-edit sequence and post-edit readback are confirmed in receipts.

Session `01a11bc4-1073-7637-99d5-1bb789b2e5e3` is retained across processes.
Without tools, the restart returns original CSV facts: pen quantity6, book
unit price11.5, cup quantity1. The persisted Session has two user messages,
seven assistant messages and six tool results. Root independently checks CSV,
JSON, public readback, Session and source/binary identity in
`root-real-task-check.json`. Reported cumulative usage: input5203,
output690, cacheRead23040, total28933 tokens. Reasoning reports137 tokens in
two messages; five messages omit that field, so their values remain unknown.
Cost is unreported, not zero.

## Final independent POST and decision

Report: `D:/workspace/env/cache/ara-port-candidates/edit-write-shared-host-review-20261008/POST-V3-FINAL.md`.
SHA256: `15e0a218a0cb334b51811cf7dd320d0c5627678230eabb0c3a4c4fa02b886dbd`.
Reviewer: independent Codex `/root/finish_edit_renderer`.
Verdict: **APPROVE the bounded shared Host consumer chain and direct URI repair**.
Root accepts that bounded scope after checking the actual outputs, artifact,
Session, delivered manifest and unchanged index. Full parents remain OPEN.

The reviewer reads all four original stdout/stderr pairs, both CAS JSON event
streams and the complete persistent parent chain. All 15 delivered mappings,
1589 build source pins, six baseline pins, six executable artifacts, four
executed artifacts, index and CAS source/script/artifact identities match.
Formatting deltas preserve behavior and every original assertion. The final
module references its build map; no second full module map is invented.

Actual recorded stage time: both builds **1129.906 s**, both test bodies
**46.989 s**, both module executions **1196.082 s**, real task runs **12.384 s**.
Compilation is 94.5% of the two module executions. The repair compiles eight
targets instead of the first batch's 16, executes four required groups and
retains the original failure; no full 15-group rerun is claimed.

Next: the real LSP supplier/startup-factory batch has independent PRE approval
and a D-only implementation is underway. It will reuse the original 15-group
gate after one coherent freeze. The source estimate of 3–6 hours is unmeasured
planning; it does not promise completion of LSP/EDIT or the complete ARA.
