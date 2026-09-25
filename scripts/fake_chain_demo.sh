#!/usr/bin/env bash
# Two-process host chain with the controlled fake upstream (no real model).
# Usage: scripts/fake_chain_demo.sh [out-dir]
set -euo pipefail
root="$(cd "$(dirname "$0")/.." && pwd)"
out="${1:-$(mktemp -d)}"
mkdir -p "$out/work"
cargo build -q --manifest-path "$root/Cargo.toml" --bins
"$root/target/debug/ara-fake-upstream" --script "$root/crates/ara-cli/tests/fixtures/plan-task.json" \
  --record "$out/requests.jsonl" --port-file "$out/url" >"$out/upstream.log" 2>&1 &
up=$!
trap 'kill $up 2>/dev/null || true' EXIT
for _ in $(seq 100); do [ -s "$out/url" ] && break; sleep 0.1; done
set +e
ARA_API_KEY=sk-fake-demo "$root/target/debug/ara" --model fake-model --base-url "$(cat "$out/url")" \
  --cwd "$out/work" --session-dir "$out/sessions" --mode json \
  "Write a short plan to notes/plan.md and check it" </dev/null >"$out/events.jsonl" 2>"$out/stderr.txt"
code=$?
set -e
echo "exit=$code out=$out"
test "$code" -eq 0
test "$(cat "$out/work/notes/plan.md")" = $'# Plan\n- port agent loop'
grep -q '"authorization":"<redacted' "$out/requests.jsonl"
test "$(grep -c '"type":"tool_execution_end"' "$out/events.jsonl")" -eq 2
echo "OK: fake chain produced notes/plan.md, 2 tool receipts, redacted request log"
