# Real-model trials: slice A1 and the A2 coding tools (2026-09-25)

Points covered:
- AGT-01 agent loop, AI-01 OpenAI Chat, SES-01 session journal, CLI-01 print host.
- TOOLS-01 read/write/bash, TOOLS-02 grep/glob, TOOLS-03 edit.

Every run used the real `ara` binary built from `dc60edb`. The only uncommitted change was the trial script. No fake upstream was involved.

## Routes (verified live before use)

| Route | Protocol | Catalogue check | Models used |
| --- | --- | --- | --- |
| Agnes, `https://api.agnes-ai.cn/v1` | OpenAI Chat Completions | `GET /models` lists `agnes-2.5-flash`, `agnes-2.5-pro` | `agnes-2.5-flash` |
| OpenRouter, `https://openrouter.ai/api/v1` | OpenAI Chat Completions | `GET /models` has 458 models, 16 of them `:free` with `tools` | `openrouter/free` (router), `qwen/qwen3.8-27b:free`, `google/gemma-4-31b-it:free` |

Keys came from environment variables only: `ARA_API_KEY` for Agnes and `OPENROUTER_API_KEY` for openrouter.ai. The CLI sends each key only to its own route. A scan of every artifact directory for both key values found nothing.

## Commands (sanitized)

```
ARA_API_KEY=<agnes> ARA_TEST_BASE_URL=https://api.agnes-ai.cn/v1 ARA_TEST_MODEL_ID=agnes-2.5-flash \
  bash scripts/real_model_trial.sh <out>              # A1 task
ARA_API_KEY=<agnes> ARA_TEST_BASE_URL=… ARA_TEST_MODEL_ID=agnes-2.5-flash \
  bash scripts/real_model_trial_a2.sh <out>           # A2 seeded bug, all tools
ARA_TEST_BASE_URL=https://openrouter.ai/api/v1 ARA_TEST_MODEL_ID=openrouter/free \
  bash scripts/real_model_trial_a2.sh <out>           # same task, second route
ARA_TRIAL_TOOLS=glob,grep,read,edit ARA_TRIAL_PROMPT="…use glob and grep…" … agnes-2.5-flash
```

Bounds for the A1 task: at most 6 model calls, 180 s and 1024 output tokens per call. Bounds for the A2 task: at most 12 calls, 300 s and 2048 tokens.

## Results

A1 task (`fib.txt` with 10 Fibonacci numbers, then `wc -l`) on `agnes-2.5-flash`:
- Exit 0 after 2 model calls in 4 s.
- `write` and `bash` ran in the same turn; both succeeded.
- The artifact equals `0 1 1 2 3 5 8 13 21 34`, one number per line. The final answer reports 10 lines.
- Usage was 3451/156 and 3652/64 tokens.

A2 seeded bug (`apply_discount` treats a percent as a fraction):
- The harness requires the baseline to fail. It recorded `Ran 2 tests … FAILED (failures=2)`.

| Model | Exit | Calls | Tool receipts | Independent test run after | Tests untouched |
| --- | --- | --- | --- | --- | --- |
| `agnes-2.5-flash` | 0 | 5 | bash (find), bash (tests fail, `isError`), read ×2 (`[shop/pricing.py#8D33]`), edit `PUT 16.=16:` → `[shop/pricing.py#87EB]`, bash (2 OK) | `Ran 2 tests … OK` | yes |
| `openrouter/free` | 0 | 9 | bash ×2, read ×2, bash (fail), edit error, read, edit, bash (OK) | `Ran 2 tests … OK` | yes |
| `agnes-2.5-flash`, tools `glob,grep,read,edit` | 0 | 4 | grep `apply_discount` (grouped `## pricing.py#8D33`, `*14:` match row), glob, read, edit with the grep/read tag, read | `Ran 2 tests … OK` | yes |

In each passing run the fix is `return amount - amount * percent / 100`.

Recovery observed with `openrouter/free`:
1. The model sent `PUT 16.=16:` without a header.
2. The edit tool returned upstream's error: `edit input must begin with "[PATH#HASH]" on the first non-blank line for anchored edits; got: "PUT 16.=16:". Example: "[src/foo.ts#1A2B]" then edit ops.`
3. The model re-read the file and sent `[shop/pricing.py#8D33]` followed by the same op, which succeeded.

The router moved between AtlasCloud, Novita, Nvidia and Cohere across calls. The history replayed correctly each time. Usage was reported on every call, including `reasoningTokens` and a zero cost for the free tier.

Failure path:
- `qwen/qwen3.8-27b:free` and `google/gemma-4-31b-it:free` both returned `429 Provider returned error`.
- `ara` exited 1. The assistant message has `stop: error` and empty (unknown) usage, not zero.
- No tool ran, the files are unchanged, and the journal records the error message.

Session journals: every assistant message and tool result is journaled. Record counts were 8, 15, 22, 13 and 5 lines per run.

## Coverage

Exercised on real models:
- The agent loop with multi-tool turns.
- OpenAI Chat on two providers, including cache-read usage from Agnes and per-call provider changes on OpenRouter.
- The session journal and JSON print mode.
- `read`, `write` and `bash` (including a failing command).
- `grep`/`glob` (grouped hashline output), and hashline `edit` with tags from both `read` and `grep`, plus its error path.

Not exercised:
- Replace, patch and apply_patch edit modes.
- Cancellation and resume on a real model.
- Non-OpenAI protocols.

Decision: the real-model requirement for AGT-01, AI-01, SES-01, CLI-01 and TOOLS-01 to TOOLS-03 is met for the behaviors listed. The not-exercised items stay open in their evidence records.
