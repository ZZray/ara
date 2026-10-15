"""One bounded real-model problem-solving trial on an isolated temporary Node.

Uses the existing cloud service without changing its configuration. Approves and
revokes only this trial's fresh device. No failed run is retried or replayed.
The working-directory restriction is an instruction, not an OS sandbox.
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
from verify_host import WS, port, require, run_frames, status_rows, stop


EXPECTED = {"ok_count": 3, "total_ms": 42, "error_count": 1}
STDOUT = "REPORT_OK total=42"


def prepare_work(work):
    (work / "logs").mkdir()
    (work / "logs" / "requests.log").write_text(
        "2026-10-15T10:00:00|OK|10\n"
        "2026-10-15T10:00:01|ERROR|timeout\n"
        "2026-10-15T10:00:02|OK|12\n"
        "2026-10-15T10:00:03|OK|20\n", encoding="utf-8")
    (work / "logs" / "last-run.log").write_text(
        "Synthetic problem input: previous application diagnostic\n"
        "aggregate.py: duration = int(parts[2])\n"
        "ValueError: invalid literal for int() with base 10: 'timeout'\n",
        encoding="utf-8")
    source = (
        "import json\nfrom pathlib import Path\n"
        "total = 1\nok_count = 0\nerror_count = 0\n"
        "for line in Path('logs/requests.log').read_text(encoding='utf-8').splitlines():\n"
        "    if not line.strip():\n        continue\n"
        "    parts = line.split('|')\n"
        "    duration = int(parts[2])\n"
        "    if parts[1] == 'OK':\n        ok_count += 1\n        total += duration\n"
        "    else:\n        error_count += 1\n"
        "Path('report.json').write_text(json.dumps({'ok_count': ok_count, 'total_ms': total, 'error_count': error_count}), encoding='utf-8')\n"
        "print('REPORT_OK total=' + str(total))\n"
    )
    (work / "aggregate.py").write_text(source, encoding="utf-8")
    (work / "AGENTS.md").write_text(
        "This is an isolated troubleshooting trial. Read and write only this working directory. "
        "Do not inspect credentials, environment variable values, other directories or running applications. "
        "Do not download, install, change system settings or start background jobs. "
        "Preserve logs/requests.log and derive the report by executing the repaired aggregate.py.\n",
        encoding="utf-8")
    return source


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--binary", type=Path, required=True)
    parser.add_argument("--output", type=Path, required=True)
    parser.add_argument("--ssh-alias", default="ray-dev")
    parser.add_argument("--remote-config", default="/opt/handwave/hub.conf")
    parser.add_argument("--remote-port", type=int, default=18080)
    parser.add_argument("--model", required=True, help="Exact previously verified inventory model ID")
    parser.add_argument("--protocol", default="openai-completions", choices=("openai-completions", "openrouter-chat"))
    parser.add_argument("--public-hub", help="Optional public HTTPS Node entry; root controller remains private over SSH")
    parser.add_argument("--experience", action="store_true", help="Reproduce original failure, then propose one evidence-bound lesson; no adoption")
    parser.add_argument("--experience-smoke", action="store_true", help="Separate small LF one-fault fixture, within the same 10-call ceiling")
    parser.add_argument("--max-calls", type=int, default=10)
    parser.add_argument("--max-seconds", type=int, default=180)
    args = parser.parse_args()
    require(args.binary.is_file(), "Node binary is unavailable")
    require(1 <= args.max_calls <= 10 and 1 <= args.max_seconds <= 180, "Trial exceeds bounded budget")
    require(1 <= args.remote_port <= 65535, "Invalid remote port")
    if args.public_hub:
        parsed = urllib.parse.urlsplit(args.public_hub)
        require(parsed.scheme == "https" and parsed.hostname and not parsed.username and not parsed.password and not parsed.query and not parsed.fragment, "Public Hub must be a credential-free HTTPS root")
    root = args.output.resolve()
    root.mkdir(parents=True, exist_ok=False)
    work, state = root / "work", root / "state"
    work.mkdir()
    state.mkdir()
    original = prepare_work(work)
    require(not args.experience_smoke or args.experience, "Experience smoke requires --experience")
    if args.experience_smoke:
        original = ("import json\nfrom pathlib import Path\n"
                    "rows = [line.split('|') for line in Path('logs/requests.log').read_text(encoding='utf-8').splitlines() if line.strip()]\n"
                    "ok = [row for row in rows if row[1] == 'OK']\n"
                    "report = {'ok_count': len(ok), 'total_ms': sum(int(row[2]) for row in rows), 'error_count': len(rows) - len(ok)}\n"
                    "Path('report.json').write_text(json.dumps(report), encoding='utf-8')\n"
                    "print('REPORT_OK total=' + str(report['total_ms']))\n")
        (work / "aggregate.py").write_text(original, encoding="utf-8", newline="\n")
    input_hash = hashlib.sha256((work / "logs" / "requests.log").read_bytes()).hexdigest()
    root_token, node_token = read_credentials(args.ssh_alias, args.remote_config)
    logs = [(root / "tunnel.log").open("wb"), (root / "node.log").open("wb")]
    tunnel = node = controller = watchdog = None
    device_fp = ""
    approval_attempted = False
    transcript, events = [], []
    started = time.monotonic()
    result = {"status": "FAIL", "scope": "one real cloud-model autonomous troubleshooting run on temporary Node; no production deployment", "model": args.model, "protocol": args.protocol,
              "scenario": "one-fault LF experience admission" if args.experience_smoke else "general report repair",
              "max_model_runs": 1, "max_provider_calls": args.max_calls, "deadline_seconds": args.max_seconds,
              "actual_provider_http_requests": None, "token_usage": None,
              "usage_boundary": "Wire model_call counts reservations, not confirmed HTTP requests. Missing supplier token usage is unknown, not zero.",
              "isolation_boundary": "fresh fixture directory and instructed scope; tools retain OS-account authority, not an OS sandbox"}
    try:
        endpoint_port = port()
        tunnel = subprocess.Popen(["ssh", "-N", "-o", "BatchMode=yes", "-o", "StrictHostKeyChecking=yes", "-o", "ExitOnForwardFailure=yes", "-L", f"127.0.0.1:{endpoint_port}:127.0.0.1:{args.remote_port}", args.ssh_alias], stdout=subprocess.DEVNULL, stderr=logs[0])
        base = f"http://127.0.0.1:{endpoint_port}"
        wait_tunnel_health(base)
        inventory, receipt = models(base, root_token)
        require(receipt["http_status"] == 200 and any(row["id"] == args.model for row in inventory), "Exact model absent from current inventory")
        result["inventory_receipt"] = receipt
        env = os.environ.copy()
        for key in list(env):
            if key.lower() in ("http_proxy", "https_proxy", "all_proxy", "no_proxy", "hw_proxy", "hw_node_state", "hw_hub", "hw_token"):
                del env[key]
        env.update(HW_TOKEN=node_token, NO_PROXY="127.0.0.1,localhost")
        env["PATH"] = str(Path(sys.executable).parent) + os.pathsep + env.get("PATH", "")
        node_args = [str(args.binary.resolve()), "--hub", args.public_hub or base, "--state-dir", str(state), "--cwd", str(work), "--max-calls", str(args.max_calls), "--max-seconds", str(args.max_seconds)]
        identity = subprocess.run(node_args + ["--identity"], env=env, capture_output=True, timeout=15)
        require(identity.returncode == 0, "Temporary identity query failed")
        device_fp = identity.stdout.decode().split("device ", 1)[-1].strip().splitlines()[0]
        require(device_fp and " " not in device_fp, "Invalid temporary identity output")
        node = subprocess.Popen(node_args, env=env, cwd=work, stdout=logs[1], stderr=logs[1])
        watchdog = threading.Timer(max(1, args.max_seconds - (time.monotonic() - started)), lambda: stop(node))
        watchdog.daemon = True
        watchdog.start()
        controller = WS(endpoint_port, "ctl", root_token)
        controller.send({"k": "hello", "fp": "problem-live-" + secrets.token_hex(8), "relay": True})
        status = controller.until(lambda message: message.get("k") == "status" and any(row.get("fp") == device_fp and row.get("nid", "").startswith("n") for row in status_rows(message)))
        device = next(row for row in status_rows(status) if row.get("fp") == device_fp)
        require(device.get("state") == "pending", "Fresh trial device unexpectedly already approved")
        approval_attempted = True
        controller.send({"k": "approve", "nid": device["nid"], "relay": True, "ttl": args.max_seconds + 30})
        controller.until(lambda message: message.get("k") == "paired")
        controller.until(lambda message: message.get("k") == "dc" and json.loads(base64.b64decode(message["d"])).get("k") == "node-hello")
        prompt = (
            f"请自主排查并修复当前临时目录 {work} 中的 Python 报表程序 aggregate.py。"
            "它读取 logs/requests.log，格式为时间|状态|耗时；OK 行的耗时应累计，ERROR 行应计数而不转换错误文字为数字。"
            "已有 logs/last-run.log 可供诊断。修复后应生成 report.json，包含 ok_count、total_ms、error_count 三项且由输入计算得出。"
            "请使用 run_script 实际执行修复后的程序并检查报告结果，保留程序的 REPORT_OK stdout。不要直接硬编码或写入期望报告代替执行。"
            "只读写此临时目录，保留输入日志，不下载、不安装、不修改系统或环境、不访问其他目录或账号数据、不启动后台任务。"
            "你自行决定查找、读取、编辑步骤；最后简洁说明根因、修复与实际验证。"
        )
        if args.experience_smoke:
            prompt += (" 本次是单一故障经验验收，小程序使用LF换行。为保留真实失败状态，原程序复现用 run_script Python直接执行 "
                       "import runpy; runpy.run_path('aggregate.py', run_name='__main__')，不要吞掉异常或把子进程失败变成外层成功。"
                       "只有10次调用预算，请聚焦该单一故障，不进行无关目录盘点。")
        if args.experience:
            prompt += (" 这次也验收经验提取：先用 run_script 运行原始 aggregate.py 复现真实错误，然后修复并实际执行、核对报告。"
                       "完成验证后，使用 experience_propose 提出且仅提出一条简短经验候选，写清适用条件，"
                       "source_ids使用本轮真实objective来源ID，tool_call_ids引用本轮实际失败与恢复成功的调用ID并按先后排序。"
                       "不要为了凑证据故意执行无关失败命令，不要采纳候选，不要声称仅凭exit0就验证了普适经验。")
        (root / "prompt.txt").write_text(prompt, encoding="utf-8")
        remaining = args.max_seconds - (time.monotonic() - started)
        require(remaining > 1, "Trial deadline exhausted before the sole run")
        model_started = time.monotonic()
        terminal, events, live = run_frames(controller, "problem-live-once", prompt, transcript, timeout=remaining, model=args.model, protocol=args.protocol)
        result["model_run_seconds"] = round(time.monotonic() - model_started, 3)
        require(not terminal.get("err") and terminal.get("code", 0) == 0, "Real model task did not settle successfully")
        require(json.loads((work / "report.json").read_text(encoding="utf-8")) == EXPECTED, "Real model artifact differs from exact expected report")
        require((work / "aggregate.py").read_text(encoding="utf-8") != original, "Program was not repaired")
        require(hashlib.sha256((work / "logs" / "requests.log").read_bytes()).hexdigest() == input_hash, "Input log was modified")
        require(any(event.get("type") == "tool_end" and event.get("title") == "run_script" and event.get("status") == "completed" and STDOUT in event.get("output", "") for event in events), "Actual run_script stdout receipt missing")
        if args.experience:
            sidecars = list((state / "sessions").rglob("*.context.json"))
            require(len(sidecars) == 1, "Expected one temporary native context sidecar")
            memories = json.loads(sidecars[0].read_text(encoding="utf-8"))["memories"]
            candidates = [m for m in memories if m.get("experience")]
            require(len(candidates) == 1 and candidates[0]["status"] == "candidate" and candidates[0].get("adoption") is None, "Experience was absent or silently adopted")
            records = candidates[0]["experience"]["receipts"]
            require(records[0]["is_error"] and not records[-1]["is_error"], "Experience lacks actual failure/recovery evidence")
            require(any(event.get("type") == "tool_end" and event.get("title") == "experience_propose" and event.get("status") == "completed" for event in events), "No actual experience proposal receipt")
            result["experience"] = {"id":candidates[0]["id"],"status":"candidate","receipt_tools":[r["tool_name"] for r in records],"task_accepted":False}
        # Independent reexecution checks repaired behavior, not the assistant's claim.
        # Never repeat the Agent request or a tool whose side effects are unknown.
        remaining = args.max_seconds - (time.monotonic() - started)
        require(remaining > 1, "No deadline budget for independent fixture verification")
        checked = subprocess.run([sys.executable, "-I", str(work / "aggregate.py")], cwd=work, capture_output=True, timeout=min(10, remaining))
        require(checked.returncode == 0 and STDOUT.encode() in checked.stdout, "Independent execution of repaired program failed")
        require(json.loads((work / "report.json").read_text(encoding="utf-8")) == EXPECTED, "Independent execution produced incorrect artifact")
        result.update(status="PASS", live_text_received=live, passed=["real_model_autonomous_repair", "actual_run_script_stdout", "exact_report_artifact", "independent_repaired_program_execution", "input_log_preserved"], artifact_sha256=hashlib.sha256((work / "report.json").read_bytes()).hexdigest(), repaired_source_sha256=hashlib.sha256((work / "aggregate.py").read_bytes()).hexdigest(), independent_stdout=checked.stdout.decode("utf-8", errors="replace")[:500])
    except Exception as error:
        # Do not include HTTP/SSH bodies, URLs containing tokens or private values.
        result["failure"] = f"{type(error).__name__}: bounded trial failed; inspect sanitized tool receipts; not replayed"
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
                result["own_temporary_approval_revoked"] = False
                result["status"] = "FAIL"
                result["cleanup_failure"] = "Own temporary approval revocation unconfirmed; bounded TTL remains"
        if controller:
            controller.close()
        stop(tunnel)
        for log in logs:
            log.close()
        all_events = [frame["event"] for frame in transcript if frame.get("k") == "agent-event" and isinstance(frame.get("event"), dict)]
        reservations = [event for event in all_events if event.get("type") == "model_call" and event.get("status") == "reserved"]
        result["observed_model_call_reservations"] = len(reservations)
        result["reservations_complete"] = any(event.get("type") == "agent_end" for event in all_events)
        usage = [event["usage"] for event in all_events if isinstance(event.get("usage"), dict)]
        if usage:
            result["reported_usage_receipts"] = usage
        result.update(elapsed_seconds=round(time.monotonic() - started, 3), binary_sha256=hashlib.sha256(args.binary.read_bytes()).hexdigest())
        for name, value in (("result", result), ("controller", transcript)):
            safe = json.dumps(value, ensure_ascii=False, indent=2)
            for credential in (root_token, node_token):
                safe = safe.replace(credential, "[REDACTED]")
            (root / f"{name}.json").write_text(safe, encoding="utf-8")
        print(json.dumps({"status": result["status"], "evidence": str(root), "elapsed_seconds": result["elapsed_seconds"]}))
    require(result["status"] == "PASS", "Live problem trial failed; evidence retained; no retry performed")


if __name__ == "__main__":
    main()
