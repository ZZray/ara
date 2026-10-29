# Production Vault owner — 2026-10-09

## Delivered scope and boundary

Fixed OMP `596f2da7101178214aa27a753529d15e6b7ad91d`; dirty `dev`
HEAD `20aa0e2ef9872f633ba2671376cdf778477e7115`.
Fourteen delivered paths connect the production Vault process/cache/protocol,
READ editable snapshots, WRITE, cached EDIT/AST paths, prompt advertising and
retained/ACP/active-plan composition. Default Vault remains disabled.
The original eighteen Vault IDs belong to **CA-MISC-RUNTIME**, not EDIT92 or
WRITE204. Their fixed inventory identities are unchanged. Full parents remain
OPEN; formal inventory remains **0/111 complete, 1 tested, 27 implementing,
83 open**. No workspace, product or full OMP acceptance is inferred.

Final execution packet:
`D:/workspace/env/cache/ara-port-candidates/vault-production-execution-v4-20261009`.
Delivered manifest SHA256:
`2c9c3d2c10b1c3c0b1831ebb3e9bafc9bf554e958151011086efa423909544f5`.
Production binary SHA256:
`82aba3c460630b393398376084db87b398e6e8f90b3a7b0382e7a1539f4f5ae3`.
Index SHA256 remains
`89d34e26be97c90a14680d44ace937e6b1ae5a86e8ee7b46dd6b90718a922475`.

## Original execution and concentrated repairs

Author preparation: **2,056.707 s**. V1 grouped typecheck failed after
**204.325 s** because `ToolError`/`ToolOutput` were imported through a private
re-export. V2 changes only their import to the existing public `ara_agent`
owner; grouped typecheck passes in **67.547 s**.

V2 selected executable build passes in **736.306 s**. Original ten groups
finish **FAIL** after **834.886 s**: nine groups PASS, Host 13/15, total
41 passed/2 failed/0 ignored. That original receipt is preserved.

The two Host failures were fixture errors, repaired only in
`crates/ara-cli/tests/host_bridges.rs`:

- Fixed `session/date-cwd-reminder.ts` injects the exact reminder plus `\n\n`
  before the first user turn of each Session. The Host uses local date and
  slash-normalized cwd (`main.rs:492`, `main.rs:3807`, `actual_host.rs:494`).
  Correlation now matches the complete original task or the complete exact
  source-backed reminder plus task. Two observed local dates allow midnight;
  no arbitrary substring is accepted. V3 omitted slash normalization and
  remains FAIL, Host 14/15. V4 saves actual matching wire text for cold,
  complete, disabled and restored Sessions in the original stderr receipt.
- The original log shows tool cancellation, `agent_end` and Unknown prompt
  settlement **before** the successful abort ACK. Waiting for that ACK already
  archives these frames. This fresh child has one Run; reuse its observed
  terminal or wait if absent. The cancellation error, Unknown, journal,
  no replay, process join and production 30-second timeout assertions remain.

V3 build **28.689 s**, Host **38.885 s**, module **69.193 s**, FAIL retained.
V4 build **13.033 s**, original Host **15/15 PASS in 38.513 s**, module
**53.183 s**, zero ignored. Both repairs leave all **1,605 other source pins**
and the production binary unchanged from V2. `repair.delta.patch`, before/after
pins and manifests retain each transition.

`combined-module-result.json` verifies the original nine PASS executables and
their unchanged source, then combines the final Host receipt: **10/10 original
selected groups, 43 passed, zero failed, zero ignored**. The reused groups are
engine14, policy4, actual-tools4, retained1, journal1, idle1, new1, clear1 and
write1. They were not rerun after a Host oracle change.

```powershell
python -X utf8 scripts/verify_edit_module.py --output '<fresh packet>/gate' --groups host-bridges --delivery-manifest 'D:/workspace/env/cache/ara-port-candidates/vault-production-execution-v4-20261009/delivered-source-manifest.json' --manifest-sha256 2c9c3d2c10b1c3c0b1831ebb3e9bafc9bf554e958151011086efa423909544f5
```

This command describes the pinned snapshot. Later source changes need their
own manifest. Full workspace/clippy/deny gates were not repeated for this
fixture repair; they remain stable milestone gates.

## Original eighteen mappings

Fixed source: `internal-urls/vault-protocol.ts`, router, settings and
`test/internal-urls/vault-protocol.test.ts:52-435`.
Inventory identities: `docs/upstream/inventory/behaviors.tsv:13316-13333`.
Rust owner below is `crates/ara-cli/src/host_vault.rs`; executable evidence is
the existing `host_bridges::vault_family` complete/validation/composition
family, through real ara and controlled Obsidian child processes.

