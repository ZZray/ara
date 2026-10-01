# Native thinking stream-close retry cap — 2026-10-01

Parent `965491e`; fixed OMP
`596f2da7101178214aa27a753529d15e6b7ad91d`.
Status: **tested/audited bounded WIP** on the final recorded source snapshot. Complete
CA-TURN-RECOVERY, CA-RPC, P1–P6 and `ported_through_commit=null` stay open.

## Fixed source and delivered scope

`coding-agent/src/session/turn-recovery.ts:1395–1412,2156–2159` caps a
nonempty JS-trim thinking failure at `min(configuredMaxRetries,1)` when:

- Provider is exactly `openrouter` and the native case-insensitive
  `server_error:\s*stream closed with reason:\s*error` pattern matches.
- Provider is exactly `github-copilot`, model exactly `grok-4.6`, API exactly
  `openai-responses`, and the native missing-terminal diagnostic matches.

The cap is recomputed for each failed message, rather than retained as a saga
limit. Existing ownership, cancellation, usage, unsafe output and unknown tool
effect gates run first. Exhaustion and `auto_retry_start.maxAttempts` use the
same effective maximum. Finite negative, zero and fractional settings retain
their existing semantics. Matching reuses JS whitespace and ASCII case folding.

`ai/src/providers/openai-responses.ts` supplies the exact EOF/DONE diagnostic.
The actual Rust parser now emits that diagnostic. `ResponsesStreamState`
separates two facts: reasoning commits the Provider stream and prevents
transparent replay, while thinking-only Session recovery remains eligible.
Function calls, unsupported native output and Codex validation failure retain
both vetoes. Structured error evidence uses the Session fact separately from
the Provider commitment; usage vetoes and prior evidence remain monotonic.

Delivered paths:

| Path | Role |
| --- | --- |
| `crates/ara-cli/src/rpc_host_retry.rs` | Native route/message cap |
| `crates/ara-cli/src/rpc_host.rs` | Same effective budget for admission/events |
| `crates/ara-ai/src/providers/openai_responses.rs` | Real parser diagnostic and failure evidence |
| `crates/ara-ai/src/responses_stream.rs` | Separate Provider and Session wire facts |
| `crates/ara-cli/tests/rpc_retry.rs` and `tests/support/rpc_stream_close_cap.rs` | One grouped real-process recovery scenario |
| `scripts/verify_turn_recovery.py` | Module runner and shared optional full gate |

All 18 fixed full source exports are pinned in
`C:\Temp\ara-turn-recovery-batch\source\source-receipt.json`. Native tests
were read and their relevant inputs reused; this is not a claim that all native
turn-recovery tests ran or were ported. Ordinary details do not receive new
standalone micro-tests.

## Executed verification

One-command module entry:

```powershell
python -X utf8 scripts/verify_turn_recovery.py --module all
```

Add `--full` for the shared final fmt/Clippy/target/doc/inventory/dependency/build
gate. This reuses the existing backend runner; Root remains the only Cargo owner.

The grouped Host scenario covers OpenRouter, Copilot EOF, Copilot DONE,
reasoning `output_item.done` followed by EOF, raw server error, another actual
Responses provider, and unknown native `computer_call`. It checks actual
request counts, effective maxAttempts, separate persistent failure flags,
failed native entries/recovery metadata, idle state and the original Session
after restarting the real Rust process. Unknown output makes one request with
no Session retry. Known failures make two requests/one Session retry; the
other-provider near miss makes three requests/two Session retries and succeeds.
Existing Responses tests verify that reasoning never authorizes transparent
Provider replay. Existing classifier tests retain structured usage vetoes
through error-envelope finalization and JSON roundtrip.

Earlier receipts remain under the same local artifact root:

- `module-20261001T094240Z`: Responses 31/0; Host 18/1. Real Copilot EOF
  exposed the conflated Provider/Session flag; only changing text was insufficient.
- `module-20261001T095045Z`: repaired four-mode scenario; Responses 31/0,
  Host 19/0; compile 19.580 s, suites 4.926 s.
- `module-20261001T095245Z`: broadened scenario stopped before any full gate
  because a new assertion incorrectly expected Chat to publish the Responses
  wire commitment flag. It was narrowed to Responses; request-count checks
  still verify no Provider replay for every protocol.

Final stable command:

```powershell
python -X utf8 scripts/verify_turn_recovery.py --module all --full --output C:/Temp/ara-turn-recovery-batch
```

