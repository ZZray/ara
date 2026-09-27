# AI-01b: explicit Mistral Chat history profile (WIP)

Date: 2026-09-28. Code snapshots: `16ce819` and `42389c1` on local
`dev`, not pushed. The latter adds the thinking-only regression case.
This extends AI-OPENAI-CHAT; it does not accept AI-01b or A4.

## Requirement and boundary

The fixed OMP commit `596f2da7101178214aa27a753529d15e6b7ad91d`
uses four Mistral Chat compatibility decisions together. Sources:
`packages/catalog/src/compat/resolve.ts:491-494` and
`packages/ai/src/providers/openai-completions.ts:212-228,1923-1933,2014-2023,2170-2278`.
The pinned behavior inventory names the Thinking/text cases
`B-8aa7018010` and `B-88e4437f95` in
`packages/ai/test/openai-completions-compat.test.ts`, and the host bridge
selection case `B-7ac38a38cc` in `packages/catalog/test/build.test.ts`.
They require nine ASCII alphanumeric tool IDs, result `name`, an assistant
bridge between tool results and following user/developer content, and prior
Thinking rendered as assistant text. The fixed XML dialect renders the
Thinking as `<thinking>\n...\n</thinking>`.

ARA adds `--chat-mistral-compat` for an explicitly selected Chat route. The
flag sets those four provider-local compatibility fields. The default Chat
wire and the other protocols are unchanged; incompatible CLI flags are
rejected before Session I/O. The request projection normalizes and
deduplicates IDs while matching each result to the final wire ID. The Session
journal retains original IDs. A tool-result image promoted into a user
message receives exactly one bridge. Thinking is replayed only when the
recorded API, provider and model match the target, since ARA has not ported
OMP's cross-provider Thinking/signature transform. This is an intentional
safe-side difference, not full Mistral route parity. ARA also requires an
explicit flag where OMP auto-selects the profile for Mistral routes;
automatic detection would change existing request bodies.

The code touches only the Chat adapter, CLI flag wiring and their tests. It
does not change Agent, Session, shared Model/Tool types or process lifecycle.
Automatic route detection, broader cross-provider Thinking demotion, and
actual Mistral endpoint validation remain open.
The Session message does not store a source base URL: if a host resumes with
a different endpoint but the same provider label/model ID and explicitly
selects this profile again, same-source Thinking can be sent to that new
endpoint. The CLI caller must treat the destination as an authorized route;
source-endpoint binding remains open for wider host integration.

## Executed evidence

Entry: `ara` CLI -> Agent -> Chat provider -> loopback `FakeUpstream` ->
real `write` -> Session journal -> new CLI process with `--resume`.

| Check on delivered code | Exit and observation |
| --- | --- |
| `cargo test -p ara-ai mistral_ --lib` | 0 on `42389c1`; five tests cover default unchanged, colliding normalized IDs with paired results, tool-result `name`, bridge placement, promoted image, same-source XML Thinking including a thinking-only turn, omitted foreign Thinking, and whole Responses composite-ID normalization. |
| `cargo test -p ara-ai --all-targets` | 0 on `42389c1`; 128 unit, 34 Anthropic HTTP, 49 Chat HTTP and 31 Responses HTTP tests passed. |
| `cargo test -p ara-cli --test e2e mistral_chat_profile` | 0; two tests passed. The real CLI wrote `mistral.txt` once, saved the original `call-write-long` ID, then resumed in a new process. The request carried one nine-character call/result ID, result `name: write` and an assistant bridge before the new user turn. A sentinel file remained after resume; two HTTP requests served and no duplicate write. Non-Chat or conflicting flags failed before Session creation. |
| `rustfmt --edition 2024 --check` on the three changed Rust files | 0; owned files formatted. |
| `cargo fmt --all -- --check` | 1 on `42389c1`; the stable formatter reports 335 pre-existing differences under vendored crates, including `pi-ast` and `pi-edit`, whose formatting config uses nightly-only options. None of the changed files appears in the diff list. Local log: `%TEMP%\ara-cargo-fmt-42389c1.log`. |
| `cargo clippy --workspace --all-targets --all-features -- -D warnings` | 0. |
| `cargo test --workspace --all-targets` with `C:\Program Files\Git\bin` prepended to `PATH` | 101 on `42389c1`; 49/50 CLI e2e passed, including both new cases. The existing `deadline_during_a_tool_and_zero_budget_exit_nonzero` exceeded its `<4s` limit. Prior Agent and AI groups passed; Cargo stopped before later workspace groups. Raw local log: `%TEMP%\ara-full-gate-42389c1.log`. |
| `cargo test --workspace --doc --all-features` | 0; all doc-test groups contained zero runnable cases. |
| `cargo deny check` | 0; advisories, bans, licenses and sources OK, with existing duplicate-version/license-field warnings. |
| `python scripts/omp_inventory.py check` | 0; fixed inventory consistent. |

The unfiltered workspace all-target gate on Windows still fails at the
existing Bash deadline test. No bounded real Mistral task was run:
an exact authorized model ID, protocol and route have not been verified.

Independent Codex pre-review `/root/a3_scope_review` checked the fixed OMP
profile and the file boundary. Independent Codex post-diff review
`/root/mistral_post_review` examined all three changed files and their
callers. It found one P2 wire-parity issue: a Responses composite ID was
shortened before Mistral normalization. The implementation now normalizes
the whole raw ID; its new regression case passed and the reviewer confirmed
the finding closed. No other high-confidence defect was reported.

Point audit: **WIP / not accepted**. The full backend gate and bounded real
model evidence remain open; the feature is explicit rather than route-default.
