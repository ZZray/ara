# Environment libraries and reusable skills — 0.2.7

Requirement: the standalone Node discovers, creates and reuses environment and
project knowledge using INDEX.md plus focused Markdown pages; it can create or
install reusable skills with resources, execute them through existing tools,
and attach authentic execution receipts. Knowledge guidance never expands the
Run's permissions or bypasses private-memory adoption. This is a product Host
delivery, separate from fixed OMP parity and shared ARA Core acceptance.

## Final local artifact and scope

Windows x86_64 static-CRT EXE: 5,349,888 bytes, SHA256
`81ec56dac455c8e20d7e2907a6f6767332bedb7748f73502a82732723c11eaf3`.
Immutable local candidate:
`D:/WorkSpace/tmp/handwave-environment-20261018/handwave-node-0.2.7-v2.exe`.
The coordinator recorded 17 implementation source pins in `source-pins-v2.json`.
Evidence root below means `D:/WorkSpace/tmp/handwave-environment-20261018/`.
The versioned [compact acceptance receipt](receipts/environment-library-20261018/acceptance.json)
preserves source/artifact hashes, checks and all actual-model trial outcomes
without copying private configuration or complete model transcripts into Git.

New environment_library/environment_tools/skill_library modules and embedded
creator/installer guides are wired through agent/config/execution/experience/
local_tools/main/transport. `knowledge_dir` can reuse an existing local library.
Existing human index text and content pages remain separate from managed notes.
Model projection receives current bounded directories, loading bodies on demand.
README, configuration example, knowledge index and engineering case HWN-032
document the contract. Earlier Web capability WIP has its own 0.2.6 evidence;
this point does not reclassify that earlier model/network acceptance.

## Executed verification

Commands use the independent `apps/handwave-node/Cargo.toml`, locked dependencies
and two build jobs. Debug target cache: `D:/WorkSpace/target/handwave-node`;
release: `D:/WorkSpace/target/handwave-node-release` with
`CARGO_TARGET_X86_64_PC_WINDOWS_MSVC_RUSTFLAGS=-C target-feature=+crt-static`.

| Check | Observed result |
| --- | --- |
| `cargo fmt --all -- --check` | PASS, `fmt-v2.log` |
| `cargo clippy --workspace --all-targets --all-features --locked -- -D warnings` | PASS, `clippy-v2.log` |
| `cargo test --workspace --all-targets --locked` | 201 PASS, 0 failed, 1 explicitly ignored public-network test; `rust-tests-v2.log` |
| `cargo deny check` | Advisories, bans, licenses and sources PASS; duplicate dependency warnings retained in `deny-v2.log` |
| `cargo build --release --locked --target x86_64-pc-windows-msvc` | PASS, 2m43s; `release-v2.log` |
| `tests/verify_environment_library.py` against final EXE and isolated signed Hub | 5/5 PASS, 55.107s, 32 controlled-model requests; `host-release-v3/result.json` |
| Existing `tests/verify_host.py --modern-hub --proxy --catalog-variants --max-seconds-total 240` | 27/27 PASS, 77.879s, 23 controlled-model requests; `host-regression-v2/result.json` |

The environment Host cases inspect actual schemas, native call/result IDs,
immutable note hashes, script output, source/installed license and resource bytes,
new-Session reuse after a Node restart, and retirement/revocation. Plan exposes
read-only tools, does not initialize these libraries, and rejects a forged
mutation call. A successful Run is not accepted as the user's Task automatically.
The existing Host regression independently reran against the exact same EXE SHA.

## Preserved failures and verification traps

`host-release-v1` exposed a real Windows canonical-path bug: probing the bare
verbatim drive prefix failed before checking complete normal path components.
The narrow repair checks every normal component with its complete root. A real
filesystem regression covers canonical cwd, both catalogs, note write/read and
hardlink rejection. Original failure artifacts remain intact.

`host-release-v2` passed the complete creation/execution/both-knowledge workflow
but failed an installation fixture's hard-coded LF expectation. Source and
installed LICENSE bytes were actually equal CRLF. The corrected fixture checks
source bytes = returned UTF-8 bytes = installed resource bytes for both license
and script; v3 passed with the same product EXE. No product expectation was
relaxed. An independent junction fixture's mixed path separators were also
corrected for cmd's mklink builtin; actual junction traversal rejection remains
mandatory. Earlier test/lint/build records, including failures, remain in the
evidence root.

## Actual-model trials and retained limits

