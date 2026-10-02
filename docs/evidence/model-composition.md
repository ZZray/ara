# MODEL-COMPOSITION-01 — native helpers and bounded static registry

2026-10-02 UTC, branch `dev`, base
`2a6b1c05c793f8c490ff05fb31f07d81e4bd863b`. Fixed OMP
`596f2da7101178214aa27a753529d15e6b7ad91d` is unchanged.
**Implementing / bounded WIP; no complete registry, parent surface or phase
acceptance. Final gate and actual-task receipts are recorded below.**

## Observable scope

| Fixed source | Rust behavior |
| --- | --- |
| `coding-agent/src/config/model-patch.ts` | `model_patch`: full patch field set, authored limits/input/cost after builder corrections, Merge/Replace transport, discovery merges, provider sets and ordered model-key merge. |
| `coding-agent/src/config/custom-models.ts`, pure `model-resolver.ts` parsing dependencies and `thinking.ts` | `custom_models`: overlays, reference/default finalization, live auth headers, suppressed selectors and alias overrides; literal IDs and UTF-16 suffix parsing. |
| `catalog/src/compat/collapse.ts`, `build.ts`, `models.ts`, identity/reference/metrics | Actual original input donor follows collapse/reconciliation/alias retargeting; materialized bundled rows share references without rebuilding. Headers remain opaque beside safe metadata. |
| Selected `model-registry.ts` static load/lookup/composition methods | `static_model_registry`: ordered bundled/cache/runtime snapshots and local overlays, hardcoded policies, metrics/interning, lazy/full lookup and availability. No I/O or discovery is performed by this adapter. |
| Existing reference CLI | `daily_model_config` and `config_request_auth` use native composition for supported wire settings and private headers. Auth:none/Codex preflight inspects actual composed source names without starting helpers. |

All source paths above are under `packages/`. Local final manifest binds
17 source/test/runner files, 35 exact upstream blobs and 57 preserved vendor/EOL
WIP hashes. Raw header/API-key sources have no Debug/Serialize representation.
`Absent`, own `Undefined`, `Null` and actual header source remain distinct.
Catalogue presence does not grant permission or an inference credential.

The daily Host retains its existing explicit CLI/environment credential
precedence. Provider authHeader is reasserted from that effective private lease
after native materialization, so its receipt and actual bearer agree. This is
an explicit Host policy, not full native AuthStorage precedence.

## Grouped executable evidence

Entry: `python -X utf8 scripts/verify_registry_composition.py`, with
`--module patch|custom|static|values|collapse|selection|host|daily-cli` for repairs;
`--full` shares the backend/inventory/deny/build gate. The local final wrapper
also builds after a retained deny failure without reporting a successful gate.

Eight families total **167/0/0**: patch 4, custom 5, static 5, values 5,
library/collapse 132, selection 7, actual Host 5 and daily CLI 4. Earlier stable
helper sources are reused; the final all-target gate covers the delivered code.
Receipts under `C:\Temp\ara-registry-compose-batch`:

- `module-20261002T154540Z`: first five families pass; selection 5/2 fails on
  earlier narrow-projection expectations. Retained as a failed invocation.
- `module-20261002T155057Z`: selection 7/0/0, source unchanged.
- `module-20261002T155115Z`: actual Host 5/0/0, including native overrides,
  authHeader environment precedence, reserved names and nested cancellation.
- `module-20261002T155501Z`: daily CLI 3/1 fails on the old output field.
  `module-20261002T155624Z`: repaired daily CLI 4/0/0, source unchanged.
- Fixed `model-patch.ts:234,273`, `build.ts:203–215` and `compat/resolve.ts`
  support the changed expectations: Responses reasoning overrides are admitted;
  this unknown custom route uses max_completion_tokens. Assertions require the
  exact value and absence of the opposite field.
- `oracle-20261002T155754Z`: retained native collapse corpus **3/0/0**,
  **1,995 cases/eight export families**, 46.382s. Corpus SHA256
  `23289a1a8fd647facbd375a756e3eeea6ecf9f1592bd0b336b152bef48dc8ba7`.
  No native corpus was regenerated.

Independent Codex `/root/remote_review` reviewed the PRE plan, exact sources,
diff and retained artifacts. Four confirmed issues were repaired: cancellation
inside nested Live sources, missed selected override/donor header guards,
authHeader/lease credential disagreement and an external auth callback invoked
while holding the registry mutex. Actual Host and primitive cancellation cases
assert started helper settlement and zero later helper starts; static families
exercise callback reentry in both full and lazy lookup.

The initial final-gate attempt `gate-20261002T155851Z` fails Clippy's two
type_complexity checks. A shared `ModelIdPredicate` type alias repairs only
declarations; no behavior or lint policy is changed. Subsequent receipts below
bind the final alias snapshot.

