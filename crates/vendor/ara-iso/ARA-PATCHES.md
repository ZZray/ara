# Initial fixed source snapshot

OMP b91db66e pi-iso src is preserved byte-for-byte; only package metadata/workspace dependency binding changes. All platform backends retained. Authored, unbuilt, unexecuted.

## Windows Git argument candidate — 2026-10-29

The candidate changes only `src/rcopy.rs`: translate rooted Windows verbatim drive and UNC prefixes to ordinary paths at the six Git argv path positions used by worktree add/remove, capture and apply. The conversion uses `Prefix` and `OsString`; it preserves non-UTF8 components. Unix paths and canonical filesystem paths used by owner identity and authority checks stay unchanged. No dependencies or assertions change.

The fixed upstream `crates/pi-iso/src/rcopy.rs` and the initial ARA copy both have SHA256 `8d3a334b717d9ce38d6d92f4724f15243b736ca86e591e4510190611e8208296`. Candidate source SHA256 is `c5b79f0af448dc61b1fed2bd4e241247463f1cfc9e90f67f620e37c6d4ef8bfa`. This is an intentional Windows subprocess compatibility difference from OMP `b91db66e9b0b65a2c977e6a5bc74b0f4b1d58d1c`.

The independent root probe at `D:/Temp/ara-task-path-probe-20261029/RESULT.json` records ordinary short-path worktree add exit 0 and verbatim short-path add exit 128 with `//?/D:/.../.git: Invalid argument`. The failed batch separately records a 273-character shallow-clone keep path failing with `Filename too long`; moving runtime artifacts and home to the short root is an execution prerequisite, independent of this source repair.

This candidate is unbuilt and unexecuted. Original Task G1/G2/G4/G5 and Host scenarios must run on the delivered candidate before acceptance; the existing three compile and nine spec identities remain unchanged. UNC conversion, non-UTF8 path behavior and Unix behavior have source review only.
