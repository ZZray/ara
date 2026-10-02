# Native message compaction and terminal recovery — bounded WIP

Base `75129f7`; fixed OMP `596f2da7101178214aa27a753529d15e6b7ad91d`.
This checkpoint extends existing parents; it does not accept a full surface.

## Requirement and source

- `packages/agent/src/compaction/compaction.ts:415–562,1315–1428,1731–1750,1808–1855`:
  reverse token accumulation and valid User/Assistant cuts, independent history
  and turn-prefix summaries, native merge text and separate raw-reserve budgets.
- `packages/ai/src/oneshot-retry.ts`: manual summary defaults to three total
  attempts, 500 ms exponential base, 75–100% jitter, 8 s pure-backoff ceiling
  and 30 s maximum wait. Header/text hints use their maximum. Automatic
  maintenance explicitly disables the inner retry owner.
- `packages/coding-agent/src/session/messages.ts:543–576` and
  `turn-recovery.ts:797–965`: orphan ToolUse has neither actual tool call nor
  visible text; mechanical unexpected Stop retains signed thinking and uses
  the original Agent with runtime-only guidance and the native three-retry cap.

## Delivered paths and observable boundaries

Core selects message cuts, checks tool ownership and source IDs, runs independent
summary branches under one cancellation/deadline owner, and retains all actual
invocation receipts. Usage admission may permit same-prompt retry; typed native
output/validation and explicit same-route veto never authorize replay. Unknown
usage stays unknown. Codex adoption checks each request's local budget separately.

Session's native commit checks the exact snapshot, source window and cumulative
raw IDs. Assistant cuts allow an unanswered user prefix while still rejecting
images, lowered developer priority, incomplete tools and unknown effects. Native
Assistant replay requires exact source IDs; the older User-only commit and
legacy imported User-summary API retain their contracts. No split flag is added.

REPL and RPC use the checked native commit and support repeated compaction.
RPC completes durable empty-terminal removal before same-Agent continuation;
mechanical recovery preserves its actual signed assistant. Smart/local
classification and accepted custom-message callers remain open.

## Reproducible module gate

```text
python -X utf8 scripts/verify_native_compaction.py --full --output <temporary-artifact-root>
```

The entry compiles selected modules once, executes cut/summary/Session/terminal/
RPC/REPL/account scenarios, then runs the shared owned-package format check,
Clippy, all-target/doc tests, inventory, dependency policy and binary build.
Root is the sole Cargo owner. Ordinary details reuse existing grouped fixtures.
Results and actual CAS artifact/restart receipts are recorded after execution.

## Executed final snapshot — 2026-10-02

Artifact root: `C:\Temp\ara-native-compaction-terminal-batch`.
Final receipt: `module-20261002T062646Z/receipt.json`; base `75129f7`.
All 17 scoped source/test/script hashes match the before/after receipt and the
delivered worktree. Root directly inspected fixed-source selection/retry/terminal
rules, current Core/Session owners, raw process frames, journal and artifacts.

| Module | Passed / failed / ignored |
| --- | --- |
| Cut / summary | 18/0/0; 14/0/0 |
| Session projection / reload / context | 10/0/0; 8/0/0; 5/0/0 |
| Terminal / RPC Host | 4/0/0; 16/0/0 |
| REPL / account fixtures | 92/0/0; 4/0/0 |
| Shared backend including doc tests | 1,528/0/20 |

Module execution: **21.463s**; selected compile: 1.811s. One stable full command:
**130.283s**, including backend 102.867s. Owned-package format, strict all-target
Clippy, target/doc tests, inventory, dependency policy and binary build pass.
Dependency policy retains its recorded license/duplicate warnings. Twenty ignored
cases are not execution evidence. Account fixtures are synthetic, not login.

Earlier failures remain recorded: the Windows default-stack Host regression
required boxing only two RPC parent await boundaries, verified by the original
six scenarios. The ineffective Core-only box candidate was removed. Two old
Session negative families assumed every Assistant cut was illegal; each now
checks a genuinely invalid Assistant cut without exact source IDs. Their other
negative, raw-history and reopen assertions remain. The existing context and
reload families are included in the module runner, with no new micro-test family.

### Actual CAS task and original-Session reopen

`live/20261002T062912Z/receipt.json`: **PASS**, **19.193s**, eight actual CAS
requests, two processes. The enabled Manager → OMP management → CAS catalogue
confirmed `deepseek-v4.1-flash`, using OpenAI-compatible Chat Completions and
private `ry_switch` authorization. No credentials were saved. Bounds: 12 relay
requests, 120s per process and 2,048 output tokens per request.

The task read `stock.csv`, wrote and re-read `stock-total.json` with
`folder=42.5`, `lamp=51.75`, `grand_total=94.25`; original CSV bytes remain.
Manual compaction made two real, tool-free history/prefix summary requests and
persisted one native merged summary. Its kept entry is Assistant, with exactly
nine raw source IDs and no invented split field or durable Developer reminder.
The original Session `01a0fb4d-78ac-74e1-b2d3-5b332f9ade07` reopened with tools
disabled and recalled quantity 5, lamp unit price 17.25, total 94.25 and
`Cedar-5729`. All observed tool results succeeded; native assistant usage remains
recorded, and absent cost/usage buckets remain unknown. No live fault was injected.
The six visible Assistant receipts report input 7,611 + cache-read 12,416 +
output 577 = total 20,604. The two summary calls' usage is not persisted by the
Host; this is not an eight-call total. Full per-invocation Host receipts stay open.

Binary SHA-256:
`4fa59b184ca39bd2136964712ae05da16da633e0aef3bb1dcd0f042151f4c906`.
Artifact SHA-256:
`82913eb0011d089b35c4aa69ecc5e6518904f3dbe8dacc888a9b750c1b2506ac`.
The binary hash is unchanged across the trial.

### Audit boundary

Independent Codex `/root/ctx_oracle_diff_review` performed PRE, scoped static
reviews and final execution/artifact review; receipts are `pre-review.json`,
`static-final-review.json`, `stack-host-boundary-review.json` and
`post-review.json`. Root's separate source/artifact inspection is `root-audit.json`.
Final independent disposition is `PASS_WITH_EXPLICIT_GAPS`, zero open findings
and all 17 scoped hashes verified. The raw backend log covers 121 result suites.
This is a bounded tested/audited WIP checkpoint. It accepts no complete parent,
phase or parity marker; the remaining requirements below remain mandatory.

## Required remaining work

The message adapter does not cover native raw metadata/custom/Bash/reset cuts,
all compaction methods/reducers/rescue, full billing/token/input/settings/receipt
contracts or every imported native transcript. Smart online needs a tiny/smol
registry/auth factory; local needs the actual tiny runtime. The accepted-empty
custom lifecycle caller is separate from ordinary RPC prompt and tool yield.
Actual OpenAI account authorization, real Responses/Codex tasks and exact Linux
verification remain unrun. P0/V1 accepted, P1–P6 open, RPC 27/42 and full marker
null retain their status. Complete OMP reproduction still precedes customization.
