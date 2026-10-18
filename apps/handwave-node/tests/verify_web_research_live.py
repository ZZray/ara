"""Bounded actual-model research through an isolated local Hub and SSH Gateway.

Only local temporary authorization is created. A transparent test budget proxy
caps output and total calls without selecting tools or changing model text.
No production Hub/device authorization or cloud configuration is modified.
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
import threading
import time
import urllib.request

from verify_cloud import read_gateway_credential, wait_tunnel_health
from verify_host import WS, port, require, run_frames, status_rows, stop
from verify_web_research import Resources


MODEL = "deepseek-v4.1-flash"


class BudgetGateway:
    def __init__(self, target, token, max_reserved_tokens, max_model_calls):
        self.target, self.token = target, token
        self.max_reserved_tokens, self.max_model_calls = max_reserved_tokens, max_model_calls
        self.calls, self.reserved, self.responses, self.denials = [], 0, [], []
        self.denied_requests = []
        self.lock = threading.Lock()
        self.deadline = None
        budget = self

        class Handler(http.server.BaseHTTPRequestHandler):
            def log_message(self, *args):
                pass

            def respond(self, code, data, content_type="application/json"):
                self.send_response(code)
                self.send_header("Content-Type", content_type)
                self.send_header("Content-Length", str(len(data)))
                self.end_headers()
                self.wfile.write(data)

            def do_GET(self):
                if self.headers.get("Authorization") != "Bearer isolated-budget-gateway-key":
                    self.respond(401, b'{}')
                    return
                if self.path not in ("/v1/models", "/models"):
                    self.respond(404, b'{}')
                    return
                try:
                    request = urllib.request.Request(budget.target + "/v1/models", headers={"Authorization": "Bearer " + budget.token})
                    with urllib.request.build_opener(urllib.request.ProxyHandler({})).open(request, timeout=12) as response:
                        self.respond(response.status, response.read(1024 * 1024))
                except Exception:
                    self.respond(502, b'{"error":"read-only Gateway catalog unavailable"}')

            def do_POST(self):
                if self.headers.get("Authorization") != "Bearer isolated-budget-gateway-key":
                    self.respond(401, b'{}')
                    return
                try:
                    require(self.path.endswith("/chat/completions"), "Unexpected model route")
                    count = int(self.headers.get("Content-Length", "0"))
                    require(0 < count <= 1024 * 1024, "Invalid request length")
                    request = json.loads(self.rfile.read(count))
                    require(request.get("model") == MODEL, "Exact model changed")
                    # This is a disclosed trial budget cap, not a product rewrite.
                    field = "max_completion_tokens" if "max_completion_tokens" in request else "max_tokens"
                    request[field] = min(1500, request.get(field, 1500))
                    data = json.dumps(request, ensure_ascii=False, separators=(",", ":")).encode()
                    reserve = len(data) + 1500
                    with budget.lock:
                        reason = None
                        if budget.deadline is None or time.monotonic() >= budget.deadline:
                            reason = "90 second trial deadline"
                        elif len(budget.calls) >= budget.max_model_calls:
                            reason = f"{budget.max_model_calls} model HTTP calls"
                        elif budget.reserved + reserve > budget.max_reserved_tokens:
                            reason = f"{budget.max_reserved_tokens} conservative UTF8 bytes plus output tokens reservation"
                        if reason:
                            budget.denials.append(reason)
                            budget.denied_requests.append({"reason": reason, "request_utf8_bytes": len(data),
                                "candidate_reserved_upper_tokens": reserve, "previous_reserved_upper_tokens": budget.reserved,
                                "candidate_cumulative_upper_tokens": budget.reserved + reserve})
                            self.respond(429, json.dumps({"error": reason}).encode())
                            return
                        budget.reserved += reserve
                        budget.calls.append({"request_utf8_bytes": len(data), "reserved_upper_tokens": reserve,
                                             "max_output_tokens": request[field], "tools": [t["function"]["name"] for t in request.get("tools", [])]})
                    upstream = urllib.request.Request(budget.target + "/v1/chat/completions", data=data,
                        headers={"Authorization": "Bearer " + budget.token, "Content-Type": "application/json"})
                    remaining = max(0.1, budget.deadline - time.monotonic())
                    with urllib.request.build_opener(urllib.request.ProxyHandler({})).open(upstream, timeout=remaining) as response:
                        payload = response.read(2 * 1024 * 1024 + 1)
                        require(len(payload) <= 2 * 1024 * 1024, "Oversized model response")
                        packets = []
                        for line in payload.decode(errors="replace").splitlines():
                            if line.startswith("data: ") and line != "data: [DONE]":
                                packets.append(json.loads(line[6:]))
                        budget.responses.append(packets)
                        self.respond(response.status, payload, response.headers.get("Content-Type", "text/event-stream"))
                except Exception as error:
                    budget.denials.append("upstream failure " + type(error).__name__)
                    self.respond(502, b'{"error":"bounded real upstream request failed; no automatic replay"}')

        self.server = http.server.ThreadingHTTPServer(("127.0.0.1", 0), Handler)
        self.server.daemon_threads = True
        threading.Thread(target=self.server.serve_forever, daemon=True).start()
        self.url = f"http://127.0.0.1:{self.server.server_port}"


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--binary", type=Path, required=True)
    parser.add_argument("--hub-binary", type=Path, required=True)
    parser.add_argument("--output", type=Path, required=True)
    parser.add_argument("--ssh-alias", default="ray-dev")
    parser.add_argument("--gateway-config", default="/opt/omp-gateway/gateway.yaml")
    parser.add_argument("--max-reserved-tokens", type=int, choices=(64000, 96000), default=64000,
                        help="Trial-only conservative request-UTF8-bytes plus output-cap cumulative ceiling")
    parser.add_argument("--max-model-calls", type=int, choices=(4, 6), default=6)
    parser.add_argument("--search-fixture", action="store_true", help="Use disclosed controlled SearXNG official leads; model and public fetch remain actual")
    args = parser.parse_args()
    root = args.output.resolve()
    root.mkdir(parents=True, exist_ok=False)
    conf, work, state = (root / name for name in ("hub-conf", "work", "state"))
    for path in (conf, work, state):
        path.mkdir()
    logs = [(root / name).open("wb") for name in ("ssh.log", "hub.log", "node.log")]
    root_token, node_token = secrets.token_hex(24), secrets.token_hex(24)
    tunnel = hub = node = controller = watchdog = gateway = resources = None
    transcript, result = [], {"status": "FAIL", "model": MODEL, "protocol": "openai-completions",
        "scope": "actual model + temporary local Hub/Node + existing read-only SSH Gateway; no production auth mutations",
        "search": "controlled SearXNG leads" if args.search_fixture else "actual default DuckDuckGo Lite",
        "budgets": {"calls": args.max_model_calls, "seconds": 90, "output_per_call": 1500,
                    "conservative_total_token_upper_reservation": args.max_reserved_tokens},
        "usage_boundary": "Reservation counts request UTF8 bytes plus output cap as a conservative ceiling; actual provider usage reported separately. Cache hit/cost not inferred."}
    started = time.monotonic()
    try:
        credential = read_gateway_credential(args.ssh_alias, args.gateway_config)
        tunnel_port = port()
        tunnel = subprocess.Popen(["ssh", "-N", "-o", "BatchMode=yes", "-o", "StrictHostKeyChecking=yes", "-o", "ExitOnForwardFailure=yes",
            "-L", f"127.0.0.1:{tunnel_port}:127.0.0.1:18799", args.ssh_alias], stdout=subprocess.DEVNULL, stderr=logs[0])
        target = f"http://127.0.0.1:{tunnel_port}"
        wait_tunnel_health(target)
        request = urllib.request.Request(target + "/v1/models", headers={"Authorization": "Bearer " + credential})
        with urllib.request.build_opener(urllib.request.ProxyHandler({})).open(request, timeout=12) as response:
            directory = json.load(response)
        selected = next((row for row in directory.get("data", []) if row.get("id") == MODEL), None)
        require(selected and "openai-completions" in selected.get("protocols", []), "Exact model/protocol absent from live catalog")
        result["model_catalog"] = selected
        gateway = BudgetGateway(target, credential, args.max_reserved_tokens, args.max_model_calls)
        env = os.environ.copy()
        for key in list(env):
            if key.lower() in ("http_proxy", "https_proxy", "all_proxy", "no_proxy", "hw_proxy", "hw_gateway_token", "handwave_search_url"):
                del env[key]
        env.update(HW_CONF=str(conf), HW_TEST_GATEWAY_TOKEN="isolated-budget-gateway-key", HW_TOKEN=node_token, NO_PROXY="127.0.0.1,localhost")
        hub_port = port()
        hub_url = f"http://127.0.0.1:{hub_port}"
        hub = subprocess.Popen([str(args.hub_binary.resolve()), "hub", "-listen", f"127.0.0.1:{hub_port}", "-token", root_token,
            "-node_token", node_token, "-gateway", gateway.url, "-gateway_token_env", "HW_TEST_GATEWAY_TOKEN", "-model", MODEL,
            "-protocol", "openai-completions", "-allow_loopback"], env=env, cwd=conf, stdout=logs[1], stderr=logs[1])
        wait_tunnel_health(hub_url)
        node_env = env.copy()
        node_env.pop("HW_TEST_GATEWAY_TOKEN")
        if args.search_fixture:
            resources = Resources(expected_query=None, official_url="https://docs.python.org/3/library/venv.html")
            node_env["HANDWAVE_SEARCH_URL"] = resources.url + "/search"
        node = subprocess.Popen([str(args.binary.resolve()), "--hub", hub_url, "--state-dir", str(state), "--cwd", str(work),
            "--max-calls", str(args.max_model_calls), "--max-seconds", "90"], env=node_env, cwd=work, stdout=logs[2], stderr=logs[2])
        controller = WS(hub_port, "ctl", root_token)
        controller.send({"k": "hello", "fp": "web-research-live-trial", "relay": True})
        inventory = controller.until(lambda m: m.get("k") == "status" and any(row.get("state") == "pending" for row in status_rows(m)))
        device = next(row for row in status_rows(inventory) if row.get("state") == "pending")
        controller.send({"k": "approve", "nid": device["nid"], "relay": True})
        controller.until(lambda m: m.get("k") == "paired")
        controller.until(lambda m: m.get("k") == "dc" and json.loads(base64.b64decode(m["d"])).get("k") == "node-hello")
        gateway.deadline = time.monotonic() + 90
        watchdog = threading.Timer(91, lambda: stop(node))
        watchdog.daemon = True
        watchdog.start()
        original = controller.frame
        controller.frame = lambda value: original(dict(value, execution_mode="plan", session_key="web-live/research") if value.get("k") == "agent-loop" else value)
        prompt = ("请联网研究 Python 项目创建隔离虚拟环境的官方推荐方式。先搜索，再打开一个相关官方网页核实；"
                  "最后简洁给出 Windows 与 Linux 的创建和激活命令，并附实际核实过的官方来源链接。"
                  "本次最多搜索一次、读网页一次。不要下载、安装、执行命令或读本机文件；若联网受阻就明确报告，不能编造已读页面。")
        terminal, events, _ = run_frames(controller, "web-live-research", prompt, transcript, timeout=90, model=MODEL, protocol="openai-completions")
        text = base64.b64decode(terminal["b64"]).decode(errors="replace")
        result["final_text"] = text
        result["terminal"] = terminal
        starts = [event.get("title") for event in events if event.get("type") == "tool_start"]
        result["actual_tool_choices"] = starts
        successful = [event for event in events if event.get("type") == "tool_end" and event.get("status") != "failed"]
        require(not terminal.get("err") and terminal.get("code", 0) == 0, "Real model task did not settle successfully")
        require("web_search" in starts and "fetch_url" in starts, "Actual model did not choose both search and page reading")
        require(any('"content_sha256"' in event.get("output", "") and "docs.python.org" in event.get("output", "") for event in successful), "No actual successful official page content receipt")
        require(any('"results"' in event.get("output", "") for event in successful), "No successful actual search receipt")
        require("docs.python.org" in text and "venv" in text and "activate" in text, "Final answer omitted verified source or environment commands")
        require(not list(work.iterdir()), "Read-only research changed work directory")
        result["status"] = "PASS"
    except Exception as error:
        result["error"] = type(error).__name__ + ": " + str(error) if isinstance(error, AssertionError) else type(error).__name__ + ": bounded trial failed; inspect receipts"
    finally:
        if watchdog:
            watchdog.cancel()
        if controller:
            controller.close()
        stop(node)
        stop(hub)
        stop(tunnel)
        if gateway:
            gateway.server.shutdown()
            result.update(model_http_calls=len(gateway.calls), conservative_reserved_upper_tokens=gateway.reserved,
                          requests=gateway.calls, budget_or_upstream_denials=gateway.denials,
                          denied_requests=gateway.denied_requests,
                          provider_usage=[packet["usage"] for response in gateway.responses for packet in response if packet.get("usage") is not None])
            (root / "provider-sse.json").write_text(json.dumps(gateway.responses, ensure_ascii=False, indent=2), encoding="utf-8")
        if resources:
            resources.server.shutdown()
        for log in logs:
            log.close()
        result.update(elapsed_seconds=round(time.monotonic() - started, 3), binary_sha256=hashlib.sha256(args.binary.read_bytes()).hexdigest(),
                      hub_sha256=hashlib.sha256(args.hub_binary.read_bytes()).hexdigest())
        (root / "result.json").write_text(json.dumps(result, ensure_ascii=False, indent=2), encoding="utf-8")
        (root / "controller.json").write_text(json.dumps(transcript, ensure_ascii=False, indent=2), encoding="utf-8")
        summary = {key: result[key] for key in ("status", "error", "model_http_calls", "actual_tool_choices", "conservative_reserved_upper_tokens", "elapsed_seconds") if key in result}
        summary["evidence"] = str(root)
        print(json.dumps(summary, ensure_ascii=False, indent=2))
    if result["status"] != "PASS":
        raise SystemExit(1)


if __name__ == "__main__":
    main()
