# Native ECMAScript matcher dependency

This directory preserves the published `regress` 0.12.0 package under its
MIT OR Apache-2.0 licenses. Both original license files and copyright notices
are retained. The published crate checksum is
`32eef8b209c3c1c15dbad02c1f30f9539f00dc7253e0cbcdaae442a50a09d7c1`.
`Cargo.toml.orig` and release tests remain available for comparison.

This is a third-party matcher dependency, rather than an OMP Rust crate.
Its registry package name remains `regress` so Cargo's scoped patch resolves
the exact `=0.12.0` dependency. The ARA directory is `ara-regress`.

## Local native changes

Only `src/unicode.rs` and `src/parse.rs` change release source behavior.
The historical benchmark document `perf.md` adds `https://` to its upstream
repository link so the project documentation checker recognizes the URL.
Its benchmark text and results remain unchanged.
They implement the ECMAScript semantics needed by fixed OMP's native Rust
port, verified against Bun 1.4.0 / JavaScriptCore:

- Legacy Canonicalize preserves multi-character uppercase expansions and
  prohibits non-ASCII to ASCII folding; literals, classes and backreferences
  use the same rule.
- Unicode set operands close case equivalence before intersection,
  subtraction and negation. Positive class escapes close before inversion;
  Unicode property complement retains its distinct `u` behavior.
- `v` string sets preserve raw multi-character string operations, support
  empty alternatives and longest-match order with backtracking, and retain
  `CanContainStrings` syntax checks. A single `&` retains the first operand.
- `v` class `\b` is a backspace character.

The Host wrapper owns RegExp source/flags/lastIndex and UTF-16 captures. It
does not rewrite character-class grammar or enumerate a BMP lookup cache.
The mandatory source oracle and Host regressions are recorded in
`docs/evidence/model-variants.md` after execution; dependency source audit
receipts are kept with that batch. No complete RegExp specification or full
OMP catalogue acceptance is implied by this dependency's targeted checks.

Future dependency upgrades must compare the original published release with
these two source patches and rerun the retained source oracle. Formatting
uses this package's original rustfmt configuration; do not apply the OMP
vendor parent's tab configuration to this release.
