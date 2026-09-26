# AGT-AGENTa: stateful Agent and queue foundation (WIP)

Source is the fixed OMP commit `596f2da7101178214aa27a753529d15e6b7ad91d`: `packages/agent/src/agent.ts` prompt/continue, run state and steering/follow-up queues, plus `packages/agent/src/agent-loop.ts` turn-boundary polling. Related upstream behavior IDs are B-db33717478, B-99897cd1ba, B-61204f7211, B-32d3bbd40f and B-37dc10bb38. This is a partial port of AGT-AGENT and CA-QUEUE, not surface parity.

`ara-agent::agent::Agent` owns one in-memory transcript, FIFO steering and follow-up queues, a single active run and its child cancellation token. Hosts own authorization and durable Session receipts. `prompt` refuses a persisted assistant tail with unpaired tool calls, because their effects may be unknown. A dropped caller waiting for the result leaves the run owned by the Agent; tool effects are not replayed. An idle `continue_run` drains own and host hook queues, preferring steering, and returns messages to its queue if cancellation or deadline is observed after an asynchronous dequeue. The existing free Agent loop and production CLI flow are unchanged.

Windows checks on the candidate:

| Check | Result |
| --- | --- |
| `cargo fmt -p ara-agent -- --check` | Passed. The whole-workspace format check remains affected by pre-existing vendored formatting. |
| `cargo test -p ara-agent --all-targets` | Passed: 28 Agent loop tests, 12 compaction, 8 summary-call, 7 cut, and 4 tokenizer tests. New cases cover concurrent prompt rejection, steering/follow-up order, idle continuation through host hooks, cancellation before and during dequeue, unknown-effect tail refusal on both prompt and continue, run abort isolation, dropped waiter with exactly one tool effect, and the Agent-owned fake HTTP tool chain. |
| `cargo clippy -p ara-agent --all-targets --all-features -- -D warnings` | Passed. |
| `cargo test -p ara-agent --doc`; `cargo deny check`; `python scripts/omp_inventory.py check` | Passed. `cargo deny` reported existing duplicate-dependency and missing-license-field warnings; advisories, bans, licenses and sources were OK. |

Independent Codex pre-plan review recommended this stateful owner over a standalone append-only context wrapper. Independent Codex post-diff review found three concrete problems: a new prompt could replay an unknown-effect tail; busy/cancel/recovery fields had separate publication races; and idle continuation ignored the base host hook queue. All were fixed and re-reviewed. A late steer arriving while a host hook waits now precedes follow-up; a deterministic test covers it. The reviewer read all changed files and the related loop/upstream source, ran `git diff --check`, and did not independently run tests.

Remaining limits: the queues are in memory and have no durable enqueue receipt; a process failure can lose not-yet-delivered input. Cancellation after the final dequeue check, or during an active run's asynchronous hook poll, can place a queued input in the transcript with an aborted assistant instead of an answer. The existing loop has no return-to-queue protocol for that case. One-at-a-time queue mode, mid-tool interruption, subscription/event fanout, a production Host binding, Session journal integration, full workspace gate and bounded real-model validation remain open. Do not count this point or either inventory surface as accepted.
