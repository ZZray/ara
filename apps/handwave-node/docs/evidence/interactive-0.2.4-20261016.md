# HandWave 0.2.4 interactive pipe processes — historical stage

Superseded for current native terminal delivery by
[PTY/ConPTY evidence](terminal-0.2.4-20261016.md). All results below describe the
pipe-stage snapshot and its then-unpublished status, not the final release.

Scope: compact standalone Host from Rust base `6e43dba`, maintained Go base
`a2500346af569857fe47777c825c5332acb2e360`. This point adds same-process input,
EOF and a folded console. Root Rust Core/OMP migration, authentication, model
routing, device grants, avatars and unrelated services remain outside scope.
The user approved an illustration direction; no new avatar was generated or
installed as part of this process capability.

## Contract and source ownership

- `managed_jobs.rs`: optional pipe input, original Goal/owner/deadline binding,
  authenticated local worker IPC and durable input intent/ACK/unknown receipts.
  Old noninteractive Spec serialization and hashes are preserved.
- `job_control.rs`, `transport.rs`, `goal_runtime.rs`: authenticated controller
  commands, cancellation fences and original-lease checks. Direct input is
  separate from model/chat history. Worker nonce is stripped at public boundaries.
- `agent.rs`, `execution.rs`: input guidance, read-only query, and ordinary
  Execute follow-up to an existing leased process without new spawn authority.
- `config.rs`, `main.rs`, Cargo manifest/lock: capability negotiation/module
  registration and version0.2.4; no dependency addition.
- Maintained Go `job_console.go`/tests, `proto.go`, and exactly two Hub routing
  cases: live paired-controller/device/Goal/Session checks and transient relay.
- Ray console JS/CSS/tests and reviewed embed: default-folded card, bounded
  logs, password input, EOF/cancel, receipt-only refresh recovery, no model poll.

The clean Go export at
`D:/WorkSpace/tmp/handwave-interactive-20261016/go-source` uses the exact base
plus this allowlist. It excludes dirty legacy auth/Web; the published stable
Web archive is hash-checked before embedding. `go-build.json` pins each overlay.
No Git index, other worker changes or production installation is altered.

## Executed checks

Evidence root: `D:/WorkSpace/tmp/handwave-interactive-20261016/`.

| Check | Actual result and boundary |
| --- | --- |
| `cargo fmt --manifest-path apps/handwave-node/Cargo.toml -- --check` | PASS; standalone product workspace only |
| `cargo clippy --manifest-path apps/handwave-node/Cargo.toml --locked --offline --workspace --all-targets --all-features -- -D warnings` | PASS |
| `cargo test --manifest-path apps/handwave-node/Cargo.toml --locked --offline --workspace --all-targets` | Final144/144 PASS, zero ignored;8.06s execution. Binary-only workspace has no library doctest target. |
| `cargo deny --manifest-path apps/handwave-node/Cargo.toml --offline check` | Advisories/licenses/bans/sources PASS with local cached advisory DB; no fresh-network advisory claim |
| Static-CRT Windows release | PASS;5,039,104bytes, +77,312bytes from published0.2.3 |
| Clean Go Hub/Ray tests, vet, rewritten stable app `node --check`, Windows builds | PASS; Hub78.652s, Ray4.351s |
| Console DOM/transport |8 groups PASS, including lost ACK/reload metadata, EOF, scope switch, duplicate packets and A->B->A log generation |
| `verify_interactive.py` on final EXE + actual Go Hub + actual Python |4 groups PASS; same worker/child across two inputs, UTF-8, duplicate/conflict, read-only ACK lookup, reconnect, cross-Session denial, EOF and normal Goal resume; ordinary Execute follow-up cannot start a new Job.6 controlled provider calls across both tasks. |
| `verify_jobs.py` on final EXE | Actual detached worker, output/artifact, duplicate invocation, descendant cancel, deadline/lease/quota and unknown-recovery regressions PASS; exact groups in jobs-regression/result.json |
| `verify_long_goals.py` on final EXE + actual Hub |4 groups PASS; detached Goal slices, root budget, review/cancel receipts, managed wait and model identity |
| Real `deepseek-v4.1-flash` / `openai-completions` | Exact live model/protocol inventory verified. One bounded regression passed in27.251s:4 HTTP attempts,3 fresh-granted slices, one actual Python Job, stdout/read_file verification of sum42/count3, awaiting_review. Waiting Job made no provider polling calls; supplier usage stays unknown. This actual-model trial tests noninteractive Job regression, not model selection of interactive login flows. |

