# Tool-call loop guard handoff — 2026-10-01

Parent `b3c84d5`; fixed OMP `596f2da`. See
[executed evidence](../evidence/tool-call-loop-guard.md).

## Current module

- Fixed detector and original 10 native tests, reused as one Rust corpus.
- Long-lived RPC Host detector: default enabled, threshold 5, exempt `hub`;
  retains state across prompt/new/switch and rebuilds on settings change/disable.
- Awaited Core completed-turn input hook persists the native notice before
  terminal decisions and user steering; existing permissions/queues stay bound.
- Tool/Gemini notices reuse existing native formatter. Old Gemini journal
  restoration is strictly limited to its exact earlier ARA template.
- UTF-16 refusal retains lossless error details and cancels both Run tokens,
  preventing automatic retry. Full lossless model-text projection remains
  mandatory parity work.

Final one-command gate: modules 20/0/0, Windows backend 1,503/0/20,
inventory/deny/build PASS, 194.995 seconds; 152 source pins unchanged.
Module suites take 4.177 seconds. Preferred real CAS task uses two controlled
read turns followed by four actual model calls; file computation and tool-disabled
original-Session recall pass in 9.011 seconds across Runs. One native notice,
four actual tool results, original Session/binary/source identity are audited.
Independent Codex review and Root audit retain the stated gaps. This is bounded
tested/audited WIP; complete parent surfaces remain open.

## Next

1. Overflow/interrupted/native special-stream-cap recovery in coherent modules.
2. Remaining RPC variants and registry/execution metadata.
3. Mandatory lossless model text, advisor and other Host/stream-guard adoption;
   no unsupported path is removed from complete OMP reproduction.
4. Actual OpenAI account authorization/model task when available; other
   Providers follow the explicit deferred register.
5. P3 parity before new ARA capabilities and product-specific customization.

P0/V1 accepted; P1–P6 open; RPC 27/42; full parity marker null.
Do not repeat this full gate or paid task after compaction on unchanged source.
Root owns Cargo; independent reviewers remain read-only.
