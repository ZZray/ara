---
name: ara-rust-core-review
description: Review Rust Agent Core and backend state-machine changes for reachable async, persistence, permission, and tool-effect failures.
---

# Review Rust Agent Core semantics

Apply after [Git scope review](../ara-git-review/SKILL.md) to the selected target revision. Read the relevant Core/host boundary in `docs/knowledge/architecture.md`. This is semantic review, not a substitute for [backend execution](../ara-backend-verification/SKILL.md).

- Trace `Project → Task → Session → Run → Job` identities through creation, continuation, restart, cancellation, and review. Check that successful Run completion cannot silently accept a Task or create a fresh Session after failed resume.
- Trace model → tool → receipt → journal transitions. Check durable ordering, crash windows, replay/duplicate effects, partial tool success, idempotency, and how unknown outcomes are surfaced. Follow calls into host ports before claiming a Core invariant is enforced.
- For async Rust, inspect spawned-task ownership, cancellation/drop behavior, timeouts, channel closure/backpressure, lock scope across `.await`, shared mutable state, resource cleanup, and process exit. Do not report a race from `Arc` or a mutex alone; show a reachable interleaving and impact.
- Follow the authorization decision to the actual file/process/network effect. Check rooted paths, grants, approval correlation, budget settlement, error propagation, secret logging, and limits of tool allowlists. A host gate is not proven by a Core type alone.
- Check raw reference revisions and compaction summaries separately: provenance, current-revision selection, cross-Run recall, revocation, and untrusted-content priority. Confirm unsupported modalities fail explicitly.

Report source/target versions, affected file/line, trigger, consequence, evidence, and suggested minimal fix. Mark uncertain issues as questions. Ask for focused fault/continuation tests through `ara-backend-verification`; do not claim they passed unless executed.
