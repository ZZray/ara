#!/usr/bin/env bash
# Bounded real-model task trial through the actual `ara` host chain.
# Requires: OPENROUTER_API_KEY (or ARA_API_KEY with --base-url) in the
# environment and ARA_TEST_MODEL_ID set to a model verified in the live
# catalogue. Never pass a key on the command line.
# Bounds: <= 6 model calls, <= 180 s wall time, <= 1024 output tokens per call.
set -euo pipefail
root="$(cd "$(dirname "$0")/.." && pwd)"
: "${ARA_TEST_MODEL_ID:?set ARA_TEST_MODEL_ID to a verified model id}"
base="${ARA_TEST_BASE_URL:-https://openrouter.ai/api/v1}"
out="${1:-$(mktemp -d)}"
mkdir -p "$out/work"
cargo build -q --manifest-path "$root/Cargo.toml" --bin ara
start=$(date +%s)
set +e
"$root/target/debug/ara" --model "$ARA_TEST_MODEL_ID" --base-url "$base" \
  --cwd "$out/work" --session-dir "$out/sessions" --mode json \
  --max-model-calls 6 --max-time 180 --max-tokens 1024 \
  "Create a file named fib.txt containing the first 10 Fibonacci numbers starting 0 1, one per line. Then run 'wc -l fib.txt' with the bash tool and report the line count." \
  </dev/null >"$out/events.jsonl" 2>"$out/stderr.txt"
code=$?
set -e
echo "exit=$code elapsed=$(( $(date +%s) - start ))s out=$out"
python3 - "$out" <<'PY'
import json, pathlib, sys
out = pathlib.Path(sys.argv[1])
events = [json.loads(l) for l in (out / "events.jsonl").read_text().splitlines() if l.strip()]
tools = [e for e in events if e.get("type") == "tool_execution_end"]
assistants = [e["message"] for e in events if e.get("type") == "message_end" and e["message"].get("role") == "assistant"]
print("model calls:", len(assistants))
print("tool calls:", [(t["toolName"], t["isError"]) for t in tools])
for a in assistants:
    print("usage:", a.get("usage"), "stop:", a.get("stopReason"), "model:", a.get("model"), "upstream:", a.get("upstreamProvider"))
fib = out / "work" / "fib.txt"
expected = "\n".join(str(x) for x in [0, 1, 1, 2, 3, 5, 8, 13, 21, 34])
actual = fib.read_text().strip() if fib.exists() else None
print("artifact ok:", actual == expected, repr(actual))
print("final text:", "".join(b.get("text", "") for b in assistants[-1]["content"] if b.get("type") == "text") if assistants else None)
PY
