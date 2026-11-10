# Real-model project skill reuse observation

2026-10-20, compact HandWave Node **0.2.8**. The user identified the remaining
real-model autonomous acceptance gap. This follow-up changes only its Python
acceptance harness and records one fresh bounded two-Session observation.
The complete skill-plus-knowledge-body gate remains **FAIL / OPEN**. Narrower
fresh-Session execution of the persisted skill is now directly observed.

## Target and scope

- Base commit: `935e5493ca71260f7f60aaf46d61baf9777ef6f6`.
- Unchanged published Windows EXE: 5,355,520 bytes, SHA256
  `2abf8e84ee4107219e798dcaa420a28fe357fef33945a8992c3f866dbdb53890`.
- Existing isolated signed Hub SHA256:
  `70e895af913ad063317772bc7ceb3f31c11269511769aac610f42facab4389c1`.
- Implementation paths: `tests/verify_environment_live.py` and new
  `tests/test_environment_live_audit.py`. No Rust, Go, UI, runtime prompt,
  model configuration, production grant, installation or service change.
- Root ARA/OMP work is excluded. No Cargo build, commit, push or deployment
  was performed for this harness-only follow-up.

Evidence root: `D:/WorkSpace/tmp/handwave-autonomy-20261020/`; actual capture:
`live-project-two-sessions/`. Compact hashes, review boundaries and observation
status are retained in [the receipt](receipts/autonomy-20261020/observation.json).
Raw provider captures and temporary identity/state files remain outside Git.

## Observation is separate from acceptance

The historical 12-call first-Session trial executed its stored script through
Python subprocess orchestration. The limited direct-command recognizer missed
that syntax and skipped reuse. Its original automatic **FAIL / reuse NOT_RUN**
is unchanged; root and independent reviewer verified all 36 historical file
hashes before and after offline replay.

The new explicit `--defer-invocation-audit` may collect a fresh Session when
invocation syntax alone is unrecognized, retaining **REVIEW_REQUIRED**, exit 2.
It still requires a successful current-Run process candidate and, for creation,
the current bundle hash and native verification metadata linked to its exact
call ID, tool name and output hash. It cannot bypass artifact counts, native
knowledge writes, indexed-note hashes, human-source preservation, reuse reads,
unchanged bundle, schema/Session checks or budgets. Hard failures take precedence
and exit 1. Default strict mode still fails unknown syntax. This is not a general
shell/Python interpreter or automatic proof that a candidate exercised a bundle.

Root and independent Codex reviewer each executed all **11 offline unittest
methods** successfully. Python compilation and scoped `git diff --check` passed.
Historical replay remained strict FAIL / deferred REVIEW_REQUIRED. These are
offline evidence checks, not model behavior or new runtime acceptance.

Offline entry point:
`python -X utf8 -m unittest discover -s apps/handwave-node/tests -p test_environment_live_audit.py -v`.
The scoped documentation check resolves all 68 local links in four changed
pages. Whole-repository `python -X utf8 scripts/verify_bootstrap.py` did not
complete within its bounded 30-second attempt and was terminated; it is **not
a passing bootstrap check**. That traversal includes unrelated root work and
does not establish or invalidate this model observation.

## One actual-model sample

The existing read-only SSH Gateway catalog was checked immediately before the
trial: exact model `deepseek-v4.1-flash`, protocol `openai-completions`, advertised
context 1,000,000 and input text/image. Advertised image capability was not tested.
The test used only a new local signed Hub/Node identity, temporary project/state
and Node home variables. This is not an OS sandbox. No production device grant
was renewed or used.

Unchanged bounds: 20 calls / 180 seconds / 1,000,000 conservative request-UTF8 plus
output reservation per Session; aggregate 28 / 300 seconds / 1,500,000; maximum
output 4096 per call. There was exactly one sample, with no retry or budget raise.

