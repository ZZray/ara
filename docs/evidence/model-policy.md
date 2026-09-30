# Fixed model policy and construction — tested WIP

## Requirement and boundary

Reproduce all fixed OMP behavior before ARA customization. This Host dependency
batch ports model classification, compiled rule resolution, endpoint detection,
model construction, reference lookup, metrics and declarative runtime/auth
policies from `596f2da7101178214aa27a753529d15e6b7ad91d`. It retains full metadata
and missing-property versus explicit-null semantics. Runtime request handlers
must eventually consume the built policy rather than independently re-infer it.

This batch does not accept PKG-CATALOG, AI-AUTH, CA-MODEL-REGISTRY, R3/R4 or a
phase. Native discovery/registry/authentication and execution metadata adoption
are not connected yet. RPC remains **27/42 bounded implementations**; P0/V1
remain accepted, P1–P6 open, `ported_through_commit=null`. Every outstanding
behavior below remains mandatory.

## Fixed-source mapping

| Fixed OMP source under `packages/catalog/src/` | Native Host owner |
| --- | --- |
| `compat/revision.ts`, `cascade.ts`, `taxonomy.ts` | `ara-cli::catalog_rules`: revision grammar/constraints, selector rank contests, source attribution, class/family/revision, overrides/expiry and complete collapse/discovery vocabulary |
| `compat/rules.json` | Complete unchanged 321,052-byte compiled data, 21 taxonomy classes, 563 cascade rules and 80 auth providers |
| `compat/resolve.ts`, `axes.ts`, `apply.ts`, `anthropic.ts`, `openai.ts` | `ara-cli::model_policy`: API defaults, host/identity flags, full axis vocabulary, sparse overrides/fixups, thinking controls and XAI effort maps |
| `build.ts`, `model-tokenizer.ts` | `ara-cli::model_policy`: full model construction, wire identity, computer-use endpoint checks, catalog assignments/corrections and tokenizer selection/cache |
| Pure helpers in `utils.ts` | `ara-cli::model_policy`: names, numeric/boolean/record handling and Anthropic OAuth token detection; network `discoveryFetch` is still mandatory at discovery integration |
| `hosts.ts` | `ara-cli::model_identity`: all 30 host classes, bounded URL cache, provider-or-URL checks and endpoint shapes |
| `identity/{id,dialect,reference,bundled,metrics,priority}.ts` | `ara-cli::model_identity`: ordered candidates, ranking, same-provider thinking, lazy bundled references, metric merging/dialect identity gates and configured/default priority |
| `provider-models/bundled-references.ts` | `ara-cli::model_identity`: full sparse spec projection and lazy exact provider/global reference resolution |
| `compat/behavior.ts`, `auth.ts` | `ara-cli::catalog_behavior`: routes, operations, quota/plan/limits/exclusions/retirement/pricing policies, full auth roster and ordered hook declarations |

Bundled materialized models are consumed directly. They are not rebuilt through
`build_model`, which is for discovered/custom/override specs. The new policy
functions preserve unknown fields; they do not authorize a model or bind a
provider. MIT notices remain in `crates/ara-cli/data/LICENSE.omp-catalog`.
Compiled data SHA256:
`9ae6cc8f5c0fb2503d7e6d5ef885c01b8d767f1f14281c4432b09efa862d4bcf`.
The parent independently compared its bytes with the fixed Git blob.

## Unicode representation

The actual unchanged OMP `stripThinkingVariantSuffix("İ-thinking😀")` produces
UTF-16 code units `[304,45,56832]`, including a lone low surrogate. Rust UTF-8
strings cannot represent that result. `strip_thinking_variant_suffix_utf16`
preserves the exact code units; the String adapter returns an explicit error,
never a replacement character. Policy variant detection uses the lossless
helper. The oracle must retain this raw JSON/code-unit case. Future variant-list
consumers must keep the lossless boundary; UTF-8 replacement is not parity.

## Executed evidence

Raw receipt root: `C:\Temp\ara-model-policy-batch`.

