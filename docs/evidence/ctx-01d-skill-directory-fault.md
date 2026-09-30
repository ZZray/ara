# CTX-01d: Skill directory I/O fault evidence

## Requirement and scope

Fixed OMP `596f2da7101178214aa27a753529d15e6b7ad91d`,
`packages/coding-agent/src/internal-urls/filesystem-resource.ts:16-24`,
awaits the complete `fs.readdir(..., { withFileTypes: true })` result before
building immutable directory text. ARA's Skill directory path must not
report a successful partial listing after an enumeration or entry-type
error. The production repair is already in `e96dc5a`; this experiment
tests that path through the actual Rust CLI, provider continuation and
Session journal.

WIP `985a4ca79899cf1909442af7c38ac3e210a413e8` adds only:

- `scripts/ctx_skill_directory_fault.c`: Linux GNU test interposition.
- `scripts/ctx_skill_directory_fault_trial.py`: three isolated host trials.
- `.github/workflows/ctx-skill-directory-fault.yml`: an Ubuntu 24.04 runner.

`git diff 7f658f0 985a4ca -- crates Cargo.toml Cargo.lock` is empty.
Ordinary-directory behavior, production code and dependencies remain at
the previous snapshot. These are controlled I/O faults, not a new
production filesystem adapter.

## Injection ownership and environment

The reviewed dependency path is Tokio 1.53.1 and Rust 1.97.1 std commit
`8bab26f4f68e0e26f0bb7960be334d5b520ea452`. Tokio prefetches up to 32
entries during `read_dir().await` and caches `std::DirEntry::file_type().ok()`.
An entry-type error can be discarded during prefetch and retried by the
public `file_type` call. The experiment therefore requires persistent
metadata EIO and at least two actual injection hits.

The shim exports only `opendir`, `readdir64`, `closedir`, `statx` and
`fstatat64`, resolving the original functions through `RTLD_NEXT`. GNU
symbol labels keep the last two hooks independent of libc's nonnull
declaration, preserving Rust's null-path availability probe. A finite
mutex-protected handle table records `DIR*`, dirfd and generation. Logging
uses an inherited regular-file fd and raw syscalls, with original errno
preserved and injected EIO set last.

Each case has a fresh work directory, home, Session and controlled Chat
SSE server. The fixture contains two directories and ten ASCII files in
`.ara/skills/probe/references`. Its arm-file is absent at CLI start. Only
the first model request creates it; the response requests
`read skill://probe/references:raw`. Successful exact-path `opendir` calls
after arming are registered. Metadata injection additionally requires the
registered dirfd, `probe-type.txt` basename and an actual `DT_UNKNOWN`
alteration. Startup discovery and the directory's initial metadata query
cannot supply those hits.

The workflow pins Ubuntu 24.04 x86_64 GNU and the reviewed Rust commit,
builds the unchanged CLI with `--locked`, and compiles the shim with:

```sh
cc -std=gnu11 -D_GNU_SOURCE -shared -fPIC -O2 \
  -Wall -Wextra -Werror -pthread -Wl,-z,defs -Wl,-z,now \
  scripts/ctx_skill_directory_fault.c -o libctx_dir_fault.so -ldl
```

The trial saves OS/glibc/cc/rustc versions, ELF headers, dynamic symbols,
resolved function addresses, exact std/Tokio sources, source snapshot
hashes and binary/shim hashes. Missing symbols, hits or scope ownership
fail the experiment. The normal control first probes actual GNU hook
activity; source inference alone does not count as Linux execution.

## Required observations

| Case | Required host and hook receipt |
| --- | --- |
| Observe | All twelve actual entries, complete EOF, exact listing text, one successful Session receipt, same text in the next request |
| Enumeration | At least one real entry followed by `readdir64` NULL/EIO, one error receipt, no successful partial listing |
| Type | Actual regular entry forced to `DT_UNKNOWN`, at least two matching `statx`/`fstatat64` EIO hits, one error receipt, no default-to-file success |

Every case requires exactly two model requests and one `read` receipt.
The CLI `tool_execution_end.isError` and Session `toolResult.isError` must
agree. Chat wire history has tool text and call identity, without an
`isError` field; its second request must contain the same checked text.
Run limits are two model calls, 30 seconds and 128 output tokens per call;
the Python process bound is 40 seconds with process-group termination.
Both successful and failed events, requests, journals, hook logs, fixture
manifests, summaries and hashes are retained. Faults happen after OS
`opendir` success, possibly during Tokio prefetch; this does not claim they
happen after Tokio `read_dir().await` has returned.

## Execution status

Python compilation, `--help`, workflow YAML parsing, documentation
bootstrap, fixed OMP inventory and scoped whitespace checks passed locally.
Independent Codex `resource_scan_plan` reviewed the implementation plan;
`ctx_point_audit` found no actionable defect in the initial static design.
Final independent Codex `oracle_diff_review` reviewed all three files,
skipping none, and found no actionable defect. It ran Python compilation,
YAML parsing and scoped whitespace checks; Linux execution was left to CI.

