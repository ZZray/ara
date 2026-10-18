# HandWave Node 0.2.6: native web research candidate

2026-10-18. Requirement: the standalone Rust computer Agent should know how to
search online, read relevant sources, and download/verify tools when requested.
Base root HEAD `be0a3aa56a21ad50f2d4be13fb52bef163f33659`; this independent product
was initially clean inside a root checkout containing592 unrelated dirty entries.
Those root changes, Shared Core/OMP ledger, production services, device grants,
login, UI and avatar are outside this change. No commit, push or deployment was
performed. This is a local candidate, not a new live installation.

## Delivered scope

- New native `web_search` and `fetch_url`, registered with the actual rpi Agent.
  Default best-effort DuckDuckGo Lite; optional Host-configured SearXNG JSON.
- Read-only research in Plan; Execute/Goal retain existing authority/budgets.
- Bounded source receipts, untrusted-content policy, UTF-8 pagination/find,
  content digest, redirect/address validation and cancellation.
- Existing download tool now accepts optional `expected_sha256`, rejects invalid
  arguments before directory/network effects, preserves the destination on a
  mismatch, and returns the actual streamed SHA256. No dependency was added.
- Explicit research and installation guidance; no browser/JS/CAPTCHA bypass,
  auto-install permissions, memory promotion, scraping framework or cloud search
  service deployment. See [durable boundaries](../knowledge/web-research.md).

## Final snapshot and artifacts

Evidence root: `D:/WorkSpace/tmp/handwave-web-20261018/`.
`final-source-pins.json` pins12 source/manifest/harness paths. The candidate EXE
`handwave-node-0.2.6.exe` is a standalone Windows x64 static-CRT release:

- Bytes: **5,213,696** (about5.21MB).
- SHA256: `3ea4cec541e4a158320752fc414ef108036dc6cb4f6c2d6b039abf70afcae2a3`.
- Historical0.2.5 release:5,164,544 bytes; increase49,152 bytes (about48KiB).
  That historical image was not rebuilt for this comparison.
- rpi source remains `8ec462672d90e62712d96074638efef89436cebe`; only this
  package's version changed in Cargo.lock.

The final Clippy-only repair constructs `Document` fields in its initializer
instead of reassigning a default. Earlier debug model receipts use image
`2031450e5bd013fb52008bf765f03f318285656b580db1b66e33479eb6e770a4`;
final debug is `1cf6c322c6a2fb5850b2248a569ddebb54795b64cb954c7d1c56dc9bf02b883f`.
Final release Host/model results are recorded separately below, not inferred
from a previous EXE.

## Commands and executed checks

Use the independent manifest `apps/handwave-node/Cargo.toml`, not the dirty root
workspace. Debug target directory `D:/WorkSpace/target/handwave-node`, release
`D:/WorkSpace/target/handwave-node-release`, Cargo jobs2.

| Check | Executed result / raw artifact |
| --- | --- |
| `cargo fmt --manifest-path apps/handwave-node/Cargo.toml --all -- --check` | PASS, scoped formatted sources. |
| `cargo clippy --manifest-path apps/handwave-node/Cargo.toml --locked --workspace --all-targets --all-features -- -D warnings` | PASS6.02s; `rust-clippy-final.log`. Initial default-field-reassignment lint failed and was repaired without suppression. |
| `cargo test --manifest-path apps/handwave-node/Cargo.toml --locked --workspace --all-targets` | **173 PASS,0 failures,1 explicit ignored public-network test**,7.34s; `rust-tests-final.log`. Binary-only package has no library doctest target. |
| `cargo deny --manifest-path apps/handwave-node/Cargo.toml check` | Advisories/bans/licenses/sources PASS; duplicate-version and unmatched-license warnings retained in `cargo-deny.log`; no policy/dependency relaxation. |
| `cargo build --manifest-path apps/handwave-node/Cargo.toml --release --locked --target x86_64-pc-windows-msvc` with `CARGO_TARGET_X86_64_PC_WINDOWS_MSVC_RUSTFLAGS=-C target-feature=+crt-static` | PASS56.10s final build; `rust-release-final.log`. Earlier pre-lint release build1m28s retained. |
| Native tool fixtures | Real HTTP bodies, streamed size cap without Content-Length, charset/type/challenge errors, redirects/private-network denial, cancel/deadline, UTF-8 page/find/hash, malicious1MiB HTML, long URL budget/redaction; all in final173. |
| Download checksum fixture | Real local HTTP, uppercase expected digest, actual digest with/without expectation, bad-argument no-directory effect, mismatch preserves original and no adjacent tmp leak; final173. |
| `python -X utf8 apps/handwave-node/tests/verify_web_research.py --binary <final-release> --hub-binary <signed0.2.5Hub> --output <host-release-final-v1> --public-fetch` | **6/6 PASS**,15.51s/14 controlled model requests/0fixture errors. Actual search→checksum download→Python artifact/output, wrong digest preserves old destination, private fetch causes no HTTP, Plan search, forged Plan download rejected before network/write, actual example.com title/body/hash/source/time. |
| `python -X utf8 apps/handwave-node/tests/verify_host.py --binary <final-release> --hub-binary <signed0.2.5Hub> --modern-hub --proxy --catalog-variants --max-seconds-total 240 --output <full-host-regression-final>` | **27/27 PASS**,41.769s/23 controlled model requests/19catalog requests/0fixture errors. Includes actual compiler/executable effects, same-process/restart continuation, duplicate/partial/cancel/disconnect fencing, actual forward-proxy download and direct loopback WS/model transport. |

## Real network and actual model boundaries