All actual-model trials use the freshly verified Gateway catalog's
`deepseek-v4.1-flash` / `openai-completions` through a read-only SSH tunnel and
isolated signed Hub/Node. Production device grants and services are untouched.
Provider usage and conservative pre-admission UTF-8/output reservations are
different measurements; these trials do not establish a cost reduction.

| Trial | Result | Forwarded HTTP calls | Conservative reservation | Provider-reported total tokens | Seconds |
| --- | --- | --- | --- | --- | --- |
| `live-two-sessions-v1` | FAIL | 13 | 530,418 | 128,960 | 62.311 |
| `live-two-sessions-v2` | FAIL; reuse NOT_RUN | 16 | 831,961 | 219,560 | 76.478 |
| `live-two-sessions-v3` | FAIL; reuse NOT_RUN | 19 | 938,037 | 241,570 | 74.475 |
| `live-project-two-sessions-v4` | FAIL; reuse NOT_RUN | 16 | 934,592 | 254,692 | 87.817 |

V1's creation response consumed all 1500 allowed output tokens as reasoning and
finished with `length`, without a tool call or text in that response. The later
dependent reuse attempt reached the 300K per-trial reservation. Its exploration
also examined real home-directory entries, contrary to the intended temporary
task scope; the ordinary command tools run with the OS user's privileges, not
an OS sandbox. Subsequent trials give only the Node temporary home variables,
leave parent SSH identity/PATH unchanged, and skip reuse when creation fails.
This test isolation is not an access-control claim about the product.

V2 allowed 4096 output tokens and 16 calls but stopped before final settlement.
The actual script produced correct counts, the bundle had native execution
evidence and both knowledge scopes had observed notes; those effects do not
make a failed Run a successful trial. After independent review of this complete
chain and Config's 24-call default, v3 used a fresh isolated 20-call allowance
without replaying v2's effects. V3 hit its unchanged 1M conservative reservation
before final settlement. Original results are retained as FAIL.

Rather than continuing to enlarge budgets for this compound eight-part task,
the last selected integration trial narrows the natural task to project skill
creation/execution, project knowledge and new-Session reuse. Environment-global
write/read/restart/revoke remains covered by the actual-process Host tests;
autonomous real-model global-note consolidation is not accepted by that selected
project trial. Runtime source and the final product EXE have not changed during
these harness corrections.

V4 retained the 20/28 HTTP-call, 180/300-second and 1M/1.5M conservative-reservation
bounds. Its next 78,043 reservation would exceed the per-trial limit after
934,592 had been reserved, so the proxy explicitly denied it before forwarding.
The Run failed and reuse was correctly NOT_RUN. The coordinator stopped further
model calls. This representative actual-model gate remains OPEN; neither
existing artifacts nor controlled Host success is substituted for a successful
autonomous two-Session workflow. Remaining investigation concerns unnecessary
exploration/checking, task granularity and bounded context use; no unmeasured
efficiency improvement is claimed.

The final fixture supports a direct Python resource invocation relative to its
actual cwd, with the exact successful tool_end step_id and current bundle hash.
Equivalent successful read_file reads of the current SKILL.md/indexed note are
recognized alongside native library reads. Candidate commands/paths still need
human inspection against real output artifacts; the bounded recognizer is not a
general shell parser. Offline historical-receipt checks and negative cases were
executed without new model calls; none rewrote the failed trials as PASS.

## Acceptance boundary

Independent Codex `/root/environment_evolution_review` completed ara-git-review,
ara-rust-core-review and point-delivery-audit, including direct final-image tests,
source pins, real command/receipt/artifact inspection and the failed trials.
Its [retained report](receipts/environment-library-20261018/review.md) approves
the bounded SOURCE / controlled-Host implementation and explicitly does not
accept the autonomous two-Session point. The coordinator independently checked
final source/artifact hashes, Host results, correct log counts and unchanged
source logs, and agrees with that limited judgment.

Local implementation/source review and actual-process checks passed. The
mandatory selected actual-model two-Session gate did not pass, so complete point
acceptance remains OPEN. Production deployment, installed-device replacement,
Linux/macOS execution, arbitrary system package installation and autonomous
promotion of private memories are outside this local candidate acceptance.
The root bootstrap scan was stopped after more than 180 seconds without a
result; it is NOT PASS. Scoped modified-document link checks passed separately.
The coordinator privately checked 164 JSON files against the actual Gateway
credential with zero matches (`evidence-secret-scan.json`). Final process
inspection found zero remaining isolated trial Node/Hub or model-tunnel processes
(`trial-process-cleanup.json`). These are scoped checks, not a blanket secret or
security audit.
