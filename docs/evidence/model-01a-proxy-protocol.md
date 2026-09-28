# MODEL-01a: explicit dual-protocol proxy selection (WIP)

Fixed source: OMP `596f2da7101178214aa27a753529d15e6b7ad91d`,
`packages/coding-agent/src/config/model-discovery.ts::discoverProxyModels`
(lines 942–1041) and `packages/coding-agent/test/model-discovery.test.ts`
(`B-b06b723628`, whose endpoint-routing assertion is covered here; its
context-length assertion is still open). Rust owner: `ara-cli` only.

Observable slice: `ara --api proxy-auto --base-url <proxy>/v1 --model <exact-id>`
queries `/v1/models` before Session creation. For the exact selected ID,
`supported_endpoint_types` containing `anthropic` selects Anthropic Messages;
otherwise `openai` selects Chat Completions. The proxy remains the provider
identity. The selected protocol reaches the existing provider with the same
credential as discovery. Existing default and explicit `--api` paths do not
probe. This is an opt-in CLI route, not a model catalogue.

Intentional differences and limits:

- OMP can fall back to a provider default when endpoint types are absent.
  `proxy-auto` has no configured fallback, so it fails closed. Missing or
  duplicate selected IDs, unsupported endpoints, malformed/oversize lists,
  failed HTTP and timeouts also stop before the Session is created.
- The configured root must be an HTTP(S) `/v1` URL. No name, context window,
  token limit, cost or other capability metadata is imported. No static
  catalogue, cache, model search or fuzzy ID matching is implemented.
- Auto mode rejects resume/continue until a Session can bind and verify its
  endpoint across runs. It rejects protocol-specific flags, custom auth
  headers, official Anthropic routes and provider labels whose Anthropic
  adapter changes to `x-api-key` authentication. Discovery and inference do
  not follow redirects. These restrictions keep this narrow route from
  silently changing credentials or replaying a Session to another source.

## Executed checks

Environment: Windows, `CARGO_TARGET_DIR=C:\Temp\ara-verify-target`, test/dev
debug info disabled, incremental disabled, two build jobs.

| Command | Result |
| --- | --- |
| `cargo fmt -p ara-cli` | exit 0 |
| `cargo test -p ara-cli --test proxy_discovery` | 6/6 pass on the current code; real CLI process, fake HTTP proxy and Session. Dual-endpoint Anthropic preference writes `proxy.txt` once, follows the tool result, records `proxy/selected-model` and final text. OpenAI-only chooses Chat; explicit API sends no discovery GET. Negative cases cover missing/duplicate ID, unsupported/missing endpoint list, 401, malformed and oversized response, timeout, redirect, invalid URL, missing prompt, special auth provider and resume. Credentials are absent from stdout/stderr and Session. |
| `python scripts/verify_backend.py` on the final code | owned format and workspace Clippy passed, then the full test phase stopped at the pre-existing Windows Bash `deadline_during_a_tool_and_zero_budget_exit_nonzero` assertion: 50/51 CLI e2e pass. Exit 101. The same failure occurred before and after the final authentication guard and tool-cycle fixture. |
| `cargo test --workspace --doc` | exit 0; all workspace doc-test groups passed. |
| `cargo deny check` | exit 0: advisories, bans, licenses and sources okay; existing duplicate/no-license-field warnings. |
| `python scripts/omp_inventory.py check` | exit 0; fixed OMP inventory consistent. |
| `python scripts/verify_bootstrap.py` | exit 0; required files, OMP marker and local links valid. |

Independent Codex post-diff reviewer checked `main.rs`,
`proxy_discovery.rs` and `tests/proxy_discovery.rs` against fixed OMP. It
found one reachable mismatch: the `opencode-go`, `opencode-zen` and `umans`
provider labels switch Anthropic inference to `x-api-key` while discovery
used Bearer. Auto mode now rejects those labels before network I/O. The
reviewer rechecked all three files and found no further high-confidence issue.
Review was read-only and did not claim test execution.

No bounded real-model task was run on this route. The full backend gate is
red, so MODEL-01a remains **implementing / WIP**, not delivered or accepted.
