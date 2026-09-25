# CTX-01a: prompt template engine, helpers, `format`

Point, requirement and exclusions:
Port OMP's Handlebars-compatible template engine, the shared prompt helpers, the compile cache, `render`, and the `format` normalizer as the Core crate `ara-prompt`.

Excluded:
- Coding-agent helpers `jtdToTypeScript` and `renderYieldSchema`. They arrive with subagents (A6).
- Function partials and runtime `helpers`/`partials`/`data` options. No upstream prompt uses them.

Upstream SHA and source/test location:
`596f2da7101178214aa27a753529d15e6b7ad91d`.
- Source: `packages/utils/src/{template,prompt}.ts`.
- Tests: `packages/utils/test/{template,prompt}.test.ts` and `packages/coding-agent/test/prompt-format.test.ts`.
- 36 behavior IDs are named per test in `crates/ara-prompt/tests/upstream.rs`.

Rust entry:
`ara_prompt::{Engine, render, compile, format, register_helper, register_partial}`.
- The Core has no runtime dependency outside Rust.
- `scripts/prompt_oracle.ts` is an optional offline tool. It ran upstream's TypeScript to produce the committed fixture `tests/fixtures/oracle.json`. Building, testing and CI need only Rust.

Environment and sanitized commands:
- `python scripts/verify_backend.py`: PASS (580 tests, 1 upstream-ignored).
- Upstream ports: `cargo test -p ara-prompt --test upstream`, 16 tests covering 36 behaviors. All 5 upstream goldens (captured from handlebars 4.7.9) match.
- Differential fixture: `cargo test -p ara-prompt --test oracle`. 700 generated cases (render, escaping engine, format), each equal to upstream's recorded output or error.
- Corpus sweep: `ARA_PROMPT_CORPUS_ORACLE=… ARA_OMP_ROOT=<omp> cargo test -p ara-prompt --test oracle`. For all 274 upstream `packages/*/src/**/*.md` files, the compile outcome, the prompt-source `format` and the post-render `format` equal upstream's (135 templates compile). Without the env vars the test skips itself.
- Review regressions: `cargo test -p ara-prompt --test review`, 8 tests.

Expected vs actual normal result:
- Goldens, JS truthiness and each-over-falsy entries, `@index`/`@key`/`@first`/`@last`/`../`/`@root`, subexpressions and hash args, the escape entity set, triple-stache and `SafeString` all behave as upstream.
- Helpers: `arg`, `list`, `join`, `default`, `pluralize`, `when`, `ifAny`, `ifAll`, `table`, `codeblock`, `xml`, `escapeXml`, `len`, `add`, `sub`, `has`, `includes`, `not`, `jsonStringify`.
- `format`: ASCII symbols outside HTML comments, RFC 2119 normalization, table compaction, blank collapsing, and blank removal before closing XML tags and pre-render `{{/…}}`.
- JS own-key order, `Number#toString` exponent forms, and `NaN`/`Infinity` rendering.

Actual: as expected.

Expected vs actual failure/recovery result:
- Upstream's own messages: `Missing helper: "x"`, `Parse error: mismatched /if`, `Parse error: unclosed block if`, `Parse error: unclosed template expression`, `Parse error: unclosed comment`, `The partial p could not be found`.
- 2000 nested blocks, or 5000 nested subexpressions, on a 2 MiB thread return `Template nesting exceeds 100 levels` instead of aborting the process.
- A helper that renders and registers on the shared engine while another thread registers completes (`outer inner 1`) without deadlock.
- Each over 20,000 items and 500 lookups over a large root is linear, well under 10 s in a debug build.

Actual: as expected.

Intentional differences:
1. A `{{else if}}` chain closes with one `{{/if}}`, as in Handlebars. Upstream reports `unclosed block if`, and accepts a double close that ARA rejects. No upstream prompt uses `else if`.
2. `Value::Null` stands for JS `undefined` (missing keys and host `None`).
3. Missing positional helper arguments read as undefined. Upstream passes its options object into that slot.
4. Nesting is capped at 100 levels. Upstream overflows the JS stack at about 10^5.
5. `NaN`/`Infinity` results are carried as their text, so they render the same but are truthy.
6. Non-string `list` `join` and `table` `headers` fail with an ARA error text where upstream throws a JS `TypeError`.

Independent review:
The review ran with the `ara-git-review` and `ara-rust-core-review` procedures, as a forked read-only agent with bun and Rust probes. It confirmed M1–M3 and L1–L6; all are fixed and each has a test in `tests/review.rs`.
- M1: quadratic `each` from cloning the whole root.
- M2: stack overflow on deep nesting.
- M3: RwLock deadlock on helper re-entry.
- L1: integer-key order.
- L2: non-finite arithmetic.
- L3: number exponent format.
- L4: JS `\s`/`\b`.
- L5: partial registry.
- L6: double-close documentation.

`Object.prototype` operator names in `when` stay unported (difference 6 class). About 300k fuzzed inputs produced no panics.

Real-model route:
Not applicable on its own. The engine reaches a model through the system prompt in CTX-01d, which carries the trial.

Decision: accepted on 2026-09-25, on the [CTX-01d real-model trial](ctx-01d-system-prompt.md) (Agnes `agnes-2.5-flash`, OpenRouter `openrouter/free`, both TRIAL PASS on the delivered code).
