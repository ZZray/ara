# MODEL-CONFIG-VALUES-01 — synchronous command credentials and headers

2026-10-02, branch `dev`, base `49edd254459d5907c197d452796c619b174c3145`.
Fixed OMP `596f2da7101178214aa27a753529d15e6b7ad91d` remains unchanged.
**Implementing / tested bounded WIP. The mandatory dependency audit fails;
this point, full registry/auth, its parent surfaces and P1–P6 are not accepted.**

## Observable requirement and scope

Reproduce the complete synchronous `model-config-values.ts` primitive, then
connect it to the supported custom OpenAI daily Host. Configured API keys and
provider/model/modelOverrides headers retain raw sources, materialize privately
in the effective project cwd, share command caches and refresh on the selected
native 401 path. A started helper must settle before normal process exit.

| Fixed source | Rust owner and verified slice |
| --- | --- |
| `packages/coding-agent/src/config/model-config-values.ts`, original `model-registry-command-values.test.ts` | `ara-cli::model_config_values`: command detection, synchronous value/header resolution, invalidation and live headers; four grouped primitive families. |
| `model-registry.ts:288–332,1340–1426`, `custom-models.ts` | `daily_model_config`, `config_request_auth`: eager provider headers before API-key installation, raw header chains and provider-wide command invalidation. Supported daily Host callers only. |
| `packages/utils/src/{env,dirs}.ts` | Exact-case nonempty environment lookup, empty-env literal fallback and Host-supplied effective project directory, including Windows. |
| `packages/ai/src/auth-retry.ts` | `model_route`: one typed, pre-output HTTP 401 command refresh; unchanged bearer is not resent. Full auth retry/rotation remains required. |
| Actual CLI startup, print/JSON/REPL, Session reopen and selected maintenance callers | Private request lease, ordinary retry reuse, tool/Session receipts, cancellation and normal-exit settlement. |

The final manifest `C:\Temp\ara-regauth-batch\final-manifest.json` binds
11 repository source/test/runner files and 21 fixed-source hashes. PRE reviewed
20 sources; `auth-retry.ts` was subsequently added and inspected in POST.
The fixed checkout is separate from ARA Git history. Existing vendor/EOL WIP,
`.codebase-memory`, private configuration and all local trial files are excluded.

## Native contract and remaining boundaries

- Commands use JavaScript trim after `!`; process-shared cache identity is the
  command alone. Nonempty success persists. Failure, empty output and invalid
  cwd are negative-cached for 30 seconds from settlement. Force/invalidation
  clears both caches. Shell timeout is 10 seconds; stdout and stderr together
  are bounded by 1 MiB. Unix SIGTERM may delay settlement, matching the native
  boundary; there is no new SIGKILL or process-tree guarantee.
- Header sources resolve in order, including overwritten helpers. Failed later
  values preserve earlier successful headers. Nested live sources snapshot
  once; raw mutation and local overlay/deletion remain observable. Primitive
  key casing follows JavaScript; HTTP merge is case-insensitive. Generated
  Authorization is last. Explicit CLI values stay literal.
- The reference Host starts eager provider headers before the provider key,
  after effective cwd is known. A controlled dependent header helper generates
  the key file; both Chat and Responses verify that native order.
- Ordinary wire retries retain the original private lease. A typed HTTP 401
  before any emitted event/content may invalidate key and all provider model/
  override header commands once. A failed refresh cannot fall back to an old
  key. A changed bearer permits one new request with its own identity receipt;
  an unchanged bearer is not replayed. Broader 403/account rotation and the
  full central auth policy remain open.
- Cancellation is checked after operation admission, after invalidation and
  before each unstarted helper. Already started helpers settle. A process-wide
  pending-operation barrier is registered before blocking-worker dispatch;
  normal CLI exit waits even if the Run consumer has dropped. The existing
  second-interrupt/hard termination boundary still has unknown effects.
- Rust suppresses raw helper stderr and reports safe error categories. Native
  Node execSync can echo raw stderr; this is an explicit credential-privacy
  adaptation. Bun compatibility has not been executed. Linux process behavior
  has not been exercised in this batch.
