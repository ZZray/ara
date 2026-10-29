# Production ACP and active plan module — 2026-10-09

## Requirement and boundary

Fixed OMP `596f2da7101178214aa27a753529d15e6b7ad91d`; delivered dirty
`dev` HEAD `20aa0e2ef9872f633ba2671376cdf778477e7115`.
The module connects actual ACP transport/peer filesystem writes, active plan
mode and typed proposal results to the existing Rust EDIT/WRITE owners.
Original ACP five behaviors retain their identities and 92+5 denominator.
Full ACP, Planning, EDIT, WRITE, P0–P6 and product acceptance remain OPEN.
Formal inventory remains **0/111 complete, 1 tested, 27 implementing, 83 open**.

Execution packet:
`D:/workspace/env/cache/ara-port-candidates/acp-plan-production-execution-v5-20261009`.
Delivered manifest SHA256:
`22d9d3a6d4f206280bb889062b9a3297c74fdc75be23b76d3b559c2f0fab382d`.
Git index remains
`89d34e26be97c90a14680d44ace937e6b1ae5a86e8ee7b46dd6b90718a922475`.
Production binary SHA256:
`9733d4c7b09691f4d8d14bd6b4dca0e2561746c675da9389748e2c0818f4967c`.

## Delivered code and root cause repair

The 15-path manifest records the exact delivered owners and composition.
V4→v5 changes only `ara-agent/src/agent_loop.rs` and the existing
`ara-cli/tests/host_bridges.rs`; 1,603 other source pins and the index remain
unchanged. Ordinary Windows READ overflow was localized with the retained
executable: adjacent stack pointers showed 145,504-byte stream and
186,272-byte loop frames. Normal boxed-future construction wrappers isolate
those frames; the original async bodies, lifetime, cancellation, discard,
guards, catch_unwind and receipt order are unchanged. Delivered PE default
stack remains 1 MiB.

The Host oracle retains the injected `i` field in the strict required schema
and checks each restored original durable error once under its exact tool ID.
Disconnect feedback and original prompt Unknown settlement are inspected
separately; unknown URI effects remain unknown.

## Original concentrated deterministic execution

Grouped affected-target typecheck: **PASS, 59.035 s**.
One executable build: **PASS, 536.342 s**, 407 targets reused, 15 compiled,
87 warnings, zero errors. This is not a clean clippy claim.
Original selected **10/10 groups PASS; 40 tests, zero ignored**.
Test bodies total **39.093 s**; build/module wall **585.345 s**.

| Existing group | Passed | Seconds |
| --- | ---: | ---: |
| engine | 14 | 1.440 |
| policy | 4 | 0.079 |
| actual-tools | 4 | 0.259 |
| retained-repl | 1 | 9.643 |
| host-bridges | 12 | 4.209 |
| journal-failure | 1 | 22.378 |
| host-idle-settlement | 1 | 0.174 |
| new-session | 1 | 0.290 |
| clear-compact | 1 | 0.521 |
| write-tools | 1 | 0.100 |

The original Host family includes replace/patch, capability off, local plan
bypass, persisted drift/tag, real mode APIs, hidden write, restore/identity,
form/refine/cancel/Unknown/approve, schema and no-form autoapproval, EOF/no
replay, malformed side channels and pending owner isolation. Raw commands,
original test names, stdout/stderr, counts, source guards and binary pins are
in `gate/build-result.json` and `gate/module-result.json`.

```powershell
python -X utf8 scripts/verify_edit_module.py --output '<fresh D packet>/gate' --groups engine policy actual-tools retained-repl host-bridges journal-failure host-idle-settlement new-session clear-compact write-tools --delivery-manifest 'D:/workspace/env/cache/ara-port-candidates/acp-plan-production-execution-v5-20261009/delivered-source-manifest.json' --manifest-sha256 22d9d3a6d4f206280bb889062b9a3297c74fdc75be23b76d3b559c2f0fab382d
```

This is a historical reproducible command for the pinned v5 snapshot; later
modules change live code and require their own manifest. Original compiler,
stack and journal failures are retained, including the unchanged-binary journal
diagnostic PASS. They are not relabeled after repair. Full workspace/clippy/deny
and complete-parent gates were not repeated for this selected module.

## Actual configured CAS task and retained failures

Exact model `deepseek-v4.1-flash`, wire `openai-completions`; current local
configuration and live `/models` checked. Manager Gateway was unavailable;
the task used the currently configured CAS endpoint directly. Credentials
remain in child environment, outside source and receipts.

First packet `real-cas-receipts/20261008T184242Z` remains **FAIL**: the
procedure demanded `PLAN.md`, while fixed templates prescribe a canonical
`<slug>-plan.md`. Actual planning/typed approval succeeded (12.246 s,
five model calls, four tools, zero peer writes); shutdown joined in 0.060 s.
The procedure was corrected against fixed source and independently reviewed.

