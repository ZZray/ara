---
name: point-delivery-audit
description: Verify and audit one bounded ARA Rust implementation point using executed normal, failure, host, and real-task evidence before acceptance.
---

# Audit a delivery point

1. Name the observable requirement, source commit or ARA decision, exclusions, target commit/snapshot, and the files/hunks needed for it.
2. Execute focused fixtures and the actual Rust binary/host path on that snapshot. Include a relevant negative case. For concurrency/streaming, observe the live event before Run completion; for persistence, inspect state and restart behavior. Check requested file/tool artifact and Task/Session/Run receipts.
3. For an integration point requiring a real model, verify the live CAS or OpenRouter route, exact model ID and protocol; bound calls, turns, tokens, and time. Inspect actual task result, tool effects, latency, and usage. Redact credentials. `HTTP 200` alone is insufficient.
4. Record exact commands, expected and actual results, environment constraints, artifacts, and failed/unavailable checks in `docs/evidence/<point-id>.md`. Do not change expectations to fit observed output.
5. Run an independent scoped review where available, then semantic review for ownership, permissions, cancellation, unknown effects, credentials, and regression risk. Re-run affected tests after fixes.
6. Mark `accepted` only when mandatory evidence passes on the delivered snapshot. Otherwise use `changes requested` or `not tested` and keep the ledger/marker truthful. A successful Run is not Task acceptance.
