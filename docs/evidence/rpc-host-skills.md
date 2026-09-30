# RPC Skill invocation and owned queued provenance

**Bounded implementation point accepted, 2026-09-30.** Fixed upstream:
`596f2da7101178214aa27a753529d15e6b7ad91d`. Baseline Rust: `6597a06`; implementation
checkpoint `677489a`, final test correction `97567f0`.
This is a bounded dependency of CTX-01e and the production RPC Host. Complete
CA-RPC, CTX-01e, P1-P6 and the full OMP marker remain open.

## Observable contract and source

- Fixed `packages/coding-agent/src/modes/rpc/rpc-mode.ts:133-200,1093-1105`
  recognizes only registered, enabled Skills in RPC `prompt`, reads fresh source
  before ACK, answers with `data.agentInvoked:true`, then invokes or queues the
  prompt. Default streaming behavior is steer; explicit followUp is supported.
- At `rpc-mode.ts:1094-1101,1146-1177`, recognized Skills ignore images, while
  unknown/disabled ordinary prompts and direct steer/follow_up/abort_and_prompt
  retain typed images. Direct commands do not use the dedicated Skill helper.
- `rpc-mode.ts:214-232` maps a skipped post-ACK dispatch to correlated
  `prompt_result` with `agentInvoked:false`, and rejection to a correlated error.
  Invoked or queued work emits no false local completion.
- Native public identity comes from the active raw Session branch, including
  source content, details and timestamp after the original Skill file is gone.
  Provider replay remains its separate recorded model projection.

The prior three-view Codex consensus and refreshed current-source scope remain
in `C:\Temp\ara-rpc-host-session-validation\rpc-skill-design.txt` and
`C:\Temp\ara-skill-source-switches\rpc-skill-next-scope.txt`. The parent also
read the pinned upstream blobs and current dependency source directly.

## Implementation ownership

| File | Required change |
| --- | --- |
| `ara-agent/src/agent.rs` | Generic `AgentInput`, additive owned prompt/queue APIs, one canonical queue, whole-envelope rollback |
| `ara-agent/src/agent_loop.rs` | Default-compatible owned hooks and input consumption; metadata excluded from provider/transcript/RunReport |
| `ara-agent/src/event.rs`, `lib.rs` | Additive awaited `emit_input`, exports; existing event enum/projections unchanged |
| `ara-cli/src/main.rs` | Pass the already-discovered Skill snapshot to RPC |
| `ara-cli/src/rpc_host.rs` | Shared pre-ACK preparation, owned dispatch, custom receipt and Run-local public sequence, atomic config/Skills adoption, active-branch restoration |
| `ara-session/src/lib.rs` | Expose the existing Skill entry decoder via `Entry::skill_prompt` |
| Existing Core/RPC/Session tests | Payload correspondence, failure, order and native restart oracles |

Shared Core transports opaque `Arc<Value>` provenance and does not interpret
Skill or Session state. RPC journals one custom receipt when the envelope is
consumed; ACK and in-memory queue admission do not claim durable enqueue.
`agent_end.messages` comes from that Run's completed public events; terminal
publication stays exclusively after join and active-slot removal.

Preserved: old Message/event APIs, queue modes/FIFO/precedence, print/REPL,
provider schema, assistant/tool journal barrier, original WireValue IDs and
Session ownership. Deferred: arbitrary custom types, full compaction public
roles, extension-task runtime, live/saved settings and independent
enableSkillCommands, remaining RPC commands, durable queues, new quotas/sandbox.

## Execution receipts

Receipt root: `C:\Temp\ara-rpc-owned-skill`. The isolated final snapshot is
`C:\Users\loveu\.codex\worktrees\rpc-owned-skill-verification\ara-github`.
`snapshot.json` pins ten scoped changed paths plus unchanged dependencies and
the original system prompt. Other-chat prompt/e2e/knowledge/ledger WIP is excluded.

Initial focused checks on the implementation:

