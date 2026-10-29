# Third-party notices

The native ECMAScript matcher is the published `regress` 0.12.0 package,
preserved in `crates/vendor/ara-regress` under MIT OR Apache-2.0. Original
licenses, notices and tests remain in that directory. Its two native source
patches and release checksum are documented in `ARA-PATCHES.md` there.

ARA contains Rust translations of behavior from [Oh My Pi](https://github.com/can1357/oh-my-pi) `v18.1.8` / `596f2da7101178214aa27a753529d15e6b7ad91d`. Ported code lives in:

- `crates/ara-ai`: from `packages/ai`, the `packages/catalog` types and tokenizer family mapping, and the `packages/utils` JSON parsing.
- `crates/ara-agent`: from `packages/agent`. `prompts/` includes the pinned compaction summary templates listed in its README.
- `crates/ara-ctok`: universal offline tokenizer adapted from fixed OMP `crates/pi-natives/src/utok`, including its ten native encodings, embedded vocabularies and original test families. `crates/ara-ctok/data/LICENSE.ctok` preserves the additional MIT notice for the Claude measured vocabulary and reconstruction. ARA replaces upstream's nightly-only `xutf` calls with stable Unicode crates; category and Han Script use Unicode 16 and NFC uses Unicode 17, matching fixed `xutf` 1.5.0. The fixed Han Script ranges retain their Unicode data notice in `crates/ara-ctok/data/LICENSE-UNICODE`.
- `crates/ara-session`: from `packages/coding-agent` session storage.
- `crates/ara-snapcompact`: from fixed `packages/snapcompact` and
  `crates/pi-natives/src/snapcompact.rs`, including its native renderer,
  prompts, shape table and tests. Bundled X.org BDF and unscii fonts retain
  their source notices. Silver is licensed separately under CC BY 4.0;
  its complete attribution is in `crates/ara-snapcompact/src/fonts/Silver.LICENSE`.
- `crates/ara-tools`: from the `packages/coding-agent` tools, plus Rust adapted from `crates/pi-natives` (`grep.rs`, `glob.rs`, `glob_util.rs`, `edit.rs`).
- `crates/ara-walk`: from `crates/pi-walker` (ignore-state traversal).
- `crates/vendor/ara-diff`, `crates/vendor/ara-ast`, `crates/vendor/ara-edit`: upstream crates `crates/pi-diff`, `crates/pi-ast` and `crates/pi-edit`, copied verbatim except for the ARA renaming and the local modifications listed in `crates/vendor/README.md` (`ara-ast` also carries upstream `pi-ast`'s own `LICENSE`).
- `crates/ara-cli`: from `packages/coding-agent` print mode, `main.ts` prompt-file discovery, model selectors and retry fallback chains, configuration schema/loading, `packages/omptype` validation semantics, `packages/ai/src/auth/sqlite-credential-store.ts` and `packages/catalog/src/model-cache.ts`. `data/omp-models.json` is the fixed `packages/catalog/src/models.json` byte stream; `data/LICENSE.omp-catalog` retains its original MIT notice.
- `crates/ara-prompt`: from `packages/utils` (`template.ts`, `prompt.ts`). `tests/fixtures/template/` holds upstream prompt templates used as goldens.
- `crates/ara-discovery`: from the `packages/coding-agent` capability, discovery, `config.ts` and skills code and from `packages/utils` `frontmatter.ts`. `tests/fixtures/skills*` are copied from upstream `test/fixtures/skills*`.
- `crates/ara-context`: from `packages/coding-agent` `system-prompt.ts`, `session/date-cwd-reminder.ts` and `utils/active-repo-context.ts`. `prompts/` holds upstream's `prompts/system/` templates, copied verbatim except for the edits listed in `crates/ara-context/prompts/README.md`.

The private Host terminal Unicode adapter in `crates/ara-cli/src/terminal/unicode` retains the exact packed Unicode 17 tables and scalar grapheme/width state from `xutf` 1.5.0, the dependency pinned by the fixed OMP Cargo.lock. Its MIT copyright and permission notice is preserved in `unicode/LICENSE.xutf`; `unicode/SOURCE-MAP.json` records source hashes and the stable UTF-8 adapter boundary. The Host's surrounding ANSI/control/tab/Hangul policy remains in the ported native text wrapper.

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
# Fixed OMP report_issue WRITE dependency

The Rust reference Host `crates/ara-cli/src/report_tool_issue.rs` and its Native
WRITE dispatch/call preview port behavior from
`packages/coding-agent/src/tools/report-tool-issue.ts` at OMP commit
`596f2da7101178214aa27a753529d15e6b7ad91d`. The grouped fixtures preserve the
original report-tool-issue and report-tool-issue-consent test identities.
Copyright (c) 2025 Mario Zechner; Copyright (c) 2025-2026 Can Bölük;
Copyright (c) 2026 Stencil Labs, Inc. These upstream materials use the MIT
license reproduced in this document.


# Fixed TUI static Markdown and CSS dependencies

The reference Host static Markdown adapter uses pulldown-cmark 0.13.0
(MIT), cssparser 0.37.0 and cssparser-color 0.5.0 (MPL-2.0), with default
features disabled. The CSS crates are external dependencies, not source
vendor copies. Their complete original LICENSE files must be retained
with any source or binary distribution, with exact Cargo.lock checksums.

Color conversion, gamut mapping and ANSI 256 quantization in
crates/ara-cli/src/terminal/markdown_css_color.rs are adapted from Bun
1.4.0 src/css/values/color.rs and src/css_jsc/color_js.rs at exact commit
34cbb9a40b4bd1bd767d134a7065e66c2432a676 (MIT). The exact source bytes
and complete upstream LICENSE.md are retained with the source evidence.
This does not link the Bun runtime or copy JavaScriptCore or its libraries.
The original files contain no additional copyright header.

These declarations document source and dependency ownership; complete
TUI runtime parity and distribution acceptance remain separate gates.

# Fixed OMP archive readers

The private READ archive library in crates/ara-tools/src/write_archive/read_facade
ports packages/utils/src/ar at fixed OMP596f2da7101178214aa27a753529d15e6b7ad91d.
Its original format tests retain source assertion identities and pinned fixtures.
OMP portions use the MIT notice reproduced above.

RAR4/RAR5 decompression structures are adapted from unrar5j by Stephane Bury,
Copyright 2025, licensed under Apache License 2.0. Original decoder notices
remain in rar4_decoder.rs and rar5_decoder.rs. The complete Apache license
from fixed upstream THIRD-PARTY-NOTICES.txt is retained in
crates/ara-tools/src/write_archive/read_facade/LICENSE.unrar5j.

HTTP source transport uses the existing reqwest 0.12 workspace dependency
with its blocking feature. This private library API does not add READ URL
routing or a public Host SDK. Distribution dependency gates remain separate.


# Fixed OMP PDF/SVG native dependencies

The PDF and SVG adapters in crates/ara-tools follow crates/pi-natives/src/pdf.rs
and svg.rs at fixed OMP 596f2da7101178214aa27a753529d15e6b7ad91d.
They retain the upstream MIT notice in source. Native conversion depends on
pdf-inspector 1.17.0 and resvg 0.48.1 with exact Cargo.lock checksums.

arrayref 0.3.9 is a resvg dependency under BSD-2-Clause. The exception in
deny.toml is limited to that exact package and version. Source and binary
distributions must reproduce its complete original license below.

```text
Copyright (c) 2015 David Roundy <roundyd@physics.oregonstate.edu>
All rights reserved.

Redistribution and use in source and binary forms, with or without
modification, are permitted provided that the following conditions are
met:

1. Redistributions of source code must retain the above copyright
   notice, this list of conditions and the following disclaimer.

2. Redistributions in binary form must reproduce the above copyright
   notice, this list of conditions and the following disclaimer in the
   documentation and/or other materials provided with the
   distribution.

THIS SOFTWARE IS PROVIDED BY THE COPYRIGHT HOLDERS AND CONTRIBUTORS
"AS IS" AND ANY EXPRESS OR IMPLIED WARRANTIES, INCLUDING, BUT NOT
LIMITED TO, THE IMPLIED WARRANTIES OF MERCHANTABILITY AND FITNESS FOR
A PARTICULAR PURPOSE ARE DISCLAIMED. IN NO EVENT SHALL THE COPYRIGHT
HOLDER OR CONTRIBUTORS BE LIABLE FOR ANY DIRECT, INDIRECT, INCIDENTAL,
SPECIAL, EXEMPLARY, OR CONSEQUENTIAL DAMAGES (INCLUDING, BUT NOT
LIMITED TO, PROCUREMENT OF SUBSTITUTE GOODS OR SERVICES; LOSS OF USE,
DATA, OR PROFITS; OR BUSINESS INTERRUPTION) HOWEVER CAUSED AND ON ANY
THEORY OF LIABILITY, WHETHER IN CONTRACT, STRICT LIABILITY, OR TORT
(INCLUDING NEGLIGENCE OR OTHERWISE) ARISING IN ANY WAY OUT OF THE USE
OF THIS SOFTWARE, EVEN IF ADVISED OF THE POSSIBILITY OF SUCH DAMAGE.

```


## Dynamic grammar direct dependency at fixed OMP b91db66e

The `ara-ast` grammar-source loader, generated wasm catalog and bundled
`src/language/testdata/ini.wasm` follow OMP
`b91db66e9b0b65a2c977e6a5bc74b0f4b1d58d1c`. The existing OMP MIT notice
above and `crates/vendor/ara-ast/LICENSE` apply. The ini fixture is the exact
upstream tree-sitter-ini 1.4.0 Wasm module; it is an executable test fixture.
The Host acquisition adapter follows `packages/coding-agent/src/utils/grammars.ts`.

`crates/vendor/tree-sitter-go` is the exact production parser vendored by that
fixed OMP target, including the Go 1.26 `new(expr)` grammar change. Its own MIT
notice (Copyright 2014 Max Brunsfeld) is retained in
`crates/vendor/tree-sitter-go/LICENSE`. Source, generated C/header files,
queries, Rust bindings and build script are retained. The workspace patch
selects this code instead of the registry tree-sitter-go 0.25.0 source.

Enabling tree-sitter 0.25.10's wasm feature adds Wasmtime 29.0.1/Cranelift
transitive dependencies. The accompanying static lock candidate preserves the
exact fixed upstream package checksums; dependency resolution, source/license
review and executable acceptance remain required before delivery.


## HOST-SERVICE-CONTROL source adaptation

Native Host service control adapts OMP commit b91db66e9b0b65a2c977e6a5bc74b0f4b1d58d1c: packages/coding-agent/src/launch/{services,client,broker,protocol,presence}.ts, session/{yield-queue,launch-completion}.ts, prompts/session/launch-completion.md, packages/utils/src/vterm/query-responder.ts, crates/pi-natives/src/pty.rs and crates/pi-shell/src/process.rs. Copyright (c) 2025 Mario Zechner; Copyright (c) 2025-2026 Can Bölük; Copyright (c) 2026 Stencil Labs, Inc. Licensed under MIT; the existing upstream MIT notice above applies. This names the adapted source, not full upstream launch/daemon parity.

### portable-pty 0.9.0

Exact official archive SHA256 b4a596a2b3d2752d94f51fac2d4a96737b8705dddd311a32b9af47211f08671e. Transitive license/MSRV/advisory resolution is a separate freeze/adoption/build gate.

MIT License

Copyright (c) 2018 Wez Furlong

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


### ARA Windows ConPTY loader adaptation

The complete official portable-pty 0.9.0 archive above is retained in crates/vendor/portable-pty, including its MIT license and original manifest/lock metadata. Only src/win/psuedocon.rs::load_conpty changes: ARA uses the existing system kernel32 ConPTY exports and does not automatically search the project directory or PATH for conpty.dll. The bare upstream lookup executes DLL entry points before export validation. ARA does not ship a paired sideloaded conpty.dll/openconsole.exe distribution; that optional distribution mechanism is not advertised. This explicit Windows Host dependency adaptation preserves the original ABI, console flags, create/resize/close, pipes, process spawning, I/O, Linux implementation and public API. Actual Windows and Linux service banks remain mandatory execution gates; a source patch is not runtime acceptance.


## Fixed b91 Task isolation and consumed Git facade

`ara-iso` retains complete OMP pi-iso platform source and tests. `ara-task-vcs` retains complete Git modules/types/errors and exact filesystem-only Jujutsu discovery; full Jujutsu operations are not claimed ported. Both use fixed OMP b91db66e9b0b65a2c977e6a5bc74b0f4b1d58d1c MIT source; see each LICENSE and ARA-PATCHES.md. No runtime acceptance is implied by vendoring.


### uluru 3.1.0 (Task Git pack cache)

The fixed OMP Git dependency `gix-pack` 0.72.0 uses the unmodified `uluru`
3.1.0 LRU cache by The Servo Project Developers and Matt Brubeck, licensed
under MPL-2.0. The full license is retained at
`crates/vendor/ara-task-vcs/LICENSE.uluru`. Source is available from
https://static.crates.io/crates/uluru/uluru-3.1.0.crate
(SHA256 `7c8a2469e56e6e5095c82ccd3afb98dad95f7af7929aab6d8ba8d6e0f73657da`),
the same version/checksum as fixed OMP b91db66e. This package/version has a
scoped license exception in `deny.toml`; the general license allowlist is
unchanged. Distribution must preserve this notice, the license and source
access. This record does not claim a binary release or runtime acceptance.
