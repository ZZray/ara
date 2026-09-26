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
expected_skill_body="$(cat "$work/.ara/skills/release-notes/SKILL.md")"
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
elapsed=$(( $(date +%s) - start ))
echo "exit=$code elapsed=${elapsed}s out=$out"
python3 - "$out" "$code" "$elapsed" "$expected_skill_body" <<'PY'
import json, pathlib, re, subprocess, sys
out = pathlib.Path(sys.argv[1])
exit_code, elapsed = map(int, sys.argv[2:4])
work = out / "work"
events = [json.loads(l) for l in (out / "events.jsonl").read_text().splitlines() if l.strip()]
starts = [e for e in events if e.get("type") == "tool_execution_start"]
# Parallel calls can finish out of order: pair by call id, not position.
ends = {e["toolCallId"]: e for e in events if e.get("type") == "tool_execution_end"}
assistants = [e["message"] for e in events if e.get("type") == "message_end" and e["message"].get("role") == "assistant"]
print("model calls:", len(assistants))
print("tool calls:", [(s["toolName"], json.dumps(s.get("args"))[:80], ends.get(s["toolCallId"], {}).get("isError")) for s in starts])
for a in assistants:
    print("usage:", a.get("usage"), "stop:", a.get("stopReason"))
args_text = [json.dumps(s.get("args")) for s in starts]
skill_calls = [(s, ends.get(s["toolCallId"])) for s, a in zip(starts, args_text)
               if s["toolName"] in {"read", "bash"}
               and ("skill://release-notes" in a or "release-notes/SKILL.md" in a)]
skill_body = sys.argv[4].replace("\r\n", "\n").strip()
def skill_text(end):
    blocks = end.get("result", {}).get("content", [])
    return "\n".join(block.get("text", "") for block in blocks if block.get("type") == "text").replace("\r\n", "\n")
def contains_skill_body(end):
    text = skill_text(end)
    if skill_body in text:
        return True
    # Local-path reads in edit mode add a hashline header and N: prefixes.
    numbered = [match.group(1) for line in text.splitlines()
                if (match := re.match(r"^\d+[:|](.*)$", line))]
    return skill_body in "\n".join(numbered)
successful_skills = [s["toolName"] for s, end in skill_calls
                     if skill_body and end and not end.get("isError") and contains_skill_body(end)]
read_skill = bool(successful_skills)
print("successful skill reads:", successful_skills)
mathx = work / "mathx.py"
agents_rule = mathx.exists() and mathx.read_text().splitlines()[:1] == ["# owner: ara-trial"]
print("AGENTS.md rule followed:", agents_rule)
works = mathx.exists() and subprocess.run([sys.executable, "-c", "import mathx; assert mathx.double(21) == 42"], cwd=work).returncode == 0
print("double(21) == 42:", works)
notes = work / "RELEASE_NOTES.md"
lines = notes.read_text().splitlines() if notes.exists() else []
skill_format = bool(lines) and lines[0] == "## vNEXT" and any(l.startswith("- [ara] ") for l in lines[1:])
print("skill format followed:", skill_format, lines[:4])
passed = exit_code == 0 and 0 < len(assistants) <= 12 and elapsed <= 300 and read_skill and agents_rule and works and skill_format
print("TRIAL PASS:", passed)
sys.exit(0 if passed else 1)
PY
