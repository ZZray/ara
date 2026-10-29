# OpenAI Chat Native initial/final policy — 2026-10-08

**WIP. Corrected v3 SOURCE approved; 15/15 selected tests and actual CAS pass.
Final independent deterministic and CAS POST pass.
No complete Chat, provider, terminal or OMP acceptance.**

## Requirement and scope

Fixed OMP `596f2da7101178214aa27a753529d15e6b7ad91d`.
Fixed `packages/ai/src/providers/openai-completions.ts:1706–1831` prepares
Native tools and the requested choice using initial policy, then resolves
history/reasoning/max-token compatibility from the surviving wire choice.
`openai-shared.ts:878–943` supplies the reasoning/choice policy predicate.

The original four delivered paths:

- `crates/ara-ai/src/providers/openai_completions/actual.rs`:
  initial controls, repeated original tools getters and Native map/some/filter,
  named narrowing and absent/empty/forced/auto guards, final compat consumers.
- `crates/ara-ai/src/reasoning.rs`: reuse the existing resolver for initial
  policy and final encoder; preserve typed preparation and other API encoders.
- `crates/ara-ai/tests/openai_http.rs`: attach Native observations to three
  existing coarse groups without new test entrypoints.
- `crates/ara-ai/tests/support/actual_completions_history.rs`: original receiver,
  access ordering/throws, sentinel vs explicit empty tools, all represented
  choices, history/reasoning/max-token controls and actual HTTP/rejection paths.

Current dirty base: `dev`, HEAD `20aa0e2ef9872f633ba2671376cdf778477e7115`.
Index before/after SHA256:
`89d34e26be97c90a14680d44ace937e6b1ae5a86e8ee7b46dd6b90718a922475`.
Root compares every live baseline and candidate hash before application;
the retained `/new` EditStore ownership fix remains. No commit/push/deployment.

## Delivered snapshot and review

Packet: `D:/workspace/env/cache/ara-port-candidates/chat-next-batch-execution-20261008`.
Delivered manifest SHA256:
`bd19746c3fb340ab3fa52742903e7c5d8db6d2d22b1d06adc0bd6cca6125c54f`.
One scoped fmt: exit 0, **0.565 s**. Independent Codex reviewer
`/root/finish_edit_renderer` applies `ara-git-review` / `ara-provider-review`:
SOURCE approve, material issues **0**. Three files are byte-identical to the
approved candidate; the history helper differs only by formatting.
Review: `chat-next-batch-source-review-20261008/DELIVERED-SOURCE.md`, SHA256
`28cfa0a1c3f00c0a5ed9e6a72ccc08fff8e3ae6c35e63b591307d7639d20e121`.
Available structural graph generation is 2026-10-03 and does not track these
new paths reliably; actual fixed/live/candidate source is the review evidence.

## One affected execution gate

`python -X utf8 D:/workspace/env/cache/ara-port-candidates/chat-next-batch-execution-20261008/run.py --output D:/workspace/env/cache/ara-port-candidates/chat-next-batch-execution-20261008`

The D executor imports the existing module receipt runner. Actual Cargo command:

`cargo test --locked -p ara-ai -p ara-cli --all-features --test openai_http --test e2e --no-run --message-format=json`

It executes the full original `openai_http` module once, including all five
SOURCE-named groups, plus three original CLI process groups:
`required_chat_reasoning_replays_tool_and_text_turns_after_restart`,
`tool_task_produces_file_and_receipts`, and
`repl_clear_and_new_keep_retained_edit_snapshots`.
Original runner records source/executable hashes, expected names, counts,
exit codes, stdout/stderr and per-stage time. Executor SHA256:
`e7355fb32c2d65c3e4fc227fca80dd627285d473c14d658326f8cf51583596dd`.
Cargo home/Rustup/target prefix stays on D, jobs=2, same profile/features.
Full workspace/Clippy/dependency gates are deferred to a stable milestone;
no manifest/dependency change is present in this batch.

### Preserved first execution and consolidated repair

v1 build PASS **309.835 s**, 404 reused / 11 compiled. Full HTTP module:
**53 passed / 1 failed**, **34.902 s**. CLI reasoning replay and tool task
failed; retained REPL passed. Total **349.460 s**, source unchanged. Original
receipts remain in the v1 packet.

The Native policy fixture inspected terminal wrapping instead of the original
exception in the Host failure receipt. Its correction retains the Error/no-POST
checks and verifies exactly one original TypeError/detail through that receipt.
The Bash fixture now checks exact stdout plus fixed wall-time notice using
journal wallTimeMs, and checks identical HTTP content. Fixed `bash.ts:504–509`
SHA256: `a70c50c2fe871aa4c700b84fc28959f4fc9a2ddde44ecb239d2a558a68d392da`.

Actual controlled CLI diagnosis confirms a real explicit reasoning-mode gap:
four requests and three successful processes retained the thinking journal and
sentinel file, but omitted reasoning_content from tool history. The repair
adds the complete existing Native compat tuple only for explicit true in
`main.rs` and `daily_model_config.rs`, and preserves it in the local materialized
policy plus any existing whenThinking object. Core/history/default catalog
behavior is unchanged. Original raw CLI reasoning/restart/no-repeat-write/
non-replay assertions remain. The existing daily selection group also observes
original Native messages through an existing HostModel reference donor, with
opposite base/whenThinking flags and None/false/true inputs, including a
text-only assistant's required empty reasoning_content.

