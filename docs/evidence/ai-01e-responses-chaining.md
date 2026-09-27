# AI-01e opt-in Responses `previous_response_id` chaining (WIP)

## Requirement and fixed source

Fixed OMP `596f2da7101178214aa27a753529d15e6b7ad91d` uses
`packages/ai/src/providers/openai-responses.ts:120-128,201-374,509-518,672-717,845-899`
and `packages/ai/src/providers/openai-shared.ts:3860-3900` to chain a stored
Responses result only when the next request has compatible options and a
strict prefix equal to the previous complete input plus replayable output.
Otherwise it sends the full history. It enables stateful storage by default
only for its official OpenAI endpoint; third-party endpoints default off.

ARA now exposes `StreamOptions::stateful_responses` in
`ara-ai::providers::openai_responses::stream`. It defaults to `false` and also
requires a host-owned `ProviderSessionState`. An enabled call sets
`store:true`. A successful terminal response with an ID and fully replayable
native items becomes a transient baseline scoped to provider, endpoint,
model and hashed authorization/routing options. The next request is first
encoded in full. Only matching top-level controls and a strict item prefix
allow `previous_response_id` plus the added input items. History edits,
option changes, no added input and an unfit response force a full request.
Concurrent calls invalidate the shared baseline so branches cannot overwrite
one another.

An explicit stale/unsupported ID rejection before `Start` permits one
full-history HTTP retry. A combined `Request blocked` and Zero Data Retention
rejection disables chaining and sends the retry with `store:false`.
Three consecutive stale rejections disable chaining for the session; only a
successful chained completion resets that counter. Streaming errors,
cancellation and unrelated HTTP errors do not retry through this path. The
provider never replays a tool effect to repair a chain.

This change does not bind the option in CLI, Agent or Session. The current CLI
continues using `store:false`; the new path is exercised through the public
provider entry with a controlled local HTTP upstream. No default third-party
storage policy or stable host lifecycle changed.

## Evidence on committed WIPs `27f4b9a` and `38c624c` (2026-09-27)

- On final code `38c624c`, `cargo test -p ara-ai --quiet` exited 0: 111 unit,
  17 Anthropic HTTP, 48 Chat HTTP and 27 Responses HTTP tests passed. The controlled HTTP cases
  inspected actual `POST /v1/responses` bodies: first full request with
  `store:true`; second request with `previous_response_id=resp_first` and only
  `second question`; tool continuation with only its paired
  `function_call_output`. Actual terminal text was checked for each request.
- HTTP failure cases covered a changed original user turn, changed request
  controls with a new subsequent baseline, changed API key, fresh session
  state, stale ID and blocked-prompt full retries, ZDR with
  blocked wording, three stale rejections, unrelated 400, in-band failure,
  cancellation before headers and overlapping branches. The assertions check
  request count, storage flag, previous ID presence and the final visible
  response. The overlap test confirms the next turn uses full history after
  two requests raced in one state object.
- With Git Bash in PATH,
  `cargo test -p ara-cli --test e2e responses_ --quiet` exited 0 (9/9). These
  are regression checks for the unchanged default CLI path, not evidence of
  host-enabled server-side chaining.
- On `38c624c`, `cargo clippy --workspace --all-targets --all-features -- -D warnings`,
  `cargo test --workspace --doc --quiet`, `cargo deny check`, fixed OMP inventory
  check and scoped `rustfmt --edition 2024 --check` on the two changed Rust
  files exited 0. `cargo deny` retained existing non-fatal warnings.
- With Git Bash in PATH,
  `cargo test --workspace --all-targets --quiet` exited 101: 33/34 CLI e2e
  cases passed, then the existing Windows
  `deadline_during_a_tool_and_zero_budget_exit_nonzero` four-second assertion
  failed. Earlier suites, including all 27 Responses HTTP cases, passed.
  `cargo fmt --all -- --check` exited 1 on unchanged vendored `pi-*` sources;
  both changed files pass scoped formatting. Local logs are
  `%TEMP%\ara-responses-chain-gate.log`,
  `%TEMP%\ara-responses-chain-fmt.log`, and
  `%TEMP%\ara-responses-chain-deny.log`.

Independent Codex plan reviewer `/root/responses_chain_plan` checked the
provider-only boundary and concurrency strategy. Independent Codex diff
reviewer `/root/responses_chain_diff_review` found the missing blocked-prompt
fallback and ZDR precedence; both were fixed and the reviewer reran focused
HTTP cases. Its final re-review found no remaining concrete issue in the two
changed files. It identified the following open parity difference.

## Open difference and acceptance

`post_with_retry` currently returns only the parsed HTTP error message. When
the upstream sends `error.code=previous_response_not_found` with an unrelated
message, the Responses provider cannot classify it for a full-history retry.
Preserving that structured code requires a separately scoped change to the
shared HTTP error contract. The CLI has no stateful opt-in binding, and the
official-endpoint default differs from fixed OMP. The full backend gate,
bounded actual-model task and independent point audit remain open. No AI-01e,
AI module or P0-P6 acceptance count advances with this WIP commit.
