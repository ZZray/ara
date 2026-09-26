# AGT-TOKENIZERa: local token estimates (WIP)

Fixed upstream: OMP `596f2da7101178214aa27a753529d15e6b7ad91d`, `packages/agent/src/tokenizer.ts` (`byteEstimate`, `byteLength`, fragment sum, `Tokenizer.countMessage`, and the byte-first branch of `checkTokenBudget`). This is an isolated first A3 slice while CTX-01d remains changes requested; it does not advance either acceptance gate.

Rust owner: `ara-agent::tokenizer`. `count_text` and `count_fragments` implement the source's `ceil(UTF-8 bytes / 4)` per fragment and raw-byte upper bound. `count_message` estimates the message variants already represented by `ara-ai`, including visible thinking, optional opaque reasoning, tool-call name and JSON arguments, and OMP's fixed 1,200-token tool-result image estimate. The Rust-only developer role uses user text rules; the Rust-only assistant image variant uses the tool-result image estimate. Like the source, user image blocks are omitted. Rust messages are owned values, so this first slice measures on demand rather than caching by object identity. The API measures raw messages, not provider wire replay: the current OpenAI adapter omits assistant reasoning and images on replay. Any later compaction trigger must size the provider-transformed context rather than use this raw estimate directly.

`probe_budget` returns `Fits` only when raw bytes are within the token budget. Otherwise it returns `NeedsExactCount`: an approximate token count cannot prove an over-budget condition. It must not be used as a rejecting context-limit gate. This is an intentional incomplete API relative to OMP, whose strict branch invokes a native exact tokenizer.

Executed on this Windows worktree after the change:

| Check | Observed result |
| --- | --- |
| `cargo test -p ara-agent --test tokenizer` | 3 passed: UTF-8 and per-fragment rounding, budget uncertainty, exact block and nested tool-argument estimates, reasoning option. |
| `cargo test -p ara-agent` | 24 integration tests passed (21 loop, 3 tokenizer), doc tests passed. |
| `cargo clippy -p ara-agent --all-targets --all-features -- -D warnings` | Exit 0. |
| `cargo fmt -p ara-agent -- --check`; `python scripts/verify_bootstrap.py`; `python scripts/omp_inventory.py check` | Exit 0 for all three. |
| `cargo deny check` | Exit 0: advisories, bans, licenses and sources ok; duplicate-version warnings remain. No dependency was added by this point. |

Independent Codex subagent reviewed the pre-implementation scope and the implementation diff. It found no blocking defect in the standalone estimator. It identified a wire-replay integration risk, incomplete block assertions, and an image-constant naming mismatch. The API and evidence now explicitly distinguish raw messages from provider replay, the tests assert the block totals and nested arguments, and the constant name matches its use. The reviewer did not run the full suite.

Relevant inventory behaviors exercised in part: B-540a38ae98 (approximate text mode) and B-abee44be18 (raw-byte mode). The AGT-TOKENIZER surface remains open: model tokenizer-family mapping and native exact counting, strict-mode fallback/error behavior, settled-message cache invalidation, provider orchestration token exclusion, and compaction integration are not implemented. A context-overflow real task has not run. Full backend verification for the delivered snapshot remains a separate gate; no AGT-TOKENIZER acceptance is claimed.
