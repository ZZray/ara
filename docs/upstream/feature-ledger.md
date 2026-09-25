# OMP feature ledger

Target source: [`omp.lock.json`](../../upstream/omp.lock.json). Status values: `open`, `implementing`, `tested`, `audited`, `accepted`, `intentional-difference`. This ledger starts empty because this new repository contains no Rust Agent implementation. The first AI must derive a complete, source-backed inventory from the pinned OMP commit before asserting a parity percentage.

| ID | Upstream source and behavior | Rust owner | Difference/reason | Executed test and artifact | Status |
| --- | --- | --- | --- | --- | --- |

For each row, include normal and failure/cancellation behavior, source commit, exact Rust test command, observed output, and reviewer conclusion. An `intentional-difference` still requires a behavioral test and explicit product reason; it is not silently counted as parity.
