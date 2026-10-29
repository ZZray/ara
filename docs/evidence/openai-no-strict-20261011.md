# OpenAI Host NO_STRICT binding — 2026-10-11

**The bounded Graph/CLI startup behavior is exercised. Full daily CLI remains
FAIL on its original Anthropic branch. Full Chat/Responses/auth surfaces remain
OPEN; formal acceptance stays 4/111.**

## Requirement and delivered scope

Fixed OMP: `596f2da7101178214aa27a753529d15e6b7ad91d`.
Source: `packages/ai/src/utils/schema/adapt.ts:14` and
`packages/utils/src/env.ts:275,294–303,424–443`.

The Host samples `ARA_NO_STRICT` once at process startup. True spellings are
exactly `1,Y,y,TRUE,true,YES,yes,ON,on`; values are not trimmed or case-folded.
Windows lookup uses ordinary OS `getenv` semantics, including case-insensitive
environment names. Only this policy key is frozen; other Daily configuration
reads retain their existing behavior. Portable Core consumes the boolean.
ARA naming replaces the upstream PI prefix, without a PI alias.

Production paths are `ara-cli/src/schema_policy.rs`, `lib.rs`, `main.rs`,
`daily_model_config.rs`, `host_edit_post_apply.rs`, `host_thinking.rs`,
`rpc_host_fallback.rs`, `rpc_host_maintenance.rs`, `rpc_host_models.rs`, and
`ara-ai/src/providers/openai_responses.rs`, `openai_responses/actual.rs`.
Four existing test paths exercise these bindings; no new test executable.

Packet: `D:/workspace/env/cache/ara-port-candidates/schema-no-strict-20261011`.
Paths below are relative to this packet. The final delivered manifest is
`NO-STRICT-DELIVERED-v3.json`, SHA256
`1e3a588a6fad390cb7186c8508414fbc330a8daf363cdf0c9b2dce14a155b748`.
Its 15 paths preserve the existing dirty checkout; the Git index stays
`89d34e26be97c90a14680d44ace937e6b1ae5a86e8ee7b46dd6b90718a922475`.

## Actual execution and retained failures

One joined build uses the existing module runner with all five original
targets and unchanged profile/features, followed by complete unfiltered groups:

```text
cargo test --locked -p ara-cli -p ara-ai --all-features --test daily_model_config --test model_route --test openai_http --test openai_codex_http --test openai_daily_cli --no-run --message-format=json
```

| Batch | Build | Artifact reuse | Complete group results | Total |
| --- | ---: | --- | --- | ---: |
| execution-v1 | 298.391s | 418 reused / 15 compiled | config12/12; route17/18; Chat54/54; Codex10/10; CLI11/12 | 520.078s |
| execution-v2 | 87.344s | 431 reused / 2 compiled | route17/18; CLI11/12 | 265.563s |
| execution-v3 | 17.437s | 431 reused / 2 compiled | route18/18; CLI11/12 | 192.671s |

All three batches have `source_unchanged=true`. No ignored/filtered test is
used to hide a failure. Only the two changed test files trigger the v2/v3
reruns; unchanged config/Chat/Codex test binaries and production binary retain
their v1 hashes, permitting explicit evidence reuse.

The first added fixtures incorrectly assumed loopback endpoints supported
strict mode. Repair v1 authors `compat.supportsStrictMode:true`, retaining
strict/schema assertions. The next execution exposed two further fixture
errors: the synthetic Codex lease lacked its required single account header,
and the CLI read schema omitted the native Agent's required intent `i`.
Repair v2 reuses the existing synthetic account-header contract and checks
exact strict `[i,path]` versus raw `[path,i]` required ordering. These repairs
change tests only. All original failure logs are retained.

Final observations:

- Four isolated Windows children mutate the real environment after startup;
  unset, `true`, mixed-case `True` and lowercase key `ara_no_strict=1` retain
  their original frozen policy. The raw spellings are checked.
