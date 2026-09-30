# Fixed model variants and lossless policy — tested WIP

Target: OMP `596f2da7101178214aa27a753529d15e6b7ad91d` (v18.1.8).
ARA base: `7622d5f`; the recorded worktree snapshot is the delivered code for
this checkpoint. This is a native Host dependency, not acceptance of the
catalogue, model commands, authentication, a phase or the complete Agent.
Full fixed OMP replication remains mandatory before ARA customization.

## Source and implementation

| Fixed source / observable behavior | Rust owner and coverage |
| --- | --- |
| `packages/catalog/src/compat/collapse.ts`: `reviewedCollapseTable`, `deriveThinkingPairFamilies`, `isCollapsedVariantSpec`, `collapseVariants`, `collapseBuiltVariants`, `resolveVariantSelector`, `resolveBareVariantSelector`, `getVariantAliasSources` | `ara-cli::model_collapse`: all eight exports, all 67 reviewed families and two templates, Cursor gates, thinking twins, stale/default/budget/extra-alias repair, lazy table-identity cache and template learning, aliases and reference retargeting |
| Mutable module alias indexes and model/array identity | One Host-owned runtime; `Arc` model identity; reverse lookup retains a shared live array for the single-index branch and creates a snapshot for the mixed branch |
| `packages/catalog/src/build.ts`, `compat/{resolve,apply,axes}.ts`, detector and taxonomy/cascade | `ara-cli::model_wire_policy::WireModelPolicy`: detector/axes seed, raw overrides, map overlay, fixup, thinking clone, thinking overrides/fixup in source order; authored nested undefined, prototypes, UTF-16 and non-finite numbers remain native values |
| JavaScript RegExp used by variant templates | `ara-cli::js_regex`: native `regress` matcher with source/flags, UTF-16 captures and offsets, and shared `lastIndex`; no JavaScript engine in the Rust execution path |
| Non-UTF-8 source strings and exception messages | `ara-rpc::WireString::from_units`, native model records and `CollapseError::message_wire`; local diagnostic Display is not the exception transport |

The default collapse runtime uses `WireModelPolicy`. The optional JSON policy
adapter remains a separate serialized boundary; it is not used to implement
lossless authored records. Host-owned state stays outside the shared Agent Core.
Existing policy/catalogue data and notices are reused.

`regress = "=0.12.0"` with `utf16` is patched to
`crates/vendor/ara-regress`. The original package name resolves the dependency;
this is a third-party dependency, not an OMP `pi-*` crate. Published checksum:
`32eef8b209c3c1c15dbad02c1f30f9539f00dc7253e0cbcdaae442a50a09d7c1`.
Both MIT and Apache-2.0 licenses and release tests/metadata are preserved.
The release-file audit finds all 45 files, with no missing file. Only
`src/parse.rs` and `src/unicode.rs` change matcher behavior; `perf.md` only adds
`https://` to an existing link. Patches and upgrade obligations are in
`ARA-PATCHES.md`. Workspace and CLI manifests/lockfile, module exports, test
replays, oracle scripts and third-party notices are the direct dependencies.

## Executed unchanged-source comparisons

Runtime: Windows, Bun 1.4.0 / JavaScriptCore; Bun SHA256
`627d2e4775c24bdedee2cd7ccc18dcadae061e5345274ab6e3c4c797927bfb8f`.
Receipt root: `C:\Temp\ara-model-variants-batch`.

1. Collapse artifact: `collapse-oracle/run-20260930T232628Z-b13acfd4/oracle.json`,
   SHA256 `23289a1a8fd647facbd375a756e3eeea6ecf9f1592bd0b336b152bef48dc8ba7`.
   **1,995 stateful sequences**, all eight exports, all **4,776 bundled rows**,
   complete reviewed-family presence masks, held references/reverse arrays,
   mutable aliases/templates and custom RegExp state. The original 15 mandatory
   lossless success sequences remain intact. A separate **26-case** native
   policy section exercises seven lossless inputs through resolve/build and six
   ambiguous IDs through both exports, preserving exception names and UTF-16
   messages, including actual U+D800. Artifact bytes and coverage counts are
   pinned; malformed/missing/duplicate or replaced inputs cannot pass the guard.
