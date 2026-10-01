# Model discovery, factories and manager — tested WIP

Fixed OMP: `596f2da7101178214aa27a753529d15e6b7ad91d`.
ARA base: `e1036bef55854a74d5c798239b5667832eb4329b`, `dev`.
Final code identity: before/after file SHA256 pins in
`C:/Temp/ara-model-discovery-batch/module-20261001T040447Z/receipt.json`.
The pins are unchanged and match the delivered worktree. This is a Host
dependency checkpoint, not full registry/AuthStorage/Codex/product acceptance.

## Source and Rust ownership

| Fixed source | Rust owner / observable boundary |
| --- | --- |
| `packages/catalog/src/discovery/{openai,codex,cursor,devin,gitlab,google*}.ts` and wire helpers | `ara-cli::catalog_discovery` and submodules: request/auth/header/body normalization, timeouts, rejection/fallback/logs and result ordering. |
| `packages/catalog/src/discovery/protobuf.ts`, Cursor/Devin wire schemas | `catalog_protobuf`, `catalog_proto_schemas`: all 635 schemas, 46 enums, native bytes/BigInt/UTF-16/reference callbacks and public codec APIs. |
| `packages/catalog/src/provider-models/*`, descriptor registry | `provider_models`: 72 descriptors, including 63 actual callable factories; shared catalogue/runtime callback identity. |
| `packages/catalog/src/model-manager.ts` and cache | `model_manager`, lossless `model_cache` companion: sources, timestamps/staleness, authoritative/additive discovery, static fingerprints and reference identity. |
| Bundled identity and reference mapping | `model_identity_wire`, `bun_hash`: native undefined/non-finite/UTF-16 values and source identity rather than JSON round trips. |
| `packages/utils/src/tls-fetch.ts` | `catalog_extra_ca`/reference carrier: shallow aliases, source fields, holes/prototypes/opaque/cycle handles; same-path filesystem invalidation. `catalog_tls` supplies actual TLS/HTTP effects. |

Transport uses a discovery-local Hyper client. The final repair restores the
workspace reqwest features to their previous values, so adding discovery
decompression does not alter other model clients. Cursor H2 uses captured
startup trust, bypasses the dynamic fetch wrapper and preserves raw response
bytes. `main` captures the process startup CA before parsing CLI arguments.

## Executed module and final gate

```text
python scripts/verify_model_discovery.py --inputs C:/Temp/ara-model-discovery-batch/module-inputs.json --output C:/Temp/ara-model-discovery-batch --full
```

The one-command runner compiles once and invokes the eight native integration
targets with their retained source inputs, then runs the final backend gates.

| Check | Actual result | Seconds |
| --- | --- | ---: |
| Shared compilation | PASS, locked manifest/lock | 54.471 |
| Discovery/descriptor/factory/manager | 692 original-source cases, guard faults and 3 native regressions; 5 tests PASS, mismatches `[]` | 15.584 |
| Protobuf | 3,388 original-source cases, corpus faults and public callback regression; 3 tests PASS, mismatches `[]` | 3.298 |
| Actual transport | 8 tests PASS: None/empty TLS, 126 redirects, compression, byte headers, cancellation and error body | 0.224 |
| Native CA filesystem | 3 tests PASS | 0.033 |
| Original CA reference helper | 58 cases PASS, mismatches `[]` | 0.178 |
| Actual CA HTTPS and same-file trust rotation | 1 test PASS against unchanged source wrapper | 1.063 |
| Actual TLS options | 23 + 8 + 4 retained source scenarios; 3 tests PASS | 0.354 |
| Cursor startup H2 trust | 5 scenarios / 18 calls / 10 server requests; 1 test PASS | 1.030 |
| Full Windows backend | fmt/all-feature Clippy/all-target/doc tests PASS, 1,469 passed / 0 failed / 20 ignored | 118.391 |
| Fixed inventory | PASS | 0.558 |
| Dependency policy | cargo-deny PASS; policy not relaxed | 3.040 |
| CLI build | PASS; binary SHA256 `048c6048becf36c26b45bd132c7ec1880d5167fd21f9e6df15f46840025b0e1a` | 0.371 |

Module execution is **25 passed / 0 failed / 0 ignored**, total pipeline
**198.755 seconds**. The full gate's 20 ignored external-artifact tests are
reported separately; this module explicitly executes its applicable ignored
tests. These counts do not stand for full OMP inventory acceptance.

