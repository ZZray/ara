# Remote compaction checkpoint — 2026-10-02

Base `2c63205939729d8e92d563f82eacd590327301d9`, branch `dev`, fixed OMP
`596f2da7101178214aa27a753529d15e6b7ad91d`. See
[evidence](../evidence/remote-compaction.md) and `CTX-REMOTE-01` in the
[ledger](../upstream/feature-ledger.md). Bounded WIP; full remote/Core/Host and
P1–P6 are not accepted. RPC stays 27/42 and the full marker stays null.

## Verified execution — reuse, do not replay

Receipts: `C:\Temp\ara-remote-batch`; module receipts use the platform's
temporary `ara-remote-batch` directory.

- `module-20261002T111811Z`: nine modules 150/0/0, 78.932s serial execution
  plus 11.696s compile. The full invocation later fails on a stale unknown-method
  Session fixture and remains FAIL. Two test inputs are updated to unknown
  `unrecognized`, preserving their no-write/error assertions.
- `completion-20261002T113007Z/receipt.json`: **composite** 1,578/0/20,
  fmt/Clippy/docs/inventory/deny/build pass, source unchanged, 36.332s.
  Reuses 87 complete unchanged targets (1,358/0/20), reruns the entire Session
  and unexecuted testkit/tools/walk (220/0/0), excludes duplicated partial
  Session counts. This is not a successful single full invocation.
- Final `host/run-20261002T113121Z`: five actual Host families pass in
  12.720s on binary `e1fde352…`; four Abort/EOF cases settle in 3–16ms with
  no publication, fallback or extra primary request.
- `live/20261002T110230.980375Z`: preferred CAS `deepseek-v4.1-flash`,
  private Manager-configured direct CAS because the Manager gateway probe
  is unavailable; six actual calls in 18.106s. Generic full/short summary,
  actual write/read and no-tools same-Session reopen produce/recall adapter
  87.5, case 33.0, total 120.5 and label Cedar-6814. All five original raw
  entries stay exact. Binary is explicitly earlier `5620b9f8…`; the only
  runtime delta is an unreachable strict empty/lazy reducer guard for these
  persisted nonempty Sessions. Two fixture updates do not affect runtime.
  No Internet trial on the final binary is claimed or repeated.
- Final manifest has 34 source/test/runner paths, SHA-256 `95b64612…`,
  ten exact upstream files and scoped patch `0df3dc7a…`.
- Independent `/root/remote_review` final POST
  `approve_bounded_wip_checkpoint`, no open blocking findings;
  `final-review.json` hash `ee4e9958…`. Root `root-audit.json` binds the final
  manifest/binary, composite/final Host and original live artifacts. Normalized
  receipt keys directly bind 26 Host and 26 earlier CAS source/prompt paths.
  The complete remote/phase acceptance boundary stays open.

## Time and remaining work

Approximately 09:57–11:31 UTC is 94min implementation/verification. Closure
is separate. Main delays: main-thread Windows stack failures, 43GB of old
generated PDBs exhausting disk, parallel ephemeral-port fixture anomalies,
one real fresh-Session reducer regression and two stale test inputs. All
failed receipts remain retained. Process-local `CARGO_BUILD_JOBS=4` and
`RUST_TEST_THREADS=1` provide the recorded Windows environment. Do not use
module micro-tests or unchanged live tasks to fill waiting time.

Next coherent snapcompact/frame-rescue batch: 120–210min planning estimate
including integration/review. Exact source/27-file preplan manifest is at
`C:\Temp\ara-snap-preplan\upstream`; reuse the original Rust renderer/font
assets and original TS/Rust test families. Keep frame-only token counting,
original image detail, archive migration/persistence and recovery headroom
separate from ordinary user-image semantics. The current DeepSeek route's
vision capability needs live confirmation before a frame-reading task.

Required remote gaps: 401/403 force refresh, V2 reasoning policy, native
special items/Azure/Lite/WS/attestation, exact tokenizer/image details,
complete generic serializer and hooks/settings/concurrency/speculation,
actual V2/overflow Host, exact publication durability injection, native/account
real tasks and Linux. Provider priority: custom OpenAI, then OpenAI account;
other compatibility remains recorded required later work.

Root owns Cargo, private route/network and Git writes. Preserve unrelated
vendor stat/EOL WIP and `.codebase-memory/`. Commit only the manifest's 34
paths and this batch's evidence/plan/roadmap/knowledge/handoff documents.
Scoped commit/push remains authorized; no deployment or history rewrite.
