# AGT-APPEND-CTX original surface closure — 2026-10-09

## Requirement and delivered scope

Fixed OMP is `596f2da7101178214aa27a753529d15e6b7ad91d` (v18.1.8).
The surface owns `packages/agent/src/append-only-context.ts` and all 59
cases in `packages/agent/test/append-only-context.test.ts`. Its behavior includes
stable prompt/tool prefixes, invalidation and version/fingerprint changes,
append-log identity/copies, synchronization/reset, message mutation and tool
normalization, intent and examples. The tool producer also uses the fixed
`agent-loop.ts:825-920`, `dialect/examples.ts` and `rendering.ts` contracts.

The reviewed eight-file implementation is applied to the actual repository,
with one additional original Host test constructor migration (`::default()` in
two existing calls). Existing assertions and the five Core/six Host entrances
remain. Other dirty work and the Git index are preserved. Extension SDK/Host
writer candidates are a separate delivery and are not included in this one.

| Delivered file | Role |
| --- | --- |
| `crates/ara-agent/src/append_only_context.rs` | Typed/Graph prefix domain and metadata |
| `crates/ara-agent/src/append_only_context/actual.rs` | Same-instance original prefix, cache and log owner |
| `crates/ara-agent/src/provider_context.rs` | Actual original-tool source consumer |
| `crates/ara-agent/src/provider_context/graph_tools.rs` | Original map/intent/examples/fingerprint order |
| `crates/ara-ai/src/schema_wire/graph.rs` | Borrowed same-heap wire entry points |
| `crates/ara-ai/src/schema_wire.rs` | Wire exports |
| `crates/ara-ai/src/lib.rs` | Public borrowed wire exports |
| `crates/ara-agent/tests/append_only_context.rs` | All original associations in the existing five banks |
| `crates/ara-cli/tests/append_only_context_host.rs` | Existing six Host tests; constructor migration only |

All receipts below are in the current local packet:
`D:/workspace/env/cache/ara-port-candidates/extension-production-batch-20261009`.
No API key, local account/profile data or unredacted authorization is copied
into this repository. No commit, push, deployment or product installation was
performed.

## Exact build and original executable groups

`append-live-build-v3-manifest.json` pins the actual repository sources before
the build; SHA256 is
`63b9dbaee6abe34e8a912d3a0ebd2cf7ac23b6f961b78fffd7f821ea58f82c98`.
The application receipt preserves each original/delivered file hash and backup
mapping. The build command is:

```text
cargo build --locked --keep-going -p ara-agent -p ara-cli --bin ara --test append_only_context --test append_only_context_host --all-features --message-format=json
```

This compiles the two original test harnesses and the actual production CLI,
without the unused CLI binary unit-test harness. On the installed Cargo,
`cargo test` does not support `--keep-going`; its rejected 4.7573852-second
attempt remains a failure receipt and is not counted as a build/test pass.

The delivered build passes in **16.5636144 s**, 413 reused artifacts, three
compiled artifacts, zero compiler errors. `run-append-live.ps1` verifies source
pins and selects the two `profile.test=true` compiler artifacts from this build.

| Original group | Actual result | Seconds | Executable SHA256 |
| --- | --- | ---: | --- |
| Core `append_only_context` | 5 passed / 0 failed / 0 ignored | 0.1024294 | `f838b19473f6f7542add7e04efb511fdc13571aae80641ca0b144a1b3ab16c8f` |
| Host `append_only_context_host` | 6 passed / 0 failed / 0 ignored | 0.6469545 | `9ea3e4fb3ac39061d6e0c3ae083e60f6146911edfe62db14793e20e6678e1180` |

Core banks are `cache_and_source_projection_group`,
`normalizer_legacy_metadata_and_examples_group`,
`loop_intent_atomic_admission_and_live_side_group`,
`owned_pair_and_visible_schema_failure_group`, and
`owned_post_hook_snapshot_empty_bypass_and_side_hook_group`.
The 59-row source/case/bank/observation mapping is recorded below; the test
count is not used as a substitute for that behavior coverage.

