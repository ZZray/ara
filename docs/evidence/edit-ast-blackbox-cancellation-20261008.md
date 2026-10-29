# Joined AST, Edit post-apply and cancellation checkpoint — 2026-10-08

**WIP; execution failed; no complete surface accepted.** Fixed OMP
`596f2da7101178214aa27a753529d15e6b7ad91d`, dirty ARA `dev`, base
`20aa0e2ef9872f633ba2671376cdf778477e7115`. Current surface map is
83 open / 27 implementing / 1 tested / 0 complete out of 111.

## Applied scope and recovery

The reviewed AST v2 + blackbox/repair v2 composition changes 32 paths.
The v6 synthetic admission metadata repair adds one distinct path, for a union
of 33: 31 Rust files and two Markdown prompts. It preserves the original
REPL continuation assertions and boxed allocation wrapper. AST uses the same
Session pending-resolution queue; post-apply uses the existing Host model/auth/
usage owners. The new synthetic failure metadata uses the Session Graph owner.

The initial applier stopped after successfully applying the 32-path patch:
its first raw hash differed because Git converted LF to CRLF. Root independently
compared all 32 live files with the reviewed candidates: 32/32 normalized byte
matches, 2/32 raw matches. There was no substantive content difference. Root
applied only the remaining metadata patch, checked its combined RPC normalized
hash, and formatted only Rust paths. No patch was replayed and no WIP reset.

Application packet:
`C:/Temp/ara-model-fallback-batch/edit-ast-blackbox-repl-metadata-application-v1`.
It retains before snapshots/backups, original apply outputs, the 32-path content
comparison, raw pins, resume-step outputs and final receipt. Receipt SHA256:
`0c987fb6f989158790f14e30d9a4f29466544e54f7cf6d395e4e5d5564bdc5b6`.
The normalized pre-format combined RPC SHA256 is
`19656e3eb3855fc7656bc00fb51e553112952245e7df676511a372b318cf98ea`;
the prior source report called this raw, but Git's delivered CRLF bytes differ.

Independent Codex `/root/repl_metadata_v6_pre` approved source/application
identity and preservation: 776 mapped paths, exactly 33 selected changes,
17 original backups, 16 original absences, original index/HEAD, wrapper and
e2e assertions. The source map covers non-vendor workspace packages and the
three changed vendor files; it is not a newly complete vendor inventory.
Source phase report:
`C:/Temp/ara-model-fallback-batch/edit-ast-blackbox-repl-metadata-independent-post-v1/SOURCE-PHASE.md`.
Source/application approval does not accept runtime behavior.

## Delivered-code execution

One locked all-feature no-run build, existing cache/jobs 2, then existing
module/harness commands with test threads 2:

```text
cargo test -p ara-agent -p ara-edit -p ara-tools -p ara-cli --all-features --locked --lib --bin ara --test agent_loop --test core_hooks --test e2e --test rpc_host --test edit --test edit_post_apply_family --no-run --message-format=json
```

Root then invokes the produced harnesses directly. Four original e2e names
are selected individually: Bash cancel/next prompt, EOF, Skill cancel/next
prompt, and tool deadline. Zero executed tests are never counted as a PASS.

| Target | Passed | Failed |
| --- | ---: | ---: |
| Agent lib / loop / hooks | 11 / 56 / 21 | 0 |
| Edit lib | 92 | 0 |
| Tools lib / edit / post-apply | 42 / 14 / 2 | 0 |
| CLI lib | 244 | 1 |
| CLI bin | 79 | 2 |
| RPC Host | 61 | 1 |
| Four selected e2e | 3 | 1 |
| Total | 625 | 5 |

Exit 101; wall **677.885s**, no-run compilation **511.594s**, harness commands
**160.993s**. Source, index and harness hashes remain unchanged during execution.
The Edit library's 92 tests are not the separate 92-row CA-TOOL-EDIT inventory.

Packet:
`C:/Temp/ara-model-fallback-batch/edit-ast-blackbox-repl-metadata-execution-v1`.
Final binary SHA256:
`c884a8df02519afb18576c5270f2d9387218cc2f4629e4e09cc9c0d90a3370e4`.

## Retained failures and current diagnosis boundary

| Failure | Actual observation | Current boundary |
| --- | --- | --- |
| Native AST grant group | Revoke branch's Debug-text assertion lacks the expected grant reason | Inspect the actual error and admission owner before choosing a correction |
| Native Edit post-apply group | Exported result has no `isError: false` property at the queried boundary | `output_graph` currently adds `isError` only when true; verify original return shape and all remaining artifact/ordering assertions |
| CLI request observer | Original five-second `run(args)` timeout | One serial rerun of the unchanged harness also fails in 5.140s; not accepted as load flakiness |
| AST RPC group | Model-cache fixture setup fails with SQLite code 14, before child AST execution | No RPC AST exercise can be credited from this failed group |
| Bash cancel/next prompt | Expected role sequence, successor text and second HTTP request pass; `entries[5].message.stopReason` is null | `journal` and `roles` each skip one entry: the checked index is toolResult, and index 6 is the abort assistant. Correct the observation and execute its original abort assertion; the actual abort value is not established by this failed log |

Skill cancellation/next prompt, EOF and deadline pass. Existing held-model RPC
success/abort/same-Session successor passes. These selected observations do not
accept the complete lifecycle. The earlier v5 95/2 and other failed attempts
remain preserved in their original packets.

Observer-only serial rerun packet:
`C:/Temp/ara-model-fallback-batch/edit-ast-blackbox-observer-unchanged-rerun-v1`.
Result is 0 passed / 1 failed, same source/harness; it is separate from 625/5.

The observer's reachable ownership chain is now source-supported: an empty
tool selection still creates ToolContext, its installed post-apply facade keeps
a borrowed factory/PreparedRoute clone, and request options retain a cloned
RequestTextObserver `mpsc::Sender`. `run` drops the main factory/provider before
awaiting the receiver task, but the facade remains alive. A scoped producer-side
repair is being prepared in Temp; no timeout/expectation relaxation is authorized
by this diagnosis and no repaired-source execution has occurred.

## Remaining gates and next work

Final independent execution POST is **REJECT** in the above independent packet:
`REVIEW.md` SHA256
`a762dabee3b35d3cba1ca25d81979dff5e59c83b44c4b53f86da686f148d8097`,
`pins.json` SHA256
`c34ef480e2a538856ea0431fb0a0f37d8dace72e5ba5c60bd08d30b9add7af6a`.
It retains historical SOURCE/APPLICATION approval, independently binds 776
source paths, 11 harnesses, the binary and original logs, and confirms the
observer's production shutdown blocker plus three observation/fixture
problems. AST revoke's actual error remains unresolved; no failed runtime
claim is promoted to acceptance.
The prepared selected CAS AST task is **unrun**, because this deterministic
gate failed. Earlier CAS edit/recall results retain only their original source
and binary scope. No actual side-model/auth/usage, account login, complete
ACP/LSP, strict workspace Clippy, release or product acceptance is claimed.

The current EDIT closure map keeps 92 original rows plus five inspection
contracts. Renderer 41 and VCS five remain implementation gaps; ACP/LSP/plan/
URI need production owners; AST has explicit source-contract differences;
repair/model-switch tasks need actual receipts. A VCS source-only candidate is
frozen separately and has no production consumer or executed acceptance.

Repair the delivered failure paths first, retain original assertions and
reuse unchanged evidence. Compose complete families for one build and grouped
execution. Complete EDIT before WRITE and Chat. No commit, push, installation,
deployment, full marker advance or durable memory promotion occurred.
