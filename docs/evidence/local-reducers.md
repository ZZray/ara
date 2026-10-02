# Native local reducers and artifact recovery — bounded WIP

Base `3a0202886d703da7d5d3449d79279dc51674e8c7`; fixed OMP
`596f2da7101178214aa27a753529d15e6b7ad91d`. This checkpoint implements local
pruning, elide/images/thinking and ordinary tiered rescue. Full compaction,
complete Host behavior, P1–P6 and the full parity marker remain open.

**Execution status, 2026-10-02:** tested/audited bounded WIP. The final stable
Windows gate, five delivered-binary Host families, actual CAS artifact task and
original-Session reopen pass. Independent Codex POST has no open source finding;
Root audit binds all 25 source/test/runner hashes, binary and task artifacts.
This does not accept the whole compaction surface or the explicitly untested
process fault below. Earlier failed receipts remain diagnostic evidence only.

## Source and observable scope

| Fixed source | Rust owner and behavior |
| --- | --- |
| `agent/src/compaction/{pruning,shake,tool-protection}.ts` | `ara-agent::compaction::local_reduction`: age, superseded/useless results, native read-selector grammar, separate prune/shake windows, presets, fence/XML regions, images and thinking |
| `coding-agent/src/session/session-maintenance.ts:463–737,2660–2748` | `ara-cli::{local_reduction,rpc_host_reduction,rpc_host_maintenance}`: checked local edits, provider refresh, retry fit, 80 percent automatic headroom and elide then images rescue |
| `coding-agent/src/session/session-stats.ts:343–390` | `ara-cli::context_budget`: pre-anchor savings, native snapshot correction, system/tools floor, pending input and invalidated usage anchors |
| `coding-agent/src/session/{artifacts,session-manager}.ts`, `internal-urls/artifact-protocol.ts` | `ara-cli::session_artifacts`, `ara-tools::read`: per-Session numeric artifact IDs, one offload blob, disk fallback, immutable selection reading and explicit Host URI precedence |
| `coding-agent/src/session/messages.ts:811–895`, `agent/src/compaction/messages.ts` | `ara-session` raw FileMention projection and pruned ToolResult projection; raw origin remains independent of wire role |

Source paths above are relative to fixed OMP `packages/`. Root and the independent
reviewer read exact Git objects. Native input families include `shake.test.ts`,
`supersede-prune.test.ts`, `agent-session-prune-persistence.test.ts`,
`agent-session-stats.test.ts` and artifact integrity/concurrency/sanitization
tests. Reusing a family does not claim every upstream test was run or ported.

## Implementation and retained boundaries

Core shares each original text slot through `Arc<str>` and returns checked
UTF-16 edits. Session compares the complete raw snapshot, changes only selected
slots, preserves IDs/parents/unknown fields/off-branch entries and publishes
atomically. Before-publication failures retain old disk/memory; a failure after
publication retains the candidate with an explicit durability-unknown error.
Recovery edits publish their matching discarded-entry marker in the same write;
later rescue layers reuse that marker. The old owner-branch equality check stays
strict, and unknown tool effects are never automatically replayed.

Reserved historical LoopGuard notices use a versioned internal reduction proof.
Their original native form and complete region/placeholder edits are validated
before publication; model/public/reopen views use actual reduced content.
Malformed proofs fail closed. Live event admission does not accept that proof.

Prune replaces complete ToolResult content. Elide replaces its first nonempty
text, removes other text and preserves nontext blocks. Presence of `prunedAt`,
including null, only normalizes provider projection; it does not rewrite raw
receipts. FileMention text becomes Developer context, images User attachments,
empty files zero fragments; it cannot be a native kept boundary.

Hosts own artifacts and URI authority. Persistent artifact save scans numeric
IDs once, stages/checks bytes and renames a single blob. Save failures produce
the native bare placeholder. The native nonpersistent save map retains IDs
0,1,…, but URI recovery remains unavailable without a disk Session, as in fixed
OMP. `/new` resets that store; `/clear` preserves it. Explicit registrations
and Removed tombstones precede native artifact fallback. Oversized artifact
selectors use the existing buffered line-window reader without whole-file
materialization, hashline admission or an editable URI store.