Final EXE SHA256:
`d84c475f774716904fa61d0412c516288c5c67d905638adf30d1fc3561e216f5`.
Clean Hub EXE SHA256:
`ca848493749146479c81d675c3a91d4ebd66e054891bb16d4e905841080f867e`.

Local test ZIP (not published):
`apps/handwave-node/dist/handwave-node-x86_64-pc-windows-msvc.zip`,3,669,865bytes,
SHA256 `95da7ec3fe696a30f136092e6fb1991be4aacf38d45b69f9c6204360b22e06d3`.
Its extracted EXE equals the tested hash above; the archive contains the explicit
distribution allowlist/licenses and no private node.conf. Scoped source/package
pins are in `final-source-package.json` under the evidence root.

The144 Rust tests include real Windows Python multi-input/EOF and immediate
exit ACK; a slow IPC client cannot block cancellation. A deterministic partial
writer writes two bytes then fails: the durable result is unknown, stdin closes,
and repeated input does not write again. Wrong owner/Goal, original lease
expiry, input quota, invalid nonce and legacy Spec hash tests also pass.
Controller-secret test values are absent from native journal/Job metadata/logs
and actual controlled provider requests; the target fixture's requested output
file intentionally contains its accepted input.

## Browser and review

The actual console component was exercised in the in-app browser using a local
controlled transport preview: initially folded, expand, submit a password field,
clear body, receive ACK, EOF disables input, reset, and dark/light375px container.
Screenshots are `browser-dark.png` and `browser-light-narrow.png` in the evidence
root. These are real browser interactions with simulated receipts, separate
from the actual Rust/Hub process acceptance. They are not authenticated production
HandWave or physical-mobile viewport acceptance. No browser credential was used.

Independent Codex `/root/interactive_review` reviewed the original plan and final
Rust/Go/UI scope using `ara-git-review` and `ara-rust-core-review`. It reproduced
and requested fixes for stale log selection and Execute resume bypassing a
persisted canceled dispatch during a crash window. Both are fixed with focused
tests. The reviewer ran console8 groups independently and reported no remaining
source blockers; final execution receipts are reviewed separately.

After independently checking the final logs, source pins, process scripts,
package hash and browser screenshots, the same reviewer accepted the bounded
local Windows PIPE software point. It explicitly excludes production release,
PTY/ConPTY, other platforms and full real browser-to-Rust acceptance. The
Node's owning knowledge links and the related Go index resolve (310 local
Markdown links checked); unrelated root bootstrap results are separate.

## Preserved failures and limits

- An initial expanded fixture sent a normal Execute with a mode incompatible
  with its already bound native Session, so the continuation was correctly
  rejected. The corrected fixture preserves the existing Smart binding and
  passes; no product guard was relaxed. Both integration-final and
  integration-followup evidence remain.
- The first cargo-deny command used an unavailable `--disable-fetch` flag and
  failed before checking. The supported cached `--offline check` then passed;
  both logs remain. Initial143-test results precede the final144-test snapshot.
- Root `scripts/verify_bootstrap.py` traverses unrelated Markdown/build trees;
  its bounded attempt was stopped without a result. This is not a bootstrap
  pass. The310 owning-page links pass the separate scoped check. Root OMP/WIP
  was not edited to accommodate the product documentation check.
- Windows Python PIPE is exercised. PTY/ConPTY, terminal cursor/resize/Ctrl+C,
  PowerShell Read-Host, Unix/macOS interactive execution and all installer/CLI
  compatibility are not accepted by these Windows checks.
- Input ACK proves a write, not that the child consumed it or authenticated.
  Third-party programs may echo input. OS-user access is not a sandbox.
  Process restart does not resurrect a lost child or replay unknown effects.
- This is a locally verified development version. No commit/push, public
  package update, installed device replacement or production deployment occurred.
  Published0.2.3 remains the current public installation until a release step.

No root OMP feature ledger or upstream marker is advanced by this Host point.