Frozen discovery artifact SHA256:
`3635ce1c92541fff971733e41a977d2ddef392c9fd28362ac49498d1e4e6b045`.
Its families are factory219, descriptor190, manager51, gitlab47, codex38,
devin31, google-headers31, openai24, cursor18, gemini18, antigravity14,
gemini-cli11. Protobuf artifact SHA256:
`177d8560a6c24b3ef51adf689efc7e9ecfb0c872b7bfbbeab065fd4b2e0b0068`.
CA helper artifact SHA256:
`a99292cfbc7806865edfcb730b7dfa15cc51cfe16184b85a0435dfd3763954b9`.
Original source/runner/license bytes and unique UTF-16 case IDs are checked.
Expected values were not changed to fit native output. Earlier guard/adapter/
lint failures and superseded passes remain in the receipt root.

## Independent review and important repairs

Independent Codex reviews used the project Git/Core procedures. The final
catalog review is `catalog-final-independent-review.json`; the factory/manager
review is `model-policy-port-independent-review.json`, under the receipt root.
The previous 13/32 source review was extended across its remaining files;
review receipts list exact target hashes and exclusions. Schema regeneration
independently reproduces all four generated files byte for byte.

Source-backed fixes include actual Arc/Weak transport identity for models.dev
sessions; nullish versus explicit empty static models; eager Promise.all failure
and continuing siblings (including Codex account header failures); original
Ollama Cloud null diagnostics; skipping own undefined Umans values; the dated
GitLab mini upstream identity; native extra-CA fetch replacement; and actual
TLS option timing/CA replacement/client material/TLS13 defaults.

Final review found public protobuf repeated callbacks held the same array mutex
while invoking a custom codec. Six unchanged-source probes establish dynamic
for-of encoding and initial-length Array.map JSON with later replacements.
The repair reads each element under a short lock and releases it before the
callback. Its module regression checks reentrant reads and mutation and fails
promptly rather than hanging. The final full/module gate follows this repair.

## Bounded real OpenAI-compatible task

Receipt: `C:/Temp/ara-openai-priority-trial/20261001T041503Z/receipt.json`;
parent-inspected artifacts/journal/usage: `accepted-observations.json` beside it.
The current catalogue confirms `deepseek-v4.1-flash`, protocol
`openai-completions`, through the local Manager → OMP management → CAS path
`http://127.0.0.1:18799/cas/v1`. Credentials stay private in the local config
and process environment; no installation/config file is changed.

The actual CLI reads a CSV, writes the correct JSON totals, reads them back,
then exits and resumes Session `01a0f5ac-4610-7233-926a-c435bc1a4ebb` and recalls
the original inputs without tools. Both Runs exit 0. Model calls: 5; successful
tools: read/write/read. Task 4.700 s, resume 1.254 s. Observed usage across the
five replies: input 3,836, output 301, cacheRead 12,800, totalTokens 16,937;
cacheWrite/reasoning total/cost are unknown where fields are absent.
Bounds: 2 Runs, at most 8 calls/180 s each, 8,192 output tokens per call.
Output SHA256 `0ea48896a15edf6b5eb0343122b74c6585aa9de0e786f68e98d3023741a971d9`.
This proves the existing explicit custom Chat route and native continuation;
it does not prove the new disconnected model registry or OpenAI account login.

## Acceptance boundary and next work

User decision 2026-10-01 changes Provider priorities: custom OpenAI-compatible
routes, then OpenAI account login, then other compatibility. See the
[provider plan](../provider-plan.md) for the deferred register, including full
cipher/BoringSSL compatibility and the open Ollama Retry-After date finding.
They remain visible and are not marked ported. No full AuthStorage, OAuth,
main registry, live metadata/journal adoption, Linux gate or product acceptance
is claimed by this catalog checkpoint. P0/V1 remain accepted; P1–P6 are open;
RPC remains 27/42; `ported_through_commit` remains null.

The batch began approximately 2026-09-30 23:58 UTC. Final gate completed around
04:08 UTC; about 250 minutes, with later finalization tracked in the handoff.
Reused fixed source/Bun/artifacts, parallel scoped review, one final module/full
command and one actual CAS task. No additional detail tests are required for
the ordinary mapping repairs in this checkpoint.
