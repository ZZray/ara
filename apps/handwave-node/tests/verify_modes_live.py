"""Bounded real DeepSeek Plan/Goal trial on one temporary Rust Node identity.

Reuses the existing private SSH control path and optional public HTTPS Node
entry. Only this fixture's fresh approval is created/revoked. No failed model
Run is retried. Work-directory instructions are not an OS sandbox.
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
import threading
import time
import urllib.parse

from verify_cloud import read_credentials, models, wait_tunnel_health
from verify_host import WS, port, require, status_rows, stop, validate_loop_events


MODEL = "deepseek-v4.1-flash"
READ_TOOLS = {"read_file", "search_files", "list_dir", "find_files", "system_info",
              "reference_read", "reference_search", "experience_search"}


def file_hashes(work):
    return {p.relative_to(work).as_posix(): hashlib.sha256(p.read_bytes()).hexdigest()
            for p in sorted(work.rglob("*")) if p.is_file()}


def run_mode(controller, run_id, prompt, transcript, deadline, **fields):
    controller.frame({"k": "agent-loop", "id": run_id, "msg": prompt,
                      "model": MODEL, "protocol": "openai-completions", **fields})
    events, output = [], bytearray()
    while time.monotonic() < deadline:
        packet = controller.receive(deadline - time.monotonic())
        if packet.get("k") != "dc":
            continue
        frame = json.loads(base64.b64decode(packet["d"]))
        transcript.append(frame)
        if frame.get("id") != run_id:
            continue
        if frame.get("k") == "agent-event":
            events.append(frame["event"])
        if frame.get("k") == "out":
            output.extend(base64.b64decode(frame.get("b64", "")))
            if frame.get("done"):
                validate_loop_events(events, run_id, frame)
                require(not frame.get("err") and frame.get("code", 0) == 0, "Real model Run did not settle successfully")
                return events, output.decode(errors="replace")
    raise TimeoutError("Mode trial deadline exhausted")


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--binary", type=Path, required=True)
    parser.add_argument("--output", type=Path, required=True)
    parser.add_argument("--ssh-alias", default="ray-dev")
    parser.add_argument("--remote-config", default="/opt/handwave/hub.conf")
    parser.add_argument("--remote-port", type=int, default=18080)
    parser.add_argument("--public-hub")
    args = parser.parse_args()
    require(args.binary.is_file() and 1 <= args.remote_port <= 65535, "Invalid binary or tunnel port")
    if args.public_hub:
        parsed = urllib.parse.urlsplit(args.public_hub)
        require(parsed.scheme == "https" and parsed.hostname and not parsed.username and not parsed.password
                and not parsed.query and not parsed.fragment, "Public Hub must be a credential-free HTTPS root")
    root = args.output.resolve()
    root.mkdir(parents=True, exist_ok=False)
    work, state = root / "work", root / "state"
    work.mkdir()
    state.mkdir()
    (work / "diagnostic.log").write_text("aggregate.py: duration = int(parts[2])\nValueError: invalid literal for int() with base 10: 'timeout'\nInput row: 2026-10-15|ERROR|timeout\n", encoding="utf-8")
    (work / "input.json").write_text(json.dumps([6, 12, 24]), encoding="utf-8")
    (work / "AGENTS.md").write_text("This is a temporary acceptance fixture. Read/write only this directory. Do not inspect credentials, environment values, other directories or applications. No download, installation, system changes or background jobs. Preserve diagnostic.log and input.json. Never adopt memory candidates yourself.\n", encoding="utf-8")
    before = file_hashes(work)
    root_token, node_token = read_credentials(args.ssh_alias, args.remote_config)
    logs = [(root / "tunnel.log").open("wb"), (root / "node.log").open("wb")]
    tunnel = node = controller = watchdog = None
    approval_attempted, device_fp = False, ""
    transcript, result = [], {"status": "FAIL", "model": MODEL, "protocol": "openai-completions",
        "scope": "two bounded real-model Runs on isolated temporary Node, no production upgrade",
        "budgets": {"plan_calls": 4, "plan_seconds": 60, "goal_calls": 8, "goal_seconds": 120, "total_seconds": 180},
        "token_usage": None, "actual_provider_http_requests": None,
        "usage_boundary": "Model call reservations are not confirmed provider HTTP counts; absent supplier token usage is unknown.",
        "isolation_boundary": "Only plan tool exposure enforces read-only effects; ordinary execute/goal tools retain OS user authority."}
    started = time.monotonic()
    deadline = started + 180
    try:
        endpoint_port = port()
        tunnel = subprocess.Popen(["ssh", "-N", "-o", "BatchMode=yes", "-o", "StrictHostKeyChecking=yes", "-o", "ExitOnForwardFailure=yes",
            "-L", f"127.0.0.1:{endpoint_port}:127.0.0.1:{args.remote_port}", args.ssh_alias], stdout=subprocess.DEVNULL, stderr=logs[0])
        base = f"http://127.0.0.1:{endpoint_port}"
        wait_tunnel_health(base)
        inventory, receipt = models(base, root_token)
        require(receipt["http_status"] == 200 and any(row["id"] == MODEL for row in inventory), "Exact DeepSeek model absent from current inventory")
        result["inventory_receipt"] = receipt
        env = os.environ.copy()
        for key in list(env):
            if key.lower() in ("http_proxy", "https_proxy", "all_proxy", "no_proxy", "hw_proxy", "hw_node_state", "hw_hub", "hw_token"):
                del env[key]
        env.update(HW_TOKEN=node_token, NO_PROXY="127.0.0.1,localhost")
        env["PATH"] = str(Path(sys.executable).parent) + os.pathsep + env.get("PATH", "")

        def launch(calls, seconds):
            return subprocess.Popen([str(args.binary.resolve()), "--hub", args.public_hub or base,
                "--state-dir", str(state), "--cwd", str(work), "--max-calls", str(calls), "--max-seconds", str(seconds)],
                env=env, cwd=work, stdout=logs[1], stderr=logs[1])

        identity = subprocess.run([str(args.binary.resolve()), "--hub", args.public_hub or base,
            "--state-dir", str(state), "--cwd", str(work), "--identity"], env=env, capture_output=True, timeout=10)
        require(identity.returncode == 0, "Temporary identity query failed")
        device_fp = identity.stdout.decode().split("device ", 1)[-1].strip().splitlines()[0]
        require(device_fp and " " not in device_fp, "Invalid temporary identity")
        node = launch(4, 60)
        watchdog = threading.Timer(max(1, deadline - time.monotonic()), lambda: stop(node))
        watchdog.daemon = True
        watchdog.start()
        controller = WS(endpoint_port, "ctl", root_token)
        controller.send({"k": "hello", "fp": "modes-live-" + secrets.token_hex(8), "relay": True})
        status = controller.until(lambda message: message.get("k") == "status" and any(row.get("fp") == device_fp and row.get("nid", "").startswith("n") for row in status_rows(message)))
        device = next(row for row in status_rows(status) if row.get("fp") == device_fp)
        require(device.get("state") == "pending", "Trial device was not fresh")
        approval_attempted = True
        controller.send({"k": "approve", "nid": device["nid"], "relay": True, "ttl": 210})
        controller.until(lambda message: message.get("k") == "paired")
        hello = controller.until(lambda message: message.get("k") == "dc" and json.loads(base64.b64decode(message["d"])).get("k") == "node-hello")
        info = json.loads(base64.b64decode(hello["d"]))
        require("run-modes-v1" in info.get("caps", info.get("info", {}).get("caps", [])), "Node did not advertise modes support")
        prompt = f"当前是只读计划模式。请只读取临时目录 {work} 的 diagnostic.log，指出错误发生原因，给出最小修复建议和验证标准。不要执行命令、脚本、编辑文件、写入记忆或进行系统盘点。你最多4次模型调用，60秒，请读日志后直接给简洁计划。"
        events, text = run_mode(controller, "modes-live-plan", prompt, transcript, min(deadline, time.monotonic() + 60), execution_mode="plan")
        require(text.strip() and any(e.get("type") == "tool_end" and e.get("title") == "read_file" and "ValueError" in e.get("output", "") for e in events), "Plan lacks actual diagnostic read receipt")
        require(all(e.get("title") in READ_TOOLS for e in events if e.get("type") in ("tool_start", "tool_end")), "Plan attempted an unavailable effectful tool")
        require(file_hashes(work) == before, "Plan changed fixture file names or contents")
        result["plan"] = {"status": "PASS", "workspace_hashes_unchanged": True, "reservations": sum(e.get("type") == "model_call" for e in events)}
        stop(node)
        node = None
        controller.until(lambda message: message.get("k") == "unpaired", timeout=8)
        require(deadline - time.monotonic() > 10, "No budget remains for Goal trial")
        node = launch(8, min(120, max(1, int(deadline - time.monotonic()))))
        controller.until(lambda message: message.get("k") == "paired", timeout=15)
        controller.until(lambda message: message.get("k") == "dc" and json.loads(base64.b64decode(message["d"])).get("k") == "node-hello")
        prompt = f"只在临时目录 {work} 完成小目标：读取 input.json，编写 calc.py 用Python实际计算列表总和及数量，必须使用run_script工具（language=python，脚本中执行calc.py）实际执行程序生成 report.json，字段为sum/count，打印 MODE_GOAL_RESULT 和计算总和。然后用read_file检查实际report.json。使用goal_update先记录简短步骤，再用真实工具结果更新done与evidence。不要硬编码报告代替计算，不下载不安装，不改输入及其他目录，不采纳记忆，不故意制造失败。最多8次模型调用和120秒，最后等待用户验收。"
        goal_id = "modes-live-goal"
        events, _ = run_mode(controller, goal_id, prompt, transcript, min(deadline, time.monotonic() + 120),
            execution_mode="goal", goal_criteria=["Execute calc.py on input.json", "Read report.json with sum=42 and count=3", "Record evidence with goal_update"])
        require(json.loads((work / "report.json").read_text(encoding="utf-8")) == {"sum": 42, "count": 3}, "Goal report differs from exact expected result")
        require((work / "calc.py").is_file(), "Goal did not create the requested program")
        require(any(e.get("type") == "tool_end" and e.get("title") == "run_script" and e.get("status") == "completed" and "MODE_GOAL_RESULT" in e.get("output", "") for e in events), "Goal lacks actual Python execution receipt")
        require(any(e.get("type") == "tool_end" and e.get("title") == "read_file" and "sum" in e.get("output", "") and "42" in e.get("output", "") for e in events), "Goal lacks actual report readback")
        raws = [p for p in (state / "sessions").rglob("*.json") if not p.name.endswith((".context.json", ".receipts.json"))]
        require(len(raws) == 1, "Unexpected native Session count")
        raw = json.loads(raws[0].read_text(encoding="utf-8"))
        require(raw["goal"]["id"] == goal_id and raw["goal"]["status"] == "awaiting_review", "Goal was accepted without explicit user review")
        require(any(e.get("type") == "tool_end" and e.get("title") == "goal_update" and e.get("status") == "completed" for e in events), "Goal progress tool was not used")
        sidecars = list((state / "sessions").rglob("*.context.json"))
        require(len(sidecars) == 1 and not any(m.get("status") == "accepted" for m in json.loads(sidecars[0].read_text(encoding="utf-8"))["memories"]), "Trial silently adopted memory")
        for name in ("diagnostic.log", "input.json", "AGENTS.md"):
            require(hashlib.sha256((work / name).read_bytes()).hexdigest() == before[name], "Goal changed preserved fixture input")
        accept_events, _ = run_mode(controller, "modes-live-review", "/goal accept " + goal_id, transcript, min(deadline, time.monotonic() + 10))
        require(not any(e.get("type") == "model_call" for e in accept_events), "Explicit goal review contacted the model")
        require(json.loads(raws[0].read_text(encoding="utf-8"))["goal"]["status"] == "accepted", "Explicit fixture acceptance was not durable")
        result.update(status="PASS", goal={"status": "PASS", "awaiting_review_before_explicit_fixture_accept": True,
            "reservations": sum(e.get("type") == "model_call" for e in events), "artifact_sha256": hashlib.sha256((work / "report.json").read_bytes()).hexdigest()},
            passed=["real_read_only_plan_and_unchanged_workspace", "real_goal_python_artifact_readback_and_progress", "same_native_session_across_restart", "explicit_goal_review_without_model_call", "no_memory_auto_adoption"])
    except Exception as error:
        result["failure"] = type(error).__name__ + ": bounded live modes trial failed; no model Run replayed"
    finally:
        if watchdog:
            watchdog.cancel()
        stop(node)
        if controller and approval_attempted and device_fp:
            try:
                controller.send({"k": "revoke", "fp": device_fp})
                controller.until(lambda message: message.get("k") == "status" and not any(row.get("fp") == device_fp and row.get("state") in ("approved", "paired", "approved-offline") for row in status_rows(message)), timeout=5)
                result["own_temporary_approval_revoked"] = True
            except Exception:
                result.update(status="FAIL", own_temporary_approval_revoked=False, cleanup_failure="Temporary approval revocation unconfirmed; bounded TTL remains")
        if controller:
            controller.close()
        stop(tunnel)
        for log in logs:
            log.close()
        result.update(elapsed_seconds=round(time.monotonic() - started, 3), binary_sha256=hashlib.sha256(args.binary.read_bytes()).hexdigest())
        for name, value in (("result", result), ("controller", transcript)):
            safe = json.dumps(value, ensure_ascii=False, indent=2)
            for credential in (root_token, node_token):
                safe = safe.replace(credential, "[REDACTED]")
            (root / (name + ".json")).write_text(safe, encoding="utf-8")
        print(json.dumps({"status": result["status"], "evidence": str(root), "elapsed_seconds": result["elapsed_seconds"]}))
    require(result["status"] == "PASS", "Bounded live modes trial failed; evidence retained")


if __name__ == "__main__":
    main()