- `cargo test -p ara-agent -p ara-session --all-features`: **137/0/0**, 14
  suites, **11.852 s**. Seven new owned-input integration tests plus one awaited
  default-sink unit cover equal model content/timestamp with different Arc
  provenance, four queue-mode combinations, native/host ordering, initial skip,
  peek/pop, cancellation rollback, deadline and model-budget commits.
- `cargo test -p ara-cli --bin ara rpc_host::tests --all-features`: **3/3**,
  **7.813 s**. Real Agent terminal gate, cancelled-before-entry exact lone-UTF16
  ID/no appended input, and real custom journal write failure before any model
  request or new tool effect.
- Initial strict Clippy found three new test-only diagnostics: a synchronous
  guard retained across await and two clone-to-slice expressions. Explicit
  lexical scope and borrowed slices fix them; repeated scoped Clippy passes.
- Initial RPC process suite: **22 passed / 4 failed**, **11.581 s**. The new
  oracles omitted existing date/CWD provider reminders and assumed identical
  Windows separator rendering. Corrections validate and strip exactly the
  existing reminder before comparing all remaining content/images, and compare
  canonical file paths. No production behavior was changed to satisfy these
  failures. Failed raw log and corrected result remain retained.

## Final isolated Windows snapshot

`snapshot.json` pins fifteen source/dependency hashes; `commit.json` matches all
ten changed Git blobs to the tested source, allowing only CRLF-to-LF Git
normalization. The main worktree's unrelated dirty set was preserved. No
provider, Skill builder/decoder, Cargo dependency, or print/REPL source changed.

| Gate | Executed result |
| --- | --- |
| `python scripts/verify_backend.py` | Formatting, strict all-feature/all-target Clippy, target tests and doc tests PASS; **1,148 passed / 0 failed / 1 ignored**, **87 suites**, **75.849 s** |
| Agent loop suite | **50/50**, **1.04 s** execution |
| CLI E2E suite | **92/92**, **9.83 s** execution |
| RPC process suite | **26/26**, **2.34 s** execution; separate focused run **15.435 s** including compile |
| Session Skill suite | **9/9** |
| `cargo deny check` | Advisories, bans, licenses and sources PASS; **2.859 s**, existing warnings retained |

Logs/results: `backend-final.*`, `rpc-process-final.*`, `cargo-deny.*`. The first
whole gate passed the same **1,148/0/1** in **108.691 s**; it is retained as
`backend-first.*` with `snapshot-first.json`.

