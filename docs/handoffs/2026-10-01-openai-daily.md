# OpenAI daily Host checkpoint — 2026-10-01

Parent `9688c584dc7d2f39f150b9d3dc9fc1f768ae7606`; fixed OMP
`596f2da7101178214aa27a753529d15e6b7ad91d`. See
[evidence](../evidence/openai-daily.md), [usage](../openai-daily.md), and
[provider ordering/deferred register](../provider-plan.md).

## Done and verified

- Custom models.yml Chat/Responses configuration, explicit overrides and private
  key resolution reach actual CLI streaming/tools/journal/restart.
- Device login/logout, private `$ARA_HOME/agent/auth.db`, per-request account
  refresh, exact-data CAS and normal-exit settlement; dedicated Codex SSE with
  native encrypted reasoning history and correct Session attribution.
- Existing CLI workflow includes `/new`, summary adoption/rejection, logout and
  interrupted refresh → durable disable → restart without unknown-grant replay.
- One-command module/full runner: 22/0/0 module; backend 1,491/0/20;
  fmt/Clippy/target/doc/inventory/deny/build PASS, source unchanged,
  134.536 seconds. No code changed after this gate.
- Actual native configured Manager → OMP → CAS `deepseek-v4.1-flash`:
  file task and original Session recall pass, 7.146 seconds; binary pinned.
- Independent source/gate reviews completed. No private credentials/config,
  SQLite or trial artifacts belong in Git.

## Remaining and next

1. Actual OpenAI account device authorization and bounded subscription-model
   task need user participation. The CLI login implementation is tested with
   synthetic OAuth; do not call that a real account acceptance.
2. Continue coherent remaining Core/Session/RPC reproduction. Other Providers
   follow the user ordering; full auth precedence/commands/usage/rotation,
   registry execution projection and Codex RPC/WS/Lite/browser/native compaction
   remain recorded required work. Unsupported execution is explicitly rejected.
3. Do not rerun unchanged module/full gates merely because context was compacted.
   Reuse receipts and hashes; rerun only changed modules or a required new gate.

P0/V1 accepted; P1–P6 open; RPC 27/42; `ported_through_commit=null`.
The custom CLI slice is bounded audited functionality; account acceptance/full
parity remain WIP. Strong termination/unwritable DB unknown refresh settlement,
Codex summary server cost limits and new Linux execution are not proven.

## Time and speed

Batch began about 04:29 UTC; final code gate/trial completed about 06:05 UTC.
Implementation, source review and fix/fixture iterations dominate elapsed time;
four final module suites take 2.499 seconds, shared backend 125.629 seconds.
Use existing OMP inputs/ARA wire helpers, batch related flows, keep one Cargo
owner and a shared stable-snapshot gate. Ordinary details do not need new micro
tests. Complete account acceptance time depends on device authorization; a full
OMP completion estimate requires measured remaining module scope.