| Final check | Result | Time |
| --- | --- | --- |
| Selected target compilation | Pass | 2.082 s |
| Responses HTTP / actual RPC Host | 31/0/0 + 19/0/0 | 0.671 + 4.219 s |
| Windows fmt, all-feature Clippy, target/doc tests | 1,504/0/20, 121 suites | 282.778 s |
| Fixed inventory | Pass | 0.452 s |
| Dependency policy | Pass | 2.543 s |
| Final CLI build | Pass | 0.379 s |
| Complete shared command | Pass; before/after source unchanged | 293.401 s |

`module-20261001T095321Z/receipt.json` pins 147 source files; Root verified
them against the delivered worktree. Full target recompilation took 2m48s,
and the two module suites took only 4.890s. This batch ran the full backend
once; the earlier failed module stopped before that gate. Development builds
selected targets only and ordinary mappings continue using existing corpora.

### Preferred actual CAS task

The current catalogue confirmed `deepseek-v4.1-flash` through
Manager → OMP management → CAS, with OpenAI Chat Completions. Local Manager
credentials stayed private in the relay; the Rust child used a dummy key.
The temporary route uses `provider=openrouter` to exercise the native matcher;
this is an alias over CAS, not an actual OpenRouter or Copilot connection.

Three real Rust process Runs retain Session
`01a0f6ea-45a4-70b1-aa66-bf7a8a1add04`:

1. Two controlled thinking-only failures stop at one Session retry, with one
   `maxAttempts=1` start, a failed end, idle state and both native errors retained.
   Entry `99f8d60c` is superseded; final error `96e27a72` stays unresolved.
2. Reopen that Session through actual CAS. The model reads `invoice.csv`, writes
   `invoice-summary.json`, then reads it to verify the output. Real tool sequence
   is `read/write/read`; line totals are 15, 25.5, 34.5 and grand total 75.
3. Reopen the same Session with tools disabled. Actual model recall returns
   `marker_quantity=4; folder_unit_price=8.5; grand_total=75`.

`live/20261001T100223Z` retains raw requests, pipe events, journal and artifact.
There are seven requests: two controlled failures plus five actual CAS calls.
Bounds are three Runs, six model calls/Run, eight relay requests, 120 seconds/Run
and 4,096 output tokens/call. The task/restart Runs take 12.332/1.802 seconds;
the complete trial takes 14.823 seconds. Five actual usage records are retained;
controlled usage is unknown and cost is unreported, not zero. Root independently
checked the numeric artifact, actual tool receipts, original IDs and recall.

Final/tested/live binary SHA-256:
`3a3f75a72bccd853d3ff50eb42f63c1bc5cf3db148b67c293963a65ecd218e9a`.
No duplicate paid task or unchanged full gate is needed after compaction.

## Independent review and acceptance boundary

Independent Codex `ctx_oracle_diff_review` reviewed PRE and POST under the
project Git/Core/Provider procedures. PRE required the real EOF/DONE wiring.
POST-01 reproduced the actual Session veto and is resolved by the distinct
wire facts. The final grouped test includes the old-evidence error path and
unknown native output; the scoped reviewer found no further blocker.
The final independent POST is `PASS_WITH_EXPLICIT_GAPS`, no blockers.
Root separately verified all 147 delivered source pins and 18 fixed exports,
actual artifacts, journal/native IDs, usage, bounds and binary identity.
Local `root-point-audit.json` records the bounded acceptance decision.

Final gate SHA-256:
`8ea7402684abc671cf5ef92f9b869573d97bd9707c46f16e4d3f94158df70941`.
Independent POST SHA-256:
`6a2b377b8d9ba0f85f91ee529115f338bd28f632ef26553a53b06b070f04d302`.

This remains audited bounded WIP after the checkpoint passes. Mandatory parent
work includes complete overflow journal rewrite/rollback and model capacity,
interrupted/unexpected/empty/malformed stop recovery, native configured fallback,
the exhausted-error presentation, complete compaction methods/triggers and
continuation/queue ownership. Full OMP reproduction is not replaced by these
seven scenarios. Actual OpenAI account authorization remains its own live gate.

The source investigation began around 09:29 UTC; final live task completed around
10:03 UTC. Investigation/integration and real wiring corrections dominate this
batch; the final shared gate takes 4m53s and the actual trial takes under 15s.

Next: implement overflow as a coherent Session/Host module, reusing the checked
compaction transaction and original Session owner. Other provider work follows
the user's recorded daily-use priority.
