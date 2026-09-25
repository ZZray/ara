# Per-point testing and acceptance

Code, a green compile, a written test, a model HTTP 200, or a plausible answer is not delivery. **Every implementation point must be executed and checked against its stated outcome before its delivery is accepted.** Run [backend verification](../.ara/skills/ara-backend-verification/SKILL.md) on the exact delivered commit or recorded worktree snapshot when Rust backend code changes. Failed or unavailable mandatory tests keep the point open.

## Evidence for one point

Record this with the change (for example in `docs/evidence/<id>.md`):

| Field | Required content |
| --- | --- |
| Requirement | Observable behavior, source OMP location/commit or ARA decision, and explicit exclusions. |
| Entry | Actual Rust binary/host/API invoked, not only a helper function. |
| Test | Exact command or reproducible procedure, environment, fixed input, expected and actual result, exit status. |
| State and effects | Task/Session/Run IDs where relevant; journal/log/budget; real file/tool artifact or UI result. |
| Failure path | Denied permission, cancellation, malformed/partial response, retry/recovery or other relevant negative case. |
| Model trial | Exact model ID and wire protocol, route, bounded calls/turns/tokens/time, actual answer/tool effects, observed usage and latency; mark unknown values unknown. |
| Review | Diff scope, independent findings, unresolved risks, tested commit, accepted/rejected decision. |

Keep durable knowledge in `docs/knowledge`; test receipts belong under `docs/evidence` and are not substituted by prose claims. WIP commits must say they are incomplete. A point with no executable evidence is `not tested`, not `accepted`.

## Test layers

1. Source-backed unit and protocol fixtures: exact wire/event shapes, state transitions, invariant and negative cases.
2. Real local process chain with controlled model upstream: Rust host → authorization/route → Provider → actual tools → persistent Session/Run/journal/budget → explicit review. Use a temporary data root and inspect produced artifacts and restart behavior. A stubbed Gateway client does not prove the production route.
3. Live observation: for streaming and background work, hold the upstream active and observe output on the client pipe/socket **before** the Run ends. After-exit stdout proves order, not live delivery.
4. Bounded real-model task trial for selected integration points: use the existing **ARA Manager → OMP management → CAS** route when currently available. Verify its live enabled state, exact model ID and OpenAI-compatible/Anthropic protocol before calling. The older local CAS path is not current-state proof. An OpenRouter free model is an alternative: query the live catalogue for a free ID, supply `OPENROUTER_API_KEY` via the environment or a CI secret, and record the actual cost/usage rather than assuming free means zero. Do not commit a key, even a free-tier key.
5. Product test: AI HandWave, Lantern/Paseo, and Lumen each exercise their own host flow while using the same Core implementation; verify identity isolation, user-visible feedback, continuation, and failure paths.

The real-model trial must complete a bounded **task**, not merely answer a ping. Check requested artifact, tool decisions/results, context carried across turns, elapsed time, usage, and whether the goal was met. If CAS or OpenRouter is unavailable, report that boundary and retain the point as open where real-model evidence is required. Controlled upstream fixtures remain necessary for deterministic faults.

## Audit and release gate

Use [audit procedure](audit.md) after the actual tests. Record defects with a triggering input, impact, source location, and evidence. Re-run the relevant test on the repaired commit. Do not modify expectations just to make current output pass.

The full OMP marker, a phase, or a product is accepted only when **all** included rows meet their evidence requirements. The new repository starts with no accepted OMP behavior. Keep implementation progress and formal acceptance separate. Publishing or deploying is a further action requiring explicit authorization and runtime verification; a pushed documentation bootstrap is not a released Agent.
