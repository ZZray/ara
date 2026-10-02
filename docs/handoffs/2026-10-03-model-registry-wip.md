# Handoff 2026-10-03 — production registry WIP

## Final validation checkpoint, 2026-10-02 21:13 UTC

[Production Registry evidence](../evidence/model-registry.md) now records the
frozen-source backend 1,707 PASS/0 FAIL/20 ignored, fmt/Clippy/inventory/build
PASS, unchanged 35 scope/57 vendor hashes and the existing dependency advisory
FAIL. Final gate is `gate-20261002T210427Z`, binary SHA256
`7817f93b6ab58f23e1779d545f9b238d3fc6ee280144c6c75f443a9ac42b485e`.
The intermediate `gate-20261002T205641Z` fails at the existing Proxy resume
assertion; pure Proxy validation is restored before readonly journal lookup.

Current preferred CAS trial `live-20261002T211014Z` PASS: 14.278s/five calls,
exact three read/write/read tool receipts, correct actual JSON, original-Session
tool-disabled recall, matching key/header proofs and source/binary hashes.
Root directly checks artifact/tool/Session receipts. Independent final
code/gate/live-artifact POST approves the bounded WIP with no unresolved finding.
No actual OpenAI authorization, full Auth/OMP/phase acceptance or new commit/push
is claimed. Counts below remain unchanged; next close evidence/plan/Git WIP,
then the recorded full Auth/selector/Host contracts. The goal remains active.

## Latest continuation, 2026-10-02 20:57 UTC

The prior state below is retained as history. Production Codex account binding,
exact-row discovery refresh and grouped runtime/account scenarios are now in
the worktree. They remain WIP; no new complete surface or phase is accepted.

Startup repairs restore the existing Host contracts: authored auth ownership
is checked before account storage and Registry helpers; the resumed Session is
read once to establish effective helper cwd, and the same journal is bound and
recovered only at the original write point. Ordinary routes without an account
database retain environment credentials. Existing accounts and selected Codex
routes share one service with the request factory and owned exit settlement.

Executed receipts under `C:/Temp/ara-registry-compose-batch/production-registry`:

- `module-20261002T203141Z`: first grouped run, 164 PASS/two Host FAIL. Illegal
  Codex configuration executed a helper before rejection; resumed helpers used
  launch cwd. Both assertions remain strict and the regressions were repaired.
- `module-20261002T204325Z`: existing Host group, 5 PASS/zero FAIL, source unchanged.
- `module-20261002T204517Z`: selection 8 PASS; daily CLI 5 PASS/one FAIL. An
  ordinary invalid configuration had created an unrelated account database;
  conditional account binding repairs that effect.
- `module-20261002T204755Z`: selection 8, daily CLI 6, loader 5, discovery 6,
  runtime 4, extensions 1, references 1 and production Registry 4 PASS. Account
  group 7 PASS/one FAIL: seed identities collapsed into one stored row.
- `module-20261002T205101Z`: explicit seed email repair alone still FAILS because
  shared JWT profile email remains an identity intersection. The native store
  is unchanged. Initial explicit and JWT emails now both identify each account;
  refreshed profile normalization and every row/grant assertion remain intact.
- `module-20261002T205158Z`: accounts 8 PASS/zero FAIL, source unchanged. The nine
  remaining families jointly have 43 PASS on their recorded snapshots; do not
  treat that composite as the final snapshot gate.

Independent Codex `/root/remote_review` directly checks source, affected Host
and account receipts, and all 57 preserved vendor hashes. Narrow PRE/POST passes.
Account fixtures use the actual bundled Codex fingerprint for fresh SQLite
catalogue rows; no synthetic bearer is intentionally sent to public discovery.

Final gate attempts `gate-20261002T205227Z`, `205434Z` and `205553Z` stop at Clippy,
before tests. Mechanical alias/Option/struct/slice/lock-scope fixes retain the
original behavior and assertions. Grouped all-target Clippy with `--keep-going`
passes in `clippy-grouped-final.log`. Candidate manifest binds 35 scope files
and the unchanged 57 vendor files. Stable gate `gate-20261002T205641Z` is running;
no real-model task, new commit or push is claimed in this checkpoint.

Complete AuthStorage ranking/usage/sticky/session-pin and in-memory block
contracts, stored login/static-key fallback, advanced account transport,
unbound selectors/Host surfaces and other Providers remain required. Normal
request authentication deliberately retains its existing LatestInteractive
selection; exact-row discovery does not accept full native AuthStorage parity.

