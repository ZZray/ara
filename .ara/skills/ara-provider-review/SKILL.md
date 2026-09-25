---
name: ara-provider-review
description: Review ARA OpenAI-compatible and Anthropic provider adapters against pinned OMP behavior and actual wire fixtures.
---

# Review provider wire behavior

Use with [Git scope review](../ara-git-review/SKILL.md) when a change affects model requests, streams, tool calls, usage, routing, or retries. The target protocols are OpenAI-compatible Chat Completions/Responses and Anthropic Messages; do not expand provider scope during review.

1. Identify the exact model protocol and pinned OMP source/test location. Compare outbound roles/content/tool IDs, request limits, and follow-up context. Intentional ARA differences need a written reason in `docs/upstream/feature-ledger.md`.
2. Trace streamed frames through partial UTF-8/JSON, multiple tool-call deltas, reasoning/text separation, finish reasons, errors, disconnect, and cancellation. Confirm assembled content and committed events match what the host observes; a `200` status is not a successful turn.
3. Verify tool-call correlation across requests and Runs. A retry must not replay an effect with unknown completion. Preserve provider error details without leaking credentials or untrusted headers into logs.
4. Distinguish absent/unknown usage from zero. Check budget reservation and settlement for normal calls, failed streams, and compaction calls. Do not infer cost or tokens from text length.
5. Require source-backed fake HTTP/SSE fixtures for normal and negative cases, a real Rust host chain with fake upstream, and a bounded real-model task where the acceptance gate calls for it. Record exact observed output, tool effects, usage, and latency in `docs/evidence/`.

Report reachable defects with target-version line and protocol frame evidence; distinguish untested concerns from findings. Review does not change provider configuration or spend model quota.