Base: clean `dev`, `05d09aad82f96563188667950d497d802e104fe9`. Final production
source is identical across the focused, oracle and full-gate receipts below.
Code WIP checkpoint: `8f4f4d41a43cc2b387ec39b5f60674de5468d565`.
Staged and committed audits compare all 15 tested source pins: 14 byte-exact,
one existing `Cargo.lock` differs only through Git CRLF normalization. The
compiled rules are byte-identical to the fixed Git blob. Receipts:
`staged-pin-audit.json` and `committed-pin-audit.json`.
The code and documentation descendant `c2efa1b` remain local: GitHub push
protection rejected four original Google OAuth app-registration constants in
the unchanged compiled data. Remote `dev` was read afterward and remains at
the base above. User approval of reviewed public upstream values or an
authorized secure-externalization/reconstruction is pending. No protection was
disabled and no history was rewritten. Push failure does not change the test
results or permit dropping authentication behavior.

- Final library: **102/0/0**, including 24 new native tests (rules 10,
  behavior/auth 2, identity/metrics 6, policy/build 6). Receipt
  `focused-20260930T214029Z-receipt.json`, 9.348 seconds, source pins unchanged.
- Final unchanged-source Bun/Rust comparison: **46,136 cases**, including
  **270,556 provider/model behavior pairs**, 86 providers and 3,146 IDs.
  All 4,776 bundled rows are compared across taxonomy, cascade, policy,
  construction, identity, tokenizer, hosts and metrics. Expanded fixtures cover
  sparse/null/unknown fields, custom APIs/URLs, thinking and computer-use,
  prototype overrides, Unicode and full behavior/auth vocabulary.
  Final replay `oracle-20260930T214353Z-receipt.json`: **2/0/0**, 29.966 seconds,
  unchanged source pins; includes executed malformed-corpus rejection tests.
  The receipt binds the actual `oracle.json` SHA256.
- Oracle generation: `scripts/model_policy_oracle.py` exports fixed Git blobs
  into `oracle/run-20260930T213257Z-4c17f316`, then executes Bun **1.4.0**.
  **49** unchanged source/fixture files and two licenses are checked by SHA256
  and Git blob ID before imports; the executed runner also checks its own hash.
  Its package facade reexports original utility leaves. Policy and utility
  functions are not replaced. Fourteen source tests supply extracted examples;
  this comparison does not execute those tests' entire assertion suites.
- Windows full gate: **1,428/0/4**, 104 suites. Owned-package formatting,
  all-feature/all-target workspace Clippy and tests, doc tests, `cargo deny`
  and binary build **PASS**; `full-20260930T214039Z-receipt.json`, **98.508 s**
  (backend 95.606, deny 2.502, build 0.396). Fifteen source pins unchanged.
  Ignored external-oracle tests are executed separately where recorded.
- After that full gate, only the new oracle test's inventory guard and negative
  tests changed. Production/data/dependencies/runner remain byte-identical.
  The final oracle above executes that test; final package fmt, all-target/
  all-feature Clippy, bootstrap/inventory checks and diff-check pass in
  `final-checks-20260930T220100Z-receipt.json` (2.019 s). No unrelated
  workspace test is rerun for this guard-only repair.

Actual entry: native `ara-cli` library and Rust integration test. These pure
Host dependencies are not yet used by the production registry/model commands.
No new real-model task is claimed. No current-batch Linux gate is claimed here.
Final binary SHA256:
`fb1afb39ca811972da24c5a51b5179eda0f9fcb5774442ac31b0af477e979d84`.

Reproduce the original-source generation with:

```text
python -X utf8 scripts/model_policy_oracle.py --upstream C:/Temp/omp-596f2da --bun C:/Temp/ara-ctx-skill-sort-oracle/bun-windows/bun-windows-x64/bun.exe --output C:/Temp/ara-model-policy-batch/oracle
```

Set `ARA_MODEL_POLICY_ORACLE` to that run's retained `oracle.json`, then run
`cargo test -p ara-cli --test model_policy_oracle --all-features -- --ignored --nocapture`.
Ordinary Cargo execution leaves this comparison explicitly ignored; a missing
oracle cannot yield a comparison pass.

