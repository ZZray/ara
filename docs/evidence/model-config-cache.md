# Native models config and cache — tested WIP

## Requirement and boundary

Reproduce **all** fixed OMP `596f2da7101178214aa27a753529d15e6b7ad91d`
before customization. This checkpoint adds native configuration validation,
file loading/migration and SQLite model metadata caching. It does not accept
CA-MODEL-REGISTRY, PKG-CATALOG, R3/R4, a new RPC command or P1–P6. RPC remains
27/42 bounded implementations; `ported_through_commit` stays null.

Base: `3a5c4914bb29a0e0be56a052e496fe754059a435`, clean dev before this batch.
Code WIP commit: `f2ad5bf459f5a319b65628fe035c439b8d4a0fac`.
The delivered snapshot is pinned in the raw gate receipts below. Previous
[model/auth foundation](model-auth-foundation.md) evidence is reused unchanged.

## Fixed source and implementation

| Fixed OMP source | Rust owner and exercised behavior |
| --- | --- |
| `packages/coding-agent/src/config/models-config-schema-bundle.ts`, `models-config.ts`; object/number/morph rules in `packages/omptype/src/{interp,compile}.ts` | `ara-cli::models_config`: complete fixed schema vocabulary and both validation modes. APIs, tokenizer/thinking, OpenAI/Anthropic/Bedrock compat, remote compaction, transport, discovery, provider/custom/override fields, unknown fields and ordered thinking normalization. JS truthiness, arrays-as-objects, empty efforts and legacy reverse ranges follow the source. |
| `packages/coding-agent/src/config/config-file.ts` | `ara-cli::model_config_file::ModelsConfigFile`: Host-injected paths; `.yml`/`.yaml` priority; JSONC migration without deleting original JSON; explicit JSON/JSONC; cached success/error/absence, invalidation, relocation, sync/async load/default/mtime, stage-specific errors and migration notices. |
| `packages/catalog/src/model-cache.ts` | `ara-cli::model_cache::SqliteModelCache`: native schema 12/WAL/secure-delete/busy timeout, old-version invalidation, sparse specs, all headers omitted with restore markers, fingerprints, TTL, explicit/shared handles, corruption-only quarantine and one retry. |

Only these new Host modules, their exports, comparison runner/test and notices
change. Core, live route, credentials, provider requests, product installations
and unrelated WIP are untouched. No new dependency or baseline SHA change.

## Executed evidence

Raw receipts: `C:\Temp\ara-model-config-batch`.

Final focused gate: `focused-receipt.json`, `focused-{0,1}.log`:

1. `cargo test -p ara-cli --lib --all-features`: **78/0/0**. This includes 38
   new tests: schema 16, file handle 7, cache 15. Tests use isolated actual
   filesystem/SQLite instances, async reads, physical corruption, write/read
   restart, legacy migration, 3-second lock failure and a separate child process.
2. `cargo test -p ara-cli --test model_config_oracle --all-features -- --ignored --nocapture`
   with `ARA_MODEL_CONFIG_ORACLE` pointing to the retained oracle: **1/0/0**,
   **842** successful source-to-Rust comparisons. Final focused time **7.286 s**
   (library 5.028 s, oracle replay 2.256 s), with 11 source pins unchanged.
3. `python -X utf8 scripts/model_config_oracle.py --upstream C:/Temp/omp-596f2da --bun C:/Temp/ara-ctx-skill-sort-oracle/bun-windows/bun-windows-x64/bun.exe --output C:/Temp/ara-model-config-batch/oracle`:
   actual **Bun 1.4.0**, 17 unchanged raw Git source blobs plus the MIT license.
   Run `run-20260930T195851Z-214beeef` retains exports, manifest, runner, stdout,
   stderr and `oracle.json`. SHA256 and Git blob guards run before imports.

The oracle executes the complete unchanged omptype, schema bundle, business
validator and ConfigFile. Package facades supply `once`, a temporary default
directory, ENOENT classification and a log sink; schema/parser/validation/I/O
implementations are not rewritten. It compares success and normalized values,
exact provider errors, and real file load statuses/stages. It does not compare
all schema diagnostic wording or every possible YAML input. The comparison is
explicitly ignored in ordinary Cargo runs and executed separately with its
retained oracle; an absent oracle cannot become a passing comparison.

The first package Clippy probe rejected four mechanical lints (nested if,
one-element loop and `to_digit().is_some()`); these were fixed without relaxing
policy. The final focused snapshot includes those repairs.