Second packet `real-cas-receipts/20261008T185049Z` also retains its original
**FAIL**, `Actual readback did not contain the complete JSON document`.
Session `01a11cda-a388-7415-b1b7-e54f3eabd4a9` contains two settled Runs:
planning **15.228 s / 5 calls / 4 tools / 0 peer writes**, mutation
**8.314 s / 6 calls / 5 tools / exactly 2 peer writes**. Typed proposal mode
ordering and approved `local://sales-summary-plan.md` are present. Final
artifact is exactly:

```json
{"pen":16.5,"book":46,"cup":12.75,"grand_total":75.25}
```

Summary SHA256:
`6307f72e39118ec4f50dcd3661025b9ed7558d0107817f70f34df2a1118c489a`.
Original ACP READ event **2040** has the whole zero-total document; EDIT
completes at **2082**, final READ at **2084** has the whole correct document.
Peer effects at **2019/2081** have exact zero/final bytes. The earlier durable
READ text was later replaced with `[Superseded by a newer read of this file]`.
Exact fixed `packages/agent/src/compaction/pruning.ts:252–310` and current
Rust `local_reduction` require this history pruning. The failure belongs to
the post-Run oracle; it does not show a failed READ or mutation. No Rust
change or effect replay was needed. Original process joined in 0.283 s.

## Same-Session checkpoint completion

The corrected procedure observes the original unique completed live READ
under exact Session/tool ID and `sessionUpdate=tool_call_update`, while retaining
durable ID/order, both peer effects and independent final-byte checks.
`--continue-from` revalidates the specific retained failure and all original
source/binary/manifest, plan/summary, receipt/protocol pins before running only
the unexecuted no-tools saved recall. It reuses the existing Peer class and
does not replay planning, WRITE or EDIT.

Final procedure SHA256:
`91d4177e5abbe8c97e1e517e44a2ef3786864f151ea668b3e6988f58a3402bfb`.
Delta SHA256:
`a72b5ca08cc62eabd1dfeb922160444388f3d0df387388c732c92f89fb85799e`.
Checkpoint `continuation-20261008T190110Z/receipt.json` SHA256:
`d571ecb9370cdc0d0d8c6ca8bb45403cdfe6fc8c3e33d060fc6d2b9835c6b04b`.
Result **PASS**, **2.703 s / 1 model call / 0 tools / 0 writes**; numeric
recall is pen quantity 6, book unit price 11.5, cup quantity 3. Recorded usage:
input 5,491, output 50, cacheRead 2,176, cacheWrite 0, totalTokens 7,717,
reasoningTokens 23. Shutdown joins in **0.022 s**. Source, binary, procedure,
old receipt/protocol, final summary and plan guards pass.

Load replays historical tool projections before its response at frame 32.
Frames after that load response contain no tool events; prompt frame 82
returns `end_turn`, close frame 83 succeeds, and the new journal assistant
contains the expected recall. Root directly checked raw protocol/journal,
artifact bytes and pins. Bounds: two model calls, 120 s, 2,048 max output
tokens, 22 s shutdown. The review gap occurred with the original process
closed: these observations do not share one continuous original 480 s clock.

## Review, mapping and remaining acceptance

Independent reviewer: Codex `/root/finish_edit_renderer`, same SOURCE/POST
packet; SOURCE SHA256
`1735d0f427141d243e7cd7488eb45e9330ac29fddf0fdf7c2b3e9443e956da2b`.
Final POST **APPROVE**: same packet `POST.md`, SHA256
`d32069c77ad6ed8d14fac8f2d413051e18eca49f6646dcdf2b1262418f06ae95`.
Root read the actual report and checked its critical source, artifact,
protocol/journal and no-replay conclusions. No unresolved material finding
remains in the bounded delivered batch. Selected actual-model time totals
26.245 s across twelve calls and exactly two original peer writes; original
failed procedure observations remain separately recorded. This point audit
supports the bounded module and its original five mapping rows, without
accepting a full parent or product.

Only original IDs `B-355ae274b5`, `B-cfb3047963`, `B-3856d00e02`,
`B-0d8fd8d258`, `B-bb4f3600f3` receive current mapping/evidence fields.
All 87 other rows, original identities, five inspection rows and old receipts
are preserved. `mapping-acp-v5-receipt.json` records before/after hashes.
The old D mirror contains historical pre-apply bytes; the canonical C map
matches the previous applied receipt, then this recorded five-row update.

Acceptance increment remains **zero**. Extension initialization and actual
JS/TS factory carrier, the next Vault owner, complete same-version EDIT92+5,
platform/service/product obligations and all other parent requirements remain
explicit. Controlled peers prove Rust process contracts, not a deployed ACP
client product or full OMP compatibility.
