# TOOLS-02d: opt-in structural search (WIP)

Fixed OMP source: commit `596f2da7101178214aa27a753529d15e6b7ad91d`,
`packages/coding-agent/src/tools/ast-grep.ts` (`AstGrepTool.execute`,
`runMultiTargetAstGrep`), `crates/pi-natives/src/ast.rs` (`ast_grep`), and
`packages/coding-agent/test/tools/ast-grep.test.ts`. OMP's
`astGrep.enabled` defaults to false; ARA keeps its six default tools and
exposes `ast_grep` only through explicit `--tools ast_grep`.

ARA owner: `crates/ara-tools/src/ast_grep.rs` with CLI selection in
`crates/ara-cli/src/main.rs`. It uses the existing search-scope parser,
`ara-walk` and vendored `pi-ast` language/pattern helpers, then matches AST
nodes directly to retain captures and syntax diagnostics. It does not change
the Agent loop, Session writer, existing tool defaults, or vendored editor.
The CLI test exercises the real `ara` process against a controlled fake
upstream, sees the explicit tool schema, one AST result in the next request,
and one matching Session tool receipt. Its fixture is
`const sharedSymbol = 1;` in `search.ts`; request two contains both
`sharedSymbol` and `search.ts#`, and the journal stores
`toolCallId=call_ast` with `matchCount=1`.

Bounded behavior: structural matching with named capture values, local
file/directory/glob/semicolon paths and loaded `skill://` paths, global
path/position ordering, `skip` pagination of 50 matches with unpaged totals,
parse issue reporting capped at 20 unique entries with the full count,
hashline snapshots/seen rows for editable local files, and cancellation.
ARA rejects `skip > 10,000` to bound retained-match memory. Other upstream
internal URL transports, TUI rendering and the full CA-TOOL-SEARCH surface
remain open. When output truncates, ARA does not mark any match lines as seen;
this conservatively avoids attributing hidden grouped rows to a file.

| Check on this WIP snapshot | Observed result |
| --- | --- |
| `cargo test -p ara-tools --lib ast_grep::tests` | 7/7 pass: captures/paging, parse error cap, glob/validation, limit/cancel, `skill://` and hashline, grouped directory layout, PlusCal/TLA parser. |
| `cargo clippy -p ara-tools --all-targets -- -D warnings` | Exit 0. |
| `cargo test -p ara-cli --test e2e ast_grep_is_opt_in_and_reaches_model_and_session_journal -- --exact --nocapture` | 1/1 pass after grouped-output change: default schema excludes the tool, explicit schema includes it, a real process forwards its result and journals its receipt. |
| `python scripts/verify_backend.py` | Owned formatting and workspace Clippy passed; CLI e2e 52/52, AST unit 7/7, tools integration 16/16. Stopped at the existing vendored `pi-edit` hashline preview parity case (9/10 in that binary; exit 101). |
| `cargo test --workspace --all-targets --all-features -- --skip hashline_streaming_preview_cases_preserve_partial_and_final_contracts --skip patcher_apply_cases` | Exit 0 on the final code. Exploratory only: the two skipped editor failures are not waived. |
| `cargo test --workspace --doc --all-features --quiet`; `cargo deny check`; `python scripts/omp_inventory.py check`; `python scripts/verify_bootstrap.py`; `git diff --check` | All exited 0. Doc tests contain no cases; deny retains existing duplicate-version and license-field warnings. |

The verifier exited 101, so its doc-test phase was not reached; doc tests
were run separately on the final code. Independent Codex pre-implementation
and post-diff reviews covered the fixed OMP source, exact ARA diff, CLI
defaults, read-only paths, paging,
diagnostics, cancellation and hashline provenance; no confirmed production
defect was found. A prior Windows gate failure in vendored `pi-edit` is
independent of this search tool. This point and the enclosing surface remain
WIP pending the no-skip gate, a bounded live task and point audit.