An explicitly enabled ignored test called both tools through `AgentTool::execute`.
Default DDG search succeeded with real official Python results (1,196 serialized
bytes); the Python page fetch then failed with a connection/request error. That
combined test is **FAIL**, retained in `public-web-tools.log`; a search success
does not erase a fetch failure. Separate ordinary Python HTTPS probes could read
both domains, so basic domain reachability alone cannot prove this tool attempt.

The current Gateway inventory was read before trials and selected exact
`deepseek-v4.1-flash` / `openai-completions`, advertised1M context and text/image.
No new multimodal behavior is claimed. Trials use a temporary local signed Hub
and device, an SSH tunnel to the existing Gateway, an independent local budget
proxy and no production grant/installation mutations. Per trial ceiling:6 model
HTTP requests,90 seconds,1,500 output tokens per request,64,000 conservative
reserved input-byte-plus-output-token units. Actual reported usage is separate.

| Trial | Actual result |
| --- | --- |
| `live-default-v1` (pre-lint debug) | Strict full-search criterion **FAIL**: real model independently chose `web_search` then `fetch_url`; DDG request failed, official Python page succeeded (25,285 text bytes,6,000-byte window), final reply disclosed failed search and cited the actual read source.3 model HTTP calls/29.79s/provider reported14,208 total tokens, reserve56,733. |
| `live-controlled-leads-v1` (pre-lint debug) | **PASS for actual model + controlled search leads + actual official page**, not a real search-engine reliability proof.3 model calls/18.806s/reported15,112 total tokens/reserve60,084. Model chose reference/search/fetch, used find, rejected an obvious fixture placeholder, returned commands and official source. |
| `live-release-controlled-leads-final-v1` (final release) | Strict **FAIL**: search and official-page reading succeeded, but the model chose a legitimate16,000-byte window. The experiment's conservative cumulative64k JSON-byte reservation refused the third model request before upstream HTTP.2 actual model calls/20.005s/reported7,576 tokens; no final reply. This is an experiment budget boundary, not successful end-to-end completion or a product tool failure. |
| `live-release-controlled-leads-96k-v1` (final release) | **PASS for final EXE + actual DeepSeek + controlled search leads + actual official page and final sourced answer**. New independent read-only Plan Session, at most4 calls/90s/1,500 output per call/96,000 reserved upper units; actual3 calls/16.06s/reported14,343 total tokens/reserve57,848. Model autonomously chose search→fetch, returned source and commands, and disclosed its6,000-byte read window. No further model calls. |

After the64k guard failure, the independent reviewer inspected the known schema
and legitimate16k-page request sizes and recommended an explicitly declared96k
experiment ceiling, without changing product limits or replaying that Session.
The new trial actually selected6k, not16k, and stayed below64k; raising the ceiling
therefore is **not proven to cause** its success. Both different model decisions
and the original failed experiment remain visible. The harness now records
rejected candidate request bytes and its configured ceilings, not just a429.

Cache-hit fields are provider reports only; unknown/zero fields do not establish
misses, cost or savings. No benchmark/general intelligence/token reduction claim.
DDG remains externally availability-dependent; a maintained configured search
backend is the recommended operational follow-up. No SearXNG instance was
installed or publicly deployed in this task.

## Independent review and retained failures

Independent Codex `/root/web_tools_review` applied `ara-git-review` and
`ara-rust-core-review` before/after implementation. It independently read every
changed Rust path and integration boundary. Its P2 long-source-URL finding was
fixed: no600/2048-byte silent URI clipping, whole-result omission when over
budget, explicit query-redaction flags. It independently ran12 web tests and
1 download test on the pre-lint test EXE, and inspected the full173-test receipt;
`review-post.md`, `review-source-pins.json` and `review-*-focused.log` retain this
coverage. It then independently re-executed the final initializer-repaired test
EXE (SHA256 `0ee4db04334d3353063e09a05a399c44a6a906519857b49ed535afbf0447b595`):
12 web tests PASS/1 public ignore/0.45s and1 download PASS/0.02s. These receipts
do not stand for production activation.

The first isolated Host run passed five core cases but its sixth assertion
incorrectly expected the HTML title inside extracted body text. Actual tool
receipt already had the correct separate title, body and hash. The harness was
corrected to validate those actual fields, preserving the original failure.
All public-network/Clippy/fixture failures are retained, not rewritten as PASS.

## Final acceptance boundary

Final release Host6/6, existing full Host compatibility27/27 and the scoped
actual-model trial above passed. Independent Codex `/root/web_tools_review`
accepted this bounded local Windows0.2.6 point after checking all12source pins,
final release size/SHA, actual tool/artifact/official-page/final-citation receipts
and the declared failures/limits. Report `review-point-final.md`, SHA256
`24f5cd0d092ea55c56de0f92077185bd9de8252998de25a6de11907ca8218df5`.
The primary coordinator independently checked the same critical results and
accepted only this boundary.22 JSON receipts were checked privately against the
actual Gateway credential and key patterns with0matches, and isolated trial
Node/Hub/SSH processes were cleaned up (`evidence-secret-scan-final.json`,
`process-cleanup-check-final.json`). No credentials are in Git artifacts.

Scoped local Markdown links and Python syntax pass. The repository-wide
`python -X utf8 scripts/verify_bootstrap.py` was run and failed on existing root
handoff/absolute-temp links and ignored generated dist README links; it also ran
while this new evidence page was being authored. The final five edited/new
product documents have0broken local links (`scoped-doc-links.json`). Those unrelated
links were not rewritten and no root acceptance marker was advanced.

Linux/macOS execution, live browser UI, public
downloads and installed Node upgrade are not claimed. The Windows-source code
paths avoid browser/runtime dependencies, but cross-platform compilation alone
would not establish those other runtime boundaries.
