# OpenAI daily CLI repair — 2026-10-08

Limited repair evidence; full OMP remains **0/111 complete**, with 1 tested,
27 implementing and 83 open. Fixed upstream:
`596f2da7101178214aa27a753529d15e6b7ad91d`. Base `dev` HEAD is
`20aa0e2ef9872f633ba2671376cdf778477e7115`; unrelated dirty work is preserved.
This does not accept an entire provider, OMP surface, phase or product.

## Initial whole daily module and observed failures

`python -X utf8 scripts/verify_openai_daily.py --module all --keep-going --output C:/Temp/ara-model-fallback-batch/current-openai-daily-module`
completed in **104.473s**, including 40.981s compilation. Config 12/12,
auth 14/14 and Codex SSE 10/10 passed; original daily CLI was **9/12**.
Source remained unchanged. Packet: `module-20261007T185536Z` under that output.

Three original failures were retained:

1. `custom_chat_configuration_and_explicit_overrides_reach_the_wire`: its
   explicit legacy Anthropic step reports `actual provider dispatch failed`.
   The current Anthropic adapter implements typed streaming, without actual
   dispatch. This protocol port remains required later under the user's provider
   priority. The mixed case stays FAILED; its final request-count/header/token
   assertions after the failed Anthropic step have not executed successfully.
2. `device_login_tool_resume_compaction_and_logout_close_the_cli_account_workflow`:
   expected `daily.txt` was missing. The generic Compatible fixture supplied
   correct `function_call_arguments.done` followed by final item arguments `{}`.
   Fixed Codex source `openai-codex-responses.ts:2441–2458` makes final item
   arguments authoritative; current Rust `responses_stream/actual/router.rs`
   preserves that contract. Only this account case now supplies consistent final
   arguments. The generic conflicting Compatible fixture and production protocol
   remain unchanged.
3. `thinking_cli_auto_actual_tool_usage_and_last_resolution`: the REPL process
   ended with main stack overflow, Windows `0xc00000fd`. A same-binary controlled
   reproduction completed print with six requests and a file, but REPL stopped
   after classifier and daily-model requests, without a file. Packet:
   `C:/Temp/ara-model-fallback-batch/daily-thinking-repl-repro-v1`.

## Two delivered paths

| Path | Change |
| --- | --- |
| `crates/ara-cli/src/main.rs` | Allocate the existing Agent driver at an ordinary-function boxed-future boundary; `/clear` and `/new` reuse the existing boxed setup. Original async body, inputs, cancellation, settlement and Session ownership remain unchanged. |
| `crates/ara-cli/tests/openai_daily_cli.rs` | Only the synthetic Codex account case copies its arguments-done value into the authoritative final item. All original assertions, file effects, scripted commands and 20s deadlines remain. |

Final path SHA256 values respectively:
`54b050f65fc31eaadf01e752eabea03d5ab5abde87384f399941ccd1f069ad2f`
and `599c55351a86497602d8abc739b5a47e582324ddc76f8efcadabcc99f43cb49f`.
Independent Codex SOURCE PRE: `/root/native_realm_pre`, approve for application
and original gates; report
`C:/Temp/ara-model-fallback-batch/daily-cli-repair-independent-pre-v1/REVIEW.md`,
SHA256 `dbba0526af4636e11219f6a64dd4fdecced190179d419f8fa5198a5b9c0288a4`.
The successful intervention supports this allocation correction; it does not
establish a unique internal stack-overflow cause.

## Original combined regression on final code

One cached build, jobs 2 and test threads 2, with target directory
`D:/Temp/ara-build/ara-github-target-20261005`:

`cargo test -p ara-cli --all-features --locked --lib --bin ara --test rpc_host --test openai_daily_cli --no-fail-fast`

| Entry | Result | Test-body seconds |
| --- | --- | ---: |
| CLI lib | 240/240 | 13.36 |
| CLI bin | 81/81 | 9.82 |
| Daily CLI, all original cases | **11 passed / 1 failed** | 55.12 |
| RPC Host | 61/61 | 117.02 |

Cargo exit **101**, because the original mixed Anthropic case still fails.
Total **313.914s**, including **115s compilation**; main scoped fmt check
passed in 0.187s. No original case was skipped or weakened. Auto print and REPL
now both execute six requests, produce `thinking.txt = owned`, preserve usage
and user receipts, and pass `/clear` and `/new`. The account case verifies real
local file effects, original-Session resume, compaction rejection/adoption,
reset identities, usage headers, logout denial and unknown-refresh non-replay.
This account fixture is not an actual OpenAI account/model trial.

Packet: `C:/Temp/ara-model-fallback-batch/daily-cli-repair-delivered-v1`.
The original `receipt.json` and its verifier assertion failure are retained.
That temporary verifier mixed backslash baseline names with forward-slash
overrides, leaving two old-hash aliases for the authorized files. The append-only
`receipt-corrected-paths.json` normalizes paths and checks all **751** unique
source paths: two candidate sources plus **749 unchanged** baseline sources.
Git index bytes remain unchanged. No source repair or Cargo rerun was needed
for this receipt correction. The original module failure is still recorded.
Prior config/auth/SSE receipts above were not rerun in this repair batch.

## Same final binary: bounded CAS REPL task

Frozen binary SHA256:
`af5258d9d5aeeb54e2085243505d4b89662f834a1acea3959b80c757ac736455`.
Fresh configured-route catalogue check found `deepseek-v4.1-flash`, protocol
`openai-completions`. The Manager local gateway was unavailable; validation
used CAS configured in Manager, with credentials only in the process environment.

Packet: `C:/Temp/ara-model-fallback-batch/runtime-integration-cas-receipts/20261007T192115Z`.

- Real REPL task edits `summary.json` from `sales.csv` and reads it back:
  **5.488s, four model calls, four tools**, total **52.5**.
- Another process continues the original Session with tools disabled:
  **1.586s, one model call**, correct original pen quantity 3, book unit price
  11.5 and cup quantity 5. Session
  `01a117d0-2264-7607-9bf5-f7db5138bb62` is identical in both processes.
- Bounds: first task six calls, recall two, 120s and 2048 output tokens per Run.
  Root checked source CSV arithmetic, final artifact, raw answers, tool receipts,
  stderr Session paths and binary identity. Source stayed unchanged.

Actual OpenAI device login, on the prior v23 binary and an isolated profile,
timed out naturally after **995.235s**, exit2, no accepted login or model task.
Its original receipt remains in `live-openai-account-receipts/20261007T185537Z`.
The command was not blindly restarted. Workspace/platform/release gates were
not repeated here; complete OMP, account real-model and product acceptance remain open.

Final independent Codex POST: `/root/native_realm_pre`, **approve the two
scoped repairs and bounded final-binary CAS REPL/continuation evidence**;
the combined gate remains FAILED, daily 11/12 and full OMP 0/111. Report:
`C:/Temp/ara-model-fallback-batch/daily-cli-repair-independent-post-v1/REVIEW.md`,
SHA256 `16ba2952299fd11a57281b3326f9c281a8e205405e60be27100cc02fb6a19517`.
Root read the full frozen report and independently checked the material results.
No complete provider, surface, phase or product acceptance is claimed.