Reference REPL adds `/shake [elide|images|thinking]`, with manual aggressive,
automatic default and rescue presets. Its original V1 `--no-session /compact`
refusal remains an explicitly incomplete native caller contract; this checkpoint
does not deliver ephemeral summary compaction. RPC stale pruning runs independent
of compaction.enabled. Host cache constants are 8,000 warm tokens and 90 minutes
idle; a stale upstream prose comment saying one hour is not the constant.

RPC checked rewrites rebuild the Provider and publish the same model/transport
binding to the live execution snapshot. Automatic headroom retains this turn's
prune savings across shake and includes pending User input. Real body rewrites
invalidate old billed anchors without fabricating usage; stale pruning preserves
that rebase, and a fresh valid report, including total-only usage, replaces it.
No-op local rescue never counts as progress. Estimates remain approximate where
the native model tokenizer is unavailable; no hard wire-fit claim is made.

## Reproducible grouped entrypoints

```text
python -X utf8 scripts/verify_local_reduction.py --module <name>
python -X utf8 scripts/verify_local_reduction.py --full --output <artifact-root>
python -X utf8 scripts/verify_local_reduction_host.py --binary <delivered-ara>
```

Modules are core/session/adapter/cut/projection/notice/terminal/host/repl.
`--full` shares fmt, strict workspace Clippy, all target/doc tests, fixed inventory,
dependency policy and final binary build. Live API tasks are bounded separately;
the deterministic runner never reads credentials. Root owns Cargo, network,
private config and Git writes. Authors own disjoint implementation files;
independent Codex review uses `ara-git-review` and `ara-rust-core-review`.

Fixed OMP already has native tests: a read-only `git ls-tree -r --name-only`
at the locked SHA finds 2,370 `.test/.spec.{ts,tsx,js}` files, including 38 under
`packages/agent` and 1,416 under `packages/coding-agent`. These are file counts,
not case counts or a promise to reproduce unrelated package tests. The user
requested module families and one stable shared gate, with no ordinary detail
micro-tests; meaningful state/protocol/tool-effect failure coverage remains.

## Final executed evidence and audit

Root artifacts: `C:\Temp\ara-local-reducers-batch`.

| Check | Final receipt/result | Measured wall time |
| --- | --- | --- |
| Stable gate | `module-20261002T084702Z/receipt.json`, exit 0, sourceUnchanged; Host 17/0, backend 1,557/0/20; fmt, strict workspace Clippy, all target/doc tests, inventory, deny and build pass | 152.621s; compile step 0.427s, Host 9.211s, backend 139.010s |
| Actual controlled Host | `host/run-20261002T084954.628815Z/receipt.json`, five families pass, source/binary unchanged | 3.756s |
| Actual CAS task | `live/20261002T085010.600732Z/receipt.json`, PASS; three processes, five actual chat calls, verified catalogue `deepseek-v4.1-flash`, OpenAI Completions | 12.512s |
| Independent POST | `/root/ctx_oracle_diff_review`, `post-review.json`, `PASS_WITH_EXPLICIT_COVERAGE_LIMITS`, findings empty; all 25 frozen hashes equal gate before/after/current | Read-only review; no duplicated Cargo/model run |
| Root acceptance audit | `root-audit.json`, PASS; hashes, binary, process logs, raw Session, wire and actual artifacts directly checked | Artifact-only audit; no repeated task |

The controlled families cover elide/artifact selector/continue/reopen,
images/thinking replay, artifact disk failure with bare placeholder, unknown
effects plus invalid-mode zero-call behavior, and large artifacts with Session
switching. The last family reads two 9,840,037-byte artifacts through selectors
A → B → A in one RPC process; whole inline read is refused and an explicit
Removed tombstone prevents native fallback. Artifact hashes remain unchanged.

The preferred local Manager gateway was unavailable. Its private configuration
provided the verified direct CAS route; no credential values were saved or
committed. `/shake` used zero model calls. The actual model then called
`read artifact://0:1-20` → `write recovered-totals.json` → `read` of that file.
The actual 94-byte JSON contains folder 87.5, lamp 65.0, grand total 152.5 and
the hidden shipment label `Cedar-2759`. The first request does not contain that
label; it must be recovered from the original receipt/artifact. All five wire
contexts exclude the old heavy marker. A tool-disabled original-Session reopen
correctly recalls the values. No failed write or real task was automatically
replayed, and the artifact and output JSON remain unchanged after reopen.

