# Production model Registry and account discovery — tested WIP

## Requirement and scope

Target remains fixed OMP `596f2da7101178214aa27a753529d15e6b7ad91d`.
Base HEAD is `10e027da122e24df8accc773bede7b8ba2e3c962`. Tested WIP commit
`eab38d36e97e8f5a679aa1d4138494d3eb96fd30` extends MODEL-COMPOSITION-01,
not the accepted surface count. It is pushed to `origin/dev`; a separate
`git ls-remote` confirms the exact remote SHA. The initial HTTP 408 attempt
does not update the remote; one HTTP/1.1 retry succeeds. A single CI snapshot
shows repository checks `37066768536`, directory I/O `37066768497` and Skill
invocation `37066768500` still in progress; no Linux PASS is claimed.
No full CA-MODEL-REGISTRY, AI-AUTH, phase or parity marker is accepted.

| Fixed source | Rust owner and exercised scope |
| --- | --- |
| `packages/coding-agent/src/config/model-registry.ts` | `model_registry{,_loader,_runtime,_extensions}`: production config/cache loading, native static composition, runtime registration/reload/source cleanup and retained private modifiers. Standard/catalog-only providers exclude runtime Managers; special providers keep their separate source rule. |
| `packages/coding-agent/src/config/model-provider-discovery.ts` | `model_registry_discovery` and provider factory/manager seams: cache/discovery publication, shared payload mapping, selected refresh and native failure retention. |
| `packages/ai/src/auth-storage.ts` and existing ARA account Host contract | `OpenAiCodexRegistryCredentials` plus `OpenAiCodexAuth`: stable owned snapshots, native no-session peek, complete-account directory union, exact-row refresh/fencing/unknown-outcome settlement and exact-token optional account identity. Normal daily requests retain LatestInteractive selection. |
| Fixed main/print disposal chain and existing ARA CLI contracts | `main.rs`, `daily_model_config`, request factory and RPC maintenance caller: one production Registry; effective resumed cwd before helpers; pure authored ownership admission; one shared account service; owned helper/auth settlement without awaiting the entire background catalogue on print exit. |

Shared Core and product state boundaries are unchanged. The 57 unrelated
vendor/EOL files remain byte-identical; `.codebase-memory/` is excluded.

## Executed checks and repairs

One-click module entry:

```text
python -X utf8 scripts/verify_registry_composition.py --module all --output <local-output>
```

The runner exposes 15 existing/grouped families. Development reuses passed
families and checks changed ones; the final snapshot gate covers every target.
Receipts are under `C:/Temp/ara-registry-compose-batch/production-registry`:

- `203141Z`: 164 PASS/two Host FAIL. Auth ownership rejection occurred after
  helpers; resumed helper cwd was launch cwd. Both are repaired with the original
  assertions. `204325Z` Host group passes 5/0.
- `204517Z`: daily CLI 5/1 exposes unnecessary account-db creation by ordinary
  invalid config. Ordinary routes without stored accounts now use the default
  Env credentials; existing/selected accounts still share the same service.
- `204755Z`: selection 8, daily CLI 6, loader 5, discovery 6, runtime 4,
  extensions 1, references 1 and production Registry 4 PASS. Account group 7/1
  exposes a seed-data premise error: same email/org/JWT profile replaces a row.
- `205101Z` retains the failed explicit-email-only correction. Initial explicit
  and JWT identities now both distinguish accounts; store behavior and all
  row/grant assertions remain intact. `205158Z` accounts passes 8/0. The nine
  remaining families jointly record 43 passes, not a substitute for the final
  snapshot gate.
- `gate-205227Z`, `205434Z` and `205553Z` stop at Clippy before tests. Mechanical
  alias/Option/struct/slice/lock-scope repairs retain behavior and assertions;
  grouped Clippy with `--keep-going` passes. `gate-205641Z` records 885/1/20:
  readonly resume lookup ran before ProxyAuto's existing resume prohibition.
  The original pure Proxy validation now precedes journal lookup. No assertion
  is weakened and no failed external task is replayed.