| Original ID | Rust consumer / observed requirement |
| --- | --- |
| B-74af50c533 | `parse` / all supported URL shapes |
| B-79d36e5fc8 | `validate_relative`, `within`, `existing_ancestor` / traversal denied; Unix symlink clauses unrun here |
| B-571d9d5f91 | `read_file`, `resolve_cached_path` / cached markdown read without discovery spawn |
| B-3abe23b835 | `root` / active-vault path from actual CLI output |
| B-02de8f13b2 | `vault_args`, `root` / named-vault argument before info command |
| B-fd01f8f51f | `write`, `resolve_cached_path` / bytes, cached EDIT/AST and readback |
| B-f29659faa9 | `list_dir` / markdown vault links |
| B-e719a708e8 | `plain_file`, `list_dir` / existing folder without trailing slash |
| B-a553c42bf5 | `binary`, `spawn` / documented missing binary error |
| B-f37945c5d9 | `invocation` / 25 exact CLI operations |
| B-0456b6c3d4 | `spawn` / stdout error despite zero exit |
| B-5c28597792 | `spawn` / nonzero stderr surfaced |
| B-69f819520c | `spawn` / actual cancellation, kill, pipe join and Unknown settlement |
| B-6bf427255a | `vault_args` / active and empty sentinel omit vault argument |
| B-b84811c53b | `read_file`, `resource` / editable plain-file receipt |
| B-bc6614a80d | `check_enabled` / disabled read/write/EDIT/AST denied |
| B-9ed9997ba9 | `has_obsidian` / disabled gate hides advertising with binary present |
| B-26364ef3d8 | `has_obsidian` / enabled advertising requires available binary |

Host configuration is published as a prepared Session snapshot instead of the
OMP global Settings singleton. Failed preparation retains the admitted gate;
successful replacement uses the new gate. The process cache survives Session
replacement; explicit Host override and Removed routes retain their ownership.
The current EDIT92 mapping remains SHA256
`0556a23909a213825882d72d5cd5321280dfc707dd10f3e59888f8a50aca74d4`;
no Vault row was added to that denominator.

## One unchanged representative actual task

Procedure SHA256 remains
`91d4177e5abbe8c97e1e517e44a2ef3786864f151ea668b3e6988f58a3402bfb`.
Receipt `real-cas-receipts/20261008T194620Z/receipt.json` is PASS, SHA256
`1b29a9612077780814286dec3a9a7de51f4c78c8b2af88c7b3ad6110b6042452`.
Current local Manager configuration and live model catalogue were checked:
`deepseek-v4.1-flash`, `openai-completions`. Manager Gateway was unavailable;
the trial used the currently configured CAS endpoint. Key stayed in child env.

Session `01a11d0d-75ef-7404-ae4d-85d22ec3a7bd`:

| Observation | Seconds | Calls | Tools | Peer writes |
| --- | ---: | ---: | ---: | ---: |
| Canonical plan and typed approval | 8.538 | 3 | 3 | 0 |
| WRITE, EDIT and actual readback | 6.672 | 6 | 5 | 2 |
| New process, same Session, no-tools recall | 0.965 | 1 | 0 | 0 |

Task time **16.175 s**; overall observation **19.574 s**. Shutdown joins in
**0.036 / 0.022 s**. Exactly two mutations at events **603 / 609**; summary:
`{"pen":5.5,"book":34.5,"cup":21.25,"grand_total":61.25}`.
This matches the original CSV quantities/prices. Summary SHA256
`a592f45b9e1c46a3d0ca4b754a81095974356961b192f177948ac020df172ade`.
Plan SHA256 `2efde017d07b6e04ac024f85338514312fa2c6e0a3c8236b302088c566431ea4`.
Bounds are three Runs, 6/6/2 calls, 2,048 max tokens, 120 s per Run and
480 s overall. Source/binary/procedure guards pass.

This model trial verifies ACP/plan/retained composition with default-disabled
Vault. It is not installed Obsidian model/product acceptance. Actual installed
Obsidian was unavailable; Unix-only symlink and other platforms are unrun.
Complete Extension/SDK, EDIT, WRITE and product obligations remain OPEN.

## Independent review

Reviewer `/root/finish_edit_renderer`, same `ara-git-review` /
`ara-rust-core-review` chain. Final SOURCE APPROVE binds V4 exact manifest,
fourteen hashes and preserved outside/index pins. Original report:
`vault-production-batch-20261009/SOURCE.md`, SHA256
`11b39fabe2c2569d08d84c343695fa73f90fad7dd09f14ec25a1d759063f54b8`.
Final same-chain **POST APPROVE**, `vault-production-batch-20261009/POST.md`,
SHA256 `9619e8a82d221e977ae0e407f9fe4a573ad96e1c8e4774603582d89c0e98a4bb`.
The reviewer independently checked final source/binary/procedure, nine reused
executable hashes, original Host names/counts, actual permanent CAS events,
plan and summary bytes, unique durable receipts, load boundary and both
close/EOF joins. Root separately checked actual CSV/artifact, two raw writes,
final readback, plan/default mode order and recall with no later tool/write.
Controlled Obsidian temporary effects are backed by executed assertions;
disposed temp files were not retrospectively re-read. This is bounded module
approval. Complete parents, installed Obsidian and Unix-only clauses remain
OPEN; acceptance increment is **0**.
