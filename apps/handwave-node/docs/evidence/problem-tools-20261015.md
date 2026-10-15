# Rust problem-solving tools — 2026-10-15

Status: **accepted for this bounded Windows tool slice and local package**.
Linux GNU same-source unit/process suite also passes under a normal user.
This record does not accept production installation or full computer use.
Machine-readable hashes/results: [receipt](problem-tools-20261015.json).

## Scope and immutable identity

Base HEAD `42c3a3977906d7ed45f2e4b4857c4eaf59d2a8c9`, branch dev.
The Node subtree began clean. Root Cargo/Core/OMP WIP remains outside this change.
Pinned `pi-rust@8ec462672d90e62712d96074638efef89436cebe`, protocol, identity,
permission, memory persistence and original exact-edit contract are retained.

Changed Rust: new file_discovery, script_tools, system_capabilities; shared
runner/output and matching excerpts in local_tools; main/agent registration and
concise tool-use policy. Tests: verify_problem_tools and verify_problem_live.
No additional dependency or embedded interpreter was introduced.

Windows x64 static-CRT release: **4,567,040 bytes**, SHA256
`080bdb85771c68d297752b1a8887c2f78bb984519c82881e1b5d50f3383f05d3`.
Previous cloud-memory image:4,518,912 bytes; increase48,128 bytes (about1.07%).
Local package: `apps/handwave-node/dist/handwave-node-x86_64-pc-windows-msvc.zip`.
The README was updated and repackaged after runtime acceptance; binary identity
is unchanged. Existing public download links and running Node are not updated.

## Executed checks

From the independent Node workspace:

```text
cargo fmt --all -- --check
cargo clippy --workspace --all-targets --all-features --locked -- -D warnings
cargo test --workspace --all-targets --locked
powershell -File scripts/build.ps1
python -B tests/verify_problem_tools.py --binary <final.exe> --hub-binary <actual-modern-hub.exe> --output <fresh>/host
python -B tests/verify_problem_live.py --binary <final.exe> --model deepseek-v4.1-flash --public-hub https://118.25.103.226/handwave/ --output <fresh>/live
```

- Windows fmt, Clippy and **60/60** tests pass on final source. Tests include real
  Python, PowerShell, cmd UTF-8/quotes, exit7, missing interpreter, timeout/cancel,
  temporary cleanup, split output tails, PS param/using and native errors, and
  readiness-marked Python descendant cleanup. Legacy shell lifecycle tests pass.
- Actual Go Hub SHA `27c9b2657da185fcd42ba4764259f48ab503011fa4e3ebbbf229f5e748cb48f7`
  and final Rust EXE pass three controlled-model scenarios in15.724s: metadata
  file search, long Chinese-line content match, actual Python repair/artifact;
  exit7+stdout/stderr tails+bounded receipt; started Python cancellation with no
  delayed write. The fixture checks actual model tool_call IDs and returned
  tool receipts; it is not a real-model intelligence result.
- Linux source was copied through source-test.tar and hashes checked. The first
  default Cargo cache lacked pinned git data and failed SSL fetch. An offline
  available dependency cache then compiled the same source successfully. Running
  its test image as root produced57pass/1fail in the existing chmod000 fixture:
  root can still read the file. The identical image executed as normal
  `ara-verify` passes **58/58**, including real sh/Python and Unix symlinks.
  Linux release packaging, pwsh, ARM, musl and macOS were not run this slice.

Raw evidence is retained at
`D:/WorkSpace/tmp/handwave-problem-tools-20261015/`: Windows logs,
linux-tests.log(initial fetch failure), linux-tests-cache.log(root failure),
linux-tests-user.log(normal user pass), source-hashes.json, host and live.
The source-test.tar16 files match final Cargo/src bytes. Failed attempts remain.

## Actual cloud-model task

Read-only live model inventory verified exact ID `deepseek-v4.1-flash` and
`openai-completions` before one trial. Temporary Node used public HTTPS HandWave;
root controller stayed behind strict-host-key SSH. Only the fresh device was
approved, then revoked (confirmed). No cloud configuration/service changed.

Task: diagnose a Python report from synthetic request/error logs, repair source,
execute it and check a derived report. No fixed tool sequence was required;
run_script execution was required. Budget:one Run,10 provider-call reservations,
180seconds maximum; failed trial replay was disabled.

Result: **PASS**, model Run18.821s, total24.150s,8 reservation events. Program
correctly initializes total to0 and parses integer duration only for OK records.
It produced `{ok_count:3,total_ms:42,error_count:1}` with actual
`REPORT_OK total=42` stdout. Independent Python reexecution confirms behavior;
input hash remains unchanged. Repaired source SHA
`060c285ad1e47f9b428ac71d42f7f7d5cd9f88ae93f30b885aa509071c9b0771`,
report SHA `4ed394d30c7479f28de05e3610f645365052896d5b96d26458cc23bbcdab5d31`.

One edit_file failed because LF old_text did not exactly match CRLF source.
The model inspected bytes, rewrote code, then executed and independently checked
its output. This is successful recovery, not zero-failure execution. find_files
and system_info are controlled-host accepted but were not chosen in this live
trial. Supplier tokens and actual provider HTTP request counts are unknown;
reservations are not HTTP receipts. No comparative speed/token percentage or
broader intelligence benchmark is claimed.

## Independent review and remaining boundaries

Independent Codex `/root/rust_tools_review` reviewed the six Rust files and both
process fixture designs, then verified Windows60 logs, Clippy/build, image hash,
controlled/live artifacts and source archive. Found P2: PowerShell initialization
before source invalidated param/using. Separate bootstrap and two actual source
syntax regressions close it. No remaining Windows blocking finding was reported.
Root-permission Linux failure was correctly kept open until the normal-user
rerun. Main agent inspected key code, actual artifacts and review evidence.

The tools retain OS-user authority. Neither tool selection nor project rules are
an OS sandbox. PowerShell uses child-process Bypass without changing machine
policy; Group Policy still applies. Cancellation cannot roll back completed
writes. Computer-use observation/action/vision is unimplemented; the audit and
next bounded slice are in [tool architecture](../knowledge/problem-solving-tools.md).
Long-session journal, cloud on-demand search/outbox and personal principals stay
in their existing separate plans. Dependency security audit was not executed;
no dependency/lockfile changed.

## Final package and documentation readback

Final ZIP3,450,267 bytes, SHA256
`d283ecb7d63844f9a42f8335b7b227d7e62f68b34cbc289eaeffcb3a208ef00e`.
Archive image hash matches the tested EXE; README matches current source; the
allowlist contains no live node.conf, key or state. Local Linux source package
`handwave-node-source-91e495059de34ea5.tar.gz` is309,482 bytes, SHA256
`91e495059de34ea57d6116be91c39eaf1606a114c13acedd7a9fcf03b5d02190`.
Its Cargo/source/test bytes match the tested snapshot. The installer script is
unchanged; no production download URL was moved to this new archive.

Changed-page relative links and source-hash stability pass. Full root bootstrap
document traversal did not finish within30 seconds; it is NOT COMPLETED rather
than a pass. Independent reviewer subsequently accepted normal-user Linux58 and
final documentation. Main agent completed the ZIP and source archive readback.
