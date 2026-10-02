# Provider delivery plan

**Current checkpoint, 2026-10-02 UTC (tested bounded native-tokenizer WIP):**
[Native tokenizer](evidence/native-tokenizer.md) restores the pinned universal
encoders/scanners/vocabularies under ara-ctok, all eight catalog families and
model-aware Core/reference Host sizing. Ten native encodings are distinct from
the eight catalog families. Grouped modules pass 200/0/0; final Windows backend
1,682/0/20, format/Clippy/inventory/build PASS. The whole gate retains the
existing ttf-parser advisory FAIL. One new default-DeepSeek CAS artifact and
tool-disabled original-Session recall passes in 14.468s/5 calls,
with exact prepared-message-text observations and no tokenizer override.
The ledger now has 60 bounded points: 35 limited acceptances, 23 implementing,
two tested. The 111-surface inventory remains 26 implementing/one tested/84 open,
zero complete acceptances. P0/V1 accepted; P1–P6 open; RPC 27/42; marker null.
Implementation/verification/review about 65min before documentation/Git closure.
Next: full registry loader/cache I/O, async discovery/hydration/coalescing,
runtime/auth/selector callers and the recorded remaining tokenizer contracts.
Reuse original module families and one stable final gate; reduce build
concurrency for memory-heavy Windows linking. No reliable full-project ETA.

**Latest checkpoint, 2026-10-02:** [Synchronous config commands](evidence/model-config-values.md)
now bind native key/provider/model/override headers into supported daily
OpenAI private leases. Eager header-before-key order and selected 401 refresh
are source-backed; normal exit waits for started helpers. Modules 48/0/0 and
Windows 1,625/0/20 pass; actual CAS task/same-Session recall takes 13.524s.
The existing renderer dependency advisory keeps the complete gate FAIL.
Full registry/auth/account, Bun/Linux and other Provider work remain open.
This batch takes about 56min before review/documentation; the final backend
takes 301.553s, including 153s recompilation. Continue grouped module checks
and one stable-source gate; full-project ETA is still unsupported.

## Current user priority — 2026-10-01

The user changes provider ordering: first OpenAI-compatible protocols with a
custom configuration scheme, then OpenAI account login. These two capabilities
are the daily-use target. Other provider compatibility and remaining fixed-source
differences are recorded and deferred. The broader independent Rust ARA/Core,
product boundaries and P0–P6 goal remain unchanged. Deferred work is not accepted
parity and does not advance `ported_through_commit`.

| Module | Current evidence | Next deliverable | Acceptance |
| --- | --- | --- | --- |
| OpenAI-compatible custom routes | Native models.yml selection/CLI overrides/private key resolution now drive Chat/Responses tools and original Session resume. Module/full gate and a CAS artifact/restart task pass. See [daily evidence](evidence/openai-daily.md) and [usage](openai-daily.md). | Keep the usable bounded CLI slice; complete all-provider execution projection/registry in its later parity batch. | Windows 22 module scenes and 1,491 backend tests pass; live configured-route artifact and same-Session recall pass. Full Provider parity stays open. |
| OpenAI account login | CLI device login/logout, private SQLite, refresh/settlement and dedicated Codex SSE route pass synthetic process/wire/restart/new-Session workflows. | Complete actual OpenAI device authorization and a bounded account-model task when user participation is available. Then finish recorded advanced transport/auth contracts. | Deterministic module/full gate passes. Actual account authorization/subscription model remains unverified; account point stays open. |
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

The final daily-use check measured **134.536 seconds**, including cached compile,
four module suites, one shared full gate, inventory, deny and binary build;
the four suites themselves took 2.499 seconds and backend took 125.629 seconds.
The final answer-free CAS artifact task and tool-disabled original-Session recall took 7.032 seconds.
Keep development checks scoped to the changed module; share the full gate only
once the batch is stable. Reuse existing protocol framing and synthetic auth
fixtures before adding a scenario. Earlier failed/superseded checks remain in
the daily evidence; they do not change the final expected outcomes.

