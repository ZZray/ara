# RPC soft compaction checkpoint — 2026-09-30

Status: **tested WIP**. This coherent
batch adds bounded `compact` and `set_auto_compaction` implementations. RPC
has **25/42 bounded implementations**; complete CA-RPC, OMP parity
and P1–P6 remain open. The fixed marker does not advance.

## Source and observable scope

Fixed OMP: `596f2da7101178214aa27a753529d15e6b7ad91d`.

- `modes/rpc/rpc-mode.ts:440–447,1376–1384`: manual compact remains serial;
  its result carries summary, first kept ID and tokens before.
- `agent/src/compaction/compaction.ts:931–960,1315–1425`: optional focus is
  appended to initial/update instructions; further soft summaries include the
  previous derived summary and the current source window.
- `session/session-maintenance.ts:738–897,1097–1150`: validate, abort/join,
  commit/project/reset provider replay, then preserve and drain queues.
- `session/session-maintenance.ts:1622–1678,2083–2140,4185–4199`: pending input
  and billed/stored threshold input; persistent enabled policy and empty method
  order repair. `agent-session-events.ts:19–35` defines actual auto events.
- `config/settings.ts:1775–1793,2712+` and `utils/dirs.ts:27`: first existing
  native `config.yml`/`config.yaml`, scoped reload/merge and atomic write.
- `session/session-manager.ts:2299–2324,265–273,1166–1174,1464`: late branch
  append restores live leaf in memory; reopen selects the last raw entry.
  The Rust checkpoint preserves this upstream limitation.

Core gains optional focus while old callers retain their existing behavior.
Session gains a projected snapshot and checked commit: session/leaf/current
summary/window must remain equal; cumulative original message IDs validate the
replaced raw prefix before a write. No new journal record/active-leaf format.

Host manual maintenance joins the owned Run, flushes pending Bash, checks idle
replacement eligibility before billing, requests a bounded no-tools summary,
commits the checked view and publishes model/public projections. Queues/modes
remain on the original Agent; provider replay history resets. Any commit I/O or
post-commit transcript failure stops the connection; ordinary validation/no-cut
errors leave it available. A late Bash remains on its original raw parent.

Automatic threshold maintenance executes at direct prompt, queued steering
admission and joined successful boundaries. It emits start/end receipts, uses
the maximum of the supported billed context and stored estimate, counts pending
input, and prevents an unchanged failing/no-progress source from being billed
again. `--compact-threshold 0` still suppresses automatic work. The enabled
policy defaults true and survives new/switch/restart in `$ARA_HOME/agent`.

## Delivered files and verification

Direct files: `ara-agent/src/compaction.rs` and its prompt/call tests;
`ara-session/src/lib.rs` and `compaction_projected.rs`;
`ara-cli/src/rpc_host.rs`, `rpc_host_settings.rs`, registration in `main.rs`,
`rpc_compaction.rs`, and the existing YAML dependency edge in CLI Cargo/lock.

Receipt root: `C:\Temp\ara-compaction-batch`. Initial compilation exposed an
ambiguous settings Result inference; the direct boolean match fixes it. A later
CLI binary `cargo check` passes. Final execution is recorded separately:

| Check | Result | Time |
| --- | --- | --- |
| Core prompt/call, projected Session, native settings | 25 + 9 + 6 passed, no failures | 4.104 + 2.611 + 15.239 s including compilation |
| Actual RPC children, final | 6/0/0 | 0.50 s execution |
| Final backend gate | 1,283/0/1, 97 suites; fmt/Clippy/target/doc tests pass | 99.717 s |
| Dependency policy | Four checks pass | 13.274 s |
| All-feature binary build | Pass | 0.329 s |

`full-receipt.json` pins all eleven delivered files before/after the final gate.
The pipeline takes 113.321 s. Final binary SHA-256 is
`a1bbe19f4390996aec6c5b3f71839c86847ea1cf161e030a96bf5673ee67a382`.
The existing ignored ara-ast fixture remains explicit.

The first child run was 5/6: an assertion demanded literal `tool_choice: none`.
Fixed summary context has no tools; the existing Chat Completions adapter omits
tool choice when tool names are empty. After source verification, only that
assertion accepts absence or `none`; the no-tools check stays independent and
explicit JSON null remains rejected. `focused-first-receipt.json` and
`focused-rpc-first-failure.log` preserve the failure; `focused-rpc-final.log`
records the corrected process test. Final full verification covers that file.

Cases cover two chained summaries/focus/source IDs/live projection/reopen;
invalid focus, tool-bearing and length-stopped summaries; summary held while
Bash starts independently, late old-parent receipt and fixed last-raw reopen;
accepted serial summary draining on EOF; true/false persistent policy with
billed threshold/new/switch/restart/one-shot terminal maintenance; and actual
commit I/O failure stopping the connection before queued prompt effects.

