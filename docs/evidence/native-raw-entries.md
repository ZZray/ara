# Grouped native raw entries — bounded WIP

Base `b1c06e26416a4d4c72cd2901b31f7a09070edfda`; fixed OMP
`596f2da7101178214aa27a753529d15e6b7ad91d`. This extends raw Session → Core →
REPL/RPC compaction. It does not accept any complete parent surface or phase.

## Requirement and source

- `packages/agent/src/compaction/compaction.ts:415–561`: native raw candidates,
  single reverse message budget, metadata/custom backtracking and turn starts.
- `packages/coding-agent/src/session/messages.ts:704–765,1117–1139,1193–1315`
  and its steering template: native custom/hook/branch/legacy projections and
  text/image ordering. Fixed-source objects, rather than a moving branch, were read.
- The existing fixed inventory records native OMP tests. This batch reuses
  grouped raw-entry input families and existing Rust module suites; test counts
  do not establish full parity.

## Delivered behavior and scope

Session owns real raw IDs, origins and zero/one/many model fragments. Core has
its own exhaustive 16-origin enum; the Host explicitly maps it without importing
Session into Core. Cuts use raw candidates, metadata backtracking, original turn
starts and one raw-message token estimate. A context-bearing group contributes
one summary source ID, regardless of its number of fragments. Metadata and
excluded Bash contribute no summary source; excluded Bash can still affect the
retention budget. Bash/legacy estimates remain text proxies.

Custom/hook text and images preserve ordered Developer plus User fragments;
image-only input retains its native attachment explanation. This preserves
context/reopen/history images in the deterministic RPC family. Image-bearing
summary input remains unsupported and is rejected before a Provider call; the
actual CAS compaction trial below is text-only. Steering uses the
fixed envelope. Verified Skill/LoopGuard records retain their specialized checks;
malformed reserved records cannot fall through to generic conversion. Verified
historical custom/LoopGuard projections can be summarized, while a genuine raw
Developer message remains protected. Branch/legacy summary User projections
neither create nor answer an actual prompt. Legacy custom Skill/steering User
projections still represent actual prompts.

Plural context/reopen preserves all fragments. Old single-message snapshots
explicitly return `MultipleMessageProjection` for multi-fragment records. RPC
history preserves native custom/hook/branch receipts. Generic custom/branch
records block synthetic tool recovery across actual context boundaries. New
checked native writes require raw metadata backtracking; readers retain safe
older V1 summaries whose kept User follows title metadata. The original
`session_name` regression was preserved. Summary folding/retry/fan-out continues
through the existing engine.

Delivered source/test/runner scope is 14 paths:

- `crates/ara-agent/src/compaction.rs`, tests `compaction_cut.rs`, `compaction_call.rs`.
- `crates/ara-session/src/lib.rs`, tests `compaction_projected.rs`, `skill_prompt.rs`, `retry_recovery.rs`.
- `crates/ara-cli/src/lib.rs`, `main.rs`, `rpc_host.rs`, new `native_compaction.rs`, tests `rpc_compaction.rs`, `e2e.rs`.
- `scripts/verify_native_compaction.py`.

## Module and final shared verification

Artifacts: `C:\Temp\ara-native-raw-entry-batch`. Root was the sole Cargo,
network, private-config and Git-write owner; independent agents reviewed source
and receipts. No per-detail mutation runs were added.

```text
python -X utf8 scripts/verify_native_compaction.py --module projection
python -X utf8 scripts/verify_native_compaction.py --full --output <artifact-root>
```

Final executed command used `RUST_TEST_THREADS=4` and
`--module account --full --output C:\Temp\ara-native-raw-entry-batch`.

| Module | Passed / failed / ignored |
| --- | --- |
| cut / summary | 20/0/0; 15/0/0 |
| projection / source | 18/0/0; 8/0/0 |
| Skill / notice | 9/0/0; 1/0/0 |
| reload / context / terminal | 8/0/0; 5/0/0; 4/0/0 |
| Host / REPL / synthetic account | 17/0/0; 94/0/0; 4/0/0 |

`module-20261002T072529Z/receipt.json` supplies only the 12 successful module
results: **203/0/0**, execution **24.464s**, compile **13.910s**. Its failed
backend is excluded. All 14 source hashes match that receipt's before/after
snapshot, the final gate and current files.

`module-20261002T072853Z/receipt.json` is the accepted final shared gate:
**1,542/0/20**, 121 result suites, backend **113.295s**, complete command
**120.816s**, `exitCode=0`, `sourceUnchanged=true`. Formatting, strict all-target/
all-feature Clippy, target/doc tests, fixed inventory, dependency policy and build
pass. Existing ignored cases and dependency warnings retain their limits. The
synthetic account module does not prove actual OpenAI authorization.

### Failed and interrupted checks

