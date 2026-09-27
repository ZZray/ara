# AI-ANTHROPICa image content conversion (WIP, 2026-09-27)

## Source and boundary

Fixed OMP `596f2da7101178214aa27a753529d15e6b7ad91d`:
`packages/ai/src/providers/anthropic.ts::normalizeAnthropicImageMediaType`,
`convertContentBlocks`, and `buildToolResultBlock` (lines 365–379,
1025–1077, 3952–4004). ARA changes only the Anthropic Messages converter and
its fake HTTP tests. Agent, Session, CLI, shared model types and other providers
are unchanged. The Rust `Model` does not yet carry OMP's vision input
capability, so non-vision-model image fallback is explicitly open.

The converter trims and lowercases the media type, aliases `image/jpg` to
`image/jpeg`, accepts JPEG/PNG/GIF/WEBP, and replaces unsupported base64
images with `[unsupported image: <original MIME>]`. It drops blank text blocks
and adds `(see attached image)` when a converted result has an image but no
nonblank text. Error tool results keep text inside the result and move only
converted image blocks after the grouped tool results. Empty converted tool
results use `""` for compatible endpoints; empty errors use the explicit
`Tool failed with no output.` text.

## Executed evidence on the candidate diff

| Check | Observed result |
| --- | --- |
| Two new fake HTTP request tests | Both failed before the implementation and passed afterward. The POST body contains normalized JPEG/PNG/GIF/WEBP, a visible SVG placeholder, image-only hint, grouped error/success tool results, and a hoisted valid error image. |
| Reviewer-directed empty success result test | Failed with `[]`, then passed with `content:""`; a separate empty error has `Tool failed with no output.`. |
| `cargo test -p ara-ai --all-targets --quiet` | Exit 0: 114 unit, 20 Anthropic HTTP, 48 Chat HTTP, 29 Responses HTTP. |
| `cargo test -p ara-cli --test e2e anthropic_ --quiet` | Exit 0, 6/6 real CLI process tests with controlled upstream. These are regression tests, not an image-specific CLI host proof. |
| `cargo clippy -p ara-ai --all-targets --all-features -- -D warnings`; changed-file `rustfmt --edition 2024 --check`; `git diff --check` | Exit 0 after fixing one Clippy formatting issue. |

Independent Codex plan reviewer `/root/anthropic_image_plan` scoped the
conversion to the provider and its HTTP tests. Independent Codex diff reviewer
`/root/anthropic_image_diff_review` found one empty-success-tool-result defect;
the new red/green HTTP assertion and fix resolved it. The final review found
no remaining reachable defect in the two-file diff. Neither review is a
real-model trial.

`python scripts/verify_backend.py` passed owned formatting and strict
workspace Clippy, then exited 101 in workspace all-target tests: CLI E2E
35/36 passed; the known Windows Bash
`deadline_during_a_tool_and_zero_budget_exit_nonzero` check exceeded its
4-second assertion. The transcript is
`%TEMP%\ara-anthropic-image-gate.log`. The verifier stopped before doc tests.
Separate `cargo test --workspace --doc --all-features --quiet`,
`cargo deny check`, and `python scripts/omp_inventory.py check` each exited 0;
`cargo deny` still reports existing duplicate-version warnings. The full
backend gate is red.

A bounded real-model image/tool task and image-specific CLI host proof were
not run. No AI-ANTHROPICa, AI module, or phase acceptance is claimed; accepted
counts remain unchanged.