Final frozen-snapshot Windows gate: **PASS, 1,404 passed / 0 failed / 2 ignored**,
103 suites. Owned-package formatting, all-feature/all-target Clippy and tests,
workspace doc tests, `cargo deny check` and binary build pass. Total **102.080 s**:
backend 99.306 s, dependency 2.431 s, binary build 0.339 s. All 11 source pins
are unchanged through the final gate. Raw receipt `full-receipt.json` and
`full-{0,1,2}.log`. The two ignored tests are not claimed executed by that gate;
the new fixed-source comparison is executed separately in the focused receipt.
`verify_bootstrap.py`, `omp_inventory.py check` and `git diff --check` also pass.
Final binary SHA256:
`95acbf7bf37983d9dcca899175dbf549cb0dc5ecbd285fc18d85c6e0a69034db`.
Staged and committed pin audits pass for all 11 tested files: eight byte-exact,
three unchanged dependency/parser files differ only by Git CRLF normalization.
Receipts: `staged-pin-audit.json` and `committed-pin-audit.json`.
Linux and real-model trials: not executed for this disconnected foundation.

## Independent review and repairs

- Independent Codex `ctx_oracle_diff_review`: implementation-before-source
  scope review; subsequent full cache/file-handle review. Four loader P2s were
  found and repaired: JSONC invalid comma acceptance, same-path relocation
  splitting caches, overflowing numeric radix saturation and YAML migration
  changing octal-looking string types. New fixtures exercise the repairs.
- Independent Codex `host_bridges_independent_review`: complete schema/16-test
  diff plus fixed source/omptype semantics; no confirmed defect or missing
  schema field. Final mechanical-hunk review resolved the lint repairs on
  `f7b5511f095cdd5999550e0f9a05b5b0ea356e0ed4f0db7fb51b9416351e94ad`.
- Independent Codex `rpc_owned_core`: complete comparison runner/test review;
  found a P2 missing-family guard gap. The test now requires schema 485,
  file 11 and both provider modes 173 each, total 842, and rejects unknown modes.
  Final independent fault verification used the actual built test executable:
  complete oracle PASS (0.08 s); removing all file cases and changing one mode
  each fail with exit 101. Original source/oracle hashes remained unchanged.
  Receipts: `oracle-guard-faults/run-20260930T201615Z-34f2bc27/receipt.json`.
  This reviewer did not independently review its own schema.

Final cache/loader review resolves all four P2s with no new confirmed defect;
it independently rehashed both focused logs and source pins. Frozen hashes:

```text
models_config.rs     f7b5511f095cdd5999550e0f9a05b5b0ea356e0ed4f0db7fb51b9416351e94ad
model_config_file.rs 0f545f06f08f877464b4ea405f6d8b4f95943ee36d55685ea451b1c4d12c038f
model_cache.rs       05d5bac45eee12e7fdff7ece9dfb53aee3e888a433bf63af41e96a19148b71fb
model_config_oracle.rs ecb1168a5fbc08895ae260b8432f24c703289b7f6d0e4b5384ef2b62a9f45c43
```

The parent reads fixed source and verifies findings, code and executed outputs.
Final reviewer hashes/results are retained separately from prose evidence.

## Mandatory remaining behavior

This batch is **WIP**, with the following still mandatory for full parity:

- Registry constructor/reload/pure availability, settings/roles/route metadata,
  provider discovery/network/background lifecycle and configured-auth queries.
- Full `buildModel`/compat/identity/reference/aliases, custom model defaults,
  merge/apply/discovery rebuild and explicit value re-assertion. The complete
  catalogue cannot yet project a full executable Rust route.
- Live header/env/command resolution, auth precedence, provider OAuth and
  refresh, usage/reserve/rotation, lifecycle and full fallback apply/revert.
- Host/RPC model/role/thinking commands and native journal/adoption saga using
  the original Agent continuation; remaining R2 recovery and all inventory rows.
- The shared YAML parser's documented Bun edge differences, including native
  non-finite values. This loader rejects those explicitly instead of silently
  saturating into valid schema numbers. Retaining unknown non-finite values
  and exact associated diagnostic stages remain open requirements.

Rust rendering of structured schema paths and always-quoted YAML strings are
language/serialization adaptations. Migration values round-trip; literal YAML
formatting is not asserted identical. Cache errors return `Result` with receipts
instead of disappearing; the future Host registry must preserve OMP best-effort
resolution by handling/reporting them. Header restoration, sparse-spec rebuild
and static fingerprint computation remain manager duties; cached metadata is
never an authenticated executable route. The specialized handle does not claim
the generic ConfigFile schema/validator API for unrelated settings families.

Full parity and formal phase acceptance cannot advance on these helper tests.