## Current progress

Fixed OMP `596f2da7101178214aa27a753529d15e6b7ad91d` remains the target.
Base HEAD is `10e027da122e24df8accc773bede7b8ba2e3c962`, branch `dev`.
The current registry worktree is not accepted, committed, or pushed.
Preserve the 57 unrelated vendor/EOL changes and `.codebase-memory/`.

Authoritative roadmap/ledger counts remain 111 surfaces: 26 implementing,
one tested, 84 open, zero complete acceptances. Started coverage is 27/111
(24.3%). The 60 recorded bounded points have 35 limited acceptances,
23 implementing, two tested. Their 58.3% accepted fraction is not an
overall reproduction completion percentage. P0/V1 accepted; P1–P6 open;
RPC 27/42; full upstream marker null.

At 2026-10-02 20:02:20 UTC the active goal timer reports 214,633 seconds
(59.6 hours, including waits), and this registry batch has run about
130 minutes since 17:52:41 UTC. Full-project ETA remains unsupported.

## Latest repair and executed evidence

Production loader/discovery/runtime extension and static composition are
connected in the current worktree, but remaining contracts below stay open.
The earlier grouped registry run has 12 PASS, four CLI timeout FAIL,
two oracle cases ignored; its failure prevented the static target running.
Log: `C:/Temp/ara-registry-compose-batch/registry-seam-modules.log`.

One isolated loopback Chat task completes its model request at about 1.74s
but takes 30.736s to exit. Temporary stage diagnostics identify roughly
13.9s of built-in discovery and 15.1s of publication; all temporary source
diagnostics have been removed. The original shared-payload optimization
does not establish a meaningful speed benefit. Fixed name-expression
reuse alone gives 29.329s; fixed private-host expression reuse gives 25.512s.

The direct exit regression is an added unconditional whole-catalog shutdown
wait. Fixed main.ts:1855–1862 starts discovery after Session construction;
print main.ts:2102–2115 and the checked dispose/SDK/runtime/lifecycle/quit
chain do not await the catalog producer on exit. Rust now starts background
discovery after Host/Session/provider construction and removes this extra
exit wait. Required selected-provider discovery and owned account/config
helper settlement remain. The same bounded task now exits in 1.884s after
one completed model call and the expected output. Retained receipts:
`C:/Temp/ara-registry-compose-batch/cli-diagnostic/after-native-exit-order.json`
and the matching before/after regex JSON receipts in that directory.

Executed current checks with `CARGO_BUILD_JOBS=4`:

- `cargo build -p ara-cli --bin ara`: PASS.
- `cargo test -p ara-cli --lib model_`: 57 PASS, 0 FAIL; 75 unrelated tests
  filtered. Log: `cli-policy-modules.log` in the batch directory.
- `cargo test -p ara-cli --test openai_daily_cli --test static_model_registry`:
  CLI 5 PASS (5.59s), static 7 PASS (1.62s).
  Log: `cli-exit-order-repaired-modules.log`.
- The first repaired CLI run is retained as 4 PASS/1 FAIL: the new fixture
  mistakenly assumed the Session file begins with its header. The actual
  file begins with a native title slot. The grouped fixture now requires
  exactly one `type=session` header with the original ID; strict tool receipt
  and Session file counts are unchanged.

Independent Codex `/root/remote_review` reviews the fixed source and current
startup/exit, fixed-expression, and fixture hunks. Root directly verifies
the critical source and actual logs. The narrow reviews pass; they do not
constitute the final registry POST or complete acceptance. No new full gate
or real-model task is run in this narrow repair turn.

## Remaining work

- Add native standard/catalog-only exclusion for providers with a runtime
  Manager; special provider filtering keeps its separate upstream rule.
- Serialize ordinary reload with extension mutations; inspect installed-key
  auth observation and reload ownership.
- Extend existing grouped scenarios for file key A → runtime key B → reload A,
  retained modifiers seeing other provider cache-only rows, and final llama.cpp
  fixups. Do not add a separate micro-test family for each mapping.
- Bind current OpenAI account credentials through production Registry/Host
  interfaces. Opaque extension object invocation is not CLI automatic API
  dispatch or actual OAuth account acceptance.
- Execute affected modules, then one stable batch backend/dependency gate,
  final independent POST/Root artifact audit, and one bounded current preferred
  CAS task. Keep the existing renderer dependency advisory failure visible.
- Update evidence, ledger and plans before scoped WIP delivery. Preserve all
  previous accepted subsets and remaining mandatory fixed OMP contracts.