v2 compiled for **102.616 s** and failed with E0308: the test matched a
ValueError before dereferencing the receipt's Arc carrier. No groups ran.
Independent review additionally found an incorrect ForAll compat-key spelling
and a fixture using an unsupported authored-config field. v3 repairs all three
in one batch; the contributor configuration allowlist is preserved.

v3 packet: `chat-next-batch-execution-v3-20261008` under the same D cache root.
Eight paths, three delta paths from v2; one scoped fmt **0.436 s**, index unchanged.
Delivered manifest SHA256:
`bbddabfd2338ee2ea2006623cfcae5cb8c4c1f3b454df4124cf424462a80e307`.
Independent reviewer `/root/finish_edit_renderer`: corrected SOURCE approve,
material issues 0. Review: `chat-v3-combined-review-20261008/REPORT.md`.
One selected build includes the original `openai_http`, `e2e` and
`daily_model_config` artifacts. Execution selects the failed original policy
rejection group, two failed original CLI groups and full daily selection module.
The v1 HTTP 53 passes retain their v1 identities; no fresh full-module pass is
synthesized. Next strict/schema preparation overlaps execution and review;
its overlapping files must rebind to this final eight-file v3 baseline.

### Corrected v3 execution and actual-model receipts

Affected build PASS **244.087 s**, **410 reused / 6 compiled**, 59 warnings / 0
errors. Module PASS **252.192 s**, source unchanged. Original selected groups:

| Group | Passed / failed / ignored | Seconds |
| --- | --- | --- |
| Native policy rejection | 1 / 0 / 0 | 0.191 |
| CLI reasoning and same-Session restart | 1 / 0 / 0 | 1.270 |
| CLI tool task, artifact and journal | 1 / 0 / 0 | 0.546 |
| Full daily_model_config module | 12 / 0 / 0 | 2.855 |
| Total selected tests | 15 / 0 / 0 | 4.862 |

Build receipt SHA256:
`62a0daa4af9d4189926fbd341625c66f607c9dd550cf47c2e2b44a1fe5c37656`.
Module receipt SHA256:
`325968a6c6c5b37dfcf03ce917f07228f939f7657e074e2c1c62ac5cd1df8528`.
Both are in the v3 packet as `build-result.json` and `module-result.json`.
Compilation accounts for **96.8%** of module time; fewer build cycles and
selected artifacts are the immediate workflow optimization. No repeated test
or model call was needed to inspect these results.

Actual configured CAS `deepseek-v4.1-flash`, `openai-completions`:

- Edit/readback: **4 calls / 4 tools**, **6.252 s**, exact artifact
  `pen=13.75, book=23.0, cup=17.0, grand_total=53.75`.
- Same-Session restart recall: **1 call / 0 tools**, **2.621 s**, original values
  `pen_quantity=5, book_unit_price=11.5, cup_quantity=4`.
- Session: `01a11b16-1ae0-7267-abc6-8743ad18cc55`.
- Binary SHA256:
  `dfc1e1e03b0642ecb7ca720c0d8f30cd63e8ae8413bdfe35bfccf61856bf2984`.
- Receipt: `real-cas-receipts/20261008T103632Z/receipt.json`, SHA256
  `41942c02ec434b12173801b78f9c04bad0364c549f955e9ffc16be47448b8daf`.
- Root artifact, recall and identity checks: `root-cas-acceptance.json`, PASS.

This uses the private configured CAS route when the management route is
unavailable; it does not prove traversal through the management service.
Both Runs retain the 120-second, 2048-token and 6/2-call bounds with retry and
fallback disabled. The task proves ordinary Chat editing/readback and saved
Session recall; real reasoning mode, WRITE and specialized providers remain
unaccepted.

Independent Codex `/root/finish_edit_renderer` directly checked all four
stdout/stderr pairs, original names/counts, eight source/build pins and five
Cargo artifact hashes: deterministic POST PASS. It then directly checked the
actual CAS receipt, original tool events/artifact, answer-free cross-process
recall and persisted journal: bounded CAS POST PASS, zero source-map mismatch.
Same-batch report: `chat-v3-combined-review-20261008/REPORT.md`, SHA256
`53be24556f798aa3c1c78fca60c0b906a4db88539cf482da740a2235be847196`.
Root read the final report and verified the report/build/module/CAS hashes;
no test or model run was repeated. Earlier v1/v2 failures remain separate
original-version receipts.

## Mandatory remaining parity

Native Kimi K3 named→required exact predicate and Zai `tool_stream` remain
recorded under later provider priority. Zai requires final thinkingFormat=zai,
zaiReasoningEffortDialect, supportsReasoningEffort and a nonempty tools Array
(`openai-shared.ts:1210–1211,1256–1268`). Strict/schema/flavor/grammar/quarantine
and strict retry/session integration are the next coherent batch. Full Chat,
the unbound historical 455 denominator, complete provider remainder and full
terminal/TUI remain OPEN. Tests exercise their stated observations; they do
not substitute for these full-parent contracts.
