# Broker server, management and installed OAuth owner — bounded tested WIP

Fixed OMP: `596f2da7101178214aa27a753529d15e6b7ad91d`. Continue AI-AUTH and
MODEL-COMPOSITION-01. Client/store and consumers retain their earlier evidence
in [broker-store](broker-store.md) and [broker-consumers](broker-consumers.md).
No new point or complete surface is claimed.

## Requirement and scope

The fixed auth-broker server/wire and auth-storage source define SQLite owner
publication, remote refresh sentinel, actual routes/stream/long-poll closure,
explicit OAuth refresh and exact-row settlement. Rust source is
`ara-cli::{auth_broker_server,auth_broker_server_snapshot,auth_broker_management,
auth_broker_mcp_refresh,auth_broker_refresher,auth_storage_broker_server}` plus
the existing AuthStorage/credential owner. The common per-ID flight is shared
by ordinary and explicit consumers. Installed providers own issuer HTTP and
original CAS; the facade does not perform a second projection after publication.
A detached owner durably settles a definitive rejected attempted row even if
the ordinary waiter cancels. Full-skew peer CAS survivors are adopted; a short
peer is preserved without being reported usable. Unknown effects are not retried.

Exact approved source packages and application receipts remain under
`C:/Temp/ara-model-fallback-batch/`: `broker-gate-joint-source-20261004T193356Z`
(pins `d10e0468bdcab9ca983df5bbe23f854cbd8005961a9095d4eb495996a822aa5c`),
`broker-custom-owner-overlay-v2` (pins
`ea5b5cd7267f660efdb885510b89445049c30f6d9113698608dede9359333a32`),
and `custom-owner-v2-applied-20261004T202201Z/receipt.json`.
Independent v2 SOURCE review SHA
`9882a3665094d2e2549d3d812dcc26ca51efc77af3925041f181412ad83fa237`.
The earlier ordinary-owner cancellation gap and v1 report remain visible.

## Original execution and affected repairs

| Retained invocation | Result | Meaning |
| --- | --- | --- |
| broker-final-gate-receipts/module-20261004T200441Z | 509/2; compile230.070s, total323.018s | Ten of12 groups passed; storage sentinel and G Echo fixture failed |
| broker-g-fixture-corrective-receipts/module-20261004T201404Z | core-hooks8/0; total26.144s | Echo now uses required text argument; real effects observed |
| custom-owner-v2-receipts/module-20261004T202315Z | compile failure71.012s; zero tests | Fixture called private lease getter |
| custom-owner-v2-receipts/module-20261004T202550Z | compile failure95.300s; zero tests | Root used nonexistent AuthAccess.row; corrected to public identity/oauth fields |
| custom-owner-v2-receipts/module-20261004T202830Z | storage238/0; server2/1; compile62.288s, total78.033s | Exact runtime v2 and remote sentinel pass; joined fixture error still open |
| custom-owner-v2-diagnostic-receipts/module-20261004T203608Z | server2/1; total19.320s | Actual ordinary-first InvalidSchema/status200/outcome_unknown=true |
| custom-owner-v3-corrective-receipts/module-20261004T203859Z | server2/1 | Wire input repaired, later expired recovery blocked on fixture gate |
| custom-v3-backtrace-receipts/20261004T204033Z | same binary fails at test621 in7.079s | No recompilation; exact second refresh waits on consumed semaphore |
| d-foundation-receipts/module-20261004T204408Z | 254/1 overall; server2/1 | Second refresh proceeds; fixture630 misreads parsed JSON as String |
| custom-owner-v5-corrective-receipts/module-20261004T205358Z | server3/0; compile15.270s, total19.787s | All3 server coarse families execute and pass on final test bytes |

The corrective changes preserve the original requirements and time bounds:

- Remote snapshots use `__remote__`, as the fixed wire requires.
- Joined waiter diagnostics now retain case and typed error; success still required.
- Ordinary Merge and expired ACK fixture responses use string projectId; explicit
  usable responses retain null/fallback projection coverage. Native optional
  projectId is a string, not null; no parser or assertion was relaxed.
- Release exactly one permit before the scripted second expired recovery request.
- `ara-testkit/src/lib.rs:167-176` already stores valid request JSON as an object.
  Read second request body.refresh directly, still expecting installed-new-refresh.

Final test SHA `b516e13b87a438fd66880b231ef832719c87f824e14d778e2b56ee376d3ed53e`.
Testkit source SHA `2d57ef60054242c880c816626ec708ac0390f4e5db8983af7f35deb9d17c3001`.
All original failed source/binary/log/receipt snapshots are retained. Earlier
locked/type/disk failures are also retained in the handoff; none ran the module.

## Actual evidence and independent audit

Root used `verify-custom-owner-v2.py --module server --output
C:/Temp/ara-model-fallback-batch/custom-owner-v5-corrective-receipts`, existing
Cargo targets/all-features/locked, jobs1 and test threads2. Final receipt SHA
`babc5bcb048692b7e1f31152798d55866dc1ae1387499d82fdc15a4ccfcbee6b`, source unchanged. The three coarse groups cover real SQLite and
controlled HTTP/device/MCP routes, shared flights, peer CAS, original grant
rejection/cancelled waiter settlement, stream/long-poll and shutdown.

Storage238/0 is reused from its unchanged runtime snapshot; it is not presented
as a newly run combined238+3 batch. Management2, accounts14, auth-command2 and
rpc-observed1 retain the original joint-batch evidence. Later custom changes
were tested in the affected storage/server groups; final test-only accessor
repair reran only server. This is concentrated module evidence, not full parity.

Independent Codex `/root/broker_server_review` APPROVE, SOURCE and bounded EXEC:
`C:/Temp/ara-model-fallback-batch/core-directives-d-custom-cas-independent-bounded-exec-review.md`,
SHA `1c5596c2e0196dd368d6c572d8f384bff19bc32999b71b09629660db598a579a`.
Root traced the actual failed cases to fixed schema, consumed gate and parsed
request body, and checked final returned receipts. Current main binary real-CAS
compatibility is recorded in [D foundation](core-tool-directives.md). It does
not prove a real OAuth/account/MCP authorization or grant refresh. The controlled
fixtures establish only their local issuer-protocol and owner contracts.

## Limits and acceptance status

Bounded installed-provider/explicit-owner and server evidence is approved.
The complete AI-AUTH/Broker/management/Provider surface remains WIP. Real
OpenAI login authorization, other deferred Providers/platform contracts,
full stable dependency/host gate and complete product acceptance remain required.
Original debug startup15s failure stays open. Formal111/60/P1-P6/full marker
are unchanged; no commit, push, install or deployment occurred.
