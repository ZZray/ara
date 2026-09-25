# Verification boundary

Every point moves through `planned → implementing → tested → audited → accepted`. A committed file or passing compilation does not skip a gate. The applicable source-backed fixture, actual Rust entry point, negative case, state/artifact inspection, and audit must run on the delivered snapshot. [Acceptance rules](../acceptance.md) define the receipt format.

Use a controlled fake model upstream for deterministic protocol faults, budgets, tool cycles, partial streams, cancellation, and restart. A fake Gateway client cannot prove the real Gateway/authorization route. For selected integration points, run a bounded real task through the current ARA Manager → OMP management → CAS route or a currently listed OpenRouter free model. Check live model ID, protocol, authorization, output, tool effects, duration, and usage. A previous route URL or a successful `/models` request is not current availability evidence.

API keys are secret even when a model is advertised as free. Keep them in environment variables or CI secrets and redact outputs. Never commit local profiles, account data, model credentials, or captured request headers. If a mandatory route cannot be exercised, report the precise missing evidence and keep that gate open.

The documentation bootstrap verifier establishes structure and link integrity only. It does not test an Agent Core, OMP parity, or a product integration.
