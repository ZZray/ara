# AGT-TOKENIZERc: model-selected Claude content counts (WIP)

Fixed OMP source: `596f2da7101178214aa27a753529d15e6b7ad91d`, `packages/catalog/src/model-tokenizer.ts:20-30,55-67`, `packages/catalog/src/build.ts:202-210`, and `packages/agent/src/tokenizer.ts:12-25,144-156`. OMP materializes an optional tokenizer family on `Model`; an explicit catalog choice wins over identity resolution, and Agent counting consumes that field. ARA has not ported OMP's full catalog taxonomy. Its `ara-ai::resolve_known_claude_tokenizer` recognizes canonical Claude names conservatively; aliases and unsupported/future names require an explicit host choice. The reference CLI accepts `--tokenizer` or `ARA_TOKENIZER` (`auto`, `none`, or one of the four Claude families). This metadata does not change the wire model ID.

`ara-agent::tokenizer::count_model_fragments` reads the host-selected `Model.tokenizer` and uses `ara-ctok` to count each text fragment with the chosen Claude family. Unknown metadata returns `UnknownTokenizer`, and checked-sum overflow returns `CountOverflow`. It counts content text only. The Agent loop does not call this API yet, and the OpenAI adapter's system, assistant, tool and image transformations mean the result is **not** a full provider-request token count. It must not trigger a hard context gate or compaction in this state. Other model families, full model catalog identity resolution, actual request projection, and context-overflow behavior remain open.

Later WIP AGT-TOKENIZERd moves this function's implementation into `ara-ai::model_tokenizer` for use on the adapter's final request value and re-exports it from the original Agent path. See [the prepared-request receipt](agt-tokenizer-request-text.md). The original selection evidence above remains the result for WIP `7cf48f1`.

Tests run on Windows on the candidate code committed locally as WIP `7cf48f1`:

| Check | Result |
| --- | --- |
| `cargo test -p ara-ai --lib` | 31 passed, including Claude generation boundaries and unknown names. |
| `cargo test -p ara-agent` | 21 loop and 4 tokenizer integration tests passed; doc tests passed. The content tests include the `ξ` byte counterexample, model-family change, and V5 versus V5Sonnet trailing whitespace. |
| `cargo test -p ara-cli --bin ara` | 5 passed, including canonical route, alias override, explicit disable and invalid value. |
| `cargo test -p ara-context --test upstream` | 21 passed after updating its `Model` construction. |
| `cargo clippy -p ara-ai -p ara-agent --all-targets --all-features -- -D warnings`; scoped `cargo fmt` | Exit 0. |
| `cargo deny check`; `python scripts/verify_bootstrap.py`; `python scripts/omp_inventory.py check` | Exit 0; deny reported existing duplicate-version warnings. |
| `python -m py_compile scripts/verify_backend.py` | Exit 0. The verifier now invokes the resolved `cargo.cmd` path and decodes Cargo metadata as UTF-8 on Windows. |

Full backend verification is **not passed**. `python scripts/verify_backend.py` now reaches format and Clippy, then stops because several `ara-discovery` integration tests use `std::os::unix::fs::symlink` on Windows. Strict CLI Clippy also reaches pre-existing Windows warnings in `ara-tools` (`bash.rs`, `write.rs`). Separately, `cargo test -p ara-ai --test openai_http` passed 16/17: `read_error_after_finish_keeps_completed_response` reproducibly received `StopReason::Error` on Windows rather than `Stop`. The fake upstream immediately sends an RST after writing SSE frames; the receiver can see the reset before parsing the finish frame. This failure has not been reclassified as a product pass or fixed by changing provider semantics. Full backend, actual host task and real-model context-overflow trials remain required before acceptance.

An independent Codex subagent reviewed the plan against the fixed OMP catalog and ARA model/provider boundaries before implementation. A second independent Codex subagent inspected the post-implementation diff (including the lockfile), ran the three focused model-selection tests and `git diff --check`, and found no confirmed actionable defect. Its read-only comparison of 433 pinned catalog rows with Claude tokenizer metadata recognized 252, left 181 unresolved (largely Bedrock-style names), found 0 conflicts among recognized rows and did not assign a Claude family to a row whose upstream tokenizer is missing. This is review evidence for the subset, not a claim of complete catalog parity or a full backend pass.
