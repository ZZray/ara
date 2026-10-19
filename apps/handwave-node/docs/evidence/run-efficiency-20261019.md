# Small HandWave Run-efficiency increment

Requirement: modestly strengthen the compact standalone Agent's use of existing
tools, knowledge and skills. Full ARA/OMP parity, new UI, permissions, cloud
configuration and deployment are excluded. Base is
`cad595debd7a3e64a487fea36ea9e04a4a61d277`; this is an unpublished worktree candidate,
not a replacement for the previous 0.2.7 production publication.

## Artifact and behavior

Windows x86_64 static-CRT release EXE: **5,355,520 bytes**, SHA256
`92470f7ab8cce9400e75ede12414466557e3e77f07bee448a32993d91101dabc`.
This is 5,632 bytes above the published 0.2.7 EXE; no dependency was added.
Evidence root: `D:/WorkSpace/tmp/handwave-efficiency-20261018/`.
`handwave-node-candidate.exe`, `source-pins.json` (42 build-input files),
`fixture-pins.json` and `artifact.json` bind the candidate and executed helpers.

- `src/agent.rs`: append a fresh local Run-budget observation to a cloned ordinary
  provider Context, after summary calls and before reservation. Remaining calls
  include the next call. Full-request overflow omits only this optional carrier.
  Raw history, summary input, Goal/root budgets and admission gates are unchanged.
- `src/environment_prompt.md`: prefer supplied directories and exact returned
  paths, reuse successful probes, avoid unrelated private-directory discovery,
  and finish honestly when allowance is low. Guidance does not grant authority.
- `src/skill_library.rs`: successful create/install returns final `bundle_dir`;
  hash, status, ownership, licenses and no-implicit-execution contract remain.
- Focused unit/Host fixtures validate these boundaries. The shared legacy fixture
  responder view omits only the final exact budget carrier; captured wire requests
  stay intact. Modes fixtures now assert the current explicit authority denial.

## Executed checks

All Cargo commands use `--manifest-path apps/handwave-node/Cargo.toml`, independent
of the dirty root ARA/OMP workspace. No root Cargo build or root-source edit ran.
Locked/offline builds use two jobs and existing caches. The relevant commands
and their actual results are:

| Command/check | Result |
| --- | --- |
| `cargo fmt --all -- --check` | PASS |
| `cargo clippy --workspace --all-targets --all-features --locked --offline -- -D warnings` | PASS; 29.75s, `cargo-clippy.log` |
| `cargo test --workspace --all-targets --locked --offline` | 203 PASS, 0 failed, 1 ignored public-network check; build 179s/tests 7.08s, `cargo-test.log` |
| `cargo deny --offline check` with existing policy/lock | Advisories/bans/licenses/sources PASS; duplicate warnings retained, `cargo-deny.log`; offline database freshness not guaranteed |
| `cargo build --release --locked --offline --target x86_64-pc-windows-msvc` | PASS; 87s, `cargo-release.log` |
| Python `test_budget_fixture.py` | 2 PASS; valid tail only, original-wire preservation, malformed/incorrect counters, ordinary user/steering and tool pairing |
| Actual `verify_environment_library.py` | 5/5 PASS, 32 requests, 41.191s; actual create/install paths, execution, restart/new-Session reuse, retirement, Plan and fresh budgets |
| Actual `verify_context.py --max-seconds-total 240` | 15/15 PASS, 198.594s; summary-call budget consumption, provider-only metadata, raw/summary separation, memory adoption/revocation, cancellation and fault paths |
| Actual `verify_host.py --modern-hub --proxy --catalog-variants --max-seconds-total 240` | 27/27 PASS, 23 requests, 77.844s; compilation/artifacts, continuation/restart, cancellation, partial stream, protocol and proxy boundaries |
| Actual `verify_web_research.py` | 5/5 PASS, 12 requests, 20.655s; HTTP/download/hash/Plan behavior |
| Actual `verify_modes.py`, v3 | 5/5 PASS; Plan rejection, busy incompatible-mode denial without extra HTTP, Goal artifact/restart/pause/review/explicit accept and cancellation |
| Scoped documentation links / Python compile / scoped diff check | PASS; 4 changed pages, 62 local links, no broken targets; fixture Python compilation and `git diff --check` clean |