- This is not full ModelRegistry load/lookup/discovery/runtime-overlay/auth
  precedence integration. Noncommand daily key selection retains the existing
  bounded Host precedence. The separate asynchronous `resolve-config-value.ts`
  Brush/cache/process-tree semantics and other Provider families remain required.

## Executed module and final snapshot receipts

Repeatable module entry:

```text
python -X utf8 scripts/verify_config_values.py
python -X utf8 scripts/verify_config_values.py --module host
python -X utf8 scripts/verify_config_values.py --full
```

`--full` shares the existing backend/inventory/deny/build procedure. The Root
final-gate wrapper also builds after a retained dependency-audit failure so the
actual WIP binary can be exercised; it preserves `wholeGatePass: false`.

The six module families total **48 passed / 0 failed / 0 ignored** on the final
source or explicitly unchanged sources: primitive 4, selection 7, route 11,
actual CLI Host 5, daily CLI 4 and promotion/RPC 17. Relevant receipts:

- Initial `module-20261002T134846Z` is under the local temporary directory:
  primitive/selection/route pass; Host 4/1 fails early normal exit. That failed
  invocation is retained; it is not a passing whole module run.
- `C:\Temp\ara-regauth-batch\module-20261002T135138Z`: repaired actual Host
  5/0/0; compile 14.539s, tests 4.453s.
- `module-20261002T135414Z`: daily CLI 4/0/0; 3.950s.
- `module-20261002T135508Z`: promotion/RPC 17/0/0; 7.728s.
- `module-20261002T140447Z`: final eager-order/cancellation Host 5/0/0;
  compile 6.348s, tests 5.751s, source unchanged.
- `module-20261002T141001Z`: final selection 7/0/0; tests 0.818s,
  source unchanged.

The final stable source is exercised by
`C:\Temp\ara-regauth-batch\gate-20261002T141025Z\receipt.json`:

| Step | Result | Seconds |
| --- | --- | ---: |
| `python -X utf8 scripts/verify_backend.py` | Format, Clippy, all-target/all-feature tests and doc tests PASS; **1,625/0/20** | 301.553 |
| `python -X utf8 scripts/omp_inventory.py check` | PASS | 0.471 |
| `cargo deny check` | FAIL, existing RUSTSEC-2026-0192 | 2.955 |
| `cargo build -p ara-cli --all-features --locked --bin ara` | PASS | 0.411 |

Scope hashes before and after are identical. Binary SHA256:
`de8340831b6cf934e3b57012507d42dcd8abd034b31348d925b64ee1d57e1e76`.
The same actual binary is used for the live task below. This is a successful
backend test invocation, **not** a successful complete gate: advisories fail on
the fixed snapcompact renderer's `fontdue -> ttf-parser 0.25.1` dependency.
`deny.toml` is unchanged; bans, licenses and sources pass. The prior image-task
failures and that dependency obligation remain recorded in [snapcompact](snapcompact.md).

Earlier final attempts remain separate failures: `gate-20261002T135617Z`
formatting order; `gate-20261002T135715Z` collapsible-if Clippy; and
`gate-20261002T135754Z` backend **783/1/11**, stopped at the original REPL
interrupted-tool recovery warning assertion. The unchanged recovery test passes
alone (`e2e-isolated.log`, 1/0/0) and in the final 94/0/0 E2E suite. Source review
finds a possible Windows taskkill fixture race: tool termination can be
journaled before Host termination, making the warning inapplicable. The failed
journal was not retained, so this is an unconfirmed cause, not a diagnosed
runtime fix; its failed receipt is preserved and expectations are unchanged.

## Bounded real command-auth task and original Session reopen

Root-only local runner:
`python -X utf8 C:\Temp\ara-regauth-batch\run_live_commands.py`.
The current Manager configuration and actual catalog confirm
`deepseek-v4.1-flash`, protocol `openai-completions`. The preferred local
management proxy is unavailable; the enabled CAS route from the Manager
configuration is used. Configuration is read privately and never changed.

