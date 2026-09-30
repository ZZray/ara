# Third-party notices

ARA contains Rust translations of behavior from [Oh My Pi](https://github.com/can1357/oh-my-pi) `v18.1.8` / `596f2da7101178214aa27a753529d15e6b7ad91d`. Ported code lives in:

- `crates/ara-ai`: from `packages/ai`, the `packages/catalog` types and Claude tokenizer policy, and the `packages/utils` JSON parsing.
- `crates/ara-agent`: from `packages/agent`. `prompts/` includes the pinned compaction summary templates listed in its README.
- `crates/ara-ctok`: Claude token-count reconstruction adapted from fixed OMP `crates/pi-natives/src/utok/{claude,utf.rs}`. The two embedded ctok vocabularies and reference fixtures come from that same commit. `crates/ara-ctok/data/LICENSE.ctok` preserves the additional MIT notice for the measured vocabulary and reconstruction; ARA replaces upstream's nightly-only `xutf` calls with stable Unicode crates.
- `crates/ara-session`: from `packages/coding-agent` session storage.
- `crates/ara-tools`: from the `packages/coding-agent` tools, plus Rust adapted from `crates/pi-natives` (`grep.rs`, `glob.rs`, `glob_util.rs`, `edit.rs`).
- `crates/ara-walk`: from `crates/pi-walker` (ignore-state traversal).
- `crates/vendor/ara-diff`, `crates/vendor/ara-ast`, `crates/vendor/ara-edit`: upstream crates `crates/pi-diff`, `crates/pi-ast` and `crates/pi-edit`, copied verbatim except for the ARA renaming and the local modifications listed in `crates/vendor/README.md` (`ara-ast` also carries upstream `pi-ast`'s own `LICENSE`).
- `crates/ara-cli`: from `packages/coding-agent` print mode, `main.ts` prompt-file discovery, model selectors and retry fallback chains, and `packages/ai/src/auth/sqlite-credential-store.ts`. `data/omp-models.json` is the fixed `packages/catalog/src/models.json` byte stream; `data/LICENSE.omp-catalog` retains its original MIT notice.
- `crates/ara-prompt`: from `packages/utils` (`template.ts`, `prompt.ts`). `tests/fixtures/template/` holds upstream prompt templates used as goldens.
- `crates/ara-discovery`: from the `packages/coding-agent` capability, discovery, `config.ts` and skills code and from `packages/utils` `frontmatter.ts`. `tests/fixtures/skills*` are copied from upstream `test/fixtures/skills*`.
- `crates/ara-context`: from `packages/coding-agent` `system-prompt.ts`, `session/date-cwd-reminder.ts` and `utils/active-repo-context.ts`. `prompts/` holds upstream's `prompts/system/` templates, copied verbatim except for the edits listed in `crates/ara-context/prompts/README.md`.

Each module names the upstream files it follows. Upstream copyright and license apply to those portions. Further dependencies are reviewed through `deny.toml` (`cargo deny check`). The search tools link the ripgrep libraries (`grep-*`, `ignore`, `globset`; MIT or Unlicense) and PCRE2 through `pcre2-sys` (PCRE2 is BSD-3-Clause). The vendored crates link tree-sitter and its grammars and `ast-grep-core` (MIT), and `xxhash-rust` (BSL-1.0).

The following is the MIT license text from the pinned Oh My Pi commit's `LICENSE` file:

```text
MIT License

Copyright (c) 2025 Mario Zechner
Copyright (c) 2025-2026 Can Bölük
Copyright (c) 2026 Stencil Labs, Inc.

Permission is hereby granted, free of charge, to any person obtaining a copy
of this software and associated documentation files (the "Software"), to deal
in the Software without restriction, including without limitation the rights
to use, copy, modify, merge, publish, distribute, sublicense, and/or sell
copies of the Software, and to permit persons to whom the Software is
furnished to do so, subject to the following conditions:

The above copyright notice and this permission notice shall be included in all
copies or substantial portions of the Software.

THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF ANY KIND, EXPRESS OR
IMPLIED, INCLUDING BUT NOT LIMITED TO THE WARRANTIES OF MERCHANTABILITY,
FITNESS FOR A PARTICULAR PURPOSE AND NONINFRINGEMENT. IN NO EVENT SHALL THE
AUTHORS OR COPYRIGHT HOLDERS BE LIABLE FOR ANY CLAIM, DAMAGES OR OTHER
LIABILITY, WHETHER IN AN ACTION OF CONTRACT, TORT OR OTHERWISE, ARISING FROM,
OUT OF OR IN CONNECTION WITH THE SOFTWARE OR THE USE OR OTHER DEALINGS IN THE
SOFTWARE.
```