2. RegExp artifact: `regex-oracle/run-20260930T225953Z-4caa02f0/oracle.json`,
   SHA256 `c596f0006a1c3a41b8e698668b4848726afc80d0592851a42676482372be4ff2`.
   **130 cases**: the original 125 plus five native matcher defect probes.
   The wrapper no longer rewrites the class grammar or scans a BMP fold cache.
3. Existing policy artifact, reused unchanged:
   `C:\Temp\ara-model-policy-batch\oracle\run-20260930T213257Z-4c17f316\oracle.json`,
   SHA256 `53c29b84b31edde4be7dda3e22ddcb2aa2121e43f66dcc6b2c8e3136cb9b1cad`.
   Original **46,136 cases / 270,556 behavior pairs** pass; **11,844** original
   policy/build inputs additionally run through the native Wire adapter.

The first collapse capture reused `file:` URL query modules on this Windows Bun
runtime. This leaked preceding mutable state into 67 reference results; that
capture is superseded, not an acceptance oracle. The final runner uses
byte-identical sibling source modules and checks **1,995 distinct namespaces,
15,960 distinct export functions and 1,995 distinct reviewed tables**. Every
copy matches the fixed source SHA256
`b2cb553c54de36eab88f2ccb1cb1256e3a3137c1f11701bc6cb7fd6e8f58ac6e`.
The final 1,995 sequences preserve the isolated baseline; the policy section is
appended. Source fixture files are retained for coverage evidence; original OMP
test assertions and discovery/model-manager integration suites were not executed.

## Final checks on the delivered snapshot

Each receipt contains the exact command, full log, elapsed time, before/after
18 source pins and oracle artifact hash when applicable. The six final runtime/
build receipts below have identical pins matching the frozen worktree.

| Check / command | Result | Receipt / seconds |
| --- | --- | --- |
| `python -X utf8 scripts/verify_backend.py` (fmt, workspace all-target/all-feature Clippy, target tests and documentation tests) | **1,451 passed / 0 failed / 10 ignored**, 106 suites | `backend-final-20260930T233444Z.json`, **99.589 s** |
| `cargo test -p ara-cli --test model_policy_oracle --all-features -- --ignored --nocapture` | **3/0/0**, original corpus, Wire adapter and rejection guard | `policy-oracle-final-20260930T233732Z.json`, **34.800 s** |
| `cargo test -p ara-cli --test model_collapse_oracle --all-features -- --ignored --nocapture` | **3/0/0**, 1,995 sequences, 26 native policy cases and rejection guard; both mismatch files `[]` | `collapse-delivered-20260930T233952Z.json`, **22.805 s** |
| `cargo test -p ara-cli --test js_regex_oracle --all-features -- --ignored --nocapture` | **2/0/0**, 130 cases and rejection guard; mismatch file `[]` | `regex-delivered-20260930T234415Z.json`, **0.420 s** |
| `cargo test --manifest-path crates/vendor/ara-regress/Cargo.toml --features utf16 --test unicodesets --test unicode_property_escapes --test tests` | **62 + 262 + 147 = 471 passed / 0 failed** | `vendor-regression-final-20260930T233913Z.json`, **7.376 s** |
| `cargo build -p ara-cli --bin ara` | PASS, source unchanged | `build-delivered-20260930T234436Z.json`, **9.480 s** |
| `cargo deny check` | advisories, bans, licenses and sources PASS; existing duplicate and tree-sitter-graphql license-field warnings remain | `deny-final-20260930T233050Z.json`, **7.614 s** |

