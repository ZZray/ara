# AI-01e coded Responses chain rejection (WIP, 2026-09-27)

## Source, scope and behavior

Fixed OMP `596f2da7101178214aa27a753529d15e6b7ad91d`,
`packages/ai/src/providers/openai-responses.ts::isOpenAIResponsesStalePreviousResponseError`
and its pre-output chained-request fallback, recognizes
`error.code=previous_response_not_found` even when the human message does
not name the old response. `packages/ai/src/error/classes.ts::OpenAIHttpError.parseEnvelope`
reads `error.code`, falling back to `error.type`.

ARA WIP `88d881b` retains the machine code in a crate-private detailed HTTP
result. The original `post_with_retry` signature still returns
`ProviderError`; Chat and Anthropic use that unchanged interface. Only
Responses consumes the detailed result. A linked request rejected by HTTP
before the stream starts can replay the complete history once when the code
is `previous_response_not_found` or `invalid_prompt`. Existing message-based
classification remains. A Zero Data Retention message takes precedence over
the code: the retry uses `store:false` and disables future chaining. An
unknown code does not create a new replay path. Cancellation prevents replay.

The path is CLI host → Agent → Responses provider → shared HTTP transport →
structured error classification → one full-history retry → Agent/tool result
and Session journal. No public error type, Chat/Anthropic error behavior,
Agent lifecycle or tool execution rule changed. This is a WIP extension of
the existing opt-in stateful route, not complete Responses chaining parity.

## Executed evidence on final code snapshot `342aac6`

Windows PowerShell; Git Bash was in `PATH` for the full CLI workspace gate.
Fixture credentials are dummy values and request logs redact authorization.

| Check | Result and observed artifact |
| --- | --- |
| New Responses fake HTTP case | It failed on the prior implementation (`output.text()` was empty instead of `recovered`). On the WIP code it passes for both `error.code` and `error.type` with unrelated message `lookup failed`: the rejected delta carries `previous_response_id`, the third request carries full history without that ID, and only one Start event is emitted. |
| Related negative HTTP cases | A code-only `invalid_prompt` replays full history; unknown `schema_invalid` with unrelated message does not; conflicting `previous_response_not_found` plus `Zero_Data_Retention` disables chaining and retries with `store:false`. Responses HTTP suite passes 29/29. |
| Real `ara` process → fake Responses HTTP → write tool (`342aac6`) | With `--responses-stateful`, the first response writes `coded-chain.txt` with exact bytes `once\n`. A code-only 400 rejects the chained tool-result request; the third request replays the initial user, function call and function output without `previous_response_id`. Process exit is 0, stdout is `Recovered.\n`, the Session has exactly one tool result, and the file has one expected write. Focused test 1/1 and Responses CLI suite 11/11 pass. |
| `cargo test -p ara-ai -p ara-agent --quiet`; strict workspace Clippy | Exit 0. AI: 113 unit, 18 Anthropic HTTP, 48 Chat HTTP, 29 Responses HTTP; Agent: 36 loop cases plus other groups. Clippy exits 0 with all targets/features and `-D warnings`. |
| Doc tests, `cargo deny check`, fixed OMP inventory, scoped formatting | Exit 0. Dependency policy reports advisories, bans, licenses and sources OK; existing warnings remain. |
| `cargo test --workspace --all-targets` | Exit 101: CLI E2E 35/36, with only the previously recorded Windows Bash `deadline_during_a_tool_and_zero_budget_exit_nonzero` four-second assertion failing. Log: `%TEMP%\ara-responses-coded-chain-342aac6-gate.log`. |
| `cargo fmt --all -- --check` | Exit 1 only for unchanged vendored `crates/vendor/pi-*` files; the four changed Rust files pass scoped formatting. Log: `%TEMP%\ara-responses-coded-chain-342aac6-fmt.log`. |

Independent Codex plan reviewer `/root/responses_error_code_plan` bounded the
change to a crate-private detailed result and the Responses consumer. Diff
reviewer `/root/responses_error_code_diff_review` found the ZDR underscore
separator priority bug; that code and its HTTP fixture were corrected. Its
final review of the four-file change and independent rerun of the CLI case
found no remaining confirmed defect.

No real model was called and no usage or cost can be inferred from these fake
responses. The known Windows gate, other AI-01e parity gaps, bounded real
model task and point audit remain open. Accepted counts do not change.