The existing groups include thrown Get/schema failures, reentry and partial
state, admission/abort/error paths, owned pairing, candidate publication and
isolation, failed settings load/store, reload/drop, and real RPC settings,
prompt and fresh-Session behavior. Compiler failures from earlier candidate
versions are retained, not rewritten as passes.

## Current actual model task and effects

Script `verify-append-context-cas-live-v3.py` preserves the reviewed original
task/oracles and 6+2 model-call limits; only source/output/build selection is
adapted. Script SHA256 is
`4035ad1d242d8b02a8afbe548c0cc19f178195bcaca7295257e693d8d0d68d0a`.
Receipt: `append-context-cas-receipts-live-v3/20261009T023645Z/receipt.json`.
It selects the unique non-test `ara` executable from the same passed build:
SHA256 `9a2d02ac37494191fa5f9f536f355468db6017542d4a58987f15627a8dae2579`.
Source and executable hashes remain unchanged through the actual task.

- Model: live configured `deepseek-v4.1-flash`, `openai-completions`.
- Route: CAS configured in ARA Manager. The local OMP management route probe
  returned `URLError`; that unavailable transport chain is not accepted.
- Isolated `appendOnlyContext:on`; no fallback; at most six task calls plus
  two recall calls, 120 s per Run and 8,192 output tokens.
- Random CSV is pen quantity 4 at 2.75, book quantity 4 at 11.5 and cup
  quantity 3 at 4.25. Actual edit then readback produces
  `{"pen":11.0,"book":46.0,"cup":12.75,"grand_total":69.75}`.
- First Run: four model calls, four successful tools, **6.438 s**.
- Restart/recall Run: one model call, zero tools, **2.324 s**. Its prompt
  contains no answers; it recalls pen quantity 4, book unit price 11.5 and
  cup quantity 3 from the earlier conversation.
- Both processes retain Session `01a11e85-370b-7463-a79a-c99344ff4327`.
  All terminal Assistant messages stop on the selected model/route. Actual
  usage is in the receipt; absent usage fields remain unknown.

## Evidence boundary and independent decision

The controlled Ark producer exercises the real Function/Get/Call/receiver,
conversion options/fallback and original wire stamp contracts. It does not
claim to execute or port the upstream ArkType package internals. The original
tool collection remains in the same heap; it is not pre-exported to serde.

Message mutation follows the original estimate/version ownership contract;
the controlled replay case also publishes that version. An aliased-object
assertion alone is not treated as proof of a fresh replay. Typed and Graph
prefix domains force the appropriate rebuild even when fingerprints match.

Complete Extension, other providers, P1–P6, product integration, packaging,
dependency/platform release gates and `ported_through_commit` remain open.
Passing these Runs does not automatically accept a product Task.

Independent Codex `/root/extension_integration_review` has reviewed the fixed
source, all eight changed files, both mechanical test migrations and actual
delivery scope in `ROOT-SOURCE-REVIEW.md`. Final independent execution audit
completed at `2026-10-09T02:44:32.3132349Z`: **APPROVE the complete
AGT-APPEND-CTX surface and all 59 original behavior associations**. There are no
remaining mandatory runtime/model/negative checks for this surface.
Report SHA256: `c41d31a286747df408c22876f8db77788df6382e254d4117d85cb57e5ae3f687`.
The reviewer checked all 863 build source pins, 838 unchanged CAS pins, the
three same-build executable hashes, all 59 unique source associations, original
Core/Host negative cases, task artifact arithmetic and the persisted same-Session
restart without tools. Root directly rechecked the report, delivered Core test,
Graph producer and archived production executable hashes, and actual receipts.
Only AGT-APPEND-CTX advances to `accepted`: **1 accepted, 1 tested,
27 implementing, 82 open, total 111**. `ported_through_commit` stays null.
The same production executable is preserved at
`append-delivered-artifacts/ara.exe` in the packet to protect this receipt from
later Cargo builds overwriting `debug/ara.exe`.

## Original 59 behavior associations

