# AI-MOCK complete closure — 2026-10-11

## Accepted checkpoint — 2026-10-11 08:18 Asia/Taipei

**AI-MOCK accepted; formal total 3/111 (2.7%)**: 3 accepted, 1 tested,
25 implementing, 82 open. v4 build PASS254.916s (418 reused/12 compiled),
original25 plus four source groups5/5 and existing registry1/1 PASS;
tests0.141s, zero failed/ignored/filtered. Independent Codex execution/point
audit APPROVE:1667 hashchecks/zero mismatches. Root read actual stdout/stderr,
review reports and critical negative/registry/NativeRun assertion ranges.
[Final mapping, receipt hashes and limits](../evidence/mock-provider-surface-20261011.md).
Earlier checkpoints below are history, not current state. Full workspace,
other surfaces and P1-P6/products/release remain open; no commit/push/install.

## Historical candidate checkpoints

Checkpoint: 2026-10-11 07:39 Asia/Taipei. Formal complete acceptance is
**2/111 (1.8%)**: 2 accepted, 1 tested, 26 implementing, 82 open.
Accepted surfaces are AGT-APPEND-CTX and CA-TOOL-EDIT. WRITE is 174/196
original observations (88.8%); its remaining 22 require actual Unix execution.
No current Cargo execution or new AI-MOCK acceptance is claimed.

Checkpoint: 2026-10-11 08:00 Asia/Taipei. Seven candidate source paths are
frozen in `candidate-manifest-v1.json` (SHA256
`bdb3e86653a26fc6a737cdda70e8496caa403732322ca5bfca0efd83b19fdf5f`).
Independent Codex `/root/print_complete_scope` completed SOURCE POST of the
full provider and seven files: REVISE with two P2 findings, cost getter
short-circuiting and synchronous thenable adoption. Root read the fixed OMP
source and both candidate hunks and confirmed both. Author is preparing one
combined v2 repair with targeted graph observations in the existing test child;
the original25 main consumer remains frozen. No Cargo or acceptance yet.

Checkpoint: 2026-10-11 08:04 Asia/Taipei. v2 source delta received independent
APPROVE and the seven paths were guarded-applied to E. The first combined
Cargo build failed in 21.895 seconds with six new mock `WireString` method
errors, before tests; 418 artifacts were reused. v3 changes only the six typed
metadata string conversions to the existing UTF-16 adapter and is frozen for
delta review. No test PASS or additional accepted surface is claimed.

The v3 build stopped in the new Host JSON error adapter (114.765 seconds;
418 reused, six compiled). v4 changes only that parser adapter to the existing
strict UTF-16 parser and received independent delta APPROVE. Its combined
build is running. Root also corrected the D delivery helper to skip writes
whose bytes are already identical: rewriting all seven files was refreshing
source timestamps and making Cargo recompile unchanged dependents. Current
running build is retained; later source corrections will preserve those
timestamps. Formal acceptance remains 2/111 until executed evidence passes.

Fixed source is OMP `596f2da7101178214aa27a753529d15e6b7ad91d`:
`packages/ai/src/providers/mock.ts` (520 lines) and
`packages/ai/test/mock-provider.test.ts` (25 original behaviors).
Existing ara-testkit FakeUpstream is an HTTP fixture, not the original MockModel.

## Scope and current work

- Preserve existing providers, the v22 runtime repairs, unrelated WIP and Git index.
- Implement the complete actual graph mock provider in ara-ai and connect the
  existing Host custom API owner to a real dispatch consumer.
- Use existing Native callable/Promise adoption and classification bindings;
  do not add another runtime driver or global registry.
- Keep READ and Print scope findings for later implementation. They lack whole
  production modules and are not immediate complete closure candidates.

Author `/root/mock_provider_complete` prepares a D-only candidate; independent
Codex `/root/print_complete_scope` performs PRE scope review. Root has read the
proposal and existing RuntimeApiRegistration/register_provider owner.

Independent PRE is APPROVE scoped implementation to proceed; Root read its
final addendum. Report `PRE-REVIEW.md` SHA256
`64fcb6328ba9b9ab7cd912bace70b226db8cfb8ecb0e823742d7dcbae80ac57e`.
This is design approval, not module acceptance. The six-path candidate must
prove actual versus typed entry reachability, asynchronous iterator ordering,
original graph identity, callback/error behavior and production custom API
dispatch. Registration storage alone is insufficient. A yielded undefined
response must not be confused with source exhaustion, and custom next must
not be artificially serialized across await. No generic network factory
rewrite or live model trial is required for this intentionally local provider.

## Time and verification

Author estimate: 60–90 minutes for the complete candidate, excluding compilation
and independent execution audit. This is an estimate for this module, not a
full-product ETA. Report expiry or scope changes rather than renewing silently.

One frozen module composition will run original25 and the additional source
branches required by the full provider. Review applies to the same candidate;
only failed or affected checks need rerunning after corrections. Do not rebuild
v22 or run an unchanged comprehensive gate to manufacture progress.

Root prepared `run_module.py` on D, reusing the existing module runner; its
plan-only command selects `cargo test --locked -p ara-cli --all-features --test
mock_provider_complete --no-run --message-format=json`. Only the build plan
has executed; no Cargo or test PASS is claimed.

Previous v22 build: 571.062 seconds, 414 reused/26 compiled; tests42.901 seconds;
runner619.127 seconds, five groups45/45 PASS with independent execution audit.
Its receipts are in [WRITE handoff](2026-10-11-write-closure.md).

Cargo/cache/toolchain/target/TEMP/TMP remain on D, jobs2. E migration is cancelled.
No commit, push, installation or deployment is authorized in this closure.

Candidate source/proposal location:
`D:/workspace/env/cache/ara-port-candidates/mock-provider-complete-20261011`.
