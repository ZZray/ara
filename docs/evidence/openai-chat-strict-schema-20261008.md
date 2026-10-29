# OpenAI Chat Native strict/schema batch — 2026-10-08

Current follow-up: the historical CLI flag binding gap below is exercised by
the [2026-10-11 Host NO_STRICT batch](openai-no-strict-20261011.md). Full parent
and the separately recorded remaining contracts are still OPEN.

**WIP. Execution-v3 SOURCE approved; original two-group gate 55/55 PASS and
bounded current-binary CAS task/restart PASS. Final independent POST PASS;
complete Chat/provider/OMP parents remain OPEN.**

## Fixed source and scope

OMP `596f2da7101178214aa27a753529d15e6b7ad91d`:

- `packages/ai/src/providers/openai-completions.ts:2416–2495`: original tools
  map/adaptation/every/for-of, strict mode and explicit false, flavor, quarantine.
- `packages/ai/src/utils/schema/normalize.ts:1405–1415,1497–1584`: shallow-frozen
  module literal and grammar walker over genuine schema slots.
- Chat `533–574,741–858` and shared `502–540,1271–1327`: Session scope and exact
  grammar/strict rejection predicates, once-only rebuild from original Context.
- Catalog `compat/resolve.ts:308–344,545–551`: Host materialized flavor facts;
  native Moonshot/Kimi precedes grammar. No provider-name inference added to AI.

Keep v3 policy/CLI fixes and unrelated WIP. Current dirty base is `dev`, HEAD
`20aa0e2ef9872f633ba2671376cdf778477e7115`. Git index remains
`89d34e26be97c90a14680d44ace937e6b1ae5a86e8ee7b46dd6b90718a922475`.
The `/new` EditStore clone remains. Root alone changes live source/fmt/Cargo.

Fifteen direct paths include Chat request/schema/callbacks, materialized policy,
two borrowed Responses/schema-wire facades, one fixed module literal in the
existing value heap, CLI fresh binding, and observations in original HTTP/route
groups. Anthropic's sole hunk fills a new private PostError test field.
Responses algorithms, transport ownership and typed callbacks stay as before;
raw rejection body capture is private/transient with a default no-op consumer.

## Candidate and review

D packets share `D:/workspace/env/cache/ara-port-candidates/`:

- Author packet: `chat-strict-schema-batch-20261008`.
- First candidate manifest: `0c7e633ef1b35123c04ccf60b05bca8319673d29e3fae05a5945df3e81a7b79c`.
- Corrected candidate v2 manifest: `494765af485b9c495ea15e6e82a443496a24b0986bdb079f583af5dac3a40787`.
- Independent reviewer: Codex `/root/finish_edit_renderer`, applying
  ara-git-review/ara-provider-review. Report: `chat-strict-schema-review-20261008/REPORT.md`.

The reviewer found one real omitted Native behavior before compilation:
quarantine warning interpolation must convert original `tool.name`, even without
a Host logging port. v2 retains original Get, ToPrimitive with string hint,
implicit Symbol TypeError and ToString; Host receives the converted WireString.
Existing coarse observations cover exactly-once conversion with/without a port
and the original thrown object receipt with zero POST. Original v1 bytes and
15 candidate files remain in `v1-frozen`. SOURCE approved the correction.

Root read all direct production deltas, new schema/support, fixed source blocks
and the corrected delta. Structural graph generation 2026-10-03 is stale or
untracked for these paths; current fixed/live/candidate source is authoritative.

## Actual execution history

Initial execution packet: `chat-strict-schema-execution-20261008`.
Root validated all 13 existing baseline hashes and two absent new paths, applied
the exact candidate, then scoped fmt **0.738 s**. All source changes were within
the 15-file allowlist; index and unrelated source unchanged.
Delivered manifest:
`229591f5aefd907226bb970ab935bf58cfb0aac648bf8761cfa92f8a2d67f083`.

The actual build failed: **exit 101, 19.822 s, 400 reused / 0 compiled,
3 errors**. All three E0609 errors were `arena.intrinsics` in the new literal.
The existing `ValueHeap` and `module_string_array` place intrinsics on `self`.
No test group ran. Original stdout/stderr and failed build receipt remain in
that packet; failed build SHA256:
`3c8859003e698855a9778745a5abd6f35f74750c760b58ace98dce9d61d3898f`.

Repair execution packet: `chat-strict-schema-execution-v2-20261008`.
Root changed only the three references to `self.intrinsics`, retaining literal
identity, lock ownership, root freeze and nested mutable descriptors. One-file
fmt **0.193 s**; all other source and index unchanged. Fifteen delivered pins,
one delta; delivered manifest:
`f71afe267f9b0a0cf3b4769b764fc3d7b36fce2b23fa28882553ea6220cb3fe7`.
The same reviewed runner is byte-identical. This second build failed with
**exit 101, 56.364 s, 400 reused / 4 compiled, 2 errors**: E0277 because the
containing PostError requires CapturedHttpError: Debug, and E0369 because the
containing ReasoningCompat requires NativeChatSchemaPolicy: PartialEq. Zero
groups ran. Original diagnostics/receipts remain in execution-v2; build receipt
SHA256 `e88eb6404175f22c2296cf2f33e050ad1af73641a89b1457d6cfbd76de3d0008`.

