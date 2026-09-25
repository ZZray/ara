# Real-model test routes

The first repository bootstrap has no Rust model client and has run no real-model task. These instructions apply when a tested Rust integration point exists. A real task trial is bounded by the point's acceptance plan and its receipt belongs in `docs/evidence/`.

## Local CAS

Open **ARA Manager → OMP management → CAS** and confirm the currently enabled route, model ID, wire protocol, grant, and quota. The route and model may change; do not infer them from a historical URL or a `/models` response alone. Supply the verified base URL and key to the test process using environment variables named in [`.env.example`](../.env.example). Record the model ID and protocol in the receipt, with the key redacted. Run a bounded task through the actual Rust host path and inspect the resulting artifact, tool receipts, Session/Run status, usage, and elapsed time.

## OpenRouter free alternative

Query OpenRouter's current model catalog and choose an ID explicitly marked free and compatible with the protocol under test. Set `OPENROUTER_API_KEY` as a local environment variable or CI secret; set `ARA_TEST_MODEL_ID` to that exact ID. Free pricing does not make a key public, guarantee quota, or prove a call had zero usage. Avoid committing a populated `.env`, request headers, or raw traces containing credentials.

## Bounded trial record

Before calling, write the maximum calls, turns, output tokens, and elapsed time for the test. After calling, record exact model ID and protocol, route class (CAS or OpenRouter), sanitized command, actual answer, tool decisions/effects, artifact checks, usage values as observed (or `unknown`), latency, and failure/stop reason. If the route fails, keep the requirement open when a real-model trial is mandatory. Deterministic fake-upstream tests still cover failure paths independently.
