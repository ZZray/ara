# Compact Host code, context and memory slice — 2026-10-14

Status: **accepted** for this bounded independent Host slice and new packages,
after final release execution, archive readback and independent point audit. This record does
not accept full ARA/OMP parity, Ray browser login, personal-account memory,
or replacement of the installed node.

## Scope and delivery criteria

The independent `apps/handwave-node` Host retains pinned
`pi-rust@8ec462672d90e62712d96074638efef89436cebe`. The root repository contains
unrelated dirty work; no root Core port, commit, push or installed service is
changed by this slice. The delivery criteria are actual bounded search/read
effects, per-run project rules, preserved sourced user text, closed-turn model
compaction, candidate/adopted separation, explicit correction/revocation,
restart and fault behavior, and a measured standalone executable.

Implementation scope: `src/local_tools.rs`, new `src/project_rules.rs`, new
`src/context_memory.rs`, integration in `src/agent.rs` and `src/main.rs`, release
LTO setting, Rustls security patch lock update, corresponding process fixtures,
documentation and regenerated dependency notices. Existing transport, identity,
native transcript gates and other product installations are retained.

The memory store uses the existing controller/canonical-cwd scope. A shared Ray
QR does not identify people. Original references retain version, source ID,
hash and received-run provenance; received input is not proof of task success.
Summary output is derived data. A model may propose but cannot use its output,
brief or tool result as a trusted adoption command. Ordinary shell tools still
run as the OS user and can edit files; no OS sandbox or protection from that
same account is claimed.

## Preserved intermediate results and corrections

- Original final40 Windows: 4,727,808 bytes, SHA256
  `09a5c3394179ea2c664eadb7c80d34ced392a081fdc3a4c872f3068382c06bde`.
  Preserved copy: `D:/WorkSpace/tmp/handwave-final40-preserved-09a5.exe`.
- Same-source fat-LTO experiment: 4,329,984 bytes versus 4,727,808 bytes
  with thin LTO (8.4146% reduction). Feature additions change the final size;
  this experiment is not the delivered feature executable.
- Intermediate feature release `19b81da149a1cd33123100ea490aa311ae1be8eec2283766ab4c7546fd3c338d`:
  4,492,288 bytes. Windows Host27, Smart4, Fault7 and Context14 passed at
  `D:/WorkSpace/tmp/handwave-{host,smart,fault,context}-final-19b8-20261014/result.json`.
  This is not the final compatibility-fix binary.
- The first actual public HTTPS/model trial on 19b8 failed after 36.155 seconds
  at summary response admission. The project-rule and first file-read checks
  passed; temporary device approval was revoked. Receipt:
  `D:/WorkSpace/tmp/handwave-context-live-final-20261014/result.json`.
  The old error did not record summary content kinds, so its exact cause is
  not retrospectively proven. Ordinary responses from that trial contained
  `thinking` plus `text` even with ThinkingOff.
- The narrow compatibility fix permits Thinking alongside complete Text,
  discards Thinking before strict-JSON summary parsing, and still rejects
  Image/ToolCall, incomplete stop reasons, errors and empty text. Text and
  Thinking streams have independent 24 KiB and 64 KiB limits. Summary result
  diagnostics record only stop reason, content kinds and error presence;
  provider error diagnostics are bounded and grant-redacted. Thinking bodies
  are not stored as summaries or diagnostic content.
- The next Windows release `23f39764c76cdee20632b3fff4c7d3656a6340a74466f621e2fb6f0e07eac06a`
  is 4,494,848 bytes. Host27, Smart4, Fault7 and Context15 pass, but its real
  cloud trial fails after 32.401 seconds at strict JSON parsing. This time
  safe diagnostics confirm `stop`, `[thinking,text]`, and no provider error.
  Thus content-kind handling passes; schema/JSON admission still fails. The
  exact text was not logged, so Markdown fences versus schema mismatch are
  not distinguished. Temporary approval is revoked. Raw receipt:
  `D:/WorkSpace/tmp/handwave-context-live-thinking-20261014/result.json`.
  These failures prompted a summary-only JSON output request through the fixed
  rpi `Model.sampling_params` extension, with strict parsing retained, rather
  than extracting arbitrary JSON from malformed output.
