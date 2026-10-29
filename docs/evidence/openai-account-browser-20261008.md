# OpenAI account browser login — 2026-10-08

**Executed bounded WIP; combined original gate FAIL / NOT ACCEPTED.**
Fixed OMP `596f2da7101178214aa27a753529d15e6b7ad91d`; dirty `dev` base
`20aa0e2ef9872f633ba2671376cdf778477e7115`. Full OMP remains **0/111 complete,
1 tested, 27 implementing, 83 open**. No parent, phase or product advances.

## Requirement and delivered scope

Port the fixed default OpenAI Codex browser issuer, retain explicit device login,
and publish confirmed credentials through the existing AuthStorage owner.
Production uses `http://localhost:1455/auth/callback`, PKCE/state, no port
fallback, first valid callback/manual winner and complete HTTP feedback before
publishing the result. Unknown token exchange effects are never replayed.
The existing terminal reader is canceled/suspended and joined by its owner.

Fixed source packet and detailed mapping:
`D:/workspace/env/cache/ara-port-candidates/openai-account-browser-scope-20261008/SCOPE.md`.
The policy source includes `packages/catalog/src/compat/rules/auth/openai-codex.kdl`.

| Delivered path | Role |
| --- | --- |
| `crates/ara-cli/src/openai_codex_browser_login.rs` | Standalone browser issuer, callback ownership and cleanup |
| `crates/ara-cli/data/oauth/openai-codex.html` | Fixed feedback template with ARA naming |
| `crates/ara-cli/src/openai_codex_auth.rs` | Existing account issuer composition |
| `crates/ara-cli/src/openai_codex_device_login.rs` | Shared existing code exchange |
| `crates/ara-cli/src/main.rs` | Default browser/explicit device and terminal/opener ownership |
| `crates/ara-cli/tests/openai_codex_auth.rs` | Browser cases in original auth families |
| `crates/ara-cli/tests/openai_daily_cli.rs` | Both issuers in the original process/account workflow |

Execution packet:
`D:/workspace/env/cache/ara-port-candidates/openai-account-browser-execution-v1-20261008`.
Delivered manifest SHA256:
`d080c848c8b04659fb334aa13e7da57a65883cb4ce3bced0fc33710f90a37db9`.
Index SHA256 stays
`89d34e26be97c90a14680d44ace937e6b1ae5a86e8ee7b46dd6b90718a922475`.

## Original execution

One build on the frozen delivered source:

```text
cargo test --locked -p ara-cli --all-features --test openai_codex_auth --test openai_daily_cli --no-run --message-format=json
```

| Stage | Actual result | Wall seconds |
| --- | --- | --- |
| Build | PASS; 410 reused / 5 compiled / 59 warnings / 0 errors | 439.708 |
| Original auth group | 14/14 PASS; no ignored/filtered cases | 26.903 |
| Original daily group | 11/12 PASS, 1 FAIL; no ignored/filtered cases | 163.533 |
| Combined module | FAIL; source_unchanged=true; full_gate=false | 632.852 |

Original integration executables ran with `--test-threads=1 --nocapture`.
Production CLI SHA256:
`f8e519bc8797ce85694006f08f40491bcdc567aa5083284834f3cc6b2fa1cb83`.
Auth executable SHA256:
`15c2a7b0d549cfd416d6bb94b58e2cf9516d44d3a049f3b9b9f5563e0a6c07d5`.
Daily executable SHA256:
`c230cc668ada5e15977854bccc565791ad228f7b3e2379ed3bf01cff27dba5d6`.

The controlled account process exercises credential publication, actual tools
and file artifact, original Session continuation/restart/compaction, logout
denial and unknown-refresh restart fencing. Auth families cover callback HTTP,
PKCE/state, manual formats, first winner, cancellation/timeout, port collisions
and partial/malformed/failed exchanges. These use synthetic upstreams.

## Failure classification without another build

The unchanged original case
`custom_chat_configuration_and_explicit_overrides_reach_the_wire` fails.
Root reran only this exact case using the same emitted executable with
`RUST_BACKTRACE=1`, `--exact --test-threads=1 --nocapture`: exit 101 in **8.625 s**,
source unchanged. Original `legacy-step-diagnostic.json` and its stdout/stderr
remain in the execution packet; this is diagnosis, not a replacement gate.

The backtrace caller is `openai_daily_cli.rs:395`, the fourth explicit
`anthropic-messages` invocation. The preceding configured Chat, CLI override
and Windows-home text assertions executed successfully. Final request-count,
header and token assertions follow the failing step and remain unverified.
`AnthropicMessagesProvider` implements typed `stream` only; actual dispatch
uses the `ModelProvider::stream_actual` UnsupportedOperation default and
`model_route.rs` publishes `actual provider dispatch failed`.
The same deferred gap is recorded in
[the previous daily repair evidence](openai-daily-repair-20261008.md).

Preserve the original whole-group FAIL and unchanged assertions. Under the
user's provider priority, retain native Anthropic dispatch for its later owning
module and continue OpenAI/EDIT work. No repeated build or unchanged failure
rerun is required before that owner changes or the original milestone gate.

## Review, static views and remaining gates

Independent Codex `/root/finish_edit_renderer` reviewed the seven delivered
paths, 17 direct dependencies, source/executable pins and original outputs.
SOURCE frozen report SHA256:
`3bd2a22d65cfaf2f483ca244f16a7b1ba30b84c9342cee5b4369c4bcca190d4b`.
Original final POST **NOT ACCEPTED**:
`openai-account-browser-review-20261008/POST-EXEC-V1-FAILED-FROZEN.md`, SHA256
`35f51775c0b8cadcdb1d0d174bf3a56e3ff1c651ae37b39e85b52a1a010dc80e`.
The original POST predated the caller diagnosis; its failure boundary is retained.
Independent diagnosis addendum confirms caller 395 and native Anthropic dispatch:
`openai-account-browser-review-20261008/POST-EXEC-V1-DIAGNOSIS-ADDENDUM.md`,
SHA256 `ab77728b95bb13710009c7dfd0cde44c0b18ae196f3f73d28a09dc2e343df826`.
The reviewer also checked this evidence page and the current strategy once;
no material correction or reduction in the parity/acceptance requirements was found.
The module stores an unchanged-source boolean, not an independent full pin map;
do not claim a second exhaustive 1589-entry comparison.

Root compared reference/delivered success-initial, manual-close and error static
views at 1280×720 with the fixed dark theme. Six tool screenshots remain in the
chat; `visual-receipt.json` records geometry and preview conditions. The preview
stubbed window.close and held the initial timer, so it does not verify actual
window closing, opener or callback timing.

OPEN: actual OpenAI authorization and subscription-model task; interactive TTY,
opener and manual cancellation; piped/nonterminal manual input/full TUI; other
viewports; raw manual query with unescaped `#` parser parity; generic OAuth,
other providers and complete AUTH/Codex/WS contracts. No real OpenAI account
was used by this batch. No commit, push, deployment or environment install.