Initial custom/metadata fixtures assumed the older rejection contract and were
migrated while keeping malformed reserved-record negatives. RPC fixtures first
confused a runtime Developer projection with the existing Chat User wire
fallback, then took an append-failure baseline before title initialization.
The REPL Skill family now asserts the actual kept raw Skill ID, full retained
content, one history summary, zero prefix summaries and same-Session reopen.

The `071603` backend was stopped after review found a contract issue; no gate
was accepted. The `072051` backend failed **1,416/1/20** on historical V1 title
metadata compatibility. Production read compatibility was repaired; the
original `session_name` test was not changed. The `072529` backend failed
**761/1/11** on the interrupted-tool warning assertion, after that same REPL
module had passed. The unchanged isolated case passed **1/0**, then the final
four-thread full gate passed. A Windows process-tree kill timing race is
plausible, but its cause is unproved because the failed journal was not retained.
No assertion or product behavior was weakened to hide that observation.

During closure, a Root attempt to inspect `verify_backend.py --help` started an
unnecessary repeat because that script currently ignores arguments. Root
identified that exact Python process and its 14 descendants and stopped only
those 15 owned verification processes. This interrupted repeat is excluded from
acceptance; the unchanged successful `072853` snapshot remains authoritative.

## Actual CAS task and exact raw provenance

`live/20261002T072929Z/acceptance.json`: **PASS**, **eight actual requests**,
three processes, **21.454s**. The preferred local Manager gateway returned
`URLError`; CAS was reached directly using the authorized private Manager
configuration. Live catalogue verification selected `deepseek-v4.1-flash`,
`openai-completions`. This does not claim successful local Manager traversal.
Bounds were 14 requests, 120s/process and 2,048 output tokens/call.

Original Session: `01a0fb84-a6d0-7412-a65d-fe95caf61907`. Actual tools read
`stock.csv`, wrote and reread `current-total.json`:
`folder=87.5`, `lamp=65`, `grand_total=152.5`; original CSV bytes were unchanged.
Controlled native custom/hook/branch/excluded-Bash/metadata receipts were then
imported into that history. The imported Bash receipt did not execute a shell.
Two actual independent summary requests had no tools and retained
`firstKeptEntryId=native-kept-metadata` with exactly these 12 IDs:

```text
fb24f914 97dd90f0 2dd9ff81 9070108c dbfad98e 22ee8f67 15e595ff 26b69eb8
native-custom native-hook native-branch native-old-answer
```

Each custom/hook/branch group occurs once in summary JSONL; excluded Bash never
occurs in summary prompt/source IDs. Tool-disabled continuation and original-
Session process reopen recover the amounts and `Cedar-2759`. The model correctly
labels that shipment label as unverified historical custom content, not a fact
from the inventory CSV or generated artifact.

The initial live `receipt.json` remains FAIL: the harness accessed `row['id']`
on a title-cache record without an ID. Root inspected requests/journal/artifacts,
corrected the parser to `.get('id')`, then ran only the remaining tool-disabled
resume. Task and compaction were not replayed. `accepted-requests.json` contains
the original seven requests plus that one resume; original `requests.json` and
failed receipt are preserved. The 21.454s excludes manual repair waiting.

Binary SHA256: `0a09ab545b082d6bf1db88448d6c6fa38b0f756517c49248be25bc2d16fa3429`.
Artifact SHA256: `0d2a4d0c8d530021c8515a925d442ee2e6c2e72ad01ab3342ee82f5f81a1b5fa`.
Six persisted assistant receipts total **20,837** tokens; summary-call usage is
not persisted, so this is not total usage for all eight requests.

## Independent review, Root audit and remaining work

Independent Codex `/root/ctx_oracle_diff_review` used `ara-git-review` and
`ara-rust-core-review`, reread fixed source and final Rust diffs, and inspected
module/backend/live logs and artifacts. `post-review.json` is
`PASS_WITH_EXPLICIT_GAPS`, SHA256
`5ed6dfbaac03a6723b8622e061cc84538540639b68e673f42c120d009c65d0ec`.
Its 14 source hashes match both executed snapshots and current files. Root's
`root-audit.json` independently checks those hashes, binary, raw sources, request
bodies, Session identity, artifact bytes and usage boundary. Documentation review
is recorded separately to avoid a receipt/document hash cycle.

Mandatory open work: image-bearing summaries; full native token accounting/hard wire fit; provider-native
legacy `providerPayload`, attribution and `historyRewriteAt` typed replay;
Python/fileMention and other raw roles; other compaction methods/reducers/rescue;
smart/local/custom Host callers and registry/auth adoption; complete input,
settings and receipt contracts; actual OpenAI account/Responses/Codex trials;
Linux and remaining native runtime owners. P0/V1 remain accepted, P1–P6 open,
RPC 27/42 and full parity marker null. This is tested/audited bounded WIP.
