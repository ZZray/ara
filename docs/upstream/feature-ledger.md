# OMP feature ledger

Target source: [`omp.lock.json`](../../upstream/omp.lock.json). Status values: `open`, `implementing`, `tested`, `audited`, `accepted`, `intentional-difference`.

The complete denominator is the generated [OMP source inventory](inventory.md): every tracked file at the baseline belongs to one surface in [`surfaces.toml`](../../upstream/inventory/surfaces.toml), and every upstream test case is a behavior item (`B-xxxxxxxxxx` in [behaviors.tsv](inventory/behaviors.tsv)). Rows below are bounded behavior points being ported; each cites its surface and the upstream behavior IDs/source it covers. A surface is only `accepted` when all of its behavior items are accounted for here. `python scripts/omp_inventory.py check` rejects citations of unknown behavior IDs.

| ID | Upstream source and behavior | Rust owner | Difference/reason | Executed test and artifact | Status |
| --- | --- | --- | --- | --- | --- |

For each row, include normal and failure/cancellation behavior, source commit, exact Rust test command, observed output, and reviewer conclusion. An `intentional-difference` still requires a behavioral test and explicit product reason; it is not silently counted as parity.