The later [ThinkingLoop/recovery batch](evidence/thinking-loop-recovery.md)
reuses the same runner and native OMP detector inputs. Four modules pass 24/0/0
in 7.520 seconds; the complete shared command takes 114.925 seconds with backend
1,499/0/20 and inventory/deny/build passing. The final binary is identical to
the bounded real CAS task/restart binary (11.338 seconds across Runs), avoiding
a duplicate paid trial. Development compilation now builds only the selected
test targets; `--full` owns the comprehensive build/test gate.

The [tool-loop batch](evidence/tool-call-loop-guard.md) reuses original detector
input families and three existing module suites: 20/0/0 in 4.177 seconds.
Its sole final full command takes 194.995 seconds, including 69 seconds rebuilding
all target tests; backend 1,503/0/20 and inventory/deny/build pass. Preferred CAS
completion/original-Session recall takes 9.011 seconds across Runs, with two
controlled tool turns separately recorded from four actual model requests.
Development continues to compile selected targets only. This Core/Host module
does not change the daily Provider ordering or accept actual account login.

## Deferred compatibility register

The [native thinking stream-close cap](evidence/thinking-stream-retry-cap.md)
uses two existing modules (50/0/0) and one shared final gate (1,504/0/20,
293.401s including a 168s all-target rebuild). Module execution takes 4.890s.
Two controlled faults plus five actual preferred CAS requests verify a file
task and original-Session tool-disabled recall in 14.823s. This does not change
Provider priorities or prove live OpenRouter/Copilot faults/account login.
Development checks stay scoped; share the final comprehensive build once.

| Item | Evidence / dependency | Status |
| --- | --- | --- |
| Full cipher expressions and suites | Current native catalog TLS uses ring's named subset. Actual Bun uses BoringSSL; the 45-case investigation demonstrates expression differences from local OpenSSL and a negotiable AES128-SHA suite absent in ring. | Explicitly deferred by provider prioritization; not complete. |
| Ollama Retry-After date parsing | Native RFC2822/RFC3339 parsing misses accepted fixed Bun `Date.parse` values, including RFC850. The source probe documents a >60-second retry decision difference. | Open source-backed review finding, deferred; do not claim complete Ollama retry parity. |
| Other provider-specific auth/OAuth, usage/reserve/rotation | Existing catalogue/store/route foundations do not implement full provider auth lifecycle. | Deferred outside the first OpenAI account slice. |
| Codex advanced transport parity | WS-first, Lite, native provider compaction and complete fixed transport recovery have their own source contracts. Daily-use SSE login must state its supported scope. | Later compatibility slice; account login cannot be labeled full Codex parity. |
| Complete OpenAI account auth | Daily selection uses latest authorization; full credential commands/precedence, reserve/usage/rotation, browser callback and multi-account ranking are not covered. | Open; the current device/SSE route is a bounded Host slice. |
| Codex RPC and full execution metadata | Current Codex CLI supports print/JSON and REPL; RPC is explicitly rejected before journal/auth DB/model work. Unsupported image/native items and execution fields fail explicitly. | Required later parity work, recorded rather than silently approximated. |
| All-provider overrides/registry/discovery and extended metadata | Preserve the existing native model data, factories and manager; do not discard unsupported metadata while projecting a usable route. | Open; wire only what the current OpenAI deliverable needs first. |
| Full registry config/auth callers and async resolver | The synchronous command/live-header primitive and selected OpenAI 401 path are now tested; full load/discovery/runtime overlays, central precedence/rotation and separate Brush/process-tree resolver are not exercised end to end. Rust suppresses raw helper stderr; Bun compatibility and Linux process execution remain unverified. | Required later parity; the bounded command checkpoint does not complete this row. |

Core/session recovery, remaining RPC commands and product integration retain
their existing plans. This provider reprioritization does not silently accept
or remove those independent requirements.