| ID | Fixed test line | Original behavior | Executed Core bank | Observed assertion | Receipt |
| --- | ---: | --- | --- | --- | --- |
| B-7d0d1aa595 | 45 | StablePrefix > builds and returns cached system prompt + tools | cache_and_source_projection_group | first build; built/version/prompt/tools | live Core PASS; independent audit APPROVE |
| B-68b6320a1d | 62 | StablePrefix > returns false on identical rebuild | cache_and_source_projection_group | identical rebuild false/version stable | live Core PASS; independent audit APPROVE |
| B-609e2fb8e4 | 71 | StablePrefix > returns true when system prompt changes | cache_and_source_projection_group | prompt change rebuild and fingerprint | live Core PASS; independent audit APPROVE |
| B-9d72d66676 | 80 | StablePrefix > returns true when tools change | cache_and_source_projection_group | tools list change rebuild and fingerprint | live Core PASS; independent audit APPROVE |
| B-6dc72c50a5 | 88 | StablePrefix > returns true when tool description changes | cache_and_source_projection_group | description change rebuild and fingerprint | live Core PASS; independent audit APPROVE |
| B-41745adbc5 | 96 | StablePrefix > invalidate forces rebuild | cache_and_source_projection_group | invalidate then rebuild same inputs | live Core PASS; independent audit APPROVE |
| B-ce24ba9623 | 108 | StablePrefix > toContext() throws when not built | cache_and_source_projection_group | PrefixNotBuilt before build | live Core PASS; independent audit APPROVE |
| B-21816f98e6 | 113 | StablePrefix > fingerprint changes across rebuilds | cache_and_source_projection_group | old/new prompt fingerprints differ | live Core PASS; independent audit APPROVE |
| B-18a1a4069e | 126 | StablePrefix > fingerprint stable for identical context | cache_and_source_projection_group | fresh identical prefix fingerprints equal | live Core PASS; independent audit APPROVE |
| B-c408c9cd7d | 137 | StablePrefix > version increases on each rebuild | cache_and_source_projection_group | versions 0/1/2/unchanged2 | live Core PASS; independent audit APPROVE |
| B-24cf0830df | 157 | AppendOnlyLog > starts empty | cache_and_source_projection_group | empty log and messages | live Core PASS; independent audit APPROVE |
| B-98e44c53a4 | 163 | AppendOnlyLog > appends messages | cache_and_source_projection_group | user/assistant append length2 | live Core PASS; independent audit APPROVE |
| B-70f1fb3c45 | 171 | AppendOnlyLog > toMessages returns a copy of the array | cache_and_source_projection_group | popped returned copy leaves log length2; actual slice identity | live Core PASS; independent audit APPROVE |
| B-473937592e | 181 | AppendOnlyLog > replaceTail replaces last entry | cache_and_source_projection_group | tail replaced length2 and last value | live Core PASS; independent audit APPROVE |
| B-737670ef32 | 189 | AppendOnlyLog > replaceTail is no-op on empty log | cache_and_source_projection_group | empty replace tail stays empty | live Core PASS; independent audit APPROVE |
| B-5870f0670c | 195 | AppendOnlyLog > extend appends multiple messages | cache_and_source_projection_group | clear then extend two members | live Core PASS; independent audit APPROVE |
| B-f02421af13 | 204 | AppendOnlyLog > clear resets the log | cache_and_source_projection_group | clear empties log | live Core PASS; independent audit APPROVE |
| B-2aa3e06b05 | 211 | AppendOnlyLog > entries readonly access returns internal array | cache_and_source_projection_group | entries preserves ordered members; actual entries array identity | live Core PASS; independent audit APPROVE |
| B-76582927b9 | 223 | AppendOnlyContextManager > build() returns context with stable prefix on first call | cache_and_source_projection_group | manager first build prompt/tools/empty messages | live Core PASS; independent audit APPROVE |
| B-c323d777f1 | 237 | AppendOnlyContextManager > build() returns same systemPrompt and tools on subsequent calls | cache_and_source_projection_group | manager repeated cached prompt/tools | live Core PASS; independent audit APPROVE |
| B-f47bd53710 | 252 | AppendOnlyContextManager > build() detects changed system prompt and rebuilds | cache_and_source_projection_group | manager changed prompt returned | live Core PASS; independent audit APPROVE |
| B-e8843a6a39 | 260 | AppendOnlyContextManager > prefix.fingerprint changes when tools change | cache_and_source_projection_group | manager tool-list fingerprint changes | live Core PASS; independent audit APPROVE |
| B-1eb29a6ff6 | 272 | AppendOnlyContextManager > appendMessage grows the log | cache_and_source_projection_group | manual append q1/a1 appear in build | live Core PASS; independent audit APPROVE |
| B-94975ad4d7 | 285 | AppendOnlyContextManager > appendMessage messages appear in every subsequent build() | cache_and_source_projection_group | build after each append retains prior messages | live Core PASS; independent audit APPROVE |
| B-dc0705408f | 299 | AppendOnlyContextManager > invalidate forces prefix rebuild | cache_and_source_projection_group | manager invalidate next build prompt | live Core PASS; independent audit APPROVE |
| B-148d9d22f5 | 308 | AppendOnlyContextManager > reset clears log and prefix | cache_and_source_projection_group | manager reset fresh prompt/empty log | live Core PASS; independent audit APPROVE |
| B-e29e55c8c1 | 321 | AppendOnlyContextManager > replaceTailMessage updates last log entry | cache_and_source_projection_group | manager replace-tail returned new tail | live Core PASS; independent audit APPROVE |
| B-d083c14f69 | 332 | AppendOnlyContextManager > build propagates tool spec description default | cache_and_source_projection_group | original Graph tool lacks description; normalized empty/source still absent | live Core PASS; independent audit APPROVE |
| B-04c0d8c3bf | 345 | AppendOnlyContextManager > tools returned from build are frozen in the cache | cache_and_source_projection_group | repeated actual root tools same_identity | live Core PASS; independent audit APPROVE |
| B-f24e204b66 | 358 | AppendOnlyContextManager > tolerates context with no tools | cache_and_source_projection_group | manager no-tools returned empty | live Core PASS; independent audit APPROVE |
| B-ec275fd0b1 | 372 | fingerprint determinism > identical context produces identical fingerprint | cache_and_source_projection_group | two StablePrefix instances equal fingerprints | live Core PASS; independent audit APPROVE |
| B-2e27c43a67 | 387 | fingerprint determinism > tool order changes fingerprint | cache_and_source_projection_group | swapped names/list order changes fingerprint | live Core PASS; independent audit APPROVE |
| B-9fa1801305 | 402 | fingerprint determinism > system prompt array structure changes fingerprint | cache_and_source_projection_group | A,B versus A-newline-B fingerprints differ | live Core PASS; independent audit APPROVE |
| B-1fac309c5d | 420 | message sync > syncMessages on first call appends all messages | cache_and_source_projection_group | initial sync all two members | live Core PASS; independent audit APPROVE |
| B-a185e67b1e | 436 | message sync > syncMessages on subsequent calls only appends delta | cache_and_source_projection_group | typed delta plus actual long growth retains prefix | live Core PASS; independent audit APPROVE |
| B-ffccd9c3d4 | 453 | message sync > syncMessages with unchanged messages is a no-op (same length, no new entries) | cache_and_source_projection_group | unchanged typed length and actual steady identities | live Core PASS; independent audit APPROVE |
| B-cf1a52eee6 | 465 | message sync > syncMessages resets log when array shrinks (compaction) | cache_and_source_projection_group | actual shrink replays only new member | live Core PASS; independent audit APPROVE |
| B-aa4c2e6d2f | 482 | message sync > build + syncMessages integration: messages come from log, not from context.messages | cache_and_source_projection_group | build uses synced log; original returned root slice | live Core PASS; independent audit APPROVE |
| B-c6533365e6 | 502 | message sync > resetSyncCursor forces full re-sync on next call | cache_and_source_projection_group | reset cursor replays fresh typed/actual state | live Core PASS; independent audit APPROVE |
| B-077080f90b | 515 | message sync > preserves the byte-stable prefix when a deep message is rewritten (#3406) | cache_and_source_projection_group | fresh assistant rewrite keeps original prefix member | live Core PASS; independent audit APPROVE |
| B-b9824b0dff | 540 | message sync > detects tool-result metadata-only rewrites before preserving a later prefix (#3406) | cache_and_source_projection_group | tool-result metadata-only replaces suffix | live Core PASS; independent audit APPROVE |
| B-a851141f27 | 576 | message sync > detects providerPayload-only rewrites before preserving a later prefix (#3406) | cache_and_source_projection_group | providerPayload-only replaces suffix | live Core PASS; independent audit APPROVE |
| B-6f9db809a6 | 619 | message sync > does not reuse a stable prefix longer than the current log after direct log clear (#3406) | cache_and_source_projection_group | actual public clear leaves cursor then repopulates bounded log | live Core PASS; independent audit APPROVE |
| B-d5baba7cdf | 645 | message sync > preserves the prefix when the tail is rewritten (#3406) | cache_and_source_projection_group | tail-only change preserves first two members | live Core PASS; independent audit APPROVE |
| B-67955b1e57 | 669 | message sync > appended new messages keep the prefix stable even when the prior tail also diverged (#3406) | cache_and_source_projection_group | rewrite prior assistant plus appended tail preserves first | live Core PASS; independent audit APPROVE |
| B-3b97105c90 | 692 | message sync > rewriting the first message still re-syncs from scratch | cache_and_source_projection_group | first message change replaces first identity | live Core PASS; independent audit APPROVE |
| B-0e1ff14281 | 707 | message sync > no-op when content unchanged | cache_and_source_projection_group | fresh equal clones preserve original identities | live Core PASS; independent audit APPROVE |
| B-7302b66e7c | 725 | message sync > invalidateForModelChange resets prefix and log | cache_and_source_projection_group | model invalidate prefix/log and subsequent rebuild | live Core PASS; independent audit APPROVE |
| B-1a97512433 | 744 | message sync > keeps sync decisions byte-faithful across steady-state, growth, rewrite, and revert | cache_and_source_projection_group | 8-member steady/growth/split5/revert actual original members | live Core PASS; independent audit APPROVE |
| B-64b5f350e1 | 793 | message sync > re-syncs an in-place rewritten message once invalidation bumps its version | cache_and_source_projection_group | owner content assignment/version bump/fresh replay original identity | live Core PASS; independent audit APPROVE |
| B-f32d28bc6d | 823 | message sync > treats fresh-object clones with identical bytes as stable and still detects real rewrites | cache_and_source_projection_group | fresh equal clones retained; real fresh rewrite replaces suffix | live Core PASS; independent audit APPROVE |
| B-134f75ea6b | 859 | intent injection through build() > injects required `i` into tool schemas when intentTracing is true | normalizer_legacy_metadata_and_examples_group | manager build intent true contains required i | live Core PASS; independent audit APPROVE |
| B-3e88c98ac2 | 875 | intent injection through build() > materializes ArkType params and keeps `i` first in authored order | cache_and_source_projection_group | original Function/toJsonSchema/assert conversion Call; i,path,content order | live Core PASS; independent audit APPROVE |
| B-ee531bb4a8 | 888 | intent injection through build() > omits `i` when intentTracing is false | normalizer_legacy_metadata_and_examples_group | manager intent false no properties i/no required i | live Core PASS; independent audit APPROVE |
| B-ff691e76b0 | 903 | intent injection through build() > intentTracing flip invalidates the fingerprint cache | normalizer_legacy_metadata_and_examples_group | manager false/true/false cache flip and original fingerprint | live Core PASS; independent audit APPROVE |
| B-1adad9aaf5 | 924 | tool examples injection through build() > always injects Python-syntax examples | normalizer_legacy_metadata_and_examples_group | manager examples Python Find files and find(paths) | live Core PASS; independent audit APPROVE |
| B-5824fdf447 | 936 | tool examples injection through build() > injects the `i` placeholder into examples when intentTracing is on | normalizer_legacy_metadata_and_examples_group | manager examples intent placeholder on build | live Core PASS; independent audit APPROVE |
| B-5e3438f1ba | 946 | tool examples injection through build() > examples presence flip invalidates the fingerprint cache | normalizer_legacy_metadata_and_examples_group | manager examples absent/present invalidation changes fingerprint | live Core PASS; independent audit APPROVE |
| B-2d432382f0 | 967 | syncMessages detects tool_calls mutation > rebuilds the log when tool_calls is mutated in place | cache_and_source_projection_group | wire tool_calls argument assigned; owner version; following equal member replaced; replay | live Core PASS; independent audit APPROVE |