The first pushed Linux run [36691270178](https://github.com/ZZray/ara/actions/runs/36691270178)
on `677489a` stopped before tests: CI Rust 1.98 introduced
`chunks_exact_to_as_chunks`, absent from local Rust 1.97. `97567f0` changes only
the test to fixed-size array pairs and the equivalent dereferenced assertion.
The final whole Windows gate above was repeated on that test correction.
Production source and EXE bytes remain unchanged, so the actual-task receipt
below remains valid. This is a tested fixture correction, not a production
behavior relaxation. The final exact-commit Linux result is recorded below.

## Bounded actual Skill task

Driver: `C:\Temp\ara-rpc-owned-skill\real_rpc_skill_trial.py`, SHA-256
`d0324f47c3fc90b5880bf4dbc51f1529465c90ea1824b897975323c0d1e00f7b`.
It accepts `--repo`, `--exe` and `--expected-sha256`, and preserves the prior
bounded adoption driver's process/token/call limits. Live catalogue validation
at **08:39:34 UTC** confirms the selected exact `deepseek-v4.1-flash` supports
OpenAI Chat Completions. Authorization stays in the environment; cost is unknown.

Limits: **3 Runs**, **3 model calls/60 s per Run**, **9 total logical calls**,
**1,024 output tokens/call**, **180 s process guard**, **4 MiB per stream**.
Failure retains effects/receipts for inspection and never automatically retries.

Final task: **PASS, 16.339 s, 3 Runs, 7 calls (3+2+2), 4 successful tool
receipts, 238 live message updates, exit 0**. Reported usage sums to **19,232
tokens**; this does not establish monetary cost.

1. A known project Agents Skill invokes read/write to produce a randomized
   nonce/sum/sorted-array artifact. Its malformed images are ignored. ACK
   precedes AgentStart; custom start/end/Run terminal/get_messages/native receipt
   retain the same exact content/details/timestamp. Exactly one custom journal
   entry and zero ordinary duplicate Skill entries exist.
2. After deleting SKILL.md, a new B Session writes an independent marker. A's
   journal bytes remain unchanged; B has no custom Skill receipt or A nonce.
3. Delete TASK_INPUT.json, switch back to A, and restore the identical historical
   public Skill without source I/O. A write-only recall task reproduces the
   exact randomized artifact. B's journal bytes remain unchanged.

Receipts: `C:\Temp\ara-rpc-owned-skill\real-skill-3qa0v6wz\summary.json`,
raw stdout/input/observations, A/B native journals, original artifacts and public
message sequences in the same directory. Parent audit independently recalculates
**26 artifact hashes**, **9 unchanged production source hashes**, **322 raw
frames**, **23 inputs**, three Run-local terminal sequences, native tool-call
before-successful-result order, original Session ownership and EOF shutdown.
The independent Codex reviewer also inspected those original files. Source
`details` objects and `originalText` metadata do not enter provider messages in
the controlled process oracle; the prepared Skill model body includes its
directory and arguments as intended. Actual
native thinking signatures stay private in public frames.

EXE SHA-256:
`588370c22e258bc86f3f2735ce2e6d82ef542b613f2297134b593a7ad71dad30`.
The final whole gate's EXE retains these bytes after the test-only correction.
`skillFixtureSha256` identifies logical LF source; `artifactHashes` pins the
physical Windows CRLF fixture, independently checked as the same normalized
source. They deliberately identify different byte representations.

## Independent audit and remaining boundary

Independent Codex `rpc_skill_independent_review` read every hunk in all ten
changed paths, the unchanged Skill adapter/decoder and reminder dependencies,
the fixed upstream source, original gate logs and actual-task artifacts.
Its raw audit agrees with all fifteen snapshot hashes, all twenty-six artifact
hashes and all ten committed blobs. A has nine ordinary messages, one custom
Skill and three tool receipts; B has four messages, zero custom Skills and one
receipt. Terminal/public/native projections and signature redaction were
verified from raw records rather than summary claims. No actionable production
defect was found; the two-line test-only Linux correction was also reviewed.

### Exact final Linux gate and point decision

[Repository checks 36692049206](https://github.com/ZZray/ara/actions/runs/36692049206)
checked out exact `97567f0d65ec18fbd843aa0e7ddba32e3e762f7a` and completed
successfully. Backend **08:48:51-08:52:50 UTC, 239 s**: formatting, strict
all-feature/all-target Clippy, target tests and doc tests PASS;
**1,151 passed / 0 failed / 1 ignored**, **87 suites**. Named raw suites:
Agent loop **50/50**, CLI E2E **95/95**, RPC **26/26**, Session Skill **9/9**.
The original Linux lint failure is closed on the actual delivered test.

Raw log: `github/logs-36692049206/0_verify.txt`, SHA-256
`1d8a4c5597b6eee5b357e40f7415a10a52c1fa18eed9babeb85cc998282e465f`;
jobs receipt and `linux-audit.json` independently recompute these results.
The two auxiliary Skill workflows succeeded on `677489a`, whose production
code is identical: `36691270059` fixed OMP invocation, `36691270110` directory
I/O faults. They are not represented as new runs on `97567f0`.

Independent Codex `rpc_skill_independent_review` read the final checkout SHA,
all raw suite results and repaired provenance tests. Combining the exact Linux
gate with its prior code/Windows/dependency/actual-task audit, it returned
**ACCEPTED for this bounded implementation point, zero confirmed defects**.
The parent rechecked source/commit mappings and original receipts and agrees.

The observed behavior does not claim arbitrary custom roles, full compaction
public rendering, settings/catalogue reconciliation, durable enqueue, or full
CTX-01e/CA-RPC acceptance. No parent gate or complete OMP marker advances.
