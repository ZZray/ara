# AI-01b: opt-in Chat reasoning history replay (WIP)

Date: 2026-09-28. Target: local WIP code commit `a95c371`; not pushed.
This is a partial fixed-OMP port, not AI-01b or P1 acceptance.

## Requirement and boundary

Fixed OMP `596f2da7101178214aa27a753529d15e6b7ad91d`,
`packages/ai/src/providers/openai-completions.ts:2009-2205` and
`packages/ai/test/deepseek-reasoning-content.test.ts:140-518`, replays
`reasoning_content` for a route that requires it on every assistant history
turn. An absent captured value is an empty string, not fabricated reasoning.
ARA already captured `reasoning_content`, `reasoning`, and `reasoning_text`
stream deltas but dropped Thinking blocks when encoding Chat history.

The new `--chat-replay-reasoning-content` flag is accepted only with
`--api openai-completions`. It sets one provider-local compatibility bit;
the default is off. The adapter replays signed Chat Thinking only when the
history message's API, provider label, and model ID match the current model.
It normalizes those three recognized field signatures into the required
`reasoning_content` key, joins multiple blocks in order, and emits an empty
string when there is no eligible block. It retains reasoning-only assistant
turns with `content: ""` and preserves tool-call/result IDs.

The default Chat wire, Responses, Anthropic, Agent loop, Session schema and
process lifecycle are outside this change. Automatic model detection,
cross-API Thinking demotion, and routes that permit synthetic `"."` content
remain open. ARA's same-source filter is an intentional safe-side difference
from OMP's wider compatibility matrix. Assistant messages do not store
`base_url`; an explicit later opt-in with the same provider label and model
ID on another endpoint can send the captured history there. The caller must
choose that opt-in only for its trusted route.

## Executed evidence

Entry: `ara` CLI -> Agent -> Chat provider -> real loopback HTTP fake
upstream -> `write` -> Session journal, then a new CLI process resumes the
same Session. `ara-testkit::FakeUpstream` records redacted requests.

| Check on this worktree | Exit/result | Observed behavior |
| --- | --- | --- |
| `cargo test -q -p ara-ai required_chat_reasoning_replays_only_matching_signed_history` | 0, 1/1 | Default omits field; opt-in replays recognized signed blocks; empty signed block, absent Thinking, foreign API/provider/model and opaque signature emit `""`; reasoning-only turn retained; tool ID/result paired. |
| `cargo test -q -p ara-ai --test openai_http` | 0, 49/49 | Existing Chat HTTP protocol fixtures remain green. |
| `cargo test -q -p ara-ai --all-targets` | 0, 123 unit + 34 Anthropic HTTP + 49 Chat HTTP + 31 Responses HTTP | Full AI crate suite passed. |
| `cargo test -q -p ara-cli --test e2e required_chat_reasoning_replays_tool_and_text_turns_after_restart` | 0, 1/1 | First stream carried `"write the file once"` and one `write(reason.txt, "once\n")`. Second request replayed exact reasoning and paired `call_reason_write` result. After restart, third request replayed that thinking and used `reasoning_content: ""` for a plain-text assistant. The journal kept the original signed Thinking block, appended to the same file, and a changed file sentinel survived resume. A fourth request without the opt-in had no reasoning field. Four HTTP requests, one tool effect. |
| `cargo test -q -p ara-cli --test e2e argument_errors_do_not_touch_a_resumed_session` | 0, 1/1 | Non-Chat `--chat-replay-reasoning-content` rejected before journal I/O; journal bytes unchanged. |
| `python scripts/verify_backend.py` | 1 | Owned fmt and workspace all-target/all-feature Clippy passed. All-target tests reached CLI e2e: 47/48 pass, including the new test; the existing `deadline_during_a_tool_and_zero_budget_exit_nonzero` failed at its `<4s` assertion on Windows Bash. The script stopped there; workspace later suites and doc tests did not run in this command. |
| `cargo test -q --workspace --doc --all-features` | 0 | Workspace doc tests passed separately; all groups reported zero runnable cases. |
| `cargo deny check` | 0 | Advisories, bans, licenses, sources OK; existing duplicate-version warnings. |
| `python scripts/omp_inventory.py check` | 0 | Fixed baseline and surface inventory consistent. |
| `git diff --check` | 0 | No whitespace errors in tracked changes. |

No API key was present in the expected environment variables, so no bounded
real-model task was run. The controlled fake upstream proves the actual host
and tool path, but does not prove a live DeepSeek-compatible endpoint accepts
the exact history. The prepared-request text observer intentionally counts
only `content` text fields, not provider-specific reasoning fields; it is not
a full token budget verdict.

Independent Codex pre-implementation review (`/root/a3_scope_review`)
approved the narrow opt-in scope and highlighted the cross-source and
reasoning-only cases, which the tests cover. Independent post-diff review
(`/root/a3_tokenizer_review`) covered all six changed code/test/doc paths
against `233dc5c`, found no blocker in the default or same-route paths, and
reported a P2 cross-endpoint continuation risk. Both changing the base URL
and re-enabling the flag are required to trigger it. The CLI flag is an
explicit host choice for the destination route, but the current Session
cannot prove that a prior reasoning block came from that endpoint. Before
accepting this surface for wider host integration, decide how to bind saved
reasoning to its source endpoint or require a separate cross-endpoint
authorization; add an A-to-B route test. This boundary is not silently
treated as passing parity.

Point audit: **WIP / not accepted**. The full backend gate failed and no
bounded real-model task ran. The independent P2 finding remains open.