- The first JSON-mode debug trial (`6559d467...576be`) failed after 27.542
  seconds with Serde `Data`, line1/column2794, 3744 bytes, object prefix true
  and fenced false. This distinguishes schema admission from JSON syntax;
  temporary approval was revoked. Receipt:
  `D:/WorkSpace/tmp/handwave-context-live-json-debug-20261014/result.json`.
  A separate single-request diagnostic through the private HandWave gateway
  reused the current prompt and reconstructed closed turn, without tools or
  node effects. In 13.031 seconds it returned six candidates whose source-ID
  elements were integers, while the other required field types were correct.
  Only whitelisted structural types/counts were retained at
  `D:/WorkSpace/tmp/handwave-summary-schema-probe-20261014.json`; response and
  Thinking bodies were not saved. This is a new reproducible schema violation,
  not proof of the exact content of previous failed responses. The prompt now
  specifies a complete candidate example and exact string source IDs; the Host
  continues to reject numeric IDs rather than guessing a source mapping.
- The complete-source-ID prompt debug trial (`cdadd77f...aaf9b3`) then failed
  after 40.122 seconds with `length` and only Thinking content: its 4096-token
  summary output allowance was consumed before Text. The fixed rpi adapter
  implements ThinkingOff as omission of reasoning effort, not a supplier-side
  disable. Temporary approval was revoked. Official
  [DeepSeek thinking controls](https://api-docs.deepseek.com/guides/thinking_mode/)
  document the explicit switch and default enabled behavior. A single bounded
  diagnostic with `thinking.type=disabled` completed in 5.874 seconds, with no
  Thinking and valid string source IDs, at
  `D:/WorkSpace/tmp/handwave-summary-thinking-probe-20261014.json`.
  The response also contained one unused extra root field. Independent review
  distinguished hard requirements from the chosen root `deny_unknown_fields`
  policy: unknown root metadata can be ignored without storing or granting it
  authority. The final narrow compatibility change keeps required known fields,
  duplicate/type/source validation and strict candidate objects, discards extra
  root fields, and limits the Thinking switch to the exact verified model ID.
  This is a declared schema-policy adjustment, not numeric-ID repair or JSON
  substring extraction. Controlled tests verify authority-like extras cannot
  alter adoption state or enter the saved working data.
- Independent review corrected sole-large-old-turn retention and rejected
  generic candidate-string replacement that could corrupt protocol fields.
  Controlled fixture corrections distinguish sidecars from native checkpoints,
  keep rule sentinels out of prompts, and wait for deliberate cancellation
  handlers to settle. These changes do not weaken application assertions.
- Rustls was upgraded only from 0.23.45 to 0.23.46, including checksum, following
  RUSTSEC-2026-0346 through 0349. Final dependency audit at
  `D:/WorkSpace/tmp/handwave-context-final-deny.log` passes advisories, bans,
  licenses and sources. Regenerated notices cover 271 packages and 524 entries.
- Linux's first root-run permission unit failed because root can read mode000
  files. The same test executable subsequently passed all24 tests as UID65534;
  no test was skipped or assertion lowered. Linux toolchain lacks rustfmt and
  clippy; that platform limitation remains distinct from Windows lint results.

## Verification commands and boundaries

Windows release uses `CARGO_BUILD_JOBS=2`,
`CARGO_TARGET_DIR=D:/WorkSpace/target/handwave-node-context-release` and
`CARGO_TARGET_X86_64_PC_WINDOWS_MSVC_RUSTFLAGS=-C target-feature=+crt-static`:

```text
cargo build --manifest-path apps/handwave-node/Cargo.toml --release --locked --target x86_64-pc-windows-msvc
cargo test --manifest-path apps/handwave-node/Cargo.toml --workspace --all-targets --locked
cargo clippy --manifest-path apps/handwave-node/Cargo.toml --workspace --all-targets --all-features --locked -- -D warnings
python -B apps/handwave-node/tests/verify_host.py --binary <final-binary> --hub-binary <isolated-hub> --modern-hub --catalog-variants --proxy --output <fresh-output>
python -B apps/handwave-node/tests/verify_smart.py --binary <final-binary> --hub-binary <isolated-hub>
python -B apps/handwave-node/tests/verify_grant_terminal.py --binary <final-binary> --output <fresh-output>
python -B apps/handwave-node/tests/verify_context.py --binary <final-binary> --hub-binary <isolated-hub> --output <fresh-output>
python -B apps/handwave-node/tests/verify_context_live.py --binary <final-binary> --output <fresh-output> --model deepseek-v4.1-flash
```

Controlled suites exercise real node/Go Hub processes and actual files with a
controlled provider. The real trial uses public HTTPS HandWave for node/model
traffic, an SSH private controller and a fresh temporary device, revoked in
cleanup. It bounds four model runs to three calls each, including summary, and
180 seconds overall. Model identity and protocol are checked against the current
catalog. Missing usage is unknown, not zero. Neither proves browser interaction.

## Final-source validation before release

The final source snapshot is pinned at
`D:/WorkSpace/tmp/handwave-context-delivery-source-pins.json` (20 Cargo/source/
Python fixture files). It is a dirty Host snapshot on repository HEAD
`20aa0e2ef9872f633ba2671376cdf778477e7115`, not a root-tree delivery commit.
Key source SHA256 values:

- `src/agent.rs`: `20fed72273c076199f0603bc004b8f60e6217d80e502cb16c0c92fb26cabba9b`
- `src/context_memory.rs`: `0d9f697ca54a8a67df0bf4ccbe104308724e375850bd94e1d5a03a37bd5b9328`
- `Cargo.lock`: `6e7f856f537b85dfef56ae29063418ed450a9a93d49be5df796ecb81405b52bc`
- `tests/verify_context.py`: `7e041dc884dda3d4b58be496d20827e6bb74b0a60e6910e0a966abec2974874f`

Final-source Windows rustfmt, clippy (all targets/features, warnings denied)
and all28 unit tests pass; actual test execution is0.60 seconds, distinct from
compilation. Original logs and commands are in
`D:/WorkSpace/tmp/handwave-context-summary-final-contract-20261014/result.json`.
The same-source debug EXE
`bcea927fc2d3aa3e0d6c69be287a285df4598cedab283c4005238456843c8c34`
passes Context15 in59.368 seconds and actual public HTTPS/model trial in39.458
seconds. The real trial uses10 model calls including one summary and confirms
Stop/Text-only summary, unchanged original reference hash, a successful sourced
memory_propose, explicit trusted adoption and one write_file recall without
reading either original file. Temporary approval is revoked. Raw receipts:

- `D:/WorkSpace/tmp/handwave-context-extra-bcea-20261014/result.json`
- `D:/WorkSpace/tmp/handwave-context-live-bounded-debug-20261014/result.json`

These debug results validate the source before release; final executable and
package acceptance are recorded separately below.

## Final release execution

| Artifact | Bytes | SHA256 |
| --- | ---: | --- |
| Windows x86_64 static-CRT EXE | 4,497,920 | `c9bfae85d2641008409d1db7f587994e36733223b9317989774143b09ebcdf97` |
| Linux x86_64 GNU executable | 5,266,000 | `a2059b86cdaaf07f43e077638ae3ac0ba5a68957929ba2252a8885f13c57546b` |

Windows release build completes in120 seconds. Its EXE is4.8625% smaller than
final40 despite the added features. This is size evidence, not a latency
benchmark. Linux all29 unit tests pass as UID/GID65534, including actual Unix
permissions. Windows all28 units and lint are recorded above.

All controlled groups are executed again on the final release binaries:

| Platform | Host27 | Smart4 | Fault7 | Context15 |
| --- | --- | --- | --- | --- |
| Windows c9bf | PASS47.198s | PASS7.653s | PASS15.933s | PASS82.925s |
| Linux a205 | PASS28.656s | PASS2.849s | PASS14.920s | PASS48.855s |

Windows receipts are
`D:/WorkSpace/tmp/handwave-{host,smart,fault,context}-delivery-c9bf-20261014/result.json`.
Linux receipts are
`D:/WorkSpace/tmp/handwave-linux-context-json-20261014/{host27,smart4,fault7,context15}/result.json`,
with build/unit commands in `checks.json`. All ten context fixture-error
receipts per platform are empty. Linux source comparison matches snapshot
`d33f04fa0e1290fe29c780423c1e391fc39dd083122e189fcadc9fea6c969cb4`.
GNU Linux testing does not establish macOS, ARM, musl or older libc support.

The first final-release real trial preserved a49.694-second FAIL at
`D:/WorkSpace/tmp/handwave-context-live-delivery-20261014/result.json`.
It passed summary, source preservation and candidate adoption, wrote the exact
original marker, then read back its newly written `recalled.txt`. The prompt
prohibited rereading the original two files, but the test's stricter assertion
prohibited every extra tool. `readback-analysis.json` records this distinction;
it is not evidence of a product memory failure. Independent review approved
clarifying the prompt to prohibit reading back the new output, keeping the
strict assertions unchanged. The product source and release binary do not change.
The final20-file pin manifest is
`D:/WorkSpace/tmp/handwave-context-delivery-final-source-pins.json`, SHA256
`200b402921b747fb37338b87f030ed3065c15d089861a2e173afaf86e8a037d6`;
only the live fixture changed from the prior manifest, to SHA256
`4212f97b480ef37dae23cc0cd2edbeae47dd035c3317df2f8d9b548cd0c4d000`.

The fresh exact-instruction final-release trial passes all four checks in30.514
seconds on the same Windows c9bf executable:
`D:/WorkSpace/tmp/handwave-context-live-delivery-exact-20261014/result.json`.
Its controller receipts show nine Agent calls plus one summary call; summary
returns Stop/Text without Thinking or provider error. Original reference hash,
successful sourced proposal, trusted adoption and exact one-write/no-reread
recall all pass. The temporary device approval is revoked. This verifies the
specific `deepseek-v4.1-flash`/`openai-completions` route; it does not establish
every catalog model or Linux real-model compatibility.

Root bootstrap validation ran and reports the same five unrelated pre-existing
links in root plan, roadmap, feature ledger and older read handoff. Its log is
`D:/WorkSpace/tmp/handwave-context-doc-bootstrap.log`; those files are outside
this change. The compact Host's own Markdown/local links and scoped whitespace
checks pass. Fixture evidence does not force kernel send-buffer saturation or
every successful-end/duplicate collision interleaving.

## Independent packages and review

New packages live under `apps/handwave-node/dist/context-memory-20261014`.
The old dist and installed executable are preserved; the installed Windows
file was rehashed and remains final40 (`09a5c339...06bde`).

| Package | Bytes | SHA256 |
| --- | ---: | --- |
| `handwave-node-windows-x86_64.zip` | 3,326,246 | `df157a7dca4b7ec08c1615896740544c0a6e9003abd96daad78a225966480104` |
| `handwave-node-linux-x86_64.tar.gz` | 2,471,194 | `c00db88071366c18bd9a6b6e5a159c63ddb1973445f47582e5ee2c8ad4e0cfe4` |

`package-receipt.json` records552/551 payload files, including545 license
payload files per platform. Physical file count is distinct from the271-package/
524-notice manifest count. Every archive member is read back and SHA-compared
with its staging source; all license copies match their source. Linux program
mode is0755. Only program, README, placeholder template, notices/licenses and
Windows launcher are included. Real node.conf, identity and Session history
are excluded. Packaging does not replace or restart an installation.

Independent Codex reviewer `/root/context_review` performed PRE and POST source
reviews, directly checked source hashes and debug/release receipts, and reviewed
the packaging allowlist/readback code. `/root/context_verify` independently ran
the actual Windows/Linux process matrices. The parent read the critical source,
reproduced cloud failures, ran final real-model tests and inspected artifact
effects; subagent completion alone was not treated as acceptance. Final point
audit: **APPROVE / accepted**, by independent Codex `/root/context_review`.
The reviewer independently rehashed all20 final source files, inspected actual
live reference/adoption/output artifacts and every552/551 archive member.

## Remaining limits

Native raw checkpoints retain the 256-message/16 MiB gate and full rewrite cost.
The 192 KiB working-context trigger is a byte heuristic, not a tokenizer or
verified model window. Sidecars are bounded to 4 MiB, 512 references (64 KiB
each), 256 memories (8 KiB each), and a 16 KiB summary. Withdrawal clears the
prior working view, including unrelated older context, while raw evidence stays.
No arbitrary-length session, vector search, binary-reference archive, personal
identity isolation, background self-improvement or measured speed gain is claimed.

Nonblocking observed model-output limitation: after a successful memory
proposal, the model once incorrectly stated that required proposal fields had
not been supplied. Actual tool_start arguments, successful tool receipt and
sidecar prove they were supplied and the proposal was saved correctly. Masked
historical candidate arguments in the working projection are the likely cause;
that attribution is not separately isolated. Trust the concrete tool receipt,
not that model self-report. No adoption or stored-source failure was observed.

At this historical checkpoint Ray authenticator login and all-service GUI
interaction remained unaccepted. The [2026-10-15 follow-up](gui-live-20261015.md)
records actual holder login and version-separated browser acceptance.
The installed node remains its existing version/configuration. Earlier automatic
approval rejected changing its private HTTPS configuration and restarting it;
that action was not retried through an alternate route. New artifacts are
delivered separately.