## Bounded actual task and audit

`real-compaction-zqcxcsr2` uses freshly verified B.AI `deepseek-v4.1-flash`,
Chat Completions, only the `write` tool and isolated home/work/session paths.
Limits are three Runs, three model calls/Run, 35 s/Run, 1,024 output tokens/call,
one bounded summary and 140 s wall. Executed `real_compaction_trial.py` is kept.

The first Run writes an unpredictable nonce artifact; the second writes an
acknowledgment without repeating it. Manual compaction summarizes all original
Run entries and retains only the later turn. The focus/restart recall prompts
contain no nonce. The derived summary contains it, while kept raw/live messages
do not. Original work artifacts are removed after preserving evidence copies;
native restart writes `recall.json` from the summary with the same value.

Observed **PASS**, **17.461 s**, **3 Runs / 7 logical model calls** (six Run
assistant terminals plus one accepted summary). Physical provider retry attempts
were not captured. Reported Run usage is retained; summary usage and total cost
are unknown. Provider request bodies were not captured; no real-model
auto-compaction acceptance is claimed. Raw inputs/stdout/observed frames, complete
tool receipts, native journals, actual JSON artifacts and source/driver/binary
pins are available for independent audit. Parent `audit_receipts.py` verifies
228 raw output frames, ten raw inputs, all three Run terminals equal their
complete message-end sequences, all three complete write receipts equal native
records, cumulative summary source IDs, artifact values and 29 retained hashes.
`receipts-audit.json` records PASS. Full gate pins cover eleven files; the
executed task driver pins six relevant source/test files and the same binary.
Reported Run usage totals 18,858 tokens; all-model usage remains unknown.

Delivered code is `599db7c362492ebe7ff7f268898fea1f80759bce`.
`committed-source-audit.json` confirms ten raw files equal committed blob bytes;
CLI `Cargo.toml` differs only by tested CRLF to committed LF normalization.
The initial all-raw-equality audit failure is retained as
`commitblob_raw_first_attempt.py`; no raw-equality claim is made for that
manifest. Source structure/dependency values are unchanged.

Exact-code Linux [repository run 36734244390](https://github.com/ZZray/ara/actions/runs/36734244390)
passes **1,286/0/1 across 97 suites**, including fmt, all-feature Clippy, target
and doc tests. First backend RUN to PASS is **272.649 s**, including fresh
compilation. `linux-final.log`, `linux-final-status.json` and `linux-audit.json`
retain the exact SHA, raw commands, result counts and hash. Same-code fixed RPC,
directory fault, Skill invocation and directory oracle workflows also report
success (36734244423, 36734244437, 36734244557, 36734244456); their status is not
a new independent artifact audit of each upstream surface.

Independent Codex `host_bridges_independent_review` reviewed the narrow plan,
implementation and resolved ordering/storage/queue findings. Initial/source
records remain in `C:\Temp\ara-bash-batch\compaction-pre-review.json` and
`compaction-source-review.json`. Final review is **BOUNDED_FINAL_PASS**, with
no new code/runtime/document blocker. The reviewer independently verified
exact-code Linux counts/time/hash, runtime terminals, complete write/native
receipts, restart recall, source IDs, 11/6 pins and the manifest newline-only
exception. `compaction-final-independent-review.json` and
`linux-independent-review.json` retain the final scope and checks; unchanged
completed artifact audits were reused without rerunning Cargo or the model.
Formal full-maintenance/CA-RPC/P1–P6 acceptance does not advance.

## Explicit remaining boundaries

Only soft whole-turn preparation is implemented. Fixed split-turn preparation,
remote/handoff/shake/snapcompact, method preference execution, speculation,
promotion, idle/incomplete/mid-turn triggers and overflow recovery continuation
remain open. The existing explicit numeric CLI threshold is used because the
current model contract has no context-window field; no capacity is invented.
Queued follow-up during an owned Run needs the later mid-turn maintenance hook.
Queued steering plus auto-compaction has source-backed admission wiring but
no new dedicated combined process oracle in this batch; that test remains
required for complete maintenance acceptance.
Summary error/abort/length/tool calls and unsafe/unknown-effect prefixes are
rejected; missing legacy summary provenance cannot be used for a chained update.
Print/REPL retain their accepted single-level V1 scope.

The narrow native policy writer preserves parsed unknown values and rereads
before writing. Full project/foreign/config overlays, lock-chain conflict
coordination, malformed-config quarantine and preservation of YAML comments,
anchor/tag syntax are not claimed. A malformed or unreadable selected config
returns an explicit error. A terminal Run or HTTP response is not acceptance.
