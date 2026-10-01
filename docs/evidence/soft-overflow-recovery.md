# Same-route soft overflow recovery checkpoint

Base `26e9dd3`; fixed OMP `596f2da7101178214aa27a753529d15e6b7ad91d`.
This is bounded WIP in CA-TURN-RECOVERY / CA-COMPACTION-HOST, not acceptance
of either complete surface. RPC remains 27/42, P1–P6 remain open, and the
full port marker remains null.

## Source and scope

The fixed `session-maintenance.ts:1840–1911,2642–2656`,
`turn-recovery.ts:998–1059,1102–1144`, `session-manager.ts:1184–1191,2654–2715`,
`session-context.ts:346–371`, and agent `compaction.ts:303–333` define the
same-route check, clean failed-turn branch, conditional rollback, reserve
provenance, payload headroom and retry-fit behavior. Four native input families
are reused: auto-compaction progress guard, payload-rejection-413, queue and
cancellation. Exact blob/hash receipts are under `C:\Temp\ara-overflow-batch`;
no moving upstream ref is used.

- Explicit validated `contextWindow` reaches the configured execution route;
  positive fractions stay numbers. Unknown/legacy/account capacities stay
  unknown. Complete bundled catalogue projection remains mandatory.
- An owned cancellable summary task retains the original Agent, Session,
  generation, native failed entry and checked compaction snapshot. Ordinary
  retry cannot take a second owner. Abort/EOF/new/switch settle the task;
  a committed summary permits a checked 100ms continuation.
- Failed/no-preparation recovery restores contentful errors with a new native
  ID. Empty errors restore the active/public tail without another raw append;
  final model projection excludes them. Compaction commit and Agent adoption
  failures retain persistence diagnostics and stop the connection.
- Known usage buckets are a lower bound. Unknown usage never becomes zero.
  An explicit payload-only rejection with low local occupancy and no usage
  overflow evidence does not bill a token summary, including raw HTTP 413
  with no usage. High occupancy/genuine token overflow retains recovery.
- Old independently admitted Bash keeps its original branch. New Bash admitted
  during owned maintenance waits for settlement; an appended receipt extends
  only the owned continuation expectation and the budget is checked again.

## Execution

Module entry: `python -X utf8 scripts/verify_overflow.py`; one-command final
gate adds `--full`. The existing runner compiles selected targets once and
then invokes them; the final gate runs owned formatting, workspace Clippy,
all-target/doc tests, fixed inventory, dependency policy and binary build.

Initial module found a test-only comparison of a generated synthetic summary
timestamp. The repaired grouped test still compares every raw entry, ID,
source projection and real message timestamp; only that generated timestamp
is normalized. It also found an actual Windows default-stack regression after
threshold compaction when `new_session` settled maintenance. There is no
reachable async recursion. Measured cleanup Future 8,656 bytes was isolated at
the new await boundary: `abort` layout 8,904 → 1,896 bytes and `compact`
8,960 → 1,952. The existing threshold/new/switch process test then passed.
Temporary layout diagnostics and its test-only stack setting were removed.
Final product/test processes use their default stack. Clippy also required
boxing the new recovered-message enum variant; no lint exemption was added.

Final command `python -X utf8 scripts/verify_overflow.py --full --output
C:/Temp/ara-overflow-batch` passes on unchanged source:

| Module / gate | Result | Seconds |
| --- | --- | ---: |
| Configuration / Session / actual RPC Host | 6 / 4 / 8 passed, 0 failed | 2.072 execution |
| Windows backend, 121 target/doc suites | 1,508 passed, 0 failed, 20 ignored | 272.771 |
| Inventory / cargo-deny / final binary | PASS | 3.504 |
| Whole command including selected compilation | PASS, source unchanged | 287.203 |

The workspace all-target recompile took 149 seconds. No completed full test
run was repeated after this stable batch. Previous failed preflights are kept
in the artifact root; an initial large-enum lint failure stopped before any
workspace test execution.

Final preferred Manager → OMP management → CAS `deepseek-v4.1-flash`,
OpenAI Chat Completions trial passes in 31.523 seconds: one controlled
contentful overflow and ten actual CAS requests. The original Session
`01a0f725-6589-74e3-bcd9-331070cd5fa9` survives three processes. Real tools
read/write/read the invoice totals (75), then a real CAS summary and
continuation write/read the adjusted total (90). While the summary HTTP
request is held, an actual new RPC Bash runs and settles; its one receipt is
parented to the new compaction and is present in the continuation request.
Tool-disabled restart recalls original invoice fields and adjusted total.
Summary calls advertise no tools, the original raw failure is off branch,
and no ordinary retry is emitted. Known input/output/cache-read/total sums
are 9,846/1,614/23,424/34,884 with one unknown error row; cache-write has ten
unknown rows, so its known sum of zero is not a known aggregate.

Artifacts:

- Gate: `C:\Temp\ara-overflow-batch\module-20261001T110050Z\receipt.json`.
- Final live: `C:\Temp\ara-overflow-batch\live\20261001T110658Z\receipt.json`.
- Binary SHA256: `e6695527ca52d1f2671028f7d0142905fd9dde236ad6e8c440b1c5d03f9a4a6c`.
- Independent Codex `/root/ctx_oracle_diff_review` POST:
  `PASS_WITH_EXPLICIT_GAPS`; source hashes cover all 31 changed source files
  and match the gate/binary. Receipt SHA256:
  `1df6ee63a777ab82dc1e23ed385bdc15aef1ad0329677d1a7d15f07010524eb7`.
- Root directly checked final source/binary hashes, branch/source IDs, Bash,
  file artifacts, usage and three-process identity in `root-point-audit.json`.

## Explicit remaining work

Context promotion, remote/snapcompact/handoff/shake, complete method ordering,
rescue reducers, incomplete/length and other terminal-stop recovery, fallback,
and full catalogue/Host parity remain mandatory. Responses `message.done`
currently shares a same-route veto with function/unknown native output;
ordinary contentful Responses overflow needs finer sourced evidence before
that veto can be relaxed. This checkpoint does not prove that surface.
Full native summary output budget and input folding also remain mandatory;
this checkpoint reuses the previously accepted checked soft summarizer and
its 13,107-token ceiling. Exact reserve provenance here proves retry fit,
not the complete native summarizer.

The durable-discard API is exercised as a Session transaction, but Host
promotion/accepted-empty-stop callers remain mandatory. Ordinary overflow
rollback in the fixed native source only changes an in-memory leaf; it does
not call durable discard. ARA retains an original empty-error raw receipt for
audit whereas native message persistence skips it. Model exclusion proves
absence of replay, not raw deletion or identical native persistence.

Explicit queued inputs survive settlement; the native Agent's clean User tail
finishes before a follow-up is dequeued in the same Run. Queue handoff priority
does not claim that every follow-up enters the first recovered request.
The controlled overflow in a CAS trial is injected locally; actual summary,
task tools and recall run on CAS. It does not prove a natural CAS overflow,
OpenAI account login, other providers or a full product release.