Persisted actual assistant usage totals 34,229 tokens, excluding imported seed
usage. Unknown individual usage buckets remain unknown. This is one bounded
model/route task with controlled historical input, not all-model acceptance.

Delivered binary SHA256:
`f8b39d5ba0097503fd37f05019d9cbda1f05f82ccabb02434d78a2d7489475fc`.
Independent POST SHA256:
`235b9cc828a46b23193bec894b7914d313c59e64481f8fa4d0e07ff6a142b35d`.
Root audit SHA256:
`91f9060524186035454ff7e2580a002f6d0be9cc952fb13108151c83dc50b0f0`.

Implementation/source mapping began about 07:44 UTC; actual live verification
finished about 08:50:23 UTC, approximately 67 minutes including integration,
review and failed checks. Documentation/Git closure follows separately. The
long failed gate required 4m27s all-target recompilation before exposing the
Windows stack regression; the successful final gate needs 2m33s. Gate backend
time includes compiler/lint work and is not pure test execution time.
Do not repeat unchanged passed Cargo/live evidence after documentation edits.

## Failures and corrections

- Initial compile failed on `&MutexGuard` where `&SessionJournal` was required;
  the actual journal reference was corrected.
- An adapter fixture assumed memory artifact URI recovery. Exact fixed OMP
  only resolves disk artifacts; that fixture was corrected without weakening
  persistent recovery assertions.
- The RPC billing-trigger fixture used a threshold below newly included native
  system/tools cost. Inputs moved from 5k/1k to 100k/50k while retaining billing
  versus local-count behavior, single summary and persistent policy assertions.
- `083301Z` ran eight successful modules, then REPL 92/2. The help insertion and
  new memory journal affected existing V1 expectations. Root restored the old
  help prefix and `--no-session /compact` refusal; original tests were retained.
  `083426Z` REPL 94/0 passed before strict Clippy stopped on new lint errors.
- Clippy corrections reduced the large error receipt through a Box, preserved
  native descending sort, made NUL literals unambiguous and removed needless
  clone/lifetime/mutability. The new Host test was corrected to read the actual
  nested execution model/Provider binding. Assertions were not weakened.
- Root found FileMention excluded by Core cuts but admitted by the Session kept
  boundary validator. The Session exclusion now matches fixed OMP and Core.
- `083910Z` backend failed 775/1/11 after 4m27s all-target compilation: the
  existing abort/join/new-Session bridge process overflowed its default Windows
  main stack after `agent_end`, before adoption ACK. Root boxed only the two
  maintenance settlement awaits. All six unchanged bridge flows passed in
  8.292s including compilation. This supports the async future-layout hypothesis
  through a before/after intervention; no backtrace identified the exact stack
  frame. No stack setting, cancellation order or assertion was changed.
- A Root `cargo fmt --all` at 08:11 unintentionally formatted 48 vendor files.
  The 07:38 original command proves vendor normalized diff was empty before this
  batch. Those formatter changes were backed up and undone; vendor actual diff
  is empty, original stat WIP remains unstaged. No claim of a captured original
  physical line-ending manifest is made. Formatting now targets ARA-owned crates.

Failed checks are retained under `C:\Temp\ara-local-reducers-batch` and excluded
from acceptance. No passed full backend or live task has been repeatedly rerun
without a concrete source change/failure. Ordinary details gained no micro-tests.

## Explicit remaining work

Remote/handoff/snapcompact and frame rescue, wider registry/auth callers,
cross-Session registry artifact search, path-only archive/search workflows,
native tokenizer/wire projection, broader method/settings/role/legacy receipt
contracts, actual OpenAI account tasks, image summaries and Linux remain open.
The exact no-soft prepublication RPC process fault was not injected: that branch
is source-reviewed and publication-phase faults run through the real Session
writer. It must not be described as a completed process fault trial.

P0/V1 remain accepted; P1–P6 open, RPC 27/42, full parity marker null. This
checkpoint does not drop the remaining required fixed OMP behavior.
