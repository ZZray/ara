# AGT-TOKENIZERb: Claude content counter (WIP)

Source is fixed OMP `596f2da7101178214aa27a753529d15e6b7ad91d`, `crates/pi-natives/src/utok/{claude,utf.rs}` and the two `ctok_*.bin.zst` blobs. ARA copied the Claude reconstruction into the Rust-only `ara-ctok` crate, omitting the unrelated BPE encodings and N-API wrapper. The pinned `data/LICENSE.ctok` is preserved verbatim with OMP's root MIT notice in `THIRD_PARTY_NOTICES.md`. Both copied blobs, both fixture files and `LICENSE.ctok` were SHA-256 compared with the fixed checkout and matched:

| File | SHA-256 |
| --- | --- |
| `ctok_v3.bin.zst` | `8791577f60f6c5cfd8b9c85c7cd560c6040e2dd25b9fbe5f6feac7e35e9748b1` |
| `ctok_v4_7.bin.zst` | `6083d7b803771bc55c537cc3a928d36fcc7104b3c802909e2a73d2bf15f7a458` |
| `LICENSE.ctok` | `9ef5a4394eb84d8182c7accd4289e67c6f024e57f0a2bde45ed4ce2844724452` |
| `fixtures.json` | `35fbb455a23e6f28f06744d4730629eab47cc18abcdcce85917dbc8ddbedca97` |
| `sonnet5_live.json` | `6ea5dfc68cf1f13d76a732bb42cb18296ec25614fcdc400d3b30c4b7eb29b6b3` |

The fixed source's `xutf` 1.5 dependency uses nightly `portable_simd` and failed to compile on stable Windows Rust (`E0554`). ARA replaced only its used normalization and Unicode category calls with stable `unicode-normalization` and `unicode-general-category`; the latter and upstream `xutf` default both use Unicode 16. The source's `utf.rs` was trimmed to the methods Claude counting uses. This is an intentional implementation adaptation, not a claim of byte-identical source. The copied source fixtures exercise the adapted normalization.

A temporary oracle outside the repository compiled upstream `xutf` with process-local `RUSTC_BOOTSTRAP=1` solely for comparison; the delivered crate and its tests use stable Rust. For all 1,112,064 valid Unicode scalar values, general category and canonical combining class matched the replacements. NFC output matched for 288 constructed base/mark/tail strings. The `is_nfc` quick checks differed for 184 of those strings, always with `xutf` reporting false and the replacement reporting true; normalized output still matched. This is useful differential evidence, but the 288 strings are not an exhaustive normalization proof.

`ara_ctok::count_content` requires an explicit `ClaudeFamily` and returns content tokens without provider frame, system prompt, tool schema or request transformation. `count_fragments` sums each fragment separately. `check_content_budget` counts with the selected Claude reconstruction every time; it has no raw-byte short circuit. Its result only compares those content tokens with a supplied budget. It is not a full request context gate and is not called by the current Agent loop or provider. Missing/unknown model family has no automatic fallback in this slice.

Executed on this worktree:

| Check | Observed result |
| --- | --- |
| `cargo test -p ara-ctok` on stable Windows Rust 1.98.1 | 8 copied source unit tests and 2 ARA integration tests passed; doc tests passed. The source tests iterate all 493 pinned ctok rows for v3/v4.7/v5 and all 63 pinned Sonnet 5 rows. |
| `cargo +1.94.1 test -p ara-ctok` on Windows | Same 10 tests and doc tests passed. The workspace's declared Rust 1.88 MSRV was not exercised. |
| `cargo clippy -p ara-ctok --all-targets --all-features -- -D warnings` | Exit 0. |
| `cargo deny check` | Exit 0; advisories, bans, licenses and sources ok. Existing duplicate-version warnings remain. |
| `cargo fmt -p ara-ctok -- --check`; `python scripts/verify_bootstrap.py`; `python scripts/omp_inventory.py check` | Exit 0. Full workspace formatting under installed stable Rust 1.98.1 exits 1 on extensive existing vendored `pi-edit` differences; no workspace format pass is claimed. |
| Temporary Unicode differential oracle (`xutf` versus stable replacements) | All 1,112,064 valid scalars match on category and combining class; NFC output matches on 288 constructed strings. The 184 quick-check differences do not change those outputs. |

The direct budget regression checks that Claude v3/v4.7/v5 count `"ξ"` as three content tokens although it is two UTF-8 bytes; a budget of two is exceeded and a budget of three fits. This prevents reintroducing the unsafe byte shortcut found in AGT-TOKENIZERa. The source tests also cover UTF-16/UTF-32 parity, normalization, controls, marker lookalikes and Sonnet 5 trailing-newline behavior.

The AGT-TOKENIZER surface remains **implementing**. Other tokenizer families, model metadata/catalog mapping, provider-wire request sizing, compaction/journal integration, broader normalization and cold/warm performance checks, full backend verification on the final snapshot, a context-overflow real task and point acceptance remain open. No CTX-01d acceptance is inferred from this work.
