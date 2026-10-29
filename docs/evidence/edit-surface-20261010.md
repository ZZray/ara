# CA-TOOL-EDIT complete original surface acceptance — 2026-10-10

## Requirement, snapshot and decision

Fixed OMP: `596f2da7101178214aa27a753529d15e6b7ad91d` (v18.1.8).
CA-TOOL-EDIT owns 24 upstream files (9 source/14 test), 3,114 source lines and
92 original behavior IDs in the fixed [inventory](../upstream/inventory.md).
All 92 retain their identity/order/source owner; their source contracts,
Rust locations, original selectors and receipts are in the
[accepted mapping](edit-surface-20261010-behaviors.tsv).

Decision: **accepted**, exactly one surface; total **2/111**. The baseline and
`ported_through_commit=null` remain unchanged. Final source changes in this closure
are one existing test Arc clone/format adjustment in
`crates/ara-cli/src/terminal/graph_renderer.rs`, and the existing verifier's
14-to-17 group selection in `scripts/verify_edit_module.py`. No assertion or
timeout was relaxed. Unrelated dirty work and the Git index are preserved.

The original two corrected mapping contracts are B-e4d7090dd9 (valid SOURCE
becomes an empty file without JSONL) and B-f32e80f947 (nested glob is in paths;
no new schema parameter). Original associations: terminal41/engine11/policy9/
repair7/AST7/streaming6/post-apply5/VCS5/self-error1 = 92.

## Actual execution

Local packet: `D:/workspace/env/cache/ara-port-candidates/extension-production-batch-20261009`. `edit-full-17-live-v4` records the actual
E worktree, 1,607 source pins, unchanged during build/tests/CAS. Build command:

```text
cargo test --locked -p ara-tools -p ara-cli -p ara-rpc -p ara-agent --all-features --lib --bin ara --test edit --test model_edit_snapshot --test edit_post_apply_family --test edit_mode_policy --test value_heap --test rpc_host --test e2e --test agent_loop --no-run --message-format=json
```

One build passes: **490.397 s**, 400 reused/27 compiled artifacts, 89 warnings,
zero errors. Existing executable groups pass **17/17, 71/71 tests**, zero
failed/ignored, with **80.871 s** test execution; whole module **579.724 s**.
Exact commands, original names/counts, binary pins and stdout/stderr are in
`edit-full-17-live-v4/module-result.json`. Failure/denial, partial application,
streaming abort, AST, Session restart, journal failure and settlement cases are
covered by the original selected groups; see each mapped contract.

| Existing group | Tests passed | Result |
| --- | ---: | --- |
| engine | 14 | PASS |
| snapshot | 3 | PASS |
| post-apply | 2 | PASS |
| policy | 4 | PASS |
| actual-tools | 4 | PASS |
| terminal | 26 | PASS |
| vcs | 1 | PASS |
| value-heap | 4 | PASS |
| rpc-ast | 1 | PASS |
| retained-repl | 1 | PASS |
| new-session | 1 | PASS |
| clear-compact | 1 | PASS |
| journal-failure | 1 | PASS |
| host-idle-settlement | 1 | PASS |
| ast-family | 5 | PASS |
| streaming-guard | 1 | PASS |
| self-error | 1 | PASS |

## Real model and effects

Live Manager local OMP management → CAS catalogue verified;
`deepseek-v4.1-flash`, `openai-completions`. Original bounds: 120 s per Run,
6+2 calls, 2,048 tokens. Edit/readback: 5.363 s, four model calls/four tools.
Separate process same-Session no-tools recall: 1.770 s, one call/zero tools.
Session `01a12609-e063-73a2-9d0d-479271e2c02c` persists two user/five assistant/
four toolResult messages; artifact is pen11/book46/cup17/grand_total74.
Actual nullable usage remains unknown where not reported. Selected credentials
were privately scanned in outputs/receipts/journal: zero leaks; no keys copied.

## Reused evidence and formatting boundary

Original repair7 receipts reuse 182 byte-identical source/root manifest pins
and 156 identical transitive locked package records. The entire Cargo.lock is
not byte-identical. Original ACP5 selected test bodies and path contracts are
unchanged; reviewed plan-first URI composition and optional Vault/root changes
do not affect those five cases. No full ACP/Vault acceptance follows.

Executed renderer raw SHA256 is
`5f90b60c821bbda88e43ba84eabbba398d9bc0de2dfe70288f9a40198bd96f27`;
current formatted raw SHA256 is
`b36d6133599505342c7cbe9278c91b7ac3cf1d53739b65f14558c3bcfafa71b4`.
Only one cfg(test) call's whitespace/optional trailing comma changes; arguments,
assertions and all outside bytes match. Independent reviewer approves equivalent
reuse. Saved standalone `rustfmt --check --edition 2024` exits 0. The formatted
rawhash was not newly executed; no full fmt/Clippy/release gate is claimed.

## Receipts and independent audit

Independent Codex `/root/compact_combined_dependency_post`: final SOURCE /
EXECUTION / AUDIT **APPROVE**, directly checked original92 identities, actual
source/outputs/binaries/CAS and the formatting/reuse boundaries. Report:
`compact-callable-next/COMBINED-ORDINARY-SOFT-SOURCE-POST.md`.

| Artifact (relative to local packet) | SHA256 |
| --- | --- |
| Final independent report | `10f1288b87d1d037e04c52b9bc60f8545e172d64765ba265b617f74d94750222` |
| edit92-map-live-v4/freeze.json | `1f70af0c65bc12ca5aef15c30df5732d42eee909c62a9eb5a43f952af960d184` |
| Reviewed candidate after TSV | `c8f8a73b24cee83b526b06e8b13aa87aa75fd2c2f306fad79d49523707a84aa3` |
| Accepted project/canonical TSV | `ef1b9f7830264d6fcd56d414348c1765ed15410909757b780e706013c7074b5a` |
| edit-full-17-live-v4/build-result.json | `1f73308ff9b18fdd1a2f1c65e17a36ca6c91f2a289f7a4e44c287ed63897a05f` |
| edit-full-17-live-v4/module-result.json | `2d70c0ef52d94c271de810cbe2dac0bc5fbe18531dbceb82a01a61b89c8f4b60` |
| edit-full-17-live-v4/real-cas-receipts/20261010T133901Z/receipt.json | `2f95a07efc1482875e3ab5575a2d20813b7c7d7c642241ce67ad30d2da22e64c` |
| edit-live-delivered-artifacts/ara.exe | `6d7db2209d7d6f826e8a32477d842eb8d1570b9fbc3e1faa073676ef0cdcc3c9` |

## Failed history and acceptance limits

D `edit-full-14-current-v80` failed build E0382 (225.546 s). D17 v2 builds in
355.808 s but passes 13/17 groups, 69 tests/four failures (451.893 s total).
Environment-only v3 clears clear-compact/journal but retains Theme callback
ownership and pre-ready RPC event failures. These remain failed receipts; the
actual E execution has no dependency on the unrelated Extension additions.
The three build attempts alone total 1,071.751 s; 8.2 minutes above describes
only the successful E build, not the full day's effort or elapsed review time.

CRATE-PI-EDIT Unix directory symlink, full Extension/LSP/Vault/physical TUI,
automatic side-model repair, remaining surfaces, P1-P6/products and release
remain open. No commit/push/deploy/install was performed. Scoped documentation
validation is recorded separately in the acceptance-application receipt.
