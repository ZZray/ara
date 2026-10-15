"""Real Rust Node + isolated actual Go Hub, with a deterministic model fixture.

Validates actual tool receipts returned to the model and filesystem artifacts.
This is host/tool acceptance, not real-model intelligence or production acceptance.
"""
from __future__ import annotations

import argparse
import base64
import hashlib
import json
import os
from pathlib import Path
import secrets
import subprocess
import sys
import tempfile
import threading
import time

from verify_host import Fixture, WS, port, require, run_frames, status_rows, stop, wait_health


NEEDLE = "中文日志末尾命中_PROBLEM_73"
SUCCESS = "PROBLEM_TOOLS_ARTIFACT_OK_73"


class ProblemFixture(Fixture):
    def __init__(self, root):
        self.receipts = []
        self.accepted = []
        super().__init__(root, modern=True)

    def reply(self, handler, body):
        messages = body.get("messages", [])
        last_user = max((i for i, row in enumerate(messages) if row.get("role") == "user"), default=-1)
        require(last_user >= 0, "Missing current user request")
        prompt = messages[last_user].get("content", "")
        require(isinstance(prompt, str), "Unexpected non-text fixture prompt")
        scenario = next((name for name in ("PROBLEM_PIPELINE", "PROBLEM_EXIT7", "PROBLEM_CANCEL") if name in prompt), None)
        require(scenario, "Unknown problem-tools scenario")
        receipts = [row for row in messages[last_user + 1:] if row.get("role") == "tool"]
        declared = {row["function"]["name"]: row["function"] for row in body.get("tools", [])}
        calls = {call["id"]: call["function"]["name"] for row in messages[last_user + 1:] for call in row.get("tool_calls", [])}
        for receipt in receipts:
            require(receipt.get("tool_call_id") in calls, "Tool receipt lost its actual preceding call identity")
            require(isinstance(receipt.get("content"), str), "Tool receipt was not text")
        tool_name, arguments = None, None
        stage = len(receipts)
        if scenario == "PROBLEM_PIPELINE":
            expected = ["system_info", "find_files", "search_files", "run_script"]
            require(set(expected) <= declared.keys(), "New tools absent from real Agent model request")
            require(stage <= len(expected), "Unexpected extra tool receipt")
            for index, receipt in enumerate(receipts):
                require(calls[receipt["tool_call_id"]] == expected[index], "Tool order or receipt mapping changed")
            if stage >= 1:
                info = json.loads(receipts[0]["content"])
                require(len(receipts[0]["content"].encode()) <= 4096, "system_info exceeded 4 KiB")
                require(info.get("os") and info.get("arch"), "Actual OS/arch receipt missing")
                require(info["programs"]["python"]["status"] == "found_candidate", "Isolated Node did not discover test Python")
                require(info["computer_ui"] == "unsupported" and info["vision_input"] == "unsupported", "Unimplemented desktop/vision falsely advertised")
            if stage >= 2:
                found = json.loads(receipts[1]["content"])
                require(any(row.get("relative_path") == "logs/fixture-长行.log" for row in found["matches"]), "find_files did not find actual fixture path")
                require("coverage" in found, "File search coverage missing")
            if stage >= 3:
                search = json.loads(receipts[2]["content"])
                require(any(row.get("line") == 2 and NEEDLE in row.get("excerpt", "") for row in search["matches"]), "Long Chinese line tail match missing from model receipt")
                require(len(receipts[2]["content"].encode()) < 32768, "Long line search flooded the model")
            if stage == 0:
                tool_name, arguments = "system_info", {}
            elif stage == 1:
                tool_name, arguments = "find_files", {"query": "fixture-长行.log", "path": str(self.root), "max_results": 5}
            elif stage == 2:
                tool_name, arguments = "search_files", {"query": NEEDLE, "path": str(self.root / "logs"), "max_results": 5}
            elif stage == 3:
                script = (
                    "from pathlib import Path\nimport runpy\n"
                    "p = Path('broken.py')\n"
                    "old = p.read_text(encoding='utf-8')\n"
                    "assert old.count('sum(values) + 1') == 1\n"
                    "p.write_text(old.replace('sum(values) + 1', 'sum(values)'), encoding='utf-8')\n"
                    "runpy.run_path(str(p), run_name='__main__')\n"
                )
                tool_name, arguments = "run_script", {"language": "python", "script": script, "cwd": str(self.root), "timeout": 15, "output_limit": 4096}
            else:
                require(SUCCESS in receipts[3]["content"], "Real script output did not reach model")
                require(json.loads((self.root / "answer.json").read_text(encoding="utf-8")) == {"sum": 42, "verified": True}, "Script artifact has incorrect contents")
                require("sum(values) + 1" not in (self.root / "broken.py").read_text(encoding="utf-8"), "Fixture code was not repaired")
                self.accepted.append(scenario)
        elif scenario == "PROBLEM_EXIT7":
            if stage == 0:
                script = "import sys\nsys.stdout.write('STDOUT_HEAD_73\\n' + 'O' * 60000 + '\\nSTDOUT_TAIL_73\\n')\nsys.stderr.write('STDERR_HEAD_73\\n' + 'E' * 60000 + '\\nSTDERR_TAIL_73\\n')\nsys.exit(7)\n"
                tool_name, arguments = "run_script", {"language": "python", "script": script, "cwd": str(self.root), "timeout": 15, "output_limit": 4096}
            else:
                require(stage == 1, "Unexpected failure-case replay")
                content = receipts[0]["content"]
                require("code 7" in content or "exit 7" in content, "Nonzero exit was lost or converted to success")
                require("STDOUT_TAIL_73" in content and "STDERR_TAIL_73" in content, "Bounded output lost stdout/stderr tails")
                require(len(content.encode()) <= 4608, "run_script output exceeded requested payload plus receipt metadata budget")
                self.accepted.append(scenario)
        else:
            require(stage == 0, "Canceled process unexpectedly continued the model loop")
            script = "from pathlib import Path\nimport time\nPath('cancel-started.txt').write_text('started')\ntime.sleep(5)\nPath('cancel-survived.txt').write_text('unsafe')\n"
            tool_name, arguments = "run_script", {"language": "python", "script": script, "cwd": str(self.root), "timeout": 15, "output_limit": 4096}
        if receipts:
            self.receipts.append({"scenario": scenario, "stage": stage, "receipts": receipts})
        if tool_name:
            require(tool_name in declared, f"Tool not declared: {tool_name}")
            properties = declared[tool_name]["parameters"].get("properties", {})
            require(arguments.keys() <= properties.keys(), f"Actual {tool_name} schema differs from fixture")
        handler.send_response(200)
        handler.send_header("Content-Type", "text/event-stream")
        handler.send_header("Connection", "close")
        handler.end_headers()
        handler.close_connection = True

        def delta(value, finish=None):
            chunk = {"id": "problem-fixture", "object": "chat.completion.chunk", "choices": [{"index": 0, "delta": value, "finish_reason": finish}]}
            handler.wfile.write(("data: " + json.dumps(chunk) + "\n\n").encode())
            handler.wfile.flush()

        if tool_name:
            delta({"tool_calls": [{"index": 0, "id": f"{scenario}-{stage}", "type": "function", "function": {"name": tool_name, "arguments": json.dumps(arguments)}}]})
            delta({}, "tool_calls")
        else:
            delta({"role": "assistant", "content": f"{scenario}_VALIDATED"})
            delta({}, "stop")
        handler.wfile.write(b"data: [DONE]\n\n")
        handler.wfile.flush()


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--binary", type=Path, required=True)
    parser.add_argument("--hub-binary", type=Path, required=True)
    parser.add_argument("--output", type=Path)
    args = parser.parse_args()
    root = args.output.resolve() if args.output else Path(tempfile.mkdtemp(prefix="handwave-problem-tools-"))
    root.mkdir(parents=True, exist_ok=True)
    require(not any(root.iterdir()), "Use a fresh empty evidence directory")
    conf, state, work = (root / name for name in ("hub-conf", "node-state", "work"))
    for directory in (conf, state, work, work / "logs"):
        directory.mkdir()
    (work / "logs" / "fixture-长行.log").write_text("header\n" + "日志普通内容" * 6000 + NEEDLE + "\n", encoding="utf-8")
    (work / "broken.py").write_text("import json\nfrom pathlib import Path\nvalues = [20, 22]\nresult = sum(values) + 1\nassert result == 42\nPath('answer.json').write_text(json.dumps({'sum': result, 'verified': True}))\nprint('" + SUCCESS + "')\n", encoding="utf-8")
    fixture = ProblemFixture(work)
    root_token, node_token = secrets.token_hex(24), secrets.token_hex(24)
    hub_port = port()
    hub_url = f"http://127.0.0.1:{hub_port}"
    (conf / "gateway.json").write_text(json.dumps({"gateway_root": f"http://127.0.0.1:{fixture.server.server_port}", "gateway_token_env": "HW_TEST_GATEWAY_TOKEN", "default_model": "fixture-model", "default_protocol": "openai-completions", "allow_loopback": True}), encoding="utf-8")
    env = os.environ.copy()
    for key in list(env):
        if key.lower() in ("http_proxy", "https_proxy", "all_proxy", "no_proxy") or key in ("HW_PROXY", "HW_GATEWAY_TOKEN"):
            del env[key]
    env.update(HW_CONF=str(conf), HW_TEST_GATEWAY_TOKEN="fixture-key", HW_TOKEN=node_token, NO_PROXY="127.0.0.1,localhost")
    node_env = env.copy()
    node_env.pop("HW_TEST_GATEWAY_TOKEN")
    # Use the already-running test interpreter; never install/modify an interpreter.
    node_env["PATH"] = str(Path(sys.executable).parent) + os.pathsep + node_env.get("PATH", "")
    hub_log, node_log = (root / "hub.log").open("wb"), (root / "node.log").open("wb")
    hub = node = controller = watchdog = None
    transcript, passed = [], []
    result = {"scope": "actual Go Hub and Rust Node; deterministic model fixture; isolated files", "passed": passed, "not_run": ["real-model intelligence", "production installation", "missing interpreter (unit-test boundary)"]}
    started = time.monotonic()
    try:
        hub = subprocess.Popen([str(args.hub_binary.resolve()), "hub", "-listen", f"127.0.0.1:{hub_port}", "-token", root_token, "-node_token", node_token], env=env, cwd=conf, stdout=hub_log, stderr=hub_log)
        wait_health(hub_url)
        node = subprocess.Popen([str(args.binary.resolve()), "--hub", hub_url, "--state-dir", str(state), "--cwd", str(work), "--max-calls", "8", "--max-seconds", "45"], env=node_env, cwd=work, stdout=node_log, stderr=node_log)
        watchdog = threading.Timer(100, lambda: (stop(node), stop(hub)))
        watchdog.daemon = True
        watchdog.start()
        controller = WS(hub_port, "ctl", root_token)
        controller.send({"k": "hello", "fp": "problem-tools-controller", "relay": True})
        inventory = controller.until(lambda message: message.get("k") == "status" and any(row.get("state") == "pending" for row in status_rows(message)))
        device = next(row for row in status_rows(inventory) if row.get("state") == "pending")
        controller.send({"k": "approve", "nid": device["nid"], "relay": True})
        controller.until(lambda message: message.get("k") == "paired")
        controller.until(lambda message: message.get("k") == "dc" and json.loads(base64.b64decode(message["d"])).get("k") == "node-hello")
        for scenario in ("PROBLEM_PIPELINE", "PROBLEM_EXIT7"):
            terminal, events, _ = run_frames(controller, scenario.lower(), scenario, transcript, timeout=40)
            require(not terminal.get("err") and terminal.get("code", 0) == 0, f"Fixture did not finish receipt validation: {scenario}")
            require(scenario in fixture.accepted, f"Model fixture did not validate actual receipts: {scenario}")
            require(f"{scenario}_VALIDATED".encode() in base64.b64decode(terminal["b64"]), "Final controller text missing")
            if scenario == "PROBLEM_EXIT7":
                require(any(event.get("type") == "tool_end" and event.get("status") == "failed" for event in events), "Nonzero script was not exposed as failed tool")
            passed.append(scenario)
        terminal, events, _ = run_frames(controller, "problem-cancel", "PROBLEM_CANCEL", transcript, cancel=True, timeout=20)
        require(terminal.get("err") or terminal.get("code", 0) != 0, "Canceled process reported success")
        require((work / "cancel-started.txt").read_text() == "started", "Cancellation did not exercise an actually started Python process")
        time.sleep(5.5)
        require(not (work / "cancel-survived.txt").exists(), "Canceled Python process survived to write delayed artifact")
        passed.append("PROBLEM_CANCEL_ACTUAL_PROCESS")
        require(not fixture.errors, "Controlled model assertions failed")
        result["status"] = "PASS"
    except Exception as error:
        result.update(status="FAIL", error=f"{type(error).__name__}: {error}")
        raise
    finally:
        if watchdog:
            watchdog.cancel()
        if controller:
            controller.close()
        stop(node)
        stop(hub)
        fixture.server.shutdown()
        hub_log.close()
        node_log.close()
        result.update(elapsed_seconds=round(time.monotonic() - started, 3), binary_sha256=hashlib.sha256(args.binary.read_bytes()).hexdigest(), hub_sha256=hashlib.sha256(args.hub_binary.read_bytes()).hexdigest(), model_requests=len(fixture.requests), fixture_errors=fixture.errors)
        for name, value in (("result", result), ("controller", transcript), ("model-receipts", fixture.receipts)):
            (root / f"{name}.json").write_text(json.dumps(value, ensure_ascii=False, indent=2), encoding="utf-8")
        print(json.dumps(result, ensure_ascii=False, indent=2))


if __name__ == "__main__":
    main()
