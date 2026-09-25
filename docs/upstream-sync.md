# Fixed OMP source and incremental synchronization

The authoritative upstream is `https://github.com/can1357/oh-my-pi.git`, version v18.1.8 at `596f2da7101178214aa27a753529d15e6b7ad91d`. The exact marker lives in [`upstream/omp.lock.json`](../upstream/omp.lock.json). It was verified against a separate local checkout when this bootstrap was prepared. No upstream Git history is part of this ARA repository.

## Marker meanings

- `baseline_commit`: immutable first target. Never rewrite it to hide a version change.
- `reviewed_upstream_commit`: latest exact upstream commit whose source and change list have been inspected. This is **not** an implementation claim.
- `ported_through_commit`: latest exact upstream commit for which the complete selected behavior inventory has been implemented and tested in Rust. It starts `null` because this new repository has no Agent runtime.

Do not mark a feature `ported` because a symbol exists or a test compiles. Keep per-feature source location, ARA implementation, any intended difference, behavioral tests, and actual output in [`docs/upstream/feature-ledger.md`](upstream/feature-ledger.md). Retain upstream license notices for copied code.

## First implementation

Use a **separate reference checkout**, outside this Git repository. For example:

```text
git clone --filter=blob:none https://github.com/can1357/oh-my-pi.git <reference-directory>
git -C <reference-directory> checkout --detach 596f2da7101178214aa27a753529d15e6b7ad91d
git -C <reference-directory> rev-parse HEAD
```

Read source in `packages/agent`, `packages/ai`, and `packages/coding-agent` plus tests, RPC contracts, tool and provider implementations. Build an inventory from actual source and public behavior; the prior ARA 255-item selection is a historical planning aid, not proof of full current parity. Map each item to a Rust test and a host-level scenario before claiming it delivered.

## Later upstream upgrade

1. Choose a **specific new SHA**. Preserve the previous `ported_through_commit`; never track rolling `main` as a target.
2. In the separate OMP checkout, run `git diff --name-status <old-sha> <new-sha>` and inspect relevant hunks/tests. Record added, changed, removed, and unchanged behaviors in [`docs/upstream/changes.md`](upstream/changes.md).
3. Update source maps and the feature ledger. Implement one bounded group in Rust; run direct comparison, actual Rust host tests, negative cases, and selected real-model trials. Record intentional differences and reasons.
4. Advance `reviewed_upstream_commit` after source review. Advance `ported_through_commit` to the new SHA **only when all selected differences through that SHA pass the full acceptance gate**. Partial work remains visible in the ledger without claiming the marker.
5. Keep ARA-specific behavior behind explicit interfaces or documented differences so an upstream change can be evaluated rather than blindly merged.

Never import the OMP `.git` directory, old ARA commits, local credentials, or copied generated artifacts into this new repository. Upstream source may be reused under MIT terms with attribution; see [third-party notices](../THIRD_PARTY_NOTICES.md).
