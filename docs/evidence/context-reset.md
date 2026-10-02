# Same-Session context reset — bounded WIP

Base `b80f542`; fixed OMP `596f2da7101178214aa27a753529d15e6b7ad91d`.
This checkpoint extends native Session/compaction and the reference REPL. It
does not accept the complete AgentSession, compaction, TUI or Provider surfaces.

## Requirement and fixed source

- `packages/coding-agent/src/session/session-manager.ts:2447`: payload-free
  `appendResetBoundary` on the existing raw parent chain.
- `packages/coding-agent/src/session/agent-session.ts:4686–4762`:
  idle context reset preserves Session identity/history/settings, rotates
  provider runtime state, resets present runtime owners and refreshes base rules.
- `packages/coding-agent/src/session/session-context.ts:302` and
  `packages/agent/src/compaction/compaction.ts:1315–1428`: latest reset defines
  the active suffix; a newer compaction supersedes that boundary normally.
- `packages/agent/test/compact-reset-boundary.test.ts`:
  `B-723a97b3e8`, `B-ed8185d6f6`, `B-d994661a46` are the three reused input
  families: no pre-clear resurrection, reset after old summary, and new summary
  after reset.
- `B-7664975ed4` in `agent-session-context-file-reload.test.ts:85` and
  `B-e2b66e1fb3` in `slash-commands/clear-alias.test.ts:9`: reread AGENTS.md
  and keep `/clear` distinct from `/new`.
- Native `session-tools.ts:1667–1676`, SDK `:3147` and the separate
  `refreshSkills:1315–1335` preserve the loaded Skill snapshot on base-prompt
  refresh. A context reset does not rediscover the command/tool Skill catalogue.

## Delivered behavior

`ara-session` appends the native reset marker using its existing append
transaction. The latest active suffix is shared by plain/model fallback,
strict source snapshots, projected summaries, checked cumulative source IDs,
safe summary prefixes, interrupted-tool recovery and failed-assistant summary
progress. Strict raw ID/parent checks still inspect the full journal. Raw
history, old unknown-effect receipts, model metadata and Session identity remain.
Existing lazy materialization is preserved: memory-only/no-assistant journals
retain the marker in memory until normal materialization. The durable append
failure fixture uses an already materialized Session; no immediate disk write
is claimed for a lazy journal.

The REPL admits `/clear` between turns. It prepares refreshed AGENTS/SYSTEM/
APPEND prompt and hooks with retained tools and Skills, then appends the reset
boundary before publishing empty context, turn zero and a fresh binding for
every protocol. Codex also receives a fresh in-memory provider Session UUID;
the journal ID stays the same. Preparation or append failure preserves the
current conversation/binding. MCP tools are retained. No RPC clear command
or durable provider-ID protocol is invented.

## Module command and meaningful coverage

```text
python -X utf8 scripts/verify_context_reset.py --module source
python -X utf8 scripts/verify_context_reset.py --full --output <artifact-root>
```

The seven modules are source, projection, reload, context, recovery, REPL and
account. Existing OMP input families share grouped scenarios. Real child
fixtures cover rules refresh, same Session/file, compact/reopen, append failure
with stateful Responses binding preservation, successful Responses chain reset,
and distinct Codex runtime attribution followed by unchanged `/new` behavior.
The synthetic account fixture is not actual OpenAI login acceptance.

Root is the sole Cargo owner. Independent reviewers use exact source and
receipts. Ordinary details receive no extra micro-tests or mutation runs.
Unchanged passed modules are reused across test-only fixture migrations.

## Execution and audit

Artifact root: `C:\Temp\ara-native-entry-batch`. Seven delivered source/test/
script paths are pinned by the final gate and independent review.

| Module | Passed / failed / ignored | Receipt |
| --- | --- | --- |
| Projection / reload / context / recovery | 14/0/0; 8/0/0; 5/0/0; 4/0/0 | `module-20261002T064543Z` |
| REPL / synthetic account | 94/0/0; 4/0/0 | `module-20261002T064543Z` |
| Strict source | 8/0/0 | `module-20261002T065000Z` |
| Final Windows backend including doc tests | 1,534/0/20; 121 result suites | `module-20261002T065000Z` |