Initial 101-test/44,322-case checks and failed expanded replays are retained as
superseded receipts. The final snapshots include all repairs. The macro
recursion limit is 256 for the complete large default-field table.

## Review and remaining acceptance

Independent Codex reviews use `ara-git-review` and `ara-rust-core-review`:

- `/root/ctx_oracle_diff_review`: all fixed identity/reference/metrics/host and
  declarative behavior/auth sources against native code, then Python/Bun/Rust
  oracle tooling. Independently rehashed all 49 source blobs and both licenses,
  runner and real corpus. Found the behavior-ID guard gap below; final repair
  review confirms it resolved, with no new finding, on adapter SHA256
  `8f2088329c70e0c6ae718d764dd6719983ef817db69f89a58b1d17ada0a89f5e`.
  This reviewer does not self-review rules.
- `/root/model_policy_port`: complete fixed revision/cascade/taxonomy and native
  rules. Found Unicode suffix-probe panics; reviewed their repair with no
  remaining confirmed rules finding. This reviewer does not self-review policy.
- `/root/rpc_owned_core`: complete fixed resolve/axes/apply/Anthropic/OpenAI,
  build/tokenizer/pure-utils and native policy implementation. Found non-Unicode
  JS regex `/i` versus Rust Unicode-fold mismatch; final source review **approve**
  on `model_policy.rs` SHA256
  `800fd478e7ab051748426a99fe67f7988dd89952207266f34715d4cc70a5c43c`.
  Additional raw-source authored/array-prototype probes pass in
  `C:\Temp\ara-model-policy-review\prototype-probe.json`; they are outside the
  46,136-case count. This reviewer does not self-review oracle tools.

Resolved findings and observed corrections:

1. Unicode unmatched suffix probes previously sliced invalid UTF-16 boundaries
   and panicked. They now return no-match; real matched malformed output retains
   the lossless boundary above. Expanded original-source comparison passes.
2. OMP `/i` without `/u` excludes long-s/Kelvin Unicode folding. Azure/Bedrock
   now use ASCII ignore-case, and Cloudflare/Vertex use scoped ASCII literals
   plus JS-dot line-terminator exclusions. All 18 endpoint predicates compare.
3. OMP `key in compat`, numeric key order and `__proto__` assignment affect
   override/thinking resolution. Full native building preserves internal
   non-serialized prototype state and spread reset. Public `Value` adapters
   operate on fresh JSON values; future retained consumers must carry any
   state required by the original source.
4. The oracle initially permitted dropping a non-bundled behavior ID and
   changing its self-reported pair count. It now fixes 3,146 IDs/270,556 pairs,
   checks inventory uniqueness and exact per-provider ID sets. Native negatives
   reject deletion with adjusted receipt, same-length replacement, duplicates,
   wrong label/provider/id binding, unknown categories and missing families.

The parent reads fixed source and actual logs and accepts this as **tested WIP**.
Pure-function comparisons, negative fixtures and an approved diff do not accept
the complete catalogue, registry/authentication or a phase.

Still mandatory: the complete `compat/collapse.ts` model-list algorithm;
generic discovery/normalization/manager, aliases and override recomposition;
native `discoveryFetch` transport including extra-CA rotation and missing-path
errors; registry/cache/fingerprints/live-header restoration; all AuthStorage
request precedence, OAuth/provider adapters, usage/reserve/refresh/session
release/rotation; complete execution projection and model/role/thinking/native
journal adoption; fallback and R2 recovery; all other fixed inventory behavior.
The preceding config/cache YAML and error-format limitations also remain open.

## Time and reuse

This batch started at approximately 20:54 UTC on 2026-09-30, after live state
verification began at 20:50 UTC. Fixed Git blobs, pinned cached Bun, complete
compiled data, independent file ownership and one shared final snapshot gate
avoid duplicate downloads, rule compilation and unrelated model trials. Record
through finalization: about 70 minutes; final replay is 29.966 seconds and the
shared full gate 98.508 seconds. Label binding uses a direct first-slash split
instead of repeatedly scanning 4,776 rows; all coverage guards remain enforced.