Bounds: two processes, five actual model calls, 90 seconds per process and
2,048 output tokens per call; no automatic replay of a failed/unknown task.
`live-20261002T141603Z/receipt.json` is **PASS**, 13.524s total:

- Four actual task calls, 8.592s: `read invoice.csv`, `write totals.json`,
  `read totals.json`, with exactly three successful actual tool receipts.
  Checked artifact is `{"cable":60,"bracket":21,"grand_total":81,
  "shipment_label":"Willow-4716"}`; SHA256
  `68e0bc446e4b0e9c0fe7a02844e4b515c38d3196e0bf00c0ea547f8c435652b3`.
- One actual reopen call, 2.103s, no tools: returns the same JSON from original
  Session `01a0fcf8-e8c4-7190-b090-aaf67cd4f240`. The prompt contains no answer;
  original raw rows and the artifact are unchanged.
- Each fresh process executes key/header helper once (four helper receipts).
  All helpers run in the effective project directory. Five incoming request
  checks confirm exact Authorization, shared-key header and proof header.
  Custom headers are verified **Rust -> checked loopback relay**; the relay
  forwards the real Bearer credential to CAS, not the custom proof headers.
- Public stdout/stderr/journal contain neither helper values nor raw commands.
  Original journal privacy is checked before making receipt copies. Values
  remain in private child environment/relay memory; receipts retain match
  booleans only. Every request has settled at process end and cleanup.
- Upstream total-token receipts are 3,194 / 3,409 / 3,387 / 3,556 / 2,988.
  Absent reasoning/cost fields remain unknown, not zero. Source/binary hashes
  before and after are identical to the final gate.

## Review, audit and progress boundary

Independent Codex `/root/remote_review` reviewed the bounded PRE plan and final
11-file POST plus source/receipt/live-runner scope. Root verifies key source
and runtime evidence. Formal `C:\Temp\ara-regauth-batch\post-review.json`
SHA256 is `4e43a4f43bca6e900c777d92b76fe118bfbc47d384df91c368302c8eaddab36e`.
Decision: `approve_bounded_tested_audited_wip_snapshot`, zero open reachable
findings, `acceptanceGranted: false`. Final manifest SHA256 is
`abeb0f13abf56010877c268d28b4056d3459bc9daa528b4fc8a22dde1a2f64da`.
The reviewer verifies 11/11 scoped files, 21/21 fixed source copies, final
gate/live identity, actual three tool receipts, exact JSON and same-Session
recall. Root independently reads tool frames, artifact, actual catalog/route
receipts and the formal POST; scope hashes and final binary match exactly.
Confirmed queued-cancellation, normal-exit settlement, eager header/key order
and refresh cancellation findings are repaired and affected grouped families
rerun. The proposed header-only promotion finding is withdrawn: its input is
rejected by existing custom-model validation before promotion. The temporary
invalid regression fails 6/1/0 (`module-20261002T140753Z`) and is removed with
the unreachable patch; no validation contract is weakened.

Root audit retains the declared source/privacy/platform/full-registry gaps and
failed dependency gate. No point or complete surface is marked accepted.
The fixed inventory still has 111 surfaces: 26 implementing, one tested and
84 open; `CA-MODEL-REGISTRY` now records its existing bounded implementation.
P0/V1 accepted, P1–P6 open, RPC 27/42 and `ported_through_commit: null` remain.
Started surfaces and bounded point counts are not a weighted completion rate.

Implementation/verification runs approximately 13:21–14:17 UTC (56 minutes);
review/documentation follows through approximately 14:23 UTC (62min overall
before Git checkpoint). The batch's initial estimate was
60–120 minutes. Most repeated checks followed concrete failures; the final
backend itself takes five minutes. Future work keeps module checks scoped and
shares one stable-source gate. Next: full registry/auth callers and recorded
Core/Host contracts, while dependency/image/Bun/Linux/account acceptance stays
explicitly required. There is no reliable total OMP/P0–P6 completion date yet.
