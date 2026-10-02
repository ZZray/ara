# AGT-TOKENIZERe — default native tokenizer and model-aware Host

2026-10-02 UTC, branch dev, base `89f65643483af2108d9f854043f93ca38fc945a4`.
Fixed OMP `596f2da7101178214aa27a753529d15e6b7ad91d` is unchanged.
**Implementing / tested bounded WIP. Dependency gate remains FAIL; no full
point, surface or phase acceptance.**

## Observable scope and original source

| Fixed OMP source | Rust behavior |
| --- | --- |
| crates/pi-natives/src/utok, data and tests | ara-ctok retains the universal BPE/Claude engines, native scanners, token IDs/counts, UTF-8/16/32 input families, vocabularies and original differential/golden test families. |
| packages/catalog/src/model-tokenizer.ts and family metadata | Host auto selection reuses the existing native catalog identity resolver; daily selection uses materialized requestModelId. ModelTokenizer now represents all eight fixed catalog families. |
| packages/agent/src/tokenizer.ts | Immutable model/policy counter, Strict/Approximate/UpperBound modes, per-fragment counting, checked totals, message text/reasoning/tools and existing image/frame recipes. |
| Existing ARA CLI/RPC compaction and reduction callers | Current-model counts reach threshold/floor, raw source cuts, handoff, remote, snapcompact and local reduction; accurate-unknown policy is captured by Host. |

Ten native encodings comprise six BPE and four Claude encodings. Eight catalog
families comprise Qwen3, DeepSeekV3, KimiK2, Glm5 and four Claude families.
Unknown OpenAI models do not acquire a catalog family by provider/API guessing.
Strict or explicitly accurate unknown counts use O200kBase. The older selected
model API continues to report UnknownTokenizer when metadata is absent.

The source manifest binds 51 fixed Git blobs; all original SHA256 and Git blob
IDs were independently checked by Root. The copied-file map records 41 files
and eight binary blobs. MIT, ctok, Unicode and regex-syntax notices are retained.
The original xutf 1.5.0 is NFC Unicode17 with category/script16, verified from the
archive matching upstream Cargo.lock. Stable replacements preserve that split,
including the exact 22 Han16 ranges; normalization is not downgraded to16.

Counts are fresh because mutable ARA values do not yet expose OMP's weak-cache
identity/version contract. Strict budget decisions always measure content:
the raw-byte fast path is unsafe for pinned Claude (`ξ`: two UTF-8 bytes, three
tokens), and a literal empty fragment can count one. Empty user/tool text blocks
are omitted; literal user/assistant empty fragments remain present. Native
counter overflow never becomes a silently substituted approximate success.
This deliberate shortcut correction is recorded separately from parity gaps.

## Grouped executable evidence

Entry: `python -X utf8 scripts/verify_native_tokenizer.py`, with existing
`--module native|claude-budget|model|agent|selection|host` and `--full` shared
backend gate. Private-route trials are separate Root-only bounded procedures.

Receipts under `C:\Temp\ara-native-tokenizer-batch`:

- `native-20261002T165206Z`: cargo test -p ara-ctok --all-targets --locked,
  41/0/0 PASS, 79.331s; native randomized scanner differential takes73.91s.
  This pre-format snapshot is superseded by the final all-target receipt.
- `module-20261002T165824Z`: compile66.656s, model137/0/0 and agent9/0/0 pass;
  selection7/1 fails because the new-family fixture inherited reasoning mode.
  Fixed catalog source marks those models as reasoning-capable. The fixture
  now explicitly sets reasoning=false; production reasoning rejection remains.
- `module-20261002T170106Z`: selection8/0/0 and actual CLI Host5/0/0 PASS;
  source unchanged throughout. The grouped successful total is200/0/0.
- `gate-20261002T170232Z`: format/Clippy PASS; backend615/1/1 fails the older
  model_catalog unsupported-tokenizer fixture, 488.865s including438s compilation.
  Qwen3 is supported by the new enum and existing from_name projection. Only
  that refusal input changes to future-tokenizer; the unknown-family check is
  retained. `projection-fixture-repair.json` records1/0/0 on that exact change.