Linux fault workflow [36646277152](https://github.com/ZZray/ara/actions/runs/36646277152)
passed on `985a4ca`. The actual runner was Ubuntu 24.04 x86_64 with glibc
2.39 and the exact reviewed Rust commit. Strict C compilation succeeded
and all five original functions resolved. The raw receipts show:

| Case | Actual entries | EIO hits | Receipt |
| --- | --- | --- | --- |
| Observe | 12, then complete EOF | 0 | Exact listing, `isError=false` |
| Enumeration | 1 before injected failure | 1 `readdir64` | `Cannot list …: Input/output error (os error 5)`, `isError=true` |
| Type | 12; selected regular entry type 8 changed to 0 | 2 `statx` | Same EIO error, `isError=true` |

Every case exited 0 after the controlled model acknowledged the tool
result, with two HTTP requests, one tool receipt and zero upstream
assertion errors. Thus Run completion does not mean the faulted tool
succeeded. The first type EIO occurs before EOF during prefetch; the
second occurs after EOF during the public retry. Both match dirfd 10,
generation 1 and `probe-type.txt`.

The downloaded artifact is at
`C:\Temp\ara-ctx-directory-fault\ctx-skill-directory-fault-linux-gnu\host-nch4acf_`.
Its summary SHA-256 is
`8d7ea3c6ab7bd0f291cb9b49318ab9bc5ff7161b11cf812999141b5800a06730`;
archive SHA-256 is
`facd810e8caffe1162b6ac9023bd4409fba915f72b1dd73f38e34b98ebbe5961`.
Parent rehash found that the uploader's default excluded the generated
hidden `.ara` fixtures. Raw host/error receipts are valid, but the
downloaded artifact cannot verify every declared fixture hash.
WIP `d05dab0b34d50878307ed7542ff3a67975012ba1` adds only
`include-hidden-files: true`. Independent Codex `oracle_diff_review`
verified the one-line correction against the uploader's actual input
definition and found no actionable defect. No test expectation or
production code changed.

### Final uploaded artifact on `d05dab0`

[Fault workflow 36646646225](https://github.com/ZZray/ara/actions/runs/36646646225)
passed all three cases again on `d05dab0`. The parent downloaded its
artifact and independently recomputed all **100** declared file hashes,
including the hidden Skill fixtures; every byte count and SHA-256 matches.
The Linux CLI and shim hashes are identical to the first run. Each case
again has exactly two requests, one tool receipt and the required
12/1/12 actual entries with 0/1/2 EIO hits. Case elapsed times are
0.570/0.550/0.553 seconds, with each process exiting 0 after its checked
tool result and final `stop`.

Final artifacts:
`C:\Temp\ara-ctx-directory-fault\run-d05dab0\ctx-skill-directory-fault-linux-gnu\host-7jsywq3a`.
The workflow's reproducible harness command is:

```sh
python scripts/ctx_skill_directory_fault_trial.py \
  --binary "$GITHUB_WORKSPACE/target/debug/ara" \
  --shim "$RUNNER_TEMP/ctx-directory-fault/libctx_dir_fault.so" \
  --output "$RUNNER_TEMP/ctx-directory-fault" \
  --std-source "$std_source" --tokio-source "${tokio_sources[0]}"
```

The variables name the exact installed Rust `sys/fs/unix.rs` and fetched
Tokio 1.53.1 `src/fs/read_dir.rs`; their original bytes are retained in
`environment/`. Build, toolchain and harness logs are retained alongside
the case receipts.

| Final artifact/source | SHA-256 |
| --- | --- |
| Summary | `7da6a3693a71afaef14f2b6defa8c9c9904cc14045f21053b10e1f105990dc1b` |
| Downloaded archive | `2a562a8b259a888e69fe5ea1877693e643acbc11a8f8ce4cfb276be766aac561` |
| Linux CLI | `f9b3d286bf28ae078d35bbaf43213099fd8f4d69a99da35d78d527b19df68343` |
| Linux shim | `cefd5eb10b4507f874c22aa79a023bce49e38aba3a8660c0b6dea34224538383` |
| C shim source | `d116328e9782b2ff806901ace6c1867b5838bfcd3be4f041b7e0277413013d2e` |
| Python source | `a53cf741f8c6ecbaa371fc69610e3ed41c6cc8b73ee362a8e91c5b02b62ae40d` |
| Final workflow | `1bc2d75b7a0c43e45940b37a3d1226841b9bad3854ae028066d471693a76b5a0` |
| Rust std source | `02bd45fd5f2e9d8b08122630f8febf15354c1784c7f6550d8cfbfb967cebf30d` |
| Tokio directory source | `b6777ba0d0261a30226a814fd9be19b22a096153be3b5cbfc15c22819efc73af` |

Repository checks on `985a4ca`
[36646276985](https://github.com/ZZray/ara/actions/runs/36646276985)
passed. The final delivered `d05dab0` repository gate
[36646646443](https://github.com/ZZray/ara/actions/runs/36646646443)
passed owned formatting, strict workspace Clippy, all-feature target tests
and documentation tests: **1,051 passed, 0 failed, 1 ignored across 74
suites**. Bootstrap and fixed OMP inventory checks also passed. Its backend
log is
`C:\Temp\ara-ctx-directory-fault\run-d05dab0\logs-36646646443\verify\7_Run python scripts_verify_backend.py.txt`,
SHA-256 `5e54fb641948f9bfd8bfe4b548c768f30e94c648e8c2019b9ce915510b633156`.
This CI does not supply selector-oracle JSON, so the 22 renderer
comparisons retain their explicit Windows invocation evidence. The
dependency policy receipt is unchanged from the scan repair; this slice
has no dependency or lockfile change.

## Bounded real-model normal path

Authenticated B.AI `GET https://api.b.ai/v1/models` was checked immediately
before use: 58 models, with `deepseek-v4.1-flash` supporting the OpenAI
endpoint. The actual Rust CLI ran OpenAI Chat Completions with the key
read from `BAI_API_KEY` through `--api-key-env`, never printed or saved.
The binary was rebuilt on `7f658f0`; its Rust sources are byte-identical
to `985a4ca`, as shown by the empty scoped diff above.

The task reads the discovered `proof` Skill and its directory, raw tail
and raw disjoint ranges, then writes `PROOF.json` under the owner rule
found only in `AGENTS.md`. The directory includes `z-dir/` before files
and the observed `a_1.txt`/`a-1.txt` punctuation pair. Bounds: six model
calls, 180 seconds and 1,024 output tokens per call; only `read` and
`write` are enabled. The parent independently checks exact tool text,
paired receipts, artifact JSON, Session content and final `stop`.

Final run `C:\Temp\ara-ctx-directory-fault\real-model-794uom1k` passed:
exit 0, four model calls, six paired tool receipts, 13.074 seconds.
The three requested results are complete directory text, `five\nsix`, and
`two\n\n…\n\nfour`. The artifact has the required owner, exact listing,
tail and selected lines. Reported input/output/cache-read usage totals
are 4,000/1,113/7,680 tokens; cost is unknown.

| Artifact | SHA-256 |
| --- | --- |
| Windows CLI | `1be1a2acc018ad0400e0b198bda5366f00f607339224e5f7b6f79cefae0bfcd6` |
| PROOF.json | `9708c01945ac17b65a61008bbf2a427cf6112d6633b76a47a9c5829845e31a6c` |
| Events | `995b26e44f9203b5ede29671fafe7a9ea0ec061d2e86091afc9ce95fd961e199` |
| Session journal | `7ad24ec5156c61b63501fe93ec7e7fb544c601a40e53b961903766cdfd9c6a25` |

The first attempt, `real-model-syouyhgt`, exited 0 but failed the parent
checker: Windows `write_text` translated the fixture to CRLF, while the
checker expected LF. The tool faithfully returned `five\r\nsix`. The
rerun writes explicit LF fixture bytes. The initial failed-check record
is preserved; no production behavior or assertion was weakened.

## Acceptance boundary

**Decision: accepted bounded CTX-01d point on `d05dab0`, 2026-09-30.**
Independent Codex `ctx_point_audit` reviewed the exact three-file test-only
slice `7f658f0..d05dab0`, all raw final Linux hook/event/request/Session
receipts, the B.AI task and both fixture attempts. It independently
recomputed all 100 uploaded hashes and matched all six runner source
snapshot hashes against raw Git blobs. It found no actionable defect.

The point audit also examined the existing fixed OMP source mapping,
[representative selector oracle](ctx-01d-skill-selector-renderer.md),
257 MiB resource scan trial and
[observed locale matrix](ctx-01d-skill-directory-sort.md). Together these
meet the recorded host/Skill gates for this point. Accepted sorting
evidence is the observed Windows zh-CN/en-US, macOS en-US and Linux en-US
fixtures; selectors are the 22 representative renderer comparisons, not
the entire generic selector surface.

The installed OMP native addon's exact build provenance remains
unverified. The prior intermittent REPL-warning failure is preserved in
the renderer evidence; its cause is still unconfirmed, and it was not
fixed by this slice. Other locales/Unicode inputs, generic selectors
(TOOLS-01a), plugin/managed Skill containment (CTX-01c) and `/skill:`
invocation (CTX-01e) keep their own open boundaries. P1/P3 and the complete
fixed-OMP marker remain unaccepted. This decision supersedes the prior
CTX-01d WIP verdict for the explicitly recorded point, with production
Rust unchanged from `96dc781`.
