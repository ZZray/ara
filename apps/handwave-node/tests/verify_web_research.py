"""Real isolated Rust Node / Go Hub / controlled-model web and download acceptance.

Exercises actual advertised schemas, tool receipts, HTTP and file/process effects.
It never modifies production auth/installations and does not prove model intelligence.
"""
from __future__ import annotations

import argparse
import base64
import hashlib
import http.server
import json
import os
from pathlib import Path
import secrets
import subprocess
import sys
import tempfile
import threading
import time
import urllib.parse

from verify_host import Fixture, WS, port, require, run_frames, status_rows, stop, wait_health


MARKER = "WEB_DOWNLOAD_SCRIPT_ACTUALLY_EXECUTED_73"
SCRIPT = ("from pathlib import Path\n"
          "Path('download-result.txt').write_text('" + MARKER + "', encoding='utf-8')\n"
          "print('" + MARKER + "')\n").encode()
SCRIPT_SHA = hashlib.sha256(SCRIPT).hexdigest()


class Resources:
    def __init__(self, expected_query="official python documentation", official_url="https://docs.python.org/3/"):
        self.requests = []
        resources = self

        class Handler(http.server.BaseHTTPRequestHandler):
            def log_message(self, *args):
                pass

            def do_GET(self):
                resources.requests.append(self.path)
                parsed = urllib.parse.urlsplit(self.path)
                if parsed.path == "/search":
                    query = urllib.parse.parse_qs(parsed.query)
                    require(query.get("format") == ["json"], "Search did not request SearXNG JSON")
                    if expected_query is not None:
                        require(query.get("q") == [expected_query], "Search query changed")
                    data = json.dumps({"results": [
                        {"title": "Python official documentation", "url": official_url, "content": "PYTHON_OFFICIAL_SOURCE_73"},
                        {"title": "Example", "url": "https://example.com/", "content": "EXAMPLE_SOURCE_73"}
                    ]}).encode()
                    content_type = "application/json"
                elif parsed.path == "/tool.py":
                    data, content_type = SCRIPT, "application/octet-stream"
                else:
                    self.send_error(404)
                    return
                self.send_response(200)
                self.send_header("Content-Type", content_type)
                self.send_header("Content-Length", str(len(data)))
                self.end_headers()
                self.wfile.write(data)

        self.server = http.server.ThreadingHTTPServer(("127.0.0.1", 0), Handler)
        self.server.daemon_threads = True
        threading.Thread(target=self.server.serve_forever, daemon=True).start()
        self.url = f"http://127.0.0.1:{self.server.server_port}"