Above shorthand uses the prefix `module-20261002T` or `gate-20261002T`.
The final gate directory is `C:/Temp/ara-registry-compose-batch/gate-20261002T210427Z`:

| Command | Result | Seconds |
| --- | --- | ---: |
| `python -X utf8 scripts/verify_backend.py` | Owned fmt, all-feature Clippy, all-target/doc PASS; **1,707 passed / 0 failed / 20 ignored** | 308.882 |
| `python -X utf8 scripts/omp_inventory.py check` | PASS | 0.451 |
| `cargo deny check` | FAIL: existing `fontdue 0.9.4 -> ttf-parser 0.25.1`, RUSTSEC-2026-0192 unmaintained advisory | 2.606 |
| `cargo build -p ara-cli --all-features --locked --bin ara` | PASS | 0.442 |

Total 312.382s. All 35 source hashes match before/after and the candidate
manifest. No dependency-policy exception is added; **the whole gate is FAIL**.
The 20 ignored checks are not claimed executed. Binary SHA256:
`7817f93b6ab58f23e1779d545f9b238d3fc6ee280144c6c75f443a9ac42b485e`.

## Bounded actual CAS task

`live-20261002T211014Z/receipt.json` passes in **14.278s/five calls** on
current Manager-configured CAS, `deepseek-v4.1-flash`, `openai-completions`.
The local management proxy is unavailable; its failed catalogue probe is
retained. Private `用户.ry_switch` is read without modification or persistence.
Bounds are two processes, five model calls, 90s/process and 2,048 output
tokens/call. Native tokenizer selection has no `none` override.

- Four calls perform exactly read(invoice.csv), write(totals.json), then
  read(totals.json), with three successful actual tool receipts.
- Actual JSON is `{"cable":65,"bracket":24,"grand_total":89,"shipment_label":"Registry-6923"}`;
  SHA256 `6207a4252420e94ced297eb1cbe07a8b3f645c4703f49a38935529c3ae7f8deb`.
- One tool-disabled original-Session call recalls the exact JSON, preserves
  original entries and leaves the file hash unchanged.
- Five incoming request key/header proofs match; four helper receipts show
  one key/one header execution per process in effective cwd. Keys/helper results
  stay outside public output/journal. No request is active at process end.
- Actual SSE reports 16,088 input/368 output tokens. Monetary cost is unknown.
- Source and binary hashes match before/after and the final gate. Root separately
  reads the real artifact, tool receipts and original/current Session entries.

Independent Codex `/root/remote_review` directly reviews fixed-source account,
extension, startup and lifecycle contracts, repairs, raw module/final gate
receipts, 35 scope hashes and all 57 preserved vendor hashes. Code/gate POST
has no unresolved finding and approves only this bounded WIP scope. Final live
POST separately verifies actual CSV/JSON/hash, tool and Session records,
tool-disabled recall, incoming proofs and actual SSE model/usage identity.

## Remaining mandatory work and progress

Full AuthStorage usage/backoff/ranking/sticky/session pins, in-memory blocks,
stored login/static-key fallback, complete selectors and execution projection,
advanced account transports, actual OpenAI authorization/model task, Bun/Linux
and deferred Providers remain required. These tests do not accept those areas.
The original [composition evidence](model-composition.md) retains its history.

Counts remain 111 surfaces: 26 implementing, one tested, 84 open, zero complete
acceptances. Started coverage is 27/111 (24.3%). Recorded bounded points remain
35 limited acceptances/23 implementing/two tested (60 total); 58.3% is not an
overall completion rate. P0/V1 accepted, P1–P6 open, RPC 27/42, full marker null.
This batch starts at 17:52:41 UTC and reaches actual validation at 21:10 UTC
(about 198min), before final documentation/Git closure. Full-project ETA lacks
a measured basis. Continue module groups, stable gates and bounded live trials.
