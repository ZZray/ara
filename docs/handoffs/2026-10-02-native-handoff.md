# Native handoff checkpoint — 2026-10-02

Base `bb25c1f65d684e76459b4ec1a363fd123b8c2991`, branch `dev`, fixed OMP
`596f2da7101178214aa27a753529d15e6b7ad91d`. See
[evidence](../evidence/native-handoff.md) and `CTX-HANDOFF-01` in the
[ledger](../upstream/feature-ledger.md). This remains bounded WIP, not full
handoff/Core/Host/phase acceptance.

## Completed execution — reuse, do not replay

Receipts: `C:\Temp\ara-handoff-batch`.

- Stable `module-20261002T092835Z/receipt.json`: modules 147/0/0, backend
  1565/0/20, fmt/Clippy/doc/inventory/deny/build pass, 461.085s, source unchanged.
- `host/run-20261002T093246Z/receipt.json`: four actual Host families pass;
  threshold/incomplete × Abort/EOF settle in 2–7ms without publication,
  fallback or primary request. Unpublished incomplete recovery is restored.
- `live/20261002T094530.121798Z/receipt.json`: preferred CAS
  `deepseek-v4.1-flash`, verified private Manager-configured direct CAS route
  because local Manager gateway was unavailable; five calls in 27.480s.
  Actual write/read produces notebook 67.5, cable 54.0, total 121.5 and label
  Maple-9307; no-tools same-Session reopen recalls all values and preserves JSON.
- `live/20261002T094018.550649Z/receipt.json` remains FAIL: one marker quote in
  a warning, no tools/effects, known settled outcome. Both attempts total six
  actual calls. Never relabel or resume/replay the failed Session.
- Independent `/root/ctx_oracle_diff_review` final POST:
  `PASS_WITH_EXPLICIT_COVERAGE_LIMITS`, H1–H4 closed; Root refreshed
  `root-audit.json` binds 24 repository hashes, final binary and nine artifacts.

Implementation and validation: about 08:55–09:46 UTC, 51min. Documentation/Git
closure is separate. Full gate 7m41s includes shared all-target rebuild 4m39s.
Use module fixtures and one stable full gate; do not repeat passed tasks for
documentation edits. `verify_backend.py --help` starts its gate; do not use it
as a harmless help probe.

## Required next work

Remote 120–240min, snapcompact 180–300min and frame rescue 30–60min net
engineering for these modules only, excluding gates/waits. Reuse existing
fixed-source inventory and native fixtures; Root owns Cargo, private routes,
network calls and Git writes. Provider priority: custom OpenAI, then OpenAI
account; other adapters remain recorded required later work.

Handoff gaps: thinking effort default/high/off/model clamp, concurrent manual
RPC, new/switch/branch reader interruption, speculation/deferred maintenance,
other native transforms/obfuscation/payload hooks and full multimodal/raw-role/
legacy behavior. Exact handoff postpublication durability injection NOT RUN;
generic phase-aware writer faults pass. Linux/account real-task gates remain
uncovered. P0/V1 accepted; P1–P6 open; RPC 27/42; full marker null.

Preserve unrelated vendor stat/EOL WIP and `.codebase-memory/`. Commit only the
manifest's 24 source/test/runner paths and this batch's evidence/plan/knowledge
documents. Scoped commit/push is authorized; no deployment or history rewrite.
