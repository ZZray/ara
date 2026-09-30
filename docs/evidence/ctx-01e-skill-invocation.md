# CTX-01e: user Skill invocation

## Requirement and current boundary

The upstream target stays `596f2da7101178214aa27a753529d15e6b7ad91d`
(OMP v18.1.8). CTX-01e remains **implementing, not accepted**. This first
slice adds explicit user invocation to the existing line REPL and preserves
the Skill as a real Session `custom_message`. It does not accept CA-RPC,
the full interactive host, P1/P3, or the full OMP marker.

Baseline: `d557334fab3c5e0daa710ca157711fef77148f3b`, clean `dev`.
[Repository checks 36649136190](https://github.com/ZZray/ara/actions/runs/36649136190)
completed successfully on that baseline. The exported source manifest is
`C:/Temp/ara-ctx-01e-source/manifest.json`; source reads use fixed Git blobs,
including files omitted by the reference clone's sparse checkout.

### Preserve / change / defer

- **Preserve:** print input stays literal; discovered Skill precedence and
  filtering; ordinary REPL input; `/new`, cancellation, restart and budgets;
  tool execution and uncertain-effect recovery; provider message types;
  the existing single-level soft-compaction policy.
- **Change:** source-backed parser and fresh-file prompt builder; known Skill
  dispatch in the REPL; one user-attributed `skill-prompt` custom entry in
  place of the projected ordinary user entry; recognized Skill projection
  and actual source IDs in Session context and soft compaction.
- **Defer:** RPC acknowledgement/queue contracts (CA-RPC/A5); live steering
  and follow-up queues; image input in the line REPL; autoload and other
  custom-message families; title/magic-keyword machinery; interactive mode
  commands that the REPL does not implement; plugin/managed Skill discovery
  and containment (CTX-01c). These remain open source surfaces.

## Source and owner mapping

| Observable behavior | Fixed OMP source | Rust owner / evidence to execute |
| --- | --- | --- |
| Leading and embedded `/skill:` parsing, surrounding prose, other command/local-execution exclusions | `extensibility/skills.ts:409-487`; the 14 deferred invocation tests | `ara-discovery`; parser fixtures and fixed-source Bun comparison |
| Fresh file read, exact LF-only frontmatter removal, JS trim, optional arguments, absolute Skill directory, body line count | `extensibility/skills.ts:492-531`, `prompts/skills/user-invocation.md` | Prompt builder using the existing `LoadedSkill` and `ara-prompt`; LF/CRLF, whitespace, file-change and read-error fixtures |
| Only a registered Skill is invoked; hidden Skills remain explicitly callable | `modes/skill-command.ts`; `interactive-mode.ts:1466-1477` | REPL lookup in the already discovered Skill list, with process tests |
| Custom payload is `skill-prompt`, `display:true`, `attribution:user`, details name/path/args/lineCount | `modes/skill-command.ts`; `session/session-manager.ts:2476-2501` | Specialized Session Skill payload and append; actual JSONL and no duplicate user entry |
| Direct user Skill projects as a user turn, including text/image content; metadata is not provider content | `session/messages.ts:1096-1103,1258-1272` | Recognized Session projection; provider request and restart checks. The line host only admits text |
| Original invocation and raw expanded body remain retrievable after compaction | ARA sourced-reference requirement; fixed custom-entry persistence | An explicit original-input detail plus raw custom entry; strict source snapshot, kept boundary, source-ID validation and reopen tests |
| Builder failure consumes a known invocation and reports load failure without running a plain prompt | `modes/skill-command.ts:invokeSkillCommandFromText` | Deleted/unreadable file through the actual REPL; next prompt still works |
| Print mode does not dispatch Skill commands | Upstream print host boundary | Actual `ara -p` request retains literal `/skill:` input |
| RPC settings, acknowledgement, steer/followUp, queueOnly and asynchronous error contracts | `modes/rpc/rpc-mode.ts:120-192`, `modes/skill-command.ts`, `agent-session.ts:6130+` | Open under the later RPC/queue slice |

## Implementation and planning checkpoint

Use the existing discovered Skill list; do not introduce a second loader.
The builder rereads `SKILL.md` on each invocation. Discovery's normalized
frontmatter/body is not the invocation body: fixed OMP only removes its
specific LF frontmatter expression and retains other source text.

The Session owns a narrow user Skill custom-message payload and its projection
to the existing `ara-ai::Message::User`. Unsupported custom families retain
their existing open boundary. A per-turn host sink adapter replaces only the
initial projected user's journal append with the custom append. It preserves
the initial events and Run report; preappend followed by an empty prompt vector
would lose those semantics. Every subsequent event uses the existing HostSink.

The recognized custom entry carries its actual entry ID into strict source
snapshots, summary source lists, kept boundaries and restart projection. The
raw JSONL is not rewritten into ordinary user messages. The original submitted
line is an ARA provenance addition in Skill details, distinct from parsed args
and from the expanded prompt. It is not injected into the provider request.

At the initial planning checkpoint, independent Codex pre-review was in progress
and no production change or executable delivery was claimed. The sections below
record the implemented slice and supersede only that checkpoint's current status.

## Required verification

1. Execute the unchanged fixed-source parser and builder under pinned Bun;
   compare Rust outputs on the same inputs, including BOM/invalid UTF-8 and
   CRLF behavior rather than assuming a decoder contract.
2. Execute actual REPL requests with a controlled upstream: known and embedded
   Skill, hidden Skill, unknown/disabled fallthrough, fresh file changes,
   load failure, ordinary next turn, and print-mode literal input.
3. Inspect raw custom entries, projected provider content, event receipts,
   restart, compaction source IDs and retained original/expanded text.
   Verify unknown custom families keep their strict rejection boundary.
4. Exercise a real tool task and relevant cancellation/provider failure without
   duplicate or replayed input/tool receipts.
5. Run the applicable full backend gate, dependency policy and documentation
   checks on the recorded delivered snapshot. Preserve failures.
6. Verify a current real-model route and run a bounded Skill task, inspect its
   actual artifact and Session receipts, then obtain independent diff review
   and point audit. Record a bounded decision without closing RPC or full parity.

## Implemented REPL and Session slice

The code snapshot is the worktree based on `d557334`; it is WIP pending Linux
execution and the final independent point audit. Production and dependency/test
hashes are retained in
`C:/Temp/ara-ctx-invocation-validation/source-snapshot.json` (17 files).

| Changed files | Requirement mapping |
| --- | --- |
| `ara-discovery/src/skill_invocation.rs`, module export and parser tests | Fixed leading/embedded parser; first ASCII-space split and ECMAScript whitespace/exclusion rules |
| `ara-discovery/src/fs.rs` | Fresh invocation read with existing nonblocking open and opened-handle regular-file check; cached discovery behavior retained |
| `ara-context/src/skill_invocation.rs`, export and builder tests | Fresh decoded body, exact frontmatter/trim behavior, unchanged template render and Skill details |
| `ara-context/prompts/skills/user-invocation.md`, prompt README, `.gitattributes` | Unchanged fixed-source MIT template, attribution and LF bytes |
| `ara-session/src/skill_prompt.rs`, `lib.rs`, Skill tests | One user-attributed custom entry; projection, real source IDs, compaction/restart and raw recovery boundary |
| `ara-cli/src/skill_command.rs`, `main.rs`, e2e tests | Existing discovered list, hidden invocation, JS trim before display sanitization, bounded preparation, custom sink and actual host evidence |
| CLI/context manifests and `Cargo.lock` | Existing `ara-prompt` production dependency and already locked base64 test decoder; no new lockfile package |
| Oracle Python/JS scripts and Linux workflow | Execute unchanged fixed parser/builder and compare Rust; retain actual Linux FIFO process receipts |
| This evidence, feature ledger, plan and handoff | Scope, commands, failures, review and truthful disposition |

`UserSkillPrompt` accepts text or typed User content, with details and initiating
timestamp. The line REPL admits text only. Raw custom entries retain their own
IDs, original submitted line (including delimiter), expanded body and details;
only content projects to a provider User message. The per-turn sink replaces the
initial ordinary User append, preserving Run/event behavior without duplication.
Public events begin with the existing printable projection so provider payloads
and signatures remain redacted.

Known load failure consumes the invocation; it does not send ordinary input to
the model. Fresh loading runs in the existing cancellation/deadline boundary.
Unknown names, disabled Skills and print mode retain their ordinary input path.
Unsupported custom families retain the strict source-projection boundary.

## Independent review and resolved findings

Independent Codex agents were used; Claude was not invoked.

- `ctx_point_audit` pre-reviewed the Session plan, then implemented the assigned
  Session files. Its implementation is not its own independent postreview.
- `oracle_diff_review` reviewed source contracts and parser/builder/FsCache,
  and implemented the fixed-source oracle. The integrated review below covers
  its scripts as well as the production changes.
- `resource_scan_plan` independently reviewed all 23 original changed paths
  against `d557334`, including tests, oracle, workflow and evidence draft.
  Final verdict on the JS-trim snapshot: **no unresolved actionable findings**.
  It independently matched all 17 source hashes, oracle blobs/cases and the
  final Windows gate. Linux/runtime evidence remains a point-audit requirement.

The review found and the final snapshot corrected these concrete issues:

1. A late `SKILL.md` replacement with a Unix FIFO could block a worker and
   runtime shutdown. Fresh loading now reuses nonblocking open and regular-file
   handle validation. The Unix process fixture bounds exit before collecting
   output; its execution is pending below.
2. Malformed recognized Skill custom entries could evade omission reporting or
   let tool recovery cross their raw user boundary. Session tests now exercise
   both omissions and the unreplayable boundary; unknown families stay unsupported.
3. Display sanitization could change invocation parsing. Fixed input-controller
   `text.trim()` is now mirrored with `ara_prompt::js::trim(&raw)` before parsing;
   e2e inputs include VT, tab, BOM and an internal tab in a name.
4. The first oracle's fresh-read case did not actually call the fixed builder
   twice. The final oracle retains both `priorResult` and the overwritten result;
   the Rust differential comparator reconstructs and compares both reads.

## Windows execution on final source

Final command (oracle environment present):

```powershell
$env:ARA_CTX_INVOCATION_ORACLE = 'C:\Temp\ara-ctx-invocation-oracle\run-20260930T004344Z-c4c13cbb\oracle.json'
python scripts/verify_backend.py
cargo deny check
python scripts/verify_bootstrap.py
python scripts/omp_inventory.py check
git diff --check
```

The full gate passed: **1,070 passed, 0 failed, 1 ignored, 77 suites**;
CLI e2e **87/87**. Formatting, workspace all-target/all-feature Clippy with
warnings denied, target tests and documentation tests passed. Oracle comparison
actually ran, rather than reporting `NOT RUN`. Log:
`C:/Temp/ara-ctx-invocation-validation/backend-windows-js-trim.log`, SHA-256
`4f18d4b0be7abfff26386023af458a0cbd889721ecd28cfb390380763eb50e37`.

Dependency policy passed advisories/bans/licenses/sources. Existing warnings
remain: tree-sitter-graphql has no license field, and duplicate versions of
base64/cfg-if/encoding_rs/getrandom/syn/windows-sys. Log:
`C:/Temp/ara-ctx-invocation-validation/cargo-deny.log`.
Bootstrap, inventory and diff whitespace checks passed.

Controlled process coverage includes hidden/embedded known Skills, real write
effects, custom events/journal and no duplicate User, restart after source
removal, unknown/disabled/print ordinary input, fresh-file changes, deleted-file
failure followed by an ordinary prompt, provider failure, actual tool
cancellation with no replay, and custom source IDs on both sides of compaction
with restart. Session coverage includes nine specialized Skill tests and the
text-only compaction rejection of image-bearing history.

Preserved failures/checkpoints:

- The initial Windows e2e expected mixed path separators; the fixture was
  corrected to construct the platform path. No production path contract changed.
- The first full gate failed a test-only Clippy cloned-reference-to-slice warning;
  corrected and rerun. Log: `backend-first-clippy-failure.log`.
- `cargo test -p ara-cli --lib` was an invalid command (binary-only package);
  correct `--bin ara` tests subsequently passed.
- `backend-windows-final.log` and `backend-windows-delivered.log` are intermediate
  gates; only `backend-windows-js-trim.log` represents the final source.

## Fixed-source execution and differential oracle

`scripts/ctx_skill_invocation_oracle.py` exports exact fixed Git blobs, then
executes `scripts/ctx_skill_invocation_oracle.mjs` with Bun **1.4.0**. Parser,
builder, Bun file read and upstream prompt/template renderer execute unchanged;
dead dependency mocks trap if called. Exported MIT LICENSE and blob hashes are
in the source manifest. Rust compares the same inputs and complete results.

Final output:
`C:/Temp/ara-ctx-invocation-oracle/run-20260930T004344Z-c4c13cbb/oracle.json`.
**25 parser + 10 builder cases**, **0 mock calls**, SHA-256
`4f659479dfcdb9d583e1fcb7ebd130a8ac1f51f47c862ac9033d8fe75cbdc682`.
Builder evidence includes removed BOM, replacement decoding for invalid UTF-8,
retained CRLF frontmatter, exact rendered bytes, read failure and actual same-path
fresh reread. The initial `run-20260930T003037Z-b60d33ed` output remains historical;
it lacked the two-read oracle assertion.

## Final-binary bounded real task

Scratch procedure:
`C:/Temp/ara-ctx-invocation-validation/real_skill_invocation_trial.py`.
Final receipts:
`C:/Temp/ara-ctx-invocation-validation/real-skill-1wuowlkd/summary.json`.

The live B.AI catalogue confirmed `deepseek-v4.1-flash` on the
OpenAI-compatible Chat route before use. `BAI_API_KEY` came from the environment.
Two real REPL processes used the same native Session. Each turn was bounded to
four model calls, 90 seconds and 1,024 output tokens per call; each process had
a 110-second timeout (total allowance eight calls). Actual result:
**6 calls, 4 successful tool receipts, 17.859 seconds**; both final stops succeeded.
Cost remains **unknown**; six per-call usage records are preserved in the summary.
Reported totals: input 4,732, output 971, cache-read 13,312 and total 19,015 tokens.

- Binary SHA-256: `e07a368a14d2553c1bf63d1701ec77022bd622c14b4a488361b088aa934c7b33`.
- Session: `01a0efd1-3fd3-7635-95ee-823d650bf00d`; Skill custom entry: `30e10d32`.
- Turn one explicitly called a hidden Skill, read its reference and wrote exactly:

```json
{"owner":"ara-ctx-01e-real","from_skill":"live-body-v2","payload":{"marker":"ctx-01e-original-reference","revision":2},"user_token":"invocation-token-01e"}
```

- The Skill source was deleted. Turn two used `--resume` on the same Session,
  read the artifact and wrote `RECALL.txt` with exact bytes
  `ctx-01e-original-reference\ninvocation-token-01e` (no final newline).
  This is artifact-based continuation, not unaided model-memory recall. The
  controlled restart test separately inspects custom content in the next
  provider request.
- Raw custom content stayed unchanged after source removal; initiating custom
  event/stored timestamps were equal; no duplicate ordinary User was appended.
- Summary records 14 artifact hashes: catalogue, commands/input, events/stderr,
  first/final Session, preserved invoked Skill, reference and output files.
  Retained files were checked for credential absence.

Earlier successful `real-skill-6iq3pua6` is on the pre-final-normalization binary;
it is retained as history and is not the final delivery receipt.

`resource_scan_plan` independently rehashed all 14 task artifacts and all 17
current source files, matched the binary, inspected durable assistant/tool
ordering, exact tool-call/file content, custom ID/timestamp and the unchanged
first-journal byte prefix after resume. It found zero ordinary Users in the
first journal and only the resumed prompt in the final one, with one nonerror
receipt per tool call. Both stages have three model calls and a final `stop`.
**Real-task audit: PASS**, with the artifact-based recall limit stated above.

## Linux and point-audit boundary

`.github/workflows/ctx-skill-invocation.yml` is authored but has not run at this
checkpoint. It pins Ubuntu 24.04, Rust 1.97.1, fixed OMP objects and Bun 1.4.0,
executes the unchanged oracle, compares Rust, runs Session/CLI/process fixtures
including Unix FIFO replacement, and uploads hidden fixture files and actual
request/event/journal receipts. The existing repository workflow also runs the
full Linux backend gate.

**Disposition: implementing (WIP), not accepted yet.** Required remaining work:
push the scoped WIP, inspect exact-SHA CI and uploaded Linux receipts, and obtain
independent point audit including the final model task. Even a bounded slice
acceptance will not close RPC/queue semantics, CTX-01e as a whole, P1/P3 or the
complete OMP parity marker.
