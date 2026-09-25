# Third-party notices

ARA contains Rust translations of behavior from [Oh My Pi](https://github.com/can1357/oh-my-pi) `v18.1.8` / `596f2da7101178214aa27a753529d15e6b7ad91d`. Ported code lives in:

- `crates/ara-ai`: from `packages/ai`, the `packages/catalog` types and the `packages/utils` JSON parsing.
- `crates/ara-agent`: from `packages/agent`.
- `crates/ara-session`: from `packages/coding-agent` session storage.
- `crates/ara-tools`: from the `packages/coding-agent` tools, plus Rust adapted from `crates/pi-natives` (`grep.rs`, `glob.rs`, `glob_util.rs`) and `crates/pi-walker` (ignore-state traversal).
- `crates/ara-cli`: from `packages/coding-agent` print mode.

Each module names the upstream files it follows. Upstream copyright and license apply to those portions. Further dependencies are reviewed through `deny.toml` (`cargo deny check`). The search tools link the ripgrep libraries (`grep-*`, `ignore`, `globset`; MIT or Unlicense) and PCRE2 through `pcre2-sys` (PCRE2 is BSD-3-Clause).

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
