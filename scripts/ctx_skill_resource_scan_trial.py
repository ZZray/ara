"""Exercise >256 MiB skill selectors through ara and a controlled Chat upstream."""

import argparse
import hashlib
import json
import math
import os
from pathlib import Path
import subprocess
import tempfile
import threading
import time
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--binary", type=Path, required=True)
    parser.add_argument("--output", type=Path, required=True)
    args = parser.parse_args()
    binary = args.binary.resolve(strict=True)
    args.output.mkdir(parents=True, exist_ok=True)
    run_dir = Path(tempfile.mkdtemp(prefix="host-", dir=args.output)).resolve()
    work = run_dir / "work"
    skill = work / ".ara/skills/scan"
    skill.mkdir(parents=True)
    (skill / "SKILL.md").write_text("---\ndescription: Large resource trial\n---\nRead the fixture.\n", encoding="utf-8")
    asset = skill / "large.txt"
    line = "x" * 4096
    block = (line + "\n").encode()
    repeats = math.ceil((257 * 1024 * 1024) / len(block))
    with asset.open("wb") as output:
        for _ in range(repeats):
            output.write(block)
        output.write(b"penultimate\nlast")
    total_lines = repeats + 2
    paths = ["skill://scan/large.txt:raw:-2", "skill://scan/large.txt:raw:1-1", "skill://scan/large.txt:raw:1-1,3-3"]
    expected = ["penultimate\nlast", f"{line}\n\n[{total_lines - 1} more lines in resource. Use :2 to continue]", f"{line}\n\n…\n\n{line}"]
    requests = []
    errors = []

    class Upstream(BaseHTTPRequestHandler):
        def log_message(self, *unused):
            pass

        def do_POST(self):
            try:
                body = json.loads(self.rfile.read(int(self.headers["Content-Length"])))
                index = len(requests)
                requests.append(body)
                if index:
                    result = next(message for message in reversed(body["messages"]) if message["role"] == "tool")
                    assert result["content"] == expected[index - 1], f"selector {index - 1} text differs"
                assert index <= 3, "model call bound exceeded"
                if index < 3:
                    delta = {"role": "assistant", "tool_calls": [{"index": 0, "id": f"call_scan_{index}", "type": "function", "function": {"name": "read", "arguments": json.dumps({"path": paths[index]})}}]}
                    reason = "tool_calls"
                else:
                    delta = {"role": "assistant", "content": "Large resource verified."}
                    reason = "stop"
                events = [
                    {"choices": [{"index": 0, "delta": delta, "finish_reason": None}]},
                    {"choices": [{"index": 0, "delta": {}, "finish_reason": reason}]},
                ]
                payload = "".join(f"data: {json.dumps(event)}\n\n" for event in events) + "data: [DONE]\n\n"
                encoded = payload.encode()
                self.send_response(200)
                self.send_header("Content-Type", "text/event-stream")
                self.send_header("Content-Length", str(len(encoded)))
                self.end_headers()
                self.wfile.write(encoded)
            except Exception as error:
                errors.append(str(error))
                self.send_error(500, "trial assertion failed")

    server = ThreadingHTTPServer(("127.0.0.1", 0), Upstream)
    thread = threading.Thread(target=server.serve_forever, daemon=True)
    thread.start()
    home = run_dir / "home"
    home.mkdir()
    sessions = run_dir / "sessions"
    env = dict(os.environ)
    for key in ["OPENROUTER_API_KEY", "ANTHROPIC_API_KEY", "ARA_API_KEY", "ARA_TEST_API_KEY", "ARA_MODEL", "ARA_BASE_URL", "ARA_TEST_BASE_URL", "OPENROUTER_BASE_URL", "ARA_TEST_MODEL_ID", "CLAUDE_CONFIG_DIR", "COPILOT_HOME", "COPILOT_CUSTOM_INSTRUCTIONS_DIRS", "WSL_DISTRO_NAME", "WSL_INTEROP"]:
        env.pop(key, None)
    env.update({"HOME": str(home), "USERPROFILE": str(home), "ARA_HOME": str(home), "ARA_API_KEY": "controlled-trial-key"})
    command = [str(binary), "--api", "openai-completions", "--model", "controlled-scan-model", "--base-url", f"http://127.0.0.1:{server.server_port}/v1", "--cwd", str(work), "--session-dir", str(sessions), "--mode", "json", "--tools", "read", "--max-model-calls", "4", "--max-time", "120", "--max-tokens", "256", "Verify large skill selectors"]
    started = time.monotonic()
    try:
        captured_stdout = b""
        captured_stderr = b""
        try:
            process = subprocess.run(command, env=env, stdin=subprocess.DEVNULL, capture_output=True, timeout=150)
            captured_stdout = process.stdout
            captured_stderr = process.stderr
        except subprocess.TimeoutExpired as timeout:
            captured_stdout = timeout.stdout or b""
            captured_stderr = timeout.stderr or b""
            raise
        finally:
            (run_dir / "events.jsonl").write_bytes(captured_stdout)
            (run_dir / "stderr.txt").write_bytes(captured_stderr)
            (run_dir / "requests.json").write_text(json.dumps(requests, indent=2), encoding="utf-8")
        assert not errors, errors
        assert process.returncode == 0, process.stderr.decode(errors="replace")
        assert len(requests) == 4, len(requests)
        journals = list(sessions.glob("*.jsonl"))
        assert len(journals) == 1, journals
        entries = [json.loads(row) for row in journals[0].read_text(encoding="utf-8").splitlines()]
        receipts = [entry["message"] for entry in entries if entry.get("message", {}).get("role") == "toolResult"]
        assert len(receipts) == 3, len(receipts)
        for index, receipt in enumerate(receipts):
            assert not receipt.get("isError"), receipt
            assert receipt["content"][0]["text"] == expected[index]
            assert receipt["details"]["totalLines"] == total_lines, receipt["details"]
        digest = hashlib.sha256()
        with asset.open("rb") as source:
            while chunk := source.read(1024 * 1024):
                digest.update(chunk)
        summary = {"status": "PASS", "binarySha256": hashlib.sha256(binary.read_bytes()).hexdigest(), "fixtureBytes": asset.stat().st_size, "fixtureSha256": digest.hexdigest(), "totalLines": total_lines, "modelCalls": len(requests), "toolReceipts": len(receipts), "elapsedSeconds": round(time.monotonic() - started, 3), "protocol": "openai-completions", "upstream": "controlled-local", "usage": "unknown", "journal": str(journals[0]), "artifacts": str(run_dir)}
        (run_dir / "summary.json").write_text(json.dumps(summary, indent=2), encoding="utf-8")
        print(json.dumps(summary, indent=2))
    finally:
        server.shutdown()
        server.server_close()
        thread.join(timeout=5)
        # Delete only this generated large fixture; retain all task receipts.
        assert asset.resolve().parent == skill.resolve()
        asset.unlink()


if __name__ == "__main__":
    main()
