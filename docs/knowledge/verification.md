# Verification boundary

Every point moves through `planned → implementing → tested → audited → accepted`. A committed file or passing compilation does not skip a gate. The applicable source-backed fixture, actual Rust entry point, negative case, state/artifact inspection, and audit must run on the delivered snapshot. [Acceptance rules](../acceptance.md) define the receipt format.

Use a controlled fake model upstream for deterministic protocol faults, budgets, tool cycles, partial streams, cancellation, and restart. A fake Gateway client cannot prove the real Gateway/authorization route. For selected integration points, run a bounded real task through the current ARA Manager → OMP management → CAS route or a currently listed OpenRouter free model. Check live model ID, protocol, authorization, output, tool effects, duration, and usage. A previous route URL or a successful `/models` request is not current availability evidence.

API keys are secret even when a model is advertised as free. Keep them in environment variables or CI secrets and redact outputs. Never commit local profiles, account data, model credentials, or captured request headers. If a mandatory route cannot be exercised, report the precise missing evidence and keep that gate open.

The documentation bootstrap verifier establishes structure and link integrity only. It does not test an Agent Core, OMP parity, or a product integration.

**User decision, 2026-10-02:** reuse fixed OMP native test input families and
verify coherent modules during implementation. A stable batch shares one full
backend command; ordinary low-impact details do not require additional micro-tests.
Retain meaningful protocol, cancellation, tool-effect and persistence failure
coverage. Source and artifact hashes allow unchanged passed evidence to be
reused; rerun only checks affected by a fix or an outstanding required gate.
Group shared dependency/interface changes before the full gate to avoid repeated
all-target compilation. Report implementation, compilation/gate and live task
time separately. [Handoff evidence](../evidence/native-handoff.md) records the
measured 461.085s gate, including 4m39s all-target compilation.