Current packet: `chat-strict-schema-execution-v3-20261008`. Root repaired only
those two obligations: Debug shows a fixed private-body placeholder plus JSON
presence, never raw contents; PartialEq applies to the existing materialized
policy fields. Scoped fmt **0.261 s**; fifteen delivered pins, two delta paths.
Delivered manifest SHA256:
`811a513e4c4f32a8bfc40cefadd558e31a285b5fbbfb8ca04ba272759bcc0257`.
Independent Codex `/root/finish_edit_renderer` directly checked the 15 snapshot/
live pins (zero mismatches), two deltas and unchanged index/runner, and returned
**SOURCE APPROVE**. `SOURCE-EXEC-V3-FROZEN.md` and then-current `REPORT.md` hash:
`07f1deae9d642e3577276c253cbd431c3f192589b1d6fbdf81ac88b02b6e4c1c`.
The affected execution-v3 build and original groups finished on their original
process; no gate was restarted. Source stayed frozen.

## Execution-v3 deterministic and real-model results

Actual affected build: **exit 0, 418.584 s, 404 reused / 11 compiled,
59 warnings / 0 errors**, source_unchanged=true. Original build receipt SHA256:
`acbb4e066fa70779fe28dbf3e6f7c54653a3f25ae893d3cf46e3bb9f6e2a2638`.

| Original group | Actual outcome | Seconds |
| --- | --- | --- |
| Full `openai_http` | 54 passed / 0 failed / 0 ignored | 36.498 |
| `codex_side_calls_preserve_cache_prefix_and_isolate_transport_state` | 1 passed / 0 failed / 0 ignored | 0.177 |

Module total **458.184 s**, source_unchanged=true; actual expected names and
executable pins match. Module receipt SHA256:
`be5b6e05a5fb686e827d47c3973cb3f02a91a659cd1e3717e167d32beb41fa06`.
Build produced current CLI binary SHA256:
`1fc084c1ab1ca8c4b6d45257212045c3dddb8729f5f6018fbf0329c86d9352a1`.

After SOURCE approval and that gate, Root ran the preprepared
`real-cas-task.py` once (SHA256
`35841052aebb1bed160aa347ad6b7fa4885f845838898af3452011e48e811367`).
Manager local management was unavailable (URLError); private configured CAS
passed its current model catalog check. Exact model `deepseek-v4.1-flash`,
protocol `openai-completions`; retry and model fallback disabled; two Runs,
6/2 maximum calls, 120 s/Run, 2048 maximum output tokens.

The first task passed **6.212 s / 4 calls / 4 tools**: read random `sales.csv`,
edit `summary.json`, then read the edited artifact. Root independently read
the CSV and artifact: 3×2.75 + 1×11.5 + 3×4.25 = **32.5**. The answer agrees.
The second process passed **2.140 s / 1 call / 0 tools** with no answer in its
prompt: original pen quantity 3, book unit price 11.5 and cup quantity 3 were
recalled correctly from the conversation. Both have terminal stop and the same
Session `01a11b3d-cec0-757b-8d90-f7276b5877cf`; one journal retains 11 messages.
Source and binary remained unchanged; Git index remains at the recorded hash.

CAS receipt: `real-cas-receipts/20261008T111953Z/receipt.json` in execution-v3,
SHA256 `f22eb6d5e90e6161a1051770f2925d03a5324aaa941fe20077f4f5837860a3a7`.
Final independent same-batch POST passes. Codex `/root/finish_edit_renderer`
directly read original outputs, artifacts, journal and receipts; all 15 scoped
snapshot/build/CAS/current pins match, CAS before/after has zero source delta,
and all four build executable pins match. The original 11-message journal has
four successful tool results and two terminal stop answers. Final report
`chat-strict-schema-review-20261008/REPORT.md` SHA256:
`ed64f36170bfbacbc413ed571a876813789cff0f5462a52a81802a6d0535fcbe`.
Subsequent separate WRITE delivery does not extend the original whole-tree
execution receipts to that new source. This task exercises current
ordinary Native Chat and persistence; deterministic fixtures separately prove
schema rejection/fallback and Session isolation. It does not prove real
reasoning replay, account login, MFJS, complete WRITE/provider/Chat or product
acceptance. No full-parent inventory state advances from these bounded results.

One Cargo command:

`cargo test --locked -p ara-ai -p ara-cli --all-features --test openai_http --test model_route --no-run --message-format=json`

Original concentrated gate:

| Artifact/filter | Required observations |
| --- | --- |
| Full original `openai_http` | Existing regressions plus strict modes, explicit false, grammar slots/identity, quarantine/Native conversion, exact HTTP predicates, original getter rebuild, once fallback, cancel/accepted-SSE no replay |
| `model_route::codex_side_calls_preserve_cache_prefix_and_isolate_transport_state` | Existing Codex side-call behavior and Native Chat same binding disabled / fresh binding strict / original binding still disabled |

Production dependency libraries compile normally; no unrelated library test
artifact is added. Cargo/Rustup/target remain on the fixed D prefixes, jobs=2,
same profile/features. Source approval is independent from compilation and
execution. Current-binary bounded CAS and final independent POST follow only
after a green deterministic gate. Both gates and the bounded CAS task now pass
for execution-v3; final independent POST also passes.

## Required remaining contracts

Full Chat/provider/TUI/OMP parents stay OPEN. MFJS Native normalization,
dereference/spill/stamps, Kimi K3/Zai special controls and reasoning-effort retry
priority/state remain required at later provider priority. CLI `PI_NO_STRICT`
environment binding is OPEN; this batch consumes a Host-materialized option.
Grammar's 128-schema-level Native RangeError is a documented carrier ceiling;
arbitrary-depth equivalence to an upstream JS engine remains OPEN. Ordinary and
over-limit observations distinguish it. These limits do not remove parity work.
