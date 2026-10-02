# Native handoff — bounded WIP

Base `bb25c1f65d684e76459b4ec1a363fd123b8c2991`; fixed OMP
`596f2da7101178214aa27a753529d15e6b7ad91d`. This checkpoint adds the
text-document handoff request, checked same-Session publication, line REPL
manual entry and automatic RPC threshold/incomplete selection. Full handoff,
compaction, Core, Host and P1–P6 remain open.

## Source and observable requirement

| Fixed OMP source under `packages/` | Rust owner |
| --- | --- |
| `agent/src/compaction/compaction.ts:1045–1122`, `prompts/handoff-document.md` | `ara-agent::handoff`: original prompt, full live Context, tool choice none, one exact status-400 auto-only rejection retry, text-only terminal extraction and typed cancellation/invocation receipts |
| `coding-agent/src/session/session-handoff.ts` | `ara-cli::handoff`, `model_route`: live system/tools/history and transform hook, fresh side protocol state, unchanged private auth resolver, original Codex cache key with unique side transport identity, manual empty error and auto empty fallback |
| `agent/src/compaction/utils.ts`, `utils/path-tree.ts`, `compaction.ts::extractFileOperations` | `handoff_file_ops`: original cut-prefix file operations and previous native details; generated summary text is not file metadata |
| `coding-agent/src/session/session-maintenance.ts`, `session/messages.ts` | `ara-session::handoff`: exact raw/projection guard, source IDs, retained cut, native details and wrapper, phase-aware atomic publication and same-Session reopen |
| automatic method order and `auto-handoff-threshold-focus.md` | RPC threshold/incomplete use the fixed focus; overflow skips handoff; empty/ordinary failure can try the next method, cancellation cannot; committed handoff can only use local rescue |

The copied prompt retains the MIT attribution through module source notices
and `THIRD_PARTY_NOTICES.md`; its Git blob is unchanged from the fixed source.
The full parity marker remains null.

## Implementation and limits

The side request does not run returned tool calls, even if its auto-only retry
produces ToolUse. Native text extraction also permits Length; an empty manual
document is rejected. Successful publication adds one native `method=handoff`
compaction entry to the original Session. The persisted document/file list
does not include its model wrapper. Reopen projects the fixed lower-trust
wrapper and the kept raw tail. Repeated handoff and later soft summaries retain
checked cumulative raw source provenance.

The Host checks cancellation after the final journal lock and before commit.
The RPC reader cancels only the handoff child on valid Abort/abort_and_prompt
and EOF, with a shared slot/pending-stop count covering enqueue-before-begin.
The guard lasts through commit and error settlement; ordinary accepted Runs
keep their original EOF contract. A cancelled threshold pass cannot launch the
pending primary call. Incomplete unpublished recovery restores its owned failed
assistant receipt. Unknown postpublication durability retains the candidate and
requires Host fail-stop.

Still required: native thinking-effort default/high/off/model clamping,
concurrent manual RPC, new/switch/branch reader-time handoff interruption,
speculation/deferred maintenance, other native transform/obfuscation/payload
hooks and full multimodal/legacy/raw-role contracts. This batch does not claim
a measured provider cache hit, account real-task acceptance, Linux acceptance,
or complete upstream tests. The exact handoff postpublication sync-error
injection is NOT RUN; it uses the existing phase-aware writer, whose generic
publication fault tests remain part of the backend gate.

## Reproduction and evidence

Root receipts are under `C:\Temp\ara-handoff-batch`.

```text
python -X utf8 scripts/verify_handoff.py --module all --full --output C:/Temp/ara-handoff-batch
python -X utf8 scripts/verify_handoff_host.py --binary E:/repos/ara-github/target/debug/ara.exe --output C:/Temp/ara-handoff-batch/host
```

`--module core|session|projection|route|host|repl` selects a focused family.
`--full` shares the workspace fmt/Clippy/all-target/doc, inventory, dependency
policy and binary build. Ordinary template/path details receive source review;
module families exercise distinct observable contracts. Real-model credentials
and the bounded live runner stay outside Git.

Early compile/lint/fixture failures are retained as diagnostics: a new test
used a nonexistent Message method, sibling maintenance visibility was private,
a full RequestOptions literal missed the new optional field, and nested saving
needed Clippy formatting. Source-confirmed fixture corrections preserve ASCII
focus and the wrapper's formatted no-trailing-newline output. The old soft-only
family now selects soft explicitly without weakening its assertions. Controlled
seed includes native startup model metadata; the Assistant split fallback uses
both native soft summary requests. These are not accepted execution receipts.

## Executed stable gate

`module-20261002T092835Z/receipt.json`: exit 0, sourceUnchanged, 461.085s
total. The six grouped modules pass 147/0/0. Backend passes 1,565/0/20,
including the original Host bridge suites; fmt/Clippy/all-target/doc, inventory,
dependency policy and build all pass. Initial focused compile takes 39.178s;
backend 398.868s includes 4m39s all-target recompilation. Final build is 0.452s.
The broad compilation is caused by the new shared prompt dependency; it is not
1,565 newly written micro-tests. Later batches should group shared dependency
and interface changes before the single stable gate.