- Final `gate-20261002T172408Z`: backend1,682/0/20,
  format/Clippy/inventory/build PASS. Backend 252.757s;
  full command 256.246s. Whole gate PASS=False.
  Existing fontdue -> ttf-parser0.25.1 / RUSTSEC-2026-0192 remains the dependency
  failure; no exception or policy waiver was introduced.
- `gate-20261002T171208Z`: the wrapper's600s limit expires after1666/0/20
  recorded tests. The local cargo child finishes its remaining all-target
  output, but no complete backend command receipt is claimed from that log.
  The final unchanged snapshot uses cached targets, a1200s wrapper bound and
  scoped CARGO_BUILD_JOBS=4. No production source or test gate is weakened.

## Actual model task and Root acceptance of artifacts

Preferred `deepseek-v4.1-flash`, OpenAI-compatible Chat Completions, route
`CAS from Manager configuration`. The selector checks the current Manager configuration and
actual live catalogue; no account/key/config is committed. Bounds: two
processes, five inference calls maximum, 90s/process and2048 output tokens/call.

The new Willow-9147 CSV task executes read(metrics.csv), write(metrics-summary.json),
read(metrics-summary.json), then resumes the same original Session with tools
disabled and an answer-free prompt. Actual expected/observed artifact:

```json
{"completed_total":60,"failed_total":5,"success_rate_percent":92.31,"dataset_label":"Willow-9147"}
```

Result PASS, 14.468s/5 calls. No `--tokenizer none` or explicit
family override is used; auto selects deepseek-v3. Every actual request reports
Exact(N) prepared-message-text counts. Root checks request sequence, text-field
count, UTF-8 byte sizes against the actual forwarded body, complete coverage,
tool order/result IDs, disk JSON, original Session identity/prior rows, unchanged
artifact on no-tool recall, private helper cwd/cache/auth/cap and source/binary
hashes. Usage comes from actual upstream SSE; missing buckets/cost stay unknown.
These are content counts, not provider billing or a complete wire/context fit.
Observed upstream usage totals:16284 input tokens and427 output tokens across
five calls. Monetary cost is unknown. The local Manager route catalogue probe
was unavailable; the current configured CAS route catalogue confirmed the
preferred model before any inference call. This is not a Manager transit claim.

Final binary SHA256: `102c0181d5cea8afe4a774687f48b8f9a4c139d9fb38e5312c72605c2e083d32`. Local final-manifest.json binds63
source/test/runner/notice paths,57 untouched vendor/EOL WIP hashes, fixed source,
final gate and actual-task receipts. Source and binary remain stable throughout
the trial. Initial failures are retained; no active/unknown request is replayed.

Independent Codex `/root/remote_review` performs PRE/source/fixture/final artifact
reviews, using ara-git-review and ara-rust-core-review procedures. POST SHA256:
`073e89934530136b2385000ea7454f7b6ed40d8582fbd7412a486b6a513dbe61`. Root independently inspects original-source hashes, scoped
diffs and all task artifacts; the review does not grant full parity acceptance.

## Required remaining work and progress

- Complete provider framing/tool/image totals, extension/native role coverage,
  weak-cache identity/version/performance and original N-API/Rayon contracts.
- Known-family Core summary chunking is native, but unknown-model accurate
  summary policy still uses its old approximate branch. Inject Host policy into
  that Core summary interface in its subsequent grouped caller batch.
- Full Registry loader/cache I/O, discovery/hydration/coalescing, runtime/source
  lifecycle, complete native selectors/auth/account/other deferred Providers.
- Bun/Linux and declared platforms, actual OpenAI account authorization/task,
  prior failed vision acceptance and the existing dependency advisory.

No required feature is removed. Parent surfaces retain26 implementing/1 tested/
84 open across111, zero complete acceptances. The ledger has60 bounded points:
35 limited acceptances,23 implementing,2 tested. P0/V1 accepted; P1–P6 open;
RPC27/42; full parity marker null. This is not a weighted total-completion rate.
This batch takes about65min through verification/review, then docs/Git.
Next batch uses original module inputs and one stable final gate, with bounded
Windows build concurrency to reduce simultaneous memory-heavy linking.
