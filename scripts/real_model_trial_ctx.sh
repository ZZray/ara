#!/usr/bin/env bash
# CTX-01 real-model task: the model must follow a rule that exists only in
# AGENTS.md and a format that exists only in a skill it has to read through
# skill://. Checks the artifacts, not the answer text.
# Requires the route's key in the environment (OPENROUTER_API_KEY for
# openrouter.ai, otherwise ARA_API_KEY) and ARA_TEST_MODEL_ID verified in the
# live catalogue. Never pass a key on the command line.
# Bounds: <= 12 model calls, <= 300 s wall time, <= 2048 output tokens per call.
set -euo pipefail
root="$(cd "$(dirname "$0")/.." && pwd)"
: "${ARA_TEST_MODEL_ID:?set ARA_TEST_MODEL_ID to a verified model id}"
base="${ARA_TEST_BASE_URL:-https://openrouter.ai/api/v1}"
out="${1:-$(mktemp -d)}"
work="$out/work"
mkdir -p "$work/.ara/skills/release-notes" "$out/home"
git -C "$work" init -q
cat >"$work/AGENTS.md" <<'MD'
# Repository rules

Every new Python file MUST start with the exact line `# owner: ara-trial`.
MD
cat >"$work/.ara/skills/release-notes/SKILL.md" <<'MD'
---
name: release-notes
description: How to record a change in this repository's release notes. Use whenever you add or change behavior.
---
Release notes live in `RELEASE_NOTES.md` at the repository root.

- The first line of the file is exactly `## vNEXT`.
- Each change is one line starting with `- [ara] ` followed by a short description.
MD
cargo build -q --manifest-path "$root/Cargo.toml" --bin ara
start=$(date +%s)
set +e
HOME="$out/home" ARA_HOME="$out/home/.ara" "$root/target/debug/ara" --model "$ARA_TEST_MODEL_ID" --base-url "$base" \
  --cwd "$work" --session-dir "$out/sessions" --mode json \
  --max-model-calls 12 --max-time 300 --max-tokens 2048 \
  "Add a Python function double(x) that returns 2*x in a new file mathx.py, then record the change in the release notes. Follow the repository's rules and skills." \
  </dev/null >"$out/events.jsonl" 2>"$out/stderr.txt"
code=$?
set -e
echo "exit=$code elapsed=$(( $(date +%s) - start ))s out=$out"
python3 - "$out" <<'PY'
import json, pathlib, subprocess, sys
out = pathlib.Path(sys.argv[1])
work = out / "work"
events = [json.loads(l) for l in (out / "events.jsonl").read_text().splitlines() if l.strip()]
starts = [e for e in events if e.get("type") == "tool_execution_start"]
ends = [e for e in events if e.get("type") == "tool_execution_end"]
assistants = [e["message"] for e in events if e.get("type") == "message_end" and e["message"].get("role") == "assistant"]
print("model calls:", len(assistants))
print("tool calls:", [(s["toolName"], json.dumps(s.get("args"))[:80], e["isError"]) for s, e in zip(starts, ends)])
for a in assistants:
    print("usage:", a.get("usage"), "stop:", a.get("stopReason"))
args_text = [json.dumps(s.get("args")) for s in starts]
via_url = [s["toolName"] for s, a in zip(starts, args_text) if "skill://release-notes" in a]
via_path = [s["toolName"] for s, a in zip(starts, args_text) if "release-notes/SKILL.md" in a and "skill://" not in a]
read_skill = bool(via_url or via_path)
print("skill read via skill:// :", via_url, "via path:", via_path)
mathx = work / "mathx.py"
agents_rule = mathx.exists() and mathx.read_text().splitlines()[:1] == ["# owner: ara-trial"]
print("AGENTS.md rule followed:", agents_rule)
works = mathx.exists() and subprocess.run([sys.executable, "-c", "import mathx; assert mathx.double(21) == 42"], cwd=work).returncode == 0
print("double(21) == 42:", works)
notes = work / "RELEASE_NOTES.md"
lines = notes.read_text().splitlines() if notes.exists() else []
skill_format = bool(lines) and lines[0] == "## vNEXT" and any(l.startswith("- [ara] ") for l in lines[1:])
print("skill format followed:", skill_format, lines[:4])
print("TRIAL PASS:", read_skill and agents_rule and works and skill_format)
PY
