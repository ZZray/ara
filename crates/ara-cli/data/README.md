# Fixed OMP bundled model catalog

`omp-models.json` is the **unmodified byte stream** from
`packages/catalog/src/models.json` at OMP commit
`596f2da7101178214aa27a753529d15e6b7ad91d`.

- Source: <https://github.com/can1357/oh-my-pi/blob/596f2da7101178214aa27a753529d15e6b7ad91d/packages/catalog/src/models.json>
- SHA-256: `4f609bf5d4f786c3164c77333ac9962dd2c8b0382291a82815667e558ebc04d0`
- Size: 12,244,267 bytes; 67 providers; 4,776 model rows.
- `LICENSE.omp-catalog` is the unmodified `packages/catalog/LICENSE` from the same commit (MIT, including the upstream copyright notices).

The source was exported using `git show <exact-sha>:<path>` as bytes, without
JSON reformatting or a moving-branch lookup. All rows in this snapshot carry
`identity`; their `provider` and `id` match their enclosing object keys.

Behavior sources at that commit:

- `packages/catalog/src/models.ts`: consume fully materialized bundled rows
  verbatim; index by literal provider and model id; preserve object/map order.
- `packages/catalog/src/build.ts`: `buildModel` is for discovered/custom/spec
  rows; do not rebuild or re-infer the bundled rows' compatibility metadata.
- `packages/catalog/src/types.ts`: `requestModelId` affects the wire name while
  local identity remains `id`; null context/output limits mean unknown; input,
  tokenizer, thinking, compat and other execution fields have distinct contracts.

The Rust Host catalog preserves every row field, explicit null and unknown
nested value. Duplicate JSON object keys follow upstream JSON import semantics:
the last value wins at the original key position. Model ids in different
providers are distinct; literal `:max`/`:auto` suffixes are not parsed here.
The pinned dataset contains literal `:max` ids and no `:auto` ids; a synthetic
case tests the latter's literal lookup contract. Malformed materialized rows
or inconsistent provider/id keys fail to load.

This is static catalog data and an exact lookup index, not configured model
availability, discovery, models.yml composition, OAuth, fallback selection or
full runtime routing. `try_execution_model` reports execution contracts not
representable by the current Rust Model instead of silently discarding them.
In particular, current bundled entries still require input/context policy and
materialized compatibility integration before they can become complete routes.