The deny receipt predates source-only Clippy repairs. Its manifests and lockfile
match the final dependency graph exactly; four source pins are older. It proves
the dependency audit, not a final-source compilation. Earlier fmt/Clippy failures
and lint-before replay receipts remain superseded records, not final gates.
The CLI library suite is **125/0/0**, including 23 new native tests.
Eight ignored corpus replays are executed explicitly above. The other two,
`ara-ast` full repository sweep and
`fixed_source_models_configuration_comparison`, were not rerun in this batch;
they retain their existing verification boundary.

Final binary `target/debug/ara.exe` SHA256:
`8a7daeadfb1aea13f0710e7c60a8df19842e5dbda3f9ee540272d0df7d1e0cf4`
(`binary-delivered.json`). This proves the build, not live catalogue integration.

Frozen production SHA256:

Scoped staged audit (`staged-pin-audit.json`) checks all 69 checkpoint paths,
including all 18 tested source pins and the complete retained matcher release.
63 blobs are byte-exact; six differ only by Git newline normalization. No
vendor target directory or unrelated staged file is included. The committed
blob audit is recorded separately after the WIP commit.

```text
model_collapse.rs    0485ff17482d1321032c7552a0fa1146fe0dd6c218a6e65972ef19f4a02a6f1b
model_wire_policy.rs 7197bfea6540983acd188835fd38b170d53832a89d00f606bb562dec93a19929
model_policy.rs      e2413ec6580aea95dd8bc1f7800a97f9430a6b882064a49c92effd70ca868a3b
js_regex.rs          db4c22f7ce9a112c3b4253f539c401c60546967b55bc42ddbac4f19918110691
vendor/unicode.rs    3d6485fcc53d815801ab8a9426c8c0d3aced862b199f9cbcbf16c623c2b4cb55
vendor/parse.rs      99fdc6af6b90cb7f6251ce2952165ed9da3abfdb2ba4166fa0cb656ff69a274e
```

Regeneration uses `scripts/model_collapse_oracle.py --upstream C:\Temp\omp-596f2da
--bun <pinned-bun.exe> --output <receipt-dir>` and
`scripts/js_regex_oracle.py --bun <pinned-bun.exe> --output <receipt-dir>`.
Set `ARA_MODEL_COLLAPSE_ORACLE`, `ARA_JS_REGEX_ORACLE` or
`ARA_MODEL_POLICY_ORACLE` to the corresponding retained artifact before its
Cargo replay. The fixed scripts reject wrong source/runtime/provenance/counts;
new capture bytes require explicit independent review, not automatic repinning.

## Independent review and acceptance boundary

Independent Codex reviewers: `/root/rpc_owned_core` reviewed native lossless
policy/collapse integration; `/root/ctx_oracle_diff_review` reviewed complete
collapse source, replay and reference isolation. They cross-reviewed matcher
and oracle work outside their authorship scopes. Confirmed prototype/effective
lookup, nested undefined, authored thinking/body, detector facts, UTF-16
exception and native matcher defects were repaired. Final source reviews and
13 Clippy diagnostic repairs approve their bounded scopes; `/root/model_policy_port`
also inspected final full/policy/collapse receipts. Source approval is distinct
from the parent-executed runtime checks.

Decision: **tested WIP**. No new bounded RPC command or phase is accepted.
P0/V1 retain prior acceptance, P1–P6 remain open, RPC remains **27/42** and
`ported_through_commit=null`. This batch has no new Linux gate or real-model
task. Main/registry has not consumed these complete native dependencies yet.
Complete discovery transports/normalization/model-manager, provider overrides,
registry, AuthStorage/OAuth/usage/reserve/refresh/rotation/release, model/role/
thinking/journal adoption, fallback/R2 and every remaining inventory/YAML
boundary remain mandatory. The existing GH013 user choice is pending; no
protection bypass, parity-data deletion, history rewrite or deployment occurred.

The resumed interval is approximately 22:57–23:44 UTC on 2026-09-30, about
47 minutes through final build; the receipt directory existed at 22:21:55 UTC
and is not a precise implementation-start timestamp. Documentation and scoped
commit time follow separately. Reused unchanged policy corpus, cached Bun and
one final full gate; reran only affected final replay checks after source lint.
