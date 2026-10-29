# Fixed OMP baseline

**Source checked, 2026-09-25:** `v18.1.8` of `https://github.com/can1357/oh-my-pi.git` resolves to `596f2da7101178214aa27a753529d15e6b7ad91d`. The upstream license at that commit is MIT. The machine-independent marker is [`upstream/omp.lock.json`](../../upstream/omp.lock.json).

`baseline_commit` never changes. `reviewed_upstream_commit` means source inspection, and `ported_through_commit` means the selected complete behavior inventory has passed Rust execution and audit. It is currently `null`. A large checklist, a previous ARA implementation, or a compile result cannot advance it.

For each upstream behavior, record exact source and test locations, Rust owner, intentional difference with reason, normal/error/cancel evidence, and status in [the feature ledger](../upstream/feature-ledger.md). Inspect the upstream packages and tests in a separate checkout. The first inventory must account for the whole fixed target before anyone reports an OMP parity percentage; previously split cards are leads, not an exhaustive denominator.

Later updates use a chosen old SHA and new SHA. Review `git diff` plus upstream tests, classify added/changed/removed behavior, run direct Rust comparisons, and record results in [the change ledger](../upstream/changes.md). Only then advance markers according to [the sync contract](../upstream-sync.md). Preserve the exact MIT notice for copied code and review dependencies individually.

**Upgrade target selected, 2026-10-14:** the user's request to update OMP source
freezes `b91db66e9b0b65a2c977e6a5bc74b0f4b1d58d1c`, described as
`v18.10.4-13-gb91db66e9b`, in `sync_target_commit`/`sync_target_version`.
The initial baseline and its inventory remain versioned evidence; reviewed
marker stays596f2da and ported-through stays null during incomplete source and
behavior reconciliation. Seven historical complete surfaces are accepted at
the old SHA; that count is not new-target acceptance. Modular delta work and
its testing boundaries live in the change ledger, not in a moving HEAD target.