```powershell
python -X utf8 apps/handwave-node/tests/verify_environment_live.py --binary D:/WorkSpace/tmp/handwave-deploy-028-20261019/handwave-node-0.2.8.exe --expected-sha256 2abf8e84ee4107219e798dcaa420a28fe357fef33945a8992c3f866dbdb53890 --hub-binary D:/WorkSpace/tmp/handwave-security-20261016/handwave-hub-final.exe --output D:/WorkSpace/tmp/handwave-autonomy-20261020/live-project-two-sessions --defer-invocation-audit
```

| Observation | Actual result |
| --- | --- |
| First Session | Automatic PASS; 17 model calls, 80.033s; correct report, native skill creation/verification and indexed knowledge write |
| Fresh second Session | Automatic FAIL; 5 calls, 12.100s; correct stored-script reuse, but no read of persisted indexed note body |
| Whole sample | FAIL / exit 1; 22 model calls, 97.532s; no Gateway/budget denials |
| First report | valid 6, invalid 2, INFO 3 / WARN 1 / ERROR 2 |
| Second report | valid 7, invalid 3, INFO 2 / WARN 2 / ERROR 3 |

Both tasks settled with successful native final responses and complete correct
statistics. The first created `jsonl-log-summary/scripts/analyze_jsonl.py`,
executed it, tested another input and four edge cases, attached authentic
execution receipts and wrote an observed indexed project note. Root inspected
the actual script: it parses its input and counts records, rather than writing
fixture totals. The bundle hash stayed
`198276ef9d8707e4bab90081c132310528170edd87a9f9a40a9d69f8e2ee1b9d`.

The second Session's initial request has no previous assistant/tool conversation
or first objective. It receives fresh library metadata/indexes, reads the
persisted skill and original rule, then executes the existing resource through
`call_00_epHyvjlVy1EDbM7pqSR51576`. Its authentic completed output hash is
`57d8e77e6d2407d96dbe79631d6ea8a5c48ca350b008dad82e7c034863705cba`.
No skill create/install, script rewrite or bundle change occurred. Both source
log hashes and the human rule/index remained intact.

However, the new Session's library carrier contains only the knowledge index,
manifest and skill metadata. Its `reference_read(ref-1)` returns the current
objective, not knowledge. No tool reads the persisted `notes/...md` body. Thus
this is actual skill reuse plus original-rule reading, not proof of autonomous
knowledge-body retrieval. The skill and original rule already contain the
instructions needed for this task; missing note retrieval alone does not prove
a broken knowledge-read tool or justify forcing irrelevant reads in production.
The established gate is preserved rather than weakened after the outcome.

## Failures, usage and review limits

Two first-Session tool failures are retained: `task_update(status=in_progress)`
was rejected by its declared enum; a combined PowerShell check failed because
`Get-FileHash` was unavailable. The model used Python hashing afterwards. Its
final claim that the latter was the only mistake is inaccurate. Generated skill
and test notes also call the first input eight lines including one blank, while
the actual file has nine lines (eight nonblank); the record's Limits still says
no automated suite despite the observed four-case suite. Generated notes are
therefore not promoted wholesale as verified project knowledge.

Provider-reported aggregate usage: prompt 266,786; completion 12,148; total
278,934; reported prompt-cache hits 219,136 and misses 47,650. Conservative
admission reservation was 1,169,436, a different measure. These are observed
provider fields, not verified billing or a causal token/cost saving. The two
tasks differ, so their durations do not establish an efficiency improvement.

Independent Codex `/root/autonomy_acceptance_review` performed PRE, harness POST
and actual-evidence POST. Root separately checked current source, raw native
commands/results, script/report/bundle hashes, Session isolation, unchanged
sources, absence of note reads and retained failures. Review reports remain in
the evidence root. The receipt records their exact hashes and accepted scope.

The complete skill-plus-indexed-body gate stays open. Next validation should
make a prior, indexed knowledge fact necessary to a new task and distinguish
retrieval from duplicated skill instructions. That is a separate reviewed trial,
not an unchanged retry or permission to relax this result. Global knowledge
autonomy, environment installation generality, long-task reliability, production
UI behavior and broader self-improvement are not accepted by this sample.
