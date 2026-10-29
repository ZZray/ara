# AI-MOCK complete fixed-source closure

Status: **accepted after independent source and execution audit**. Fixed OMP commit:
`596f2da7101178214aa27a753529d15e6b7ad91d`.

The complete module is also accepted for fixed b91db66e on2026-10-14 after
original25/source-bank replay, both new metadata banks and independent fresh
Codex full audit841eedea. [Upgraded acceptance and representation limits](omp-upgrade-uri-mock-20261014.md#complete-upgraded-ai-mock-acceptance).
This upgrades the existing point, not an eighth old-baseline surface.

## Requirement and delivered scope

Port the complete local `packages/ai/src/providers/mock.ts` (520 lines) and
retain all 25 original `packages/ai/test/mock-provider.test.ts` behaviors.
Production ownership is `ara-ai::providers::mock`, with reference Host bindings
and existing custom API storage in `ara-cli`. HTTP FakeUpstream is separate
test infrastructure and is not the original MockModel.

Seven scoped files export the provider and Host adapter, extract the existing
custom API registration owner, and supply one original25 grouped consumer plus
four source-body groups. The original25 grouped test is one Rust test, not 25.
The additional existing registry target checks shared registration behavior.
No dependency, product, network factory or fixed upstream marker change is
part of this closure.

## Source review and repairs

Independent Codex `/root/print_complete_scope` applied scoped Git/Rust Core
review to the complete provider, original assertions and all seven files.
v1 requested two corrections: cost getter short-circuiting and synchronous
iterator thenable adoption. Both were repaired with actual graph observations
in the existing source test groups. v2 delta review approved both repairs.

The first delivered build stopped before tests after 21.895 seconds, with
six nonexistent `WireString::to_string_lossy` method calls and 418 reused
artifacts. v3 replaces only those typed metadata conversions with the existing
`String::from_utf16_lossy(wire.units())` adapter. Independent v3 delta review
approved the six conversions; original25 and source consumers are unchanged.

Packet: `D:/workspace/env/cache/ara-port-candidates/mock-provider-complete-20261011`.

| Artifact | SHA256 |
| --- | --- |
| `candidate-manifest-v3.json` | `9d7d23359a96134b1ebce3f1bba879c184756d547488927211b59aad9b100b8d` |
| `source-review-v3.json` | `12a2be234bce9db8cedc198a25b5b00a0b45cbcfb7ffb7c5074b788fcd5a075a` |
| `delivered-v3.json` | `729edaa889845d022a49fd298e460241a4a000e4879b5d7c96c5695c943b20a9` |
| Original25 consumer | `89120d87611b33209e331dabe91d9721c2e0770baa90195456ff9f8390ebdcaf` |

The subsequent v3 build stopped at one Host JSON error type mismatch after
114.765 seconds (418 reused, six compiled). v4 changes only that derived
metadata parser to the existing strict UTF-16 parser. Independent source
delta review approved it, preserving all test consumers. Final accepted pins:

| Artifact | SHA256 |
| --- | --- |
| `candidate-manifest-v4.json` | `efb6eb59b8f984fce5c680bfd8dd6da2744fb731f7673853ed85279f723088f3` |
| `source-review-v4.json` | `31fca57fc920456537c1e8e55e389fd911f822f9078acac0cbcd7386f05bb410` |
| `delivered-v4.json` | `08fad13f59774456db1c19a52a871a2039c02df31a67950a97d3b7da413f207b` |
| `execution-v4/build-result.json` | `5e21cc8204440d5aa6ef6ebe25e481e6c19e56e65a3ac0da3e3f63c87e21c629` |
| `execution-v4/module-result.json` | `753e97904bcddc989ed987e02b6d766c59d73b8d538d7f7ca5ff6c5d034ce9a5` |
| `execution-review-v4.json` | `ea3627baceb98014402bb1c92ae61316d4e63f2b58415092f9e4c179933c00be` |
| `EXECUTION-POST-v4.md` | `7fd5ad869fb57cc96442e2031b48084936fa8800e469c4c56cca5d816d6d32b6` |

## Execution

The existing module runner is reused through the D wrapper `run_module.py`.
It runs one combined Cargo build for `mock_provider_complete` and
`model_registry_extensions`, followed by actual binaries with serial test
execution. Environment: Cargo/toolchain/target/TEMP/TMP on D, jobs=2.
The delivered manifest, workspace source pins, binary hashes, test listings,
counts, stdout/stderr and build timing are retained in `execution-v4`.

Final combined build passes in 254.916 seconds: 418 reused, 12 compiled,
zero errors. Both executed binaries are newly built from the delivered E
sources. Mock target passes 5/5 test functions; registry target passes 1/1,
zero failed/ignored/filtered. Receipted test execution totals 0.141 seconds;
runner totals 256.718 seconds. Original25 unique begin/pass IDs and complete25
are all present, together with six source-body pass markers. The actual
registered wrapper/simple calls, native original error identity, callback
gate, raw content, short-circuit/adoption corrections, same-model NativeRun
replacement and old in-flight ExpiredCapability without replay are exercised.
Delay cancellation evidence is the original pre-aborted script; an additional
mid-delay race is not claimed. Per-original associations are in
[the 25-row mapping](mock-provider-surface-20261011-behaviors.tsv).

Independent Codex `/root/print_complete_scope` approves point acceptance:
1,667 hash checks, zero mismatches, including all 1,639 build source pins,
seven delivered/frozen files, binary hashes, exact binary test listings and
Git index. Root read actual module stdout/stderr and critical assertion ranges,
confirmed source/review pins and accepted only AI-MOCK. The inventory therefore
advances to **3 accepted, 1 tested, 25 implementing, 82 open (111 total)**.

Three build attempts total 391.576 seconds; tests total 0.141 seconds. The
delivery helper now preserves identical file timestamps to prevent unnecessary
Cargo invalidation in later corrections. This optimization does not claim a
measured speedup for the already completed v4 build.

## Representation and acceptance limits

Rust ports translate class methods, handler functions and iterator futures;
native Context/options/content/tool arguments/error handles remain original.
The graph model metadata is stable, and `.model()` returns the same Rust
instance. This does not implement a general ECMAScript class or generator
engine. A Host AbortController bridge binds the original signal to its token.
The reference Host catalog accepts scalar Unicode identifiers; its derived
identity alone is imported. Typed metadata uses a derived UTF-16 adapter.

Actual entry points are `.stream`, `stream_mock`, registered stream/simple
functions retrieved from the production registry, and `stream_actual`.
The typed `ModelProvider::stream` facade explicitly rejects unsupported graph
transport and is not evidence of source parity.

This intentionally local mock module needs no HTTP/CAS trial. Full workspace
fmt/clippy/tests and complete AI-REGISTRY, Extension, P1-P6, products and release
are outside this module receipt. All 84 compiler warning records are retained;
the new mock provider/Host/test paths emit no compiler warning. This is not a
zero-warning workspace or clippy claim. Dependency advisory and doc tests are
unrun. No commit, push, installation or deployment
is performed. Full OMP parity remains open and `ported_through_commit=null`.

Inventory consistency and scoped `git diff --check` pass. The existing full
bootstrap documentation scanner was stopped while traversing the whole
repository; its required-file/marker/placeholder/Skill/link rules were reused
on the eight changed pages. All new Mock links pass. Three unchanged historical
links to the old `C:/Temp/ara-model-fallback-batch/.../STATUS-20261006T0448Z.md`
remain outside the repository and unresolved in roadmap/plan/ledger; this is
not a full documentation-gate pass. Receipt: packet `documentation-check.json`.