class WebFixture(Fixture):
    def __init__(self, root, resources):
        self.resources = resources
        self.receipts, self.accepted = [], []
        super().__init__(root, modern=True)

    def reply(self, handler, body):
        messages = body.get("messages", [])
        last_user = max(i for i, row in enumerate(messages) if row.get("role") == "user")
        prompt = messages[last_user].get("content", "")
        require(isinstance(prompt, str), "Expected text prompt")
        scenario = next((name for name in (
            "WEB_PIPELINE", "WEB_BAD_HASH", "WEB_PRIVATE_FETCH", "WEB_PLAN_SEARCH",
            "WEB_PLAN_FORGED_DOWNLOAD", "WEB_PUBLIC_FETCH") if name in prompt), None)
        require(scenario, "Unexpected bounded web scenario")
        receipts = [row for row in messages[last_user + 1:] if row.get("role") == "tool"]
        calls = {call["id"]: call["function"]["name"] for row in messages[last_user + 1:]
                 for call in row.get("tool_calls", [])}
        schemas = {row["function"]["name"]: row["function"]["parameters"] for row in body.get("tools", [])}
        require({"web_search", "fetch_url"} <= schemas.keys(), "Native web tools absent from actual model request")
        for receipt in receipts:
            require(receipt.get("tool_call_id") in calls, "Receipt has no preceding actual call")
            require(isinstance(receipt.get("content"), str), "Expected text receipt")
        stage = len(receipts)
        tool, arguments = None, None
        if scenario in ("WEB_PIPELINE", "WEB_PLAN_SEARCH"):
            if "PLAN" in scenario:
                require(not {"download_file", "exec", "run_script", "write_file"} & schemas.keys(), "Plan advertises effect tools")
            else:
                require("expected_sha256" in schemas["download_file"].get("properties", {}), "Download checksum schema absent")
            if stage == 0:
                tool, arguments = "web_search", {"query": "official python documentation", "max_results": 2}
            else:
                found = json.loads(receipts[0]["content"])
                require(found.get("untrusted") is True and found.get("retrieved_at"), "Source trust/time metadata absent")
                require(found.get("provider") == "host_configured_searxng", "Configured search provider not used")
                require(found["results"][0]["url"] == "https://docs.python.org/3/", "Source URL lost")
                require("PYTHON_OFFICIAL_SOURCE_73" in receipts[0]["content"], "Search content missing")
                if scenario == "WEB_PIPELINE" and stage == 1:
                    tool, arguments = "download_file", {"url": self.resources.url + "/tool.py", "path": str(self.root / "tool.py"), "expected_sha256": SCRIPT_SHA}
                elif scenario == "WEB_PIPELINE" and stage == 2:
                    require(SCRIPT_SHA in receipts[1]["content"], "Download receipt omitted actual SHA256")
                    require((self.root / "tool.py").read_bytes() == SCRIPT, "Actual download differs")
                    tool, arguments = "run_script", {"language": "python", "script": "import runpy\nrunpy.run_path('tool.py', run_name='__main__')", "cwd": str(self.root), "timeout": 15, "output_limit": 4096}
                else:
                    require(stage == (3 if scenario == "WEB_PIPELINE" else 1), "Unexpected tool replay")
                    if scenario == "WEB_PIPELINE":
                        require(MARKER in receipts[2]["content"], "Actual Python output did not reach model")
                        require((self.root / "download-result.txt").read_text() == MARKER, "Downloaded tool artifact absent")
                    self.accepted.append(scenario)
        elif scenario == "WEB_BAD_HASH":
            if not receipts:
                tool, arguments = "download_file", {"url": self.resources.url + "/tool.py", "path": str(self.root / "preserved.txt"), "expected_sha256": "0" * 64}
            else:
                require(stage == 1 and "sha256" in receipts[0]["content"].lower(), "Checksum rejection receipt missing")
                require((self.root / "preserved.txt").read_bytes() == b"ORIGINAL_KEEP_73", "Bad checksum replaced original destination")
                require(not list(self.root.glob(".handwave-download-*.tmp")), "Failed download temporary file leaked")
                self.accepted.append(scenario)
        elif scenario == "WEB_PLAN_FORGED_DOWNLOAD":
            require("download_file" not in schemas, "Plan advertises download")
            if not receipts:
                tool, arguments = "download_file", {"url": self.resources.url + "/tool.py", "path": str(self.root / "forbidden.py")}
            else:
                require(stage == 1 and not (self.root / "forbidden.py").exists(), "Plan executed forged download")
                self.accepted.append(scenario)
        elif scenario == "WEB_PRIVATE_FETCH":
            if not receipts:
                tool, arguments = "fetch_url", {"url": self.resources.url + "/private-must-not-be-requested"}
            else:
                require(stage == 1, "Private fetch replayed")
                require("public" in receipts[0]["content"].lower() or "private" in receipts[0]["content"].lower(), "Private URL rejection missing")
                require(not any("private-must" in path for path in self.resources.requests), "Private fetch reached HTTP server")
                self.accepted.append(scenario)
        else:
            if not receipts:
                tool, arguments = "fetch_url", {"url": "https://example.com/", "max_bytes": 512}
            else:
                require(stage == 1, "Public fetch replayed")
                page = json.loads(receipts[0]["content"])
                require(page.get("untrusted") is True and page.get("retrieved_at"), "Public page provenance missing")
                require(len(page.get("content_sha256", "")) == 64 and page.get("title") == "Example Domain"
                        and "documentation examples" in page["text"], "Public page content/hash missing")
                self.accepted.append(scenario)
        if receipts:
            self.receipts.append({"scenario": scenario, "stage": stage, "receipts": receipts})
        if tool and scenario != "WEB_PLAN_FORGED_DOWNLOAD":
            require(tool in schemas and arguments.keys() <= schemas[tool].get("properties", {}).keys(), "Actual schema mismatch: " + tool)
        handler.send_response(200)
        handler.send_header("Content-Type", "text/event-stream")
        handler.send_header("Connection", "close")
        handler.end_headers()
        handler.close_connection = True

        def delta(value, finish=None):
            chunk = {"id": "web-fixture", "object": "chat.completion.chunk", "choices": [{"index": 0, "delta": value, "finish_reason": finish}]}
            handler.wfile.write(("data: " + json.dumps(chunk) + "\n\n").encode())
            handler.wfile.flush()

        if tool:
            delta({"tool_calls": [{"index": 0, "id": f"{scenario}-{stage}", "type": "function", "function": {"name": tool, "arguments": json.dumps(arguments)}}]})
            delta({}, "tool_calls")
        else:
            delta({"role": "assistant", "content": scenario + "_VALIDATED"})
            delta({}, "stop")
        handler.wfile.write(b"data: [DONE]\n\n")
        handler.wfile.flush()


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--binary", type=Path, required=True)
    parser.add_argument("--hub-binary", type=Path, required=True)
    parser.add_argument("--output", type=Path)
    parser.add_argument("--public-fetch", action="store_true", help="Also read actual public example.com through the real registered tool")
    args = parser.parse_args()
    root = args.output.resolve() if args.output else Path(tempfile.mkdtemp(prefix="handwave-web-host-"))
    root.mkdir(parents=True, exist_ok=True)
    require(not any(root.iterdir()), "Use a fresh empty evidence directory")
    conf, state, work = (root / name for name in ("hub-conf", "node-state", "work"))
    for directory in (conf, state, work):
        directory.mkdir()
    (work / "preserved.txt").write_bytes(b"ORIGINAL_KEEP_73")
    resources = Resources()
    fixture = WebFixture(work, resources)
    root_token, node_token = secrets.token_hex(24), secrets.token_hex(24)
    hub_port = port()
    hub_url = f"http://127.0.0.1:{hub_port}"
    env = os.environ.copy()
    for key in list(env):
        if key.lower() in ("http_proxy", "https_proxy", "all_proxy", "no_proxy") or key in ("HW_PROXY", "HW_GATEWAY_TOKEN", "HANDWAVE_SEARCH_URL"):
            del env[key]
    env.update(HW_CONF=str(conf), HW_TEST_GATEWAY_TOKEN="fixture-key", HW_TOKEN=node_token,
               NO_PROXY="127.0.0.1,localhost", HANDWAVE_SEARCH_URL=resources.url + "/search")
    node_env = env.copy()
    node_env.pop("HW_TEST_GATEWAY_TOKEN")
    node_env["PATH"] = str(Path(sys.executable).parent) + os.pathsep + node_env.get("PATH", "")
    hub_log, node_log = (root / "hub.log").open("wb"), (root / "node.log").open("wb")
    hub = node = controller = watchdog = None
    transcript, passed = [], []
    result = {"scope": "actual isolated Go Hub + Rust Node; deterministic model; HTTP/file/process receipts", "passed": passed,
              "not_run": ["real-model reasoning", "production deployment", "arbitrary tool installation"]}
    started = time.monotonic()
    try:
        hub = subprocess.Popen([str(args.hub_binary.resolve()), "hub", "-listen", f"127.0.0.1:{hub_port}", "-token", root_token,
            "-node_token", node_token, "-gateway", f"http://127.0.0.1:{fixture.server.server_port}",
            "-gateway_token_env", "HW_TEST_GATEWAY_TOKEN", "-model", "fixture-model", "-protocol", "openai-completions", "-allow_loopback"],
            env=env, cwd=conf, stdout=hub_log, stderr=hub_log)
        wait_health(hub_url)
        node = subprocess.Popen([str(args.binary.resolve()), "--hub", hub_url, "--state-dir", str(state), "--cwd", str(work), "--max-calls", "8", "--max-seconds", "45"],
                                env=node_env, cwd=work, stdout=node_log, stderr=node_log)
        watchdog = threading.Timer(180, lambda: (stop(node), stop(hub)))
        watchdog.daemon = True
        watchdog.start()
        controller = WS(hub_port, "ctl", root_token)
        controller.send({"k": "hello", "fp": "web-host-controller", "relay": True})
        inventory = controller.until(lambda message: message.get("k") == "status" and any(row.get("state") == "pending" for row in status_rows(message)))
        device = next(row for row in status_rows(inventory) if row.get("state") == "pending")
        controller.send({"k": "approve", "nid": device["nid"], "relay": True})
        controller.until(lambda message: message.get("k") == "paired")
        controller.until(lambda message: message.get("k") == "dc" and json.loads(base64.b64decode(message["d"])).get("k") == "node-hello")
        original_frame = controller.frame
        scenarios = ["WEB_PIPELINE", "WEB_BAD_HASH", "WEB_PRIVATE_FETCH", "WEB_PLAN_SEARCH", "WEB_PLAN_FORGED_DOWNLOAD"]
        if args.public_fetch:
            scenarios.append("WEB_PUBLIC_FETCH")
        for index, scenario in enumerate(scenarios):
            mode = "plan" if "PLAN" in scenario else "execute"
            controller.frame = lambda value, mode=mode, index=index: original_frame(dict(value, execution_mode=mode, session_key=f"web-host/case-{index}") if value.get("k") == "agent-loop" else value)
            before = len(resources.requests)
            terminal, events, _ = run_frames(controller, scenario.lower(), scenario, transcript, timeout=50)
            require(not terminal.get("err") and terminal.get("code", 0) == 0, "Model receipt acceptance failed: " + scenario)
            require(scenario in fixture.accepted, "Model never accepted actual receipts: " + scenario)
            require((scenario + "_VALIDATED").encode() in base64.b64decode(terminal["b64"]), "Missing final controller text")
            if scenario in ("WEB_BAD_HASH", "WEB_PRIVATE_FETCH", "WEB_PLAN_FORGED_DOWNLOAD"):
                require(any(event.get("type") == "tool_end" and event.get("status") == "failed" for event in events), "Expected failed tool event missing")
            if scenario == "WEB_PLAN_FORGED_DOWNLOAD":
                require(len(resources.requests) == before, "Plan forged download caused network side effect")
            passed.append(scenario)
        require(not fixture.errors, "Controlled provider assertion errors")
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
        resources.server.shutdown()
        hub_log.close()
        node_log.close()
        result.update(elapsed_seconds=round(time.monotonic() - started, 3), binary_sha256=hashlib.sha256(args.binary.read_bytes()).hexdigest(),
                      hub_sha256=hashlib.sha256(args.hub_binary.read_bytes()).hexdigest(), model_requests=len(fixture.requests), fixture_errors=fixture.errors,
                      resource_requests=resources.requests)
        for name, value in (("result", result), ("controller", transcript), ("model-receipts", fixture.receipts)):
            (root / f"{name}.json").write_text(json.dumps(value, ensure_ascii=False, indent=2), encoding="utf-8")
        print(json.dumps(result, ensure_ascii=False, indent=2))


if __name__ == "__main__":
    main()
