# Fixed upstream source

`src/frame.rs`, `src/input.rs`, and `src/writer.rs` port behavior from
Oh My Pi commit `596f2da7101178214aa27a753529d15e6b7ad91d`:

- `packages/coding-agent/src/modes/rpc/rpc-frame.ts`
- `packages/coding-agent/src/modes/rpc/rpc-input.ts`
- `packages/utils/src/stream.ts` (`readLines`)
- `packages/coding-agent/src/modes/rpc/rpc-mode.ts` (ordered stdout)

The wire JSON compatibility module implements the required ECMAScript value
semantics in Rust; its number formatter uses `ryu-js` 1.0.3, licensed under
`Apache-2.0 OR BSL-1.0` (verified from its downloaded Cargo manifest).
Golden tests execute the unchanged upstream modules and tests through
`scripts/rpc_transport_oracle.py` and `.mjs`.

## Upstream MIT notice

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