- Three Graph protocols × three flag states × four resolver generations:
  **36 requests**, strict flag and actual schema required/additionalProperties
  assertions PASS. Synthetic Codex account headers reach every request.
- Actual CLI startup emits `ARA-NO-STRICT CLI_STARTUP PASS 3/3`, with original
  request counts and native read schema assertions. The same original test
  then executes the legacy Anthropic command and fails at `success`, reporting
  `actual provider dispatch failed`. The full CLI group remains **11 PASS / 1
  FAIL**, not a green gate. This deferred provider gap is not changed here.
- Final route group: 18 PASS, 0 FAIL/ignored, 6.703s. Full CLI: 167.187s.

Final build receipt SHA256:
`3e75e3a47b51cba06df1b730b886c3527b99c84091e79c669fec007e9cc2e5ff`.
Final module receipt SHA256:
`7bc19055fef624ce4b002f46b56b52d22b5e89a5fd41df2751b482eb8265f738`.

## Bounded real task

`execution-v2/real-cas-receipts/20261011T050659Z/receipt.json` PASS, SHA256
`ddb02f273bb2fb5aed259f6f02f5d0105cf542e267c4d52dfc865b4beb28779b`.
The currently verified Manager local OMP management → CAS route supplies
`deepseek-v4.1-flash`, `openai-completions`. Bounds: two Runs, task6/recall2
maximum model calls, 120s per Run, 2048 maximum output tokens.

The task reads random sales data, edits `summary.json`, and reads the edited
file: 4 model calls / 4 tool calls / 5.718s. Root verifies pen11, book34.5,
cup8.5, grand_total54 against CSV quantities4/3/2. A second process recalls
pen quantity4, book unit price11.5, cup quantity2 from the same Session without
tools: 1 model call / 1.735s. Both Runs end with the expected terminal and model.
Session: `01a1295b-6fc7-7718-9a42-bb3ad7a89345`.

Normal `ara.exe` SHA256 remains
`a4f8ebcf9fb6b7f862816443cec1efffc210702a01b0078a9a625a79d5ef957f`
across all three builds. Test-only repairs do not repeat the CAS task.

## Independent review and limits

Independent Codex `/root/read_complete_scope` reviews the 15-path source and
runner scope, then each two-file fixture repair. SOURCE reports are APPROVE:

- `SOURCE-REVIEW-v1.json`: `ebe8c37a1cab9d695e65789a804b18e368bab9392d20b9fcb66d9b799e99b6d1`.
- `strict-fixture-repair-v1/SOURCE-REVIEW-v1.json`: `5416ae07281d08d7addef4795caea7b08c74b090b2858dba1988d803b647def1`.
- `strict-fixture-repair-v2/SOURCE-REVIEW-v2.json`: `e372030b00a9f97540b0420e77a84b8805fb588963148e5cd317f9ce3688681a`.

Final independent POST `POST-REVIEW-v1.json`, SHA256
`10df8c87c158076cfce369467020f35b535581db46108832be849d84c83ab9e8`,
returns **APPROVE_FOCUSED_EXECUTED_BEHAVIOR_ONLY**. It verifies all15 delivered
hashes, unchanged production/four reusable binary hashes, raw Windows/Graph/CLI
observations and actual CAS tool/artifact/recall evidence. Root independently
checks these pins, test counts, markers and CSV arithmetic. Combined original
five-module evidence is **105 PASS / 1 FAIL**; the full CLI and parent remain
unaccepted. A partial group or successful model Run does not accept a parent.

Typed serde Chat/Responses/Codex builders do not consume this flag and remain
OPEN. Resolver generations are not actual RPC reload/newSession command
acceptance. Synthetic Codex authorization is not real account login. Arbitrary
provider compatibility, real reasoning, complete Chat/Responses/auth, terminal,
P1–P6, products and release gates remain OPEN; `ported_through_commit=null`.
