# Fixed OMP baseline

**Source checked, 2026-09-25:** `v18.1.8` of `https://github.com/can1357/oh-my-pi.git` resolves to `596f2da7101178214aa27a753529d15e6b7ad91d`. The upstream license at that commit is MIT. The machine-independent marker is [`upstream/omp.lock.json`](../../upstream/omp.lock.json).

`baseline_commit` never changes. `reviewed_upstream_commit` means source inspection, and `ported_through_commit` means the selected complete behavior inventory has passed Rust execution and audit. It is currently `null`. A large checklist, a previous ARA implementation, or a compile result cannot advance it.

For each upstream behavior, record exact source and test locations, Rust owner, intentional difference with reason, normal/error/cancel evidence, and status in [the feature ledger](../upstream/feature-ledger.md). Inspect the upstream packages and tests in a separate checkout. The first inventory must account for the whole fixed target before anyone reports an OMP parity percentage; previously split cards are leads, not an exhaustive denominator.

Later updates use a chosen old SHA and new SHA. Review `git diff` plus upstream tests, classify added/changed/removed behavior, run direct Rust comparisons, and record results in [the change ledger](../upstream/changes.md). Only then advance markers according to [the sync contract](../upstream-sync.md). Preserve the exact MIT notice for copied code and review dependencies individually.
