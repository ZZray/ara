# Reviewed experience / self-evolution slice — 2026-10-15

Status: **accepted for this bounded Node experience capability and local package**.
This is evidence-backed candidate/adoption/recall, not autonomous code rewriting
or model training. [Machine-readable results and hashes](evolution-20261015.json).

## Scope and exact delivery

Base `d565d62914c68d9791cb76982540af5e68184dd5`, dev branch. The Node subtree
began clean; root OMP/Core WIP is excluded. Pinned rpi, dependencies, route/grant,
identity, Task acceptance, ordinary memory contract and native storage ceiling
are retained. New experience.rs and its context-tool tests reuse existing Memory
states. agent.rs adds a current-Run evidence gate and optional provider-only
receipt ID directory. No second ledger/background learner was created.

Final Windows x64 static-CRT EXE: **4,602,880 bytes**, SHA256
`c3ff87a704d89b81bf4584057757efdd4be612d58583df185f4cf308326f1059`.
Increase over the prior tool release:35,840 bytes (about0.78%). Local ZIP:
`apps/handwave-node/dist/handwave-node-x86_64-pc-windows-msvc.zip`.
Existing public download URLs and persistent Node installation remain unchanged.

## Observable contract and test execution

A model can propose a conditional lesson only by selecting actual current-Run
failed→recovered tool pairs and active user sources (including a current
objective/steering source). The Host copies IDs/names/status/full redacted-output
hashes/bounded excerpts. Model-authored status/output is not evidence. At most
one candidate per Run is persisted. Tools returning success do not establish
Task acceptance or universal causal truth.

The existing exact controller commands inspect/adopt/revoke without a model
request. Only adopted/current experiences are searchable; source corrections
and revocations invalidate them. Candidates are excluded from accepted views,
retrieval and proposal-argument projections. The raw audit remains. An ordinary
assistant discussion may still mention a candidate; this never adopts it.

Executed in the independent Node workspace:

```text
cargo fmt --all -- --check
cargo clippy --workspace --all-targets --all-features --locked -- -D warnings
cargo test --workspace --all-targets --locked
powershell -File scripts/build.ps1
python -B tests/verify_evolution.py --binary <final.exe> --hub-binary <actual-modern-hub.exe> --output <fresh>/host-delivery
python -B tests/verify_context.py --binary <final.exe> --hub-binary <actual-modern-hub.exe> --output <fresh>/context-delivery
python -B tests/verify_problem_live.py --binary <final.exe> --experience --experience-smoke --model deepseek-v4.1-flash --public-hub https://118.25.103.226/handwave/ --output <fresh>/live-delivery
```

- **Windows80/80**, fmt and strict Clippy pass. New tests cover native pairing,
  forged/duplicate/missing/reversed IDs, name mismatch, text hashes and UTF-8,
  old optional schema, source currentness, explicit adoption/revocation,
  candidate projection/summary exclusion, canceled/snapshot failure/save failure
  with no active-store replacement, one proposal per Run, and complete escaped
  JSON8KiB output. The near-context-limit test preserves original projection
  bytes when optional ID metadata cannot fit.
- **Linux78/78** passes under normal `ara-verify` on WSL x86_64 GNU/Rust1.97.1.
  Offline native source compilation succeeds; extracted hashes match delivered
  Cargo/src bytes. Test image SHA is in JSON. This is not Linux release packaging
  or acceptance of macOS/ARM/musl/pwsh.
- Final actual Go Hub + EXE, controlled-model evolution: **5 scenarios pass**,
  24 requests over multiple Runs in10.043s. Actual Python exit7/recovery/file,
  full native output-hash/status provenance, forged/cross-Run rejection, restart,
  explicit accept/search/revoke without model HTTP, source invalidation, actual
  provider directory IDs and no raw/cross-Run directory persistence are checked.
  Controller acceptance here is a fixture action, not a human production review.
- Existing actual-process context regression: **15 scenarios pass** in58.257s on
  the final image, including rules/ownership, exact references, candidate
  injection, save failure, summary/restart and malformed/unknown-source/tool/
  cancellation/budget failures. No fixture result is called real-model learning.

## Preserved real-model failures and the resulting interface fix

Initial image `e47d8700de4d11ee550b8620bf91c8056ec7ac9a1be807516fe1de433498c753`
passed Windows76/Linux74 and controlled experience4/context15. Two bounded real
trials then failed and remain in the private evidence:

1. `live`: general report task repaired the program and generated a report but
   exhausted10 call reservations before a final response/proposal (20.220s Run,
   23.339s total). Original CRLF exact edits failed before successful recovery.
2. `live-smoke`: separate one-fault LF program was repaired and verified, but the
   model guessed labels such as call-run_script-1 instead of actual opaque
   protocol IDs. Strict proposals were rejected; no candidate existed. The Run
   consumed10 reservations (35.736s Run,39.282s total). Only tool API usability,
   not schema strictness, was changed after inspecting this evidence.

Both temporary approvals were revoked. No unknown-effect task was blindly
replayed and the10-call ceiling was not inflated. A short current-Run native
receipt directory now supplies the two exact IDs/names/error flags in provider
context, without output bodies, persistence or an extra call. collect validation
is unchanged. Directory metadata is omitted if it would exceed context budget,
without deleting original evidence. Independent review found and closed both
the full-JSON8KiB and optional-context-budget defects.

## Final actual-model verification

Read-only inventory verified exact `deepseek-v4.1-flash` / openai-completions.
Final image used public HTTPS HandWave with root control behind strict-host-key
SSH. One separate fresh-device LF one-fault task, maximum10 reservations and
180seconds, then **passes** in29.309s Run /32.779s total with7 reservations.

The real failed run_script and successful run_script are paired in mem-1,
status=candidate, adoption=null. The model copied actual directory IDs. It
repaired aggregation to convert only OK rows, executed and checked the report:
`{ok_count:3,total_ms:42,error_count:1}` and `REPORT_OK total=42`.
Independent Python execution verifies the repaired behavior; the input log hash
is preserved. Report SHA
`4ed394d30c7479f28de05e3610f645365052896d5b96d26458cc23bbcdab5d31`;
repaired source SHA
`ff57926a750b1ad04e1f048dae7490a84620841bc08ae8987640e5e7ed096355`.
The model attempted a redundant second proposal, which the per-Run limit
rejected; this is not zero-error execution. A successful final response did not
adopt either proposal or accept the Task automatically. The sole candidate
remained unadopted, and temporary device approval revocation was confirmed.

Actual supplier tokens and HTTP request counts remain unknown; reservations are
not confirmed upstream requests. These task timings are not a comparative
performance benchmark or a token-saving percentage. No automatic cross-Session
or cloud learning, Skill promotion or universal lesson correctness is claimed.

## Review, artifacts and boundaries

Independent Codex `/root/evolution_pre_review` reviewed source, schema, locks,
checkpoint/unknown-effect gates, provenance, projection and test designs before
and after fixes. Main agent inspected final actual receipts, native candidate,
report/source hashes and source identity. Final independent receipt readback is
recorded in JSON. Knowledge ownership and future gates are in
[reviewed experiences](../knowledge/reviewed-experiences.md).

Raw receipts are retained at
`D:/WorkSpace/tmp/handwave-evolution-20261015/`, including both failed live
trials, source archives/hashes, Windows/Linux logs, host-delivery,
context-delivery and live-delivery. Native histories, private credentials and
model payloads are not copied into Git. No dependency was added or lockfile
changed; a dependency security audit was not executed this slice. The tools keep
OS-account authority and cancellation cannot undo completed effects.


## Final archive and documentation readback

Main-agent ZIP readback matches the tested image SHA and current README; no live
node.conf or key is included. ZIP3,466,979 bytes, SHA256
`b9e9946b5e7337dfae21eca8810d348c6544a20b526624d6cf3b4fd7671f0454`.
Local source archive `handwave-node-source-5b9df47655687915.tar.gz` is327,466 bytes,
SHA256 `5b9df47655687915ee8057cf3985e574066d11eaf63f3ab5e85fed1b229863f2`. Cargo/src and fixture hashes
match the delivered tests; installer bytes are unchanged. Existing public URLs
were not repointed. Independent Codex readback accepted the bounded scope and
confirmed the real candidate's two output hashes against native tool results.

Changed-page relative links and source-hash stability pass. Root bootstrap
traversal did not finish within30 seconds and is NOT COMPLETED, not a pass.
This documentation limitation does not substitute for or erase the separate
executed Node/Host/model acceptance above.
