#!/usr/bin/env bash
# A2 slice real-model task: find and fix a seeded bug in a small repo with
# the search and hashline edit tools, then pass its test.
# Requires the route's key in the environment (OPENROUTER_API_KEY for
# openrouter.ai, otherwise ARA_API_KEY) and ARA_TEST_MODEL_ID verified in the
# live catalogue. Never pass a key on the command line.
# ARA_TRIAL_TOOLS restricts the tool set; ARA_TRIAL_PROMPT replaces the task text.
# Bounds: <= 12 model calls, <= 300 s wall time, <= 2048 output tokens per call.
set -euo pipefail
root="$(cd "$(dirname "$0")/.." && pwd)"
: "${ARA_TEST_MODEL_ID:?set ARA_TEST_MODEL_ID to a verified model id}"
base="${ARA_TEST_BASE_URL:-https://openrouter.ai/api/v1}"
out="${1:-$(mktemp -d)}"
work="$out/work"
mkdir -p "$work/shop" "$work/tests"
cat >"$work/shop/__init__.py" <<'PY'
PY
cat >"$work/shop/pricing.py" <<'PY'
"""Order pricing."""

TAX_RATE = 0.08


def subtotal(items):
    """Sum of price * quantity over (price, quantity) pairs."""
    total = 0
    for price, quantity in items:
        total += price * quantity
    return total


def apply_discount(amount, percent):
    """Reduce amount by percent (0-100)."""
    return amount - amount * percent


def order_total(items, discount_percent=0):
    discounted = apply_discount(subtotal(items), discount_percent)
    return round(discounted * (1 + TAX_RATE), 2)
PY
: >"$work/tests/__init__.py"
cat >"$work/tests/test_pricing.py" <<'PY'
import unittest

from shop.pricing import apply_discount, order_total


class PricingTest(unittest.TestCase):
    def test_discount_is_a_percentage(self):
        self.assertAlmostEqual(apply_discount(200, 10), 180)

    def test_order_total(self):
        self.assertEqual(order_total([(10, 2), (5, 4)], discount_percent=25), 32.4)


if __name__ == "__main__":
    unittest.main()
PY
run_tests() { (cd "$work" && python3 -m unittest -q 2>&1 | grep -E '^(Ran|OK|FAILED)' | tr '\n' ' ') || true; }
before=$(run_tests)
case "$before" in *"Ran 2 tests"*FAILED*) ;; *) echo "seeded bug not detected: $before" >&2; exit 2 ;; esac
cp "$work/tests/test_pricing.py" "$out/test_pricing.orig"
cargo build -q --manifest-path "$root/Cargo.toml" --bin ara
start=$(date +%s)
set +e
"$root/target/debug/ara" --model "$ARA_TEST_MODEL_ID" --base-url "$base" \
  --cwd "$work" --session-dir "$out/sessions" --mode json \
  --max-model-calls 12 --max-time 300 --max-tokens 2048 \
  ${ARA_TRIAL_TOOLS:+--tools "$ARA_TRIAL_TOOLS"} \
  "${ARA_TRIAL_PROMPT:-The test suite in this repository fails. Find the bug in the source code and fix it with a minimal edit. Do not modify the tests. Run 'python3 -m unittest -q' with the bash tool to confirm the tests pass, then summarize the fix.}" \
  </dev/null >"$out/events.jsonl" 2>"$out/stderr.txt"
code=$?
set -e
echo "exit=$code elapsed=$(( $(date +%s) - start ))s out=$out"
echo "tests before: $before"
after=$(run_tests)
echo "tests after (independent run): $after"
cmp -s "$out/test_pricing.orig" "$work/tests/test_pricing.py" && echo "tests untouched: True" || echo "tests untouched: False"
python3 - "$out" <<'PY'
import json, pathlib, sys
out = pathlib.Path(sys.argv[1])
events = [json.loads(l) for l in (out / "events.jsonl").read_text().splitlines() if l.strip()]
tools = [e for e in events if e.get("type") == "tool_execution_end"]
assistants = [e["message"] for e in events if e.get("type") == "message_end" and e["message"].get("role") == "assistant"]
print("model calls:", len(assistants))
print("tool calls:", [(t["toolName"], t["isError"]) for t in tools])
for a in assistants:
    print("usage:", a.get("usage"), "stop:", a.get("stopReason"), "model:", a.get("model"))
src = (out / "work" / "shop" / "pricing.py").read_text()
print("fixed line present:", "amount * percent / 100" in src or "percent / 100" in src)
print("final text:", "".join(b.get("text", "") for b in assistants[-1]["content"] if b.get("type") == "text")[:600] if assistants else None)
PY