`gate-20261002T160100Z` then passes format/Clippy but stops with
880 passed/2 failed/20 ignored in 503.258s (365s compilation). The two RPC
compaction fixtures also assumed max_tokens. They now require the native
max_completion_tokens field and opposite-field absence; split budgets 250/384,
fold budget 16,384, tool vetoes and original-branch atomicity assertions remain.
This failed invocation is retained. Production logic is unchanged by the
subsequent fixture repair.

## Required remaining work

- Full loader's resolved header presence/eager credential installation; config
  and cache I/O, async discovery/hydration/coalescing, llama.cpp fixups, runtime
  registration/hooks/modifiers/source cleanup and complete native find/fuzzy
  selector/auth lifecycle. Inputs here are explicit already resolved snapshots.
- Native alias-rekey debug logging has no Host logger binding yet.
- Complete execution projection of catalog metadata. Authored thinking,
  WebSockets, omitMaxOutputTokens, image decoder, compactionModel and enabled
  disableStrictTools are explicitly rejected by this daily adapter. Native
  metadata remains in the Host catalog. Promotion selection is still a bounded
  caller and does not claim the full resolver.
- Bun/Linux, actual OpenAI account authorization/tasks, deferred Providers,
  failed vision acceptance and the existing dependency advisory remain required.
  No required feature is omitted or accepted by these bounded tests.

## Final receipts

`C:\Temp\ara-registry-compose-batch\final-manifest.json` binds the final
source, 35 upstream blobs, retained corpus, receipts and binary. Final binary
SHA256 `747d0fed8a05a271bfcf933051381e0686431f8eea34540d9c95e43c02a16477`.

| Final stable gate `gate-20261002T161136Z` | Result | Seconds |
| --- | --- | ---: |
| `python -X utf8 scripts/verify_backend.py` | Format/Clippy/all-target/doc PASS, **1,643/0/20** | 220.072 |
| `python -X utf8 scripts/omp_inventory.py check` | PASS | 0.577 |
| `cargo deny check` | FAIL, existing RUSTSEC-2026-0192 | 3.080 |
| `cargo build -p ara-cli --all-features --locked --bin ara` | PASS | 0.394 |

Before/after source hashes match. The whole gate remains **FAIL** on the fixed
snapcompact `fontdue -> ttf-parser 0.25.1` advisory; no policy exception is added.

Actual preferred route: current Manager configuration/catalog → enabled CAS,
`deepseek-v4.1-flash`, `openai-completions`. The local management proxy remains
unavailable. Root reads configuration privately without modifying it. Bounds:
two processes, five model calls, 90s per process, configured 2,048 output tokens
per call; no CLI output-cap flag masks the modelOverrides result.

`live-20261002T161740Z/receipt.json` **PASS**, **14.880s / five calls**:

- Four actual calls / 8.765s execute read(invoice.csv), write(totals.json),
  read(totals.json), with exactly three successful tool receipts. Artifact is
  `{"cable":56,"bracket":27,"grand_total":83,"shipment_label":"Cedar-5812"}`;
  SHA256 `27a72c106dd7de0aec236725c0b3ea2e984a9575cb04c1559fdd2c2f268d7583`.
- Same original Session reopens with tools disabled, recalls the exact JSON
  in one call / 3.417s, preserves original journal rows and artifact hash.
- All five actual incoming requests have the configured 2,048 wire cap and
  selected override header/key proofs. Four helper receipts prove one key and
  one header execution per fresh process in the actual project cwd. Credentials
  and helper results stay outside public stdout/stderr/journal. No requests are
  active at process completion or cleanup.
- Observed upstream usage totals **16,106 input / 381 output tokens**. Monetary
  cost is unknown; authored test pricing is not a billing receipt.
- Explicit `--tokenizer none` is required by this bounded Host. The initial
  `live-20261002T161637Z` fails selection in 4.161s on the generated native
  tokenizer, with zero model calls, zero active requests and no helper start.
  The failure is retained; no unknown external effect was replayed. The earlier
  runner brace syntax error occurs before execution or private config access.

Root independently checks raw tool receipts, actual JSON/hash, five request
bodies/statuses, original Session preservation, helper/auth checks and unchanged
source/binary/vendor hashes. Independent final POST is in the local
`post-review.json`; its review outcome and acceptance limits are recorded in
the handoff. This point remains implementing/tested bounded WIP.

Implementation/verification runs about **14:26–16:19 UTC (113min)**, followed by
documentation/review/Git closure. The original estimate is 90–150min. The slow
whole-target compile takes 365s; the final cached backend takes 220.072s.
Follow-up optimization: avoid constructing a full static registry on a CLI path
that cannot perform a catalog lookup; investigate source-native lazy reuse in
the full-registry batch. No extra source change invalidates this tested binary.