Seven modules total **137/0/0**. The six reused modules executed in **13.596s**;
their five owned source/test hashes match the final snapshot. Final strict
source execution was **0.400s**. One stable final command took **107.813s**,
including backend **102.632s**; owned-package formatting, strict all-target
Clippy, all-target/doc tests, inventory, dependency policy and build pass.
The final receipt has `exitCode=0`, `sourceUnchanged=true`. Twenty ignored cases
and existing dependency license/duplicate warnings retain their recorded limits.

Failures remain recorded, without promoting failed checks: initial API enum/
string comparison failed compilation; a reset fixture assumed JSON map order;
the first shared gate found the old source fixture still rejected native reset
as unported. That existing family was migrated while preserving undecodable,
compaction/custom/branch/future-entry negatives, raw bytes and active-only IDs.
Its first migration used two independently timestamped UserMessage values;
the final fixture compares the original input Message. Production bytes stayed
unchanged during that migration. No extra micro-test or mutation family was
added. The failed backend in `module-20261002T064543Z` is not acceptance evidence.

### Actual CAS task, clear, summary and reopen

`live/20261002T065214Z/receipt.json`: **PASS**, **15.817s**, **nine actual CAS
requests**, two processes. The current Manager local gateway catalogue returned
`URLError`; the authorized fallback used CAS directly from private Manager
`ry_switch` configuration. The live catalogue confirmed `deepseek-v4.1-flash`
on OpenAI-compatible Chat Completions at `https://cas.ciqtek.com`. This does not
claim successful traversal of the unavailable local gateway. No credentials
were saved. Bounds: 14 relay requests, 120s per process, 2,048 output tokens/call.

Original Session `01a0fb62-8e34-7769-895e-c6e906895325` first recorded
`Juniper-OLD-908`, then `/clear` preserved its raw journal and emptied the active
context. Every later wire request, including summaries/reopen, excludes that
old label. The actual task read `stock.csv`, wrote/re-read `current-total.json`
with **folder=87.5, lamp=65, grand_total=152.5**, and left CSV bytes unchanged.
The original Session's two independent summaries retain `Maple-7241`, use the
native Assistant kept boundary and exactly nine post-reset raw source IDs.
Tool-disabled process reopen returns:

```text
folder_quantity=7; lamp_unit_price=16.25; grand_total=152.5; shipment_label=Maple-7241
```

Final binary SHA256:
`3fb08a651d73da3b09b895ffc5bf2e2832303326237d0cb8ab05b35d128200eb`.
Artifact SHA256:
`0d2a4d0c8d530021c8515a925d442ee2e6c2e72ad01ab3342ee82f5f81a1b5fa`.
Seven visible assistant receipts total **23,498** tokens; two actual summary
calls are not persisted as Host message usage, so this is **not** a total for
all nine requests. Unknown usage is not zero.
The small live example's estimate grows from 386 to 766 tokens because summary
wrappers have overhead; this trial proves context/source behavior and recall,
not token savings.

### Independent and Root audit

`/root/ctx_oracle_diff_review` uses `ara-git-review` and `ara-rust-core-review`,
covering all seven source/test/script files and their hunks. The pre-review's
Skill snapshot inconsistency was fixed and rechecked against fixed native
prompt refresh. `post-reset-review.json` records exact hashes, real logs and
explicit gaps. Root directly inspected fixed source, final source, gate logs,
wire requests, title/header/journal, summary source IDs, tool artifact and
reopened reply; `root-audit.json` pins these observations. This is a tested/
audited bounded WIP checkpoint, not full-surface or phase acceptance.
Final independent result: `PASS_WITH_EXPLICIT_GAPS`; frozen POST SHA256
`39e9a0d4f1a078c363a147663eaa76284dedac71183a73b4fa08035e1dbb3188`.

## Remaining scope

- Complete raw metadata/custom/branch/Bash adaptation, grouped multi-message
  projection and native raw cut/source identity remain the next coherent group.
- All original compaction methods/reducers/rescue, registry/auth, smart/local
  and custom callers, input/token/settings/receipt contracts remain mandatory.
- No ARA equivalent of every OMP async job/advisor/checkpoint/plan owner is
  exercised by idle reference REPL reset. Full host teardown remains open.
- Real Responses/Codex/account routes and exact Linux verification remain open.
  Codex provider-side state after process reconstruction is not established;
  persistent model context is tested separately from server-side memory.
- P0/V1 accepted, P1–P6 open, RPC 27/42 and full port marker null remain unchanged.