`host/run-20261002T093246Z/receipt.json`: exit 0, sourceUnchanged, four grouped
actual Host flows pass in 27.311s during concurrent all-target compilation.
Manual handoff/reopen keep live system/tools/history and native file details;
returned side ToolCall has no effect. Empty manual causes no handoff write,
empty auto falls back to the native two-call split soft summary, and incomplete
handoff excludes failed output from its request and resumes from the wrapper.
Threshold/incomplete × Abort/EOF all settle in 2/5/3/7ms, publish no compaction,
make no fallback/primary request and restore unpublished failed output.

Delivered binary SHA256:
`5af6f93dd32f6c95365d1db60083d234f88e2484804eeebc7f8288f052e3e03c`.
The frozen scope is `source-manifest.json`: 24 repository source/test/runner
paths, including Cargo.lock/package manifest and the copied prompt. Independent
POST checks all of them against the gate before/after/current hashes. The
temporary real-task runner is an additional reviewed path.

## Retained first live attempt

`live/20261002T094018.550649Z/receipt.json` remains FAIL, one actual CAS request,
11.401s total. Manager gateway is unavailable; its configured CAS catalogue
confirms `deepseek-v4.1-flash` and the actual protocol is OpenAI Completions.
The original runner observes its relay still active immediately after client
exit and stops. Independent inspection then proves complete stop/usage/[DONE],
HTTP 200, no active request or relay failure and one committed same-Session
handoff. Actual upstream usage is 18,364 prompt + 822 completion = 19,186 tokens;
unknown buckets remain unknown. No tool ran and the workspace contains only
the original empty `.git` directory.

That document quotes the discarded marker once in a "do not copy" warning,
although none of the repeated filler payload survives. It fails the original
strict marker criterion and is not relabeled PASS. No write/task continuation
is replayed from this Session. The original runner/receipts/journal/SSE are
retained. The fresh trial uses explicit no-marker-even-in-warnings focus and
a new label/Session, with seven requests maximum so both attempts total at most
eight actual API requests. Its runner fixes bounded relay settlement and
permits only the known native leading title slot; every original raw field and
order remain checked. This is a manual Root decision after known outcome
inspection, not an automatic failed-task replay.

## Final real task and audit

`live/20261002T094530.121798Z/receipt.json`: PASS, 27.480s, three processes,
five actual CAS calls (handoff one; invoice write/read/final three; tool-disabled
original-Session reopen one). The local Manager gateway remains unavailable;
its private configured CAS route/catalogue confirms `deepseek-v4.1-flash` on
OpenAI Completions. The first handoff receives the controlled imported receipt;
subsequent wire contains the persisted handoff and never the discarded marker.
No original source file exists. The next user task supplies no invoice facts.

The actual tool chain is exactly one `write totals.json` then one `read
totals.json`, with successful receipts. The verified JSON is
`{"notebook":67.5,"cable":54.0,"grand_total":121.5,"shipment_label":"Maple-9307"}`.
Its SHA256 is `c13d62273cbed05ccdd4e97ed1ead3e66f92ab65c0c0d9e2aa0e19272e765f3e`.
Tool-disabled reopen correctly recalls all four values, advertises/executes no
tools and leaves the JSON unchanged. Every original raw field and order remains
exact, including native source IDs and the kept tail; the writer adds its
normal native title header slot. The accepted attempt reports 49,173 actual
upstream total tokens across five calls; the earlier failed document reports
19,186 separately. Unknown buckets are not converted into zero. Total actual
model calls across both attempts are six; each uses its own two-probe maximum.

Independent Codex `/root/ctx_oracle_diff_review` reviews the fixed source,
24 repository paths and the external runner, resolves H1–H4 and inspects actual
gate/Host/task artifacts. Root `root-audit.json` directly binds those repository
hashes to the gate/review/current bytes, binary, complete raw Session, wire,
two actual tool receipts, JSON and earlier failed attempt. This audit accepts
only the stated document path as tested/audited WIP; the required limits above
remain open. PRE, earlier diagnostic receipts, final POST and Root audit stay
in the receipt directory. Source changes require affected checks again; changes
to evidence/documentation alone do not rerun the completed real task.

Final independent POST verdict is `PASS_WITH_EXPLICIT_COVERAGE_LIMITS`;
`post-review.json` SHA256:
`1d9c69705a16f9b874c7de7a00b878772b01e728ded795b956e6843534c00326`.
The Root audit was refreshed against that final POST without executing the task
again; `root-audit.json` SHA256:
`6c56560a9abb75b492a39626e7bbc0cb1524579ee07a4ca671727212f061bd81`.

## Timing and next work

Implementation starts about 08:55 UTC; successful fresh live validation ends
about 09:46 UTC, approximately 51 minutes before documentation/Git closure.
The stable gate takes 7m41s, mainly the 4m39s shared-dependency all-target rebuild.
The regular module executions are subsecond for Core/Session/route and about
18s for RPC/REPL. Independent review finds four necessary lifecycle/fallback
fixes; ordinary template/path details do not receive extra micro-tests.

Next: remote 120–240min, snapcompact 180–300min, frame rescue 30–60min net
engineering for those modules only, excluding shared gates/environment waits.
These estimates do not describe the remaining whole-project duration. P0/V1
accepted; P1–P6 open; RPC 27/42; full marker null remain unchanged. Provider
priority stays custom OpenAI, then OpenAI account, with other adapters recorded
as required later work. Full parity is not inferred from test/checkpoint counts.