These 57 actual Host groups use the exact release EXE and a local signed Go Hub
with a deterministic provider. They establish covered effects and fault behavior,
not autonomous model intelligence. Unit cases cover the last-call/max=1/exhausted
states and exact full-request boundary. This binary-only crate has no separate
documentation-test target. Linux/macOS follow-up and production UI are untested.
The required root `scripts/verify_bootstrap.py` was attempted, but its unbounded
whole-repository Markdown traversal produced no result in 146s and was stopped.
The scoped link check above passed separately; it is not a full bootstrap PASS.

## Retained failures and actual-model observation

Modes v1 failed because the old Plan set omitted seven existing read-only tools.
V2 then exposed its older silent-follow-inheritance expectation. V3 requires the
current transport's explicit rejection and completion of the unchanged original
Run; no product permission gate was loosened. Both failed directories remain.

Exactly one bounded actual-model trial ran against the freshly verified
`deepseek-v4.1-flash` / `openai-completions` catalog through the existing read-only
Gateway route. It used isolated local Node/Hub identities and temporary home
variables, with no production grant, install or configuration mutation. Limits
were unchanged: per task 20 calls/180s/1M conservative reservation, aggregate
28/300s/1.5M, output cap 4096. Isolation is not an OS sandbox.

`live-project-two-sessions/result.json` remains **FAIL**, reuse **NOT_RUN**.
It forwarded 12 HTTP calls, reserved 697,095 conservative UTF-8/output units and
took 78.048s overall. The first task returned a real successful terminal/final
response in 73.096s. Independent review of the actual native call/result, current
bundle/hash, note/index and artifacts confirms a narrower observation:

- The installed Python script actually ran through `subprocess.run` with its
  stored resource path. `first-summary.json` is valid=6, invalid=2,
  INFO=3/WARN=1/ERROR=2; both original input hashes remain unchanged.
- Same-Run alternate input, fixture comparison and same-input/output rejection
  are supported by actual execution/output. Skill evidence matches the successful
  execution receipt and current bundle hash; the project note matches its index.
- The harness's direct-Python invocation recognizer missed Python orchestration,
  leading to its failed assertion. This does not turn the raw trial into PASS or
  provide any evidence of independent new-Session autonomous reuse.
- Two PowerShell failures and unnecessary probes remain in the actual transcript.
  The captured budget is current at every call (reserved 0..11, remaining 20..9);
  all are `working`. This trial does not test model behavior in closing/last-call
  phases and does not establish that the new hints caused any speed/token saving.

No budget increase or second model run was used to replace this result. Previous
four failed model trials also remain unchanged. Controlled restart reuse and
actual-model first-Session completion must not be substituted for the open
new-Session autonomous-reuse gate.

## Review and delivery boundary

Independent Codex `/root/environment_evolution_review` performed PRE and POST
using `ara-git-review`, `ara-rust-core-review` and bounded point-audit procedures;
reports are `review-pre.md` and `review-post.md` in the evidence root. The reviewer
checked actual source, complete-request fallback, shared counters, Plan/private
memory/Goal authority and receipt paths. Final judgment is **APPROVE / ACCEPT this
bounded local Windows point**, with the autonomous-reuse and publication limits
retained. Final audit and compact pins are linked
from the versioned [acceptance receipt](receipts/run-efficiency-20261019/acceptance.json).

This candidate is not committed, pushed, installed or deployed. Existing product
authorization and the previous publication remain unchanged. Full OMP alignment,
general self-improvement, model cache savings and autonomous cross-Session reuse
are not accepted by this change.
