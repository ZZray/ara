# Production Extension Settings source checkpoint

Fixed OMP: `596f2da7101178214aa27a753529d15e6b7ad91d`.
This is **reviewed source WIP, not executable acceptance**. Formal inventory
remains **1/111 accepted, 1 tested, 27 implementing, 82 open**. No Cargo,
runtime, model, commit, push or product installation was performed for this
successor. The terminal v45 receipts are preserved and are not recounted.

## Original behavior and actual preparation

Fixed `settings.ts:166-174,637-654,2958-3001` defines nested path lookup,
undefined-only defaults, a resolved-value cache and ordered recursive merges.
The four current production consumers are `skillful` (schema default true),
`tools.approvalMode` (yolo), `tools.approval` (original Object default), and
`skills.enableSkillCommands` (true).

- `NativeSettingsSource` reads the existing native global first-present
  `config.yml`/`config.yaml`, project `.ara/config.yml`, then explicit overlays.
  It retains parsed source documents, without logging/exporting credentials.
- The CLI setup retains that source receipt and uses it for Skill command
  discovery. Initial unpublished Native preparation imports the documents
  directly into the same heap, performs the original nested merge, and installs
  the executable `NativeSettingsGraph.get/isConfigured` on the prepared owner.
  JSON conversion is avoided because it would erase nonfinite Numbers.
- Object leaves merge recursively; arrays/null/scalars replace. A quoted dotted
  disk key does not replace the original nested setting. User deny/prompt policy
  values remain original Graph handles, with stable repeated-get identity.
- Retained preparation reuses the same parsed source receipt and Settings
  owner/cache. An inconsistent source Arc is rejected before publication.
  Missing/foreign Settings owners and unsupported paths fail explicitly.

The reviewed graft was checked and applied to the tooling production candidate:
three existing source paths plus a new Settings module and its module export.
It does not replace entire author files or the live checkout implementation.

## Exact source and independent review

Packet: `D:/workspace/env/cache/ara-port-candidates/extension-production-batch-20261009`.
Reviewed isolated successor: `context-usage-owner-next`.

| Source | SHA256 |
| --- | --- |
| `settings.rs` | `039172247ee217c3fdac6d7025c15fd384985efb4c6372d1db3acf64ab253d72` |
| `rpc_host_settings.rs` | `e978ffe9c9919427fb8ac17d1f7950b0b8594135f6b0e57acbf361451329d3dd` |
| `main.settings.rs` | `be64847219b7d7f9498921df1a95b59cbe3bb3cc7ed70247d20f0dd54887bf47` |
| `native_setup.settings.rs` | `67a58f497abd1e9e8e7017cba23235e2e71d8c8e4ef1d6e2ad46550d8bb1212b` |

Independent Codex `/root/extension_integration_review` used the ARA source/diff
and Rust semantic review procedures. Limited **SOURCE APPROVE** report:
`REVIEW-ROOT-SETTINGS-SOURCE.md`, SHA256
`7c57c498eb4bf4c47b2a123f4e775cbf764a08f7d945142e23aa95dc6ce971e5`.
The same review closed the retained-source reparse discrepancy and module
export gap. This does not approve the complete Extension surface.

## Remaining gates

One grouped configuration/Graph observation was added to the existing bin
test bank: deep merged deny/prompt, nested dotted keys, parent replacement,
raw null/NaN, repeated policy identity, wrong receiver, unsupported path,
missing overlay and malformed source. **It has not been compiled or run.**

Complete production bootstrap/context publication and actual consumers remain
to be composed and exercised. Full Settings paths/APIs, runtime overrides,
reload/adoption publication, persistence/hooks and foreign capability discovery
remain open requirements. Context usage still needs its prompt/finally,
assistant-before-journal, mutation invalidation and history-adoption wiring.
No getter or narrow source observation substitutes for these parent gates.

Next execution remains one coherent D-target/jobs2 module build and the
existing process/Host gates after composition, preserving original deadlines
and the v45 unknown-effect/no-replay boundary.
