# Provider delivery plan

## Current user priority — 2026-10-01

The user changes provider ordering: first OpenAI-compatible protocols with a
custom configuration scheme, then OpenAI account login. These two capabilities
are the daily-use target. Other provider compatibility and remaining fixed-source
differences are recorded and deferred. The broader independent Rust ARA/Core,
product boundaries and P0–P6 goal remain unchanged. Deferred work is not accepted
parity and does not advance `ported_through_commit`.

| Module | Current evidence | Next deliverable | Acceptance |
| --- | --- | --- | --- |
| OpenAI-compatible custom routes | Chat/Responses adapters and explicit CLI model/base URL/key environment configuration exist. The 2026-10-01 CAS task writes and verifies a file, then resumes the original Session. | Connect the existing native models config/cache to the main host; select custom profiles with CLI overrides, private key resolution, protocol options and journal attribution. | Module tests, a controlled upstream failure and one bounded CAS task on the delivered path. |
| OpenAI account login | Native credential SQLite/CAS revisions/leases and per-request route resolver exist. No native login command or Codex provider binding is accepted. | Login, private persistence, refresh, account-specific Codex Responses requests, continuation and logout through existing Host interfaces. | One login/auth/transport module suite; account authorization and an actual bounded task. No successful token exchange alone counts as delivery. |
| Other provider compatibility | Discovery/factory/manager dependencies have bounded source/native evidence; main registry is still disconnected. | Retain current implementation; address deferred contracts when the two daily-use modules are usable. | Separate later parity evidence; not a blocker for the daily-use milestone. |

### Verification and speed

- Reuse fixed OMP source and original tests/input families. The inventory contains
  27,404 upstream test cases; no wholesale rewrite of those tests is required.
- Group checks by module. Add a regression only for a concrete important defect,
  protocol change or user workflow. Ordinary mappings use source review and the
  existing module corpus; avoid a separate micro-test for each mapping.
- Use one final backend/dependency gate for a stable batch. Run affected module
  checks after a meaningful repair. Preserve logs and exact code identity.
- `scripts/verify_model_discovery.py --inputs <local-paths.json> --output <dir>`
  runs this catalog module once. Add `--full` for fmt, Clippy, all target/doc
  tests, inventory consistency, cargo-deny and the CLI build in one command.
  The local inputs contain artifact/tool paths, not credentials. Source exports
  are retained and reused; the Rust binaries still check the frozen source data.
- Live API priority: local ARA Manager configuration, OMP management proxy, CAS
  `deepseek-v4.1-flash`. Read the current `用户.ry_switch` route, enabled state,
  credential source and actual catalogue before use. Do not change that file or
  commit its contents. Keep trial bounds and credentials separate from receipts.

### Estimates and reporting

Report module status, formal gate status, elapsed time, next work and the current
estimate during execution. Formal acceptance remains P0 accepted, P1–P6 open;
V1 remains accepted and RPC has 27/42 bounded implementations. These counts are
not an estimate of total implementation effort.

The catalog final check measured 198.755 seconds including compilation;
its backend gate took 118.391 seconds. The CAS task and resume took 5.954 seconds.
The original catalog batch spent substantial time on coverage expansion and
repeated pre-fix checks. New provider work uses the module-first procedure above.
Implementation estimates are updated after the actual login/config source scope
review; account authorization time depends on the user's interaction. A full
OMP/P0–P6 completion date is not yet supported by measured remaining throughput.

## Deferred compatibility register

| Item | Evidence / dependency | Status |
| --- | --- | --- |
| Full cipher expressions and suites | Current native catalog TLS uses ring's named subset. Actual Bun uses BoringSSL; the 45-case investigation demonstrates expression differences from local OpenSSL and a negotiable AES128-SHA suite absent in ring. | Explicitly deferred by provider prioritization; not complete. |
| Ollama Retry-After date parsing | Native RFC2822/RFC3339 parsing misses accepted fixed Bun `Date.parse` values, including RFC850. The source probe documents a >60-second retry decision difference. | Open source-backed review finding, deferred; do not claim complete Ollama retry parity. |
| Other provider-specific auth/OAuth, usage/reserve/rotation | Existing catalogue/store/route foundations do not implement full provider auth lifecycle. | Deferred outside the first OpenAI account slice. |
| Codex advanced transport parity | WS-first, Lite, native provider compaction and complete fixed transport recovery have their own source contracts. Daily-use SSE login must state its supported scope. | Later compatibility slice; account login cannot be labeled full Codex parity. |
| All-provider overrides/registry/discovery and extended metadata | Preserve the existing native model data, factories and manager; do not discard unsupported metadata while projecting a usable route. | Open; wire only what the current OpenAI deliverable needs first. |

Core/session recovery, remaining RPC commands and product integration retain
their existing plans. This provider reprioritization does not silently accept
or remove those independent requirements.
