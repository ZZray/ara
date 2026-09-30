# Fixed OMP compiled compatibility rules

`omp-compat-rules.json` is the unmodified byte stream exported with `git show`
from `packages/catalog/src/compat/rules.json` at
`596f2da7101178214aa27a753529d15e6b7ad91d`.

- Size: 321,052 bytes.
- SHA-256: `9ae6cc8f5c0fb2503d7e6d5ef885c01b8d767f1f14281c4432b09efa862d4bcf`.
- 21 taxonomy classes and 563 cascade rules; all remaining taxonomy,
  behavior, authentication and source-attribution fields are preserved.
- License: the unchanged MIT `LICENSE.omp-catalog` in this directory,
  from the same upstream commit, also covers this catalog data and its
  translated runtime algorithms.

`catalog_rules.rs` ports `compat/revision.ts`, `compat/cascade.ts` and
`compat/taxonomy.ts` from that commit. It consumes the compiled JSON without
recompilation or rule reordering. The rule compiler, catalog builder,
`compat/collapse.ts` model-list algorithm, runtime behavior and authentication
consumers have separate implementation and acceptance scopes. This file is
source provenance, not a claim that all catalog behavior is accepted.
