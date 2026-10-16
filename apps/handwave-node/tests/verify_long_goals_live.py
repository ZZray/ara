"""One bounded actual-model Goal/Job trial through an isolated local Hub.

No production services/configuration/devices are changed. Credentials and raw
process output stay in memory; evidence contains only sanitized trial receipts.
AGENTS and the task constrain effects to a fresh fixture directory. Those
instructions are not an OS sandbox. Never automatically repeat a failed trial.
"""
from __future__ import annotations

import argparse
import ast
import base64
import hashlib
import json
import os
from pathlib import Path
import re
import secrets
import select
import subprocess
import sys
import tempfile
import threading
import time

from verify_cloud import read_gateway_credential, safe_get, wait_tunnel_health
from verify_host import WS, port, require, status_rows, stop


MODEL, PROTOCOL = "deepseek-v4.1-flash", "openai-completions"
MAX_CALLS, MAX_SLICES, GOAL_SECONDS, WATCHDOG_SECONDS = 12, 6, 180, 210
TERMINAL_JOBS = {"succeeded", "failed", "cancelled", "timed_out", "lease_expired", "log_limit", "interrupted"}
ALLOWED_TOOLS = {"read_file", "list_dir", "job_start", "job_status", "job_read_log", "goal_update"}
REFERENCE_JOB_SCRIPT = '''import json
import time
from pathlib import Path

numbers = json.loads(Path("input.json").read_text(encoding="utf-8"))
time.sleep(2)
report = {"sum": sum(numbers), "count": len(numbers)}
Path("report.json").write_text(json.dumps(report), encoding="utf-8")
print("LIVE_GOAL_REPORT sum=%d count=%d" % (report["sum"], report["count"]))
'''


class PrivateOutput:
    """Drain process pipes without writing credentials/raw logs to evidence."""
    def __init__(self, process):
        self.data = bytearray()
        self.lock = threading.Lock()
        self.thread = threading.Thread(target=self._read, args=(process.stdout,), daemon=True)
        self.thread.start()

    def _read(self, stream):
        while chunk := stream.read1(4096):
            with self.lock:
                self.data.extend(chunk)
                del self.data[:-1024 * 1024]

    def grant_runs(self):
        with self.lock:
            text = bytes(self.data).decode("utf-8", errors="replace")
        # Extract only public Run IDs, never access codes, decisions, or tokens.
        return re.findall(r"smart grant issued: fp=[A-Za-z0-9_-]+ run=([A-Za-z0-9_-]+) ", text)


def read_json(path):
    require(path.stat().st_size <= 4 * 1024 * 1024, "Fixture metadata exceeds limit")
    return json.loads(path.read_text(encoding="utf-8"))


def native_goal(state, goal_id):
    found = []
    for path in state.rglob("head.json"):
        try:
            goal = read_json(path)["head"]["metadata"].get("goal")
            if goal and goal.get("id") == goal_id:
                found.append((path, goal))
        except (OSError, ValueError, KeyError):
            continue  # Atomic head replacement may overlap an observation.
    require(len(found) <= 1, "Goal appeared in multiple native Sessions")
    return found[0] if found else (None, None)


def job_snapshot(state):
    paths = list(state.rglob("request.json"))
    require(len(paths) <= 1, "Trial started more than one managed Job")
    if not paths:
        return None, None
    directory = paths[0].parent
    try:
        return directory, read_json(directory / "receipt.json")
    except (OSError, ValueError):
        return directory, None


def cancel_own_jobs(state):
    if state is None or not state.exists():
        return
    for path in state.rglob("request.json"):
        directory, receipt = path.parent, None
        try:
            receipt = read_json(directory / "receipt.json")
        except (OSError, ValueError):
            pass
        if not receipt or receipt.get("state") not in TERMINAL_JOBS:
            (directory / "cancel.json").write_text(json.dumps({"requested_at": int(time.time())}), encoding="utf-8")


def private_environment():
    env = os.environ.copy()
    for key in list(env):
        if key.lower() in {"http_proxy", "https_proxy", "all_proxy", "no_proxy", "hw_proxy", "hw_token", "hw_hub", "hw_conf", "hw_dev"}:
            del env[key]
    env["NO_PROXY"] = "127.0.0.1,localhost"
    env["PATH"] = str(Path(sys.executable).parent) + os.pathsep + env.get("PATH", "")
    return env


def same_existing_directory(candidate, expected):
    """Compare filesystem identity, including Windows extended-length aliases."""
    candidate, expected = Path(candidate), Path(expected)
    return candidate.is_dir() and expected.is_dir() and candidate.samefile(expected)


def inspect_tool(event, work, *, require_template=True):
    """Detect scope violations from real receipts; this is not a pre-effect gate."""
    require(event.get("title") in ALLOWED_TOOLS, "Model attempted a tool outside the fixture scope")
    args = json.loads(event.get("detail", "{}"))
    if event["title"] == "list_dir":
        path = Path(args["path"])
        path = path if path.is_absolute() else work / path
        require(same_existing_directory(path, work), "Directory listing exceeded the fixture cwd root")
    if event["title"] == "read_file":
        path = Path(args["path"])
        path = path if path.is_absolute() else work / path
        require(path.resolve() in {work / name for name in ("AGENTS.md", "input.json", "report.json")}, "Read exceeded fixture files")
    if event["title"] == "job_start":
        require(args.get("language") == "python", "Job language exceeded fixture scope")
        path = Path(args.get("cwd", str(work)))
        require(same_existing_directory(path if path.is_absolute() else work / path, work), "Job cwd exceeded fixture scope")
        script = ast.parse(args["script"])
        for node in ast.walk(script):
            if isinstance(node, ast.Import):
                require(all(alias.name in {"json", "time", "pathlib"} for alias in node.names), "Unexpected Job import")
            if isinstance(node, ast.ImportFrom):
                require(node.module in {"json", "time", "pathlib"}, "Unexpected Job import")
            if isinstance(node, ast.Call):
                if isinstance(node.func, ast.Name):
                    require(node.func.id in {"Path", "print", "sum", "len", "str", "list", "dict", "tuple", "int", "bool", "float", "open", "sleep", "loads", "dumps", "load", "dump"}, "Unexpected Job function")
                    if node.func.id in {"Path", "open"}:
                        require(node.args and isinstance(node.args[0], ast.Constant) and node.args[0].value in {"input.json", "report.json"}, "Job file exceeded fixture scope")
                else:
                    require(isinstance(node.func, ast.Attribute) and node.func.attr in {"loads", "dumps", "load", "dump", "sleep", "read_text", "write_text", "read", "write"}, "Unexpected Job operation")
                if isinstance(node.func, ast.Name) and node.func.id == "sleep" or isinstance(node.func, ast.Attribute) and node.func.attr == "sleep":
                    require(node.args and isinstance(node.args[0], ast.Constant) and isinstance(node.args[0].value, (int, float)) and 1 <= node.args[0].value <= 2, "Job sleep exceeded fixture duration")
        # This last trial uses one fixed, offline-audited script. Scope checks
        # remain, and exact AST equality rejects alternate file/operation paths.
        if require_template:
            require(ast.dump(script, include_attributes=False) == ast.dump(ast.parse(REFERENCE_JOB_SCRIPT), include_attributes=False),
                    "Job script differed from the fixed fixture template")


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--binary", required=True, type=Path)
    parser.add_argument("--hub-binary", required=True, type=Path)
    parser.add_argument("--output", type=Path)
    parser.add_argument("--terminal", action="store_true", help="Require the same fixed task through native PTY/ConPTY")
    args = parser.parse_args()
    require(args.binary.is_file() and args.hub_binary.is_file(), "Provide existing final Node and local Hub binaries")
    root = args.output.resolve() if args.output else Path(tempfile.mkdtemp(prefix="hw-long-goal-live-"))
    root.mkdir(parents=True, exist_ok=True)
    require(not any(root.iterdir()), "Use a fresh empty evidence directory")
    work, state, conf = root / "work", root / "state", root / "hub-conf"
    for directory in (work, state, conf):
        directory.mkdir()
    started, wall_started = time.monotonic(), int(time.time())
    result = {"status": "FAIL", "scope": "actual cloud model via temporary local Hub and temporary Node; production HandWave not accepted",
              "model": MODEL, "protocol": PROTOCOL, "limits": {"root_http_calls": MAX_CALLS, "slices": MAX_SLICES, "goal_seconds": GOAL_SECONDS,
              "calls_per_slice": 2, "seconds_per_slice": 60, "watchdog_seconds": WATCHDOG_SECONDS}, "terminal_required": args.terminal,
              "binary_sha256": hashlib.sha256(args.binary.read_bytes()).hexdigest(),
              "hub_binary_sha256": hashlib.sha256(args.hub_binary.read_bytes()).hexdigest(),
              "no_os_sandbox_claim": True, "no_automatic_retry": True}
    processes, captures, events, snapshots = [], [], [], []
    waiting_samples, head_paths, call_count, hub_capture = [], set(), 0, None
    controller = None
    expired = threading.Event()
    stage = "initialization"
    gateway_token = root_token = node_token = ""

    def launch(argv, env=None, cwd=None):
        process = subprocess.Popen(argv, env=env, cwd=cwd, stdin=subprocess.DEVNULL,
                                   stdout=subprocess.PIPE, stderr=subprocess.STDOUT)
        processes.append(process)
        capture = PrivateOutput(process)
        captures.append(capture)
        return process, capture

    def watchdog_stop():
        expired.set()
        try:
            cancel_own_jobs(state)
        finally:
            for process in reversed(processes):
                if process.poll() is None:
                    try:
                        process.kill()
                    except OSError:
                        pass

    watchdog = threading.Timer(WATCHDOG_SECONDS, watchdog_stop)
    watchdog.daemon = True
    watchdog.start()
    try:
        stage = "read_only_gateway_credential"
        gateway_token = read_gateway_credential("ray-dev", "/opt/omp-gateway/gateway.yaml")
        root_token, node_token = secrets.token_hex(24), secrets.token_hex(24)
        tunnel_port = port()
        gateway = f"http://127.0.0.1:{tunnel_port}"
        stage = "read_only_ssh_tunnel"
        launch(["ssh", "-N", "-o", "BatchMode=yes", "-o", "StrictHostKeyChecking=yes", "-o", "ExitOnForwardFailure=yes",
                "-L", f"127.0.0.1:{tunnel_port}:127.0.0.1:18799", "ray-dev"])
        wait_tunnel_health(gateway)
        stage = "verify_exact_model_inventory"
        inventory, inventory_receipt = safe_get(gateway, "/v1/models", gateway_token)
        rows = inventory.get("data", []) if isinstance(inventory, dict) else inventory
        require(inventory_receipt["http_status"] == 200 and isinstance(rows, list), "Actual Gateway model inventory unavailable")
        selected = next((row for row in rows if isinstance(row, dict) and row.get("id") == MODEL), None)
        require(selected and PROTOCOL in selected.get("protocols", []), "Exact model/protocol absent from actual Gateway inventory")
        result["model_inventory"] = {"path": "/v1/models", "http_status": 200, "exact_model_protocol_verified": True}
        hub_port = port()
        hub_base = f"http://127.0.0.1:{hub_port}"
        hub_env = private_environment()
        hub_env.update({"HW_CONF": str(conf), "HW_LONG_GOAL_GATEWAY_TOKEN": gateway_token})
        stage = "temporary_local_hub"
        _, hub_capture = launch([str(args.hub_binary.resolve()), "hub", "-listen", f"127.0.0.1:{hub_port}", "-token", root_token,
                "-node_token", node_token, "-gateway", gateway, "-gateway_token_env", "HW_LONG_GOAL_GATEWAY_TOKEN",
                "-model", MODEL, "-protocol", PROTOCOL, "-allow_loopback"], env=hub_env, cwd=conf)
        wait_tunnel_health(hub_base)
        (work / "input.json").write_text("[6,12,24]\n", encoding="utf-8")
        (work / "AGENTS.md").write_text(
            "# Authorized isolated acceptance fixture\nThis task explicitly authorizes exactly one managed Python job_start in this directory.\n"
            "A read-only list_dir of cwd itself is allowed; do not list parent directories, subdirectories, or other locations.\n"
            "Read relative input.json in cwd, wait two seconds, write relative report.json with sum and count, and print LIVE_GOAL_REPORT sum=42 count=3.\nUse only json/time/pathlib; do not use os, __file__, or the private script directory to locate data.\n"
            "Effects must stay in this directory. Do not read credentials, probe the environment, download or install anything, detach exec processes,\n"
            "or adopt memories/Skills. Read Job stdout and report.json afterward; report actual observations.\n"
            "For job_start.script, copy the following exact reference script without rewriting it; only whitespace may differ.\n```python\n"
            + REFERENCE_JOB_SCRIPT + "```\n", encoding="utf-8")
        node_env = private_environment()
        node_env.update({"HW_HUB": hub_base, "HW_TOKEN": node_token})
        stage = "temporary_node_identity"
        identity = subprocess.run([str(args.binary.resolve()), "--state-dir", str(state), "--cwd", str(work), "--identity"],
                                  env=node_env, capture_output=True, timeout=15, check=False)
        require(identity.returncode == 0, "Temporary Node identity query failed")
        device_fp = identity.stdout.decode("utf-8").split("device ", 1)[-1].strip().splitlines()[0]
        require(re.fullmatch(r"[A-Za-z0-9_-]{1,96}", device_fp), "Unexpected temporary identity output")
        stage = "temporary_node_approval"
        launch([str(args.binary.resolve()), "--hub", hub_base, "--cwd", str(work), "--state-dir", str(state),
                "--max-calls", "2", "--max-seconds", "60"], env=node_env, cwd=work)
        controller = WS(hub_port, "ctl", root_token)
        owner = "live-goal-" + secrets.token_hex(8)
        controller.send({"k": "hello", "fp": owner, "relay": True})
        status = controller.until(lambda message: message.get("k") == "status" and any(row.get("fp") == device_fp for row in status_rows(message)))
        device = next(row for row in status_rows(status) if row.get("fp") == device_fp)
        require(device.get("state") == "pending", "Fresh trial device unexpectedly preapproved")
        controller.send({"k": "approve", "nid": device["nid"], "ttl": WATCHDOG_SECONDS, "relay": True})
        controller.until(lambda message: message.get("k") == "paired")
        controller.until(lambda message: message.get("k") == "dc" and json.loads(base64.b64decode(message["d"])).get("k") == "node-hello")
        goal_id, session_key = "live-goal-" + secrets.token_hex(8), "acceptance/managed-job"
        expires_at = min(int(time.time()) + GOAL_SECONDS, wall_started + GOAL_SECONDS)
        require(expires_at - int(time.time()) >= 60, "Setup consumed the bounded trial window")
        prompt = (f"Complete this small authorized fixture task only inside {work}. input.json contains three numbers. "
                  "You may use read-only list_dir on cwd itself (path '.') to inspect these fixture filenames; no other directory listing is authorized. "
                  "Use exactly one job_start with language python and a stable op_id to independently read input.json, sleep two seconds, "
                  "write report.json as a JSON object containing the computed sum and count, and print LIVE_GOAL_REPORT sum=42 count=3. "
                  "The script is stored in a private Job directory; input.json and report.json are relative to cwd, not __file__. Use only json/time/pathlib and relative data paths, without os or __file__. This managed Job is explicitly authorized by the fixture AGENTS.md. After the Host resumes this same Goal, use job_read_log "
                  "to inspect its stdout and read_file to verify report.json. Check actual results against sum 42 and count 3 before reporting. "
                  "Do not run environment discovery, downloads, installations, credential reads, memory adoption, other shell commands, or operations "
                  "outside this temporary directory. The Host will wait and continue bounded slices; do not poll the Job with model calls or claim "
                  "a result before reading the real receipts. A goal_update plan is optional. Finish with a concise factual result for user review. "
                  "For job_start.script copy this exact fixed reference script, without substitutions or alternate implementations (whitespace may differ):\n```python\n"
                  + REFERENCE_JOB_SCRIPT + "```\n")
        if args.terminal:
            prompt += " For job_start set terminal=true explicitly; keep the reference script unchanged. The worker must use a native PTY/ConPTY, not pipes."
        stage = "single_actual_model_goal"
        controller.frame({"k": "agent-loop", "id": goal_id, "session_key": session_key, "execution_mode": "goal", "msg": prompt,
            "goal_criteria": ["One independent Python Job writes report.json with sum 42 and count 3", "Job stdout and report.json are actually read and verified"],
            "model": MODEL, "protocol": PROTOCOL, "goal_policy": {"expires_at": expires_at, "max_calls": MAX_CALLS,
            "max_slices": MAX_SLICES, "allow_jobs": True}})
        head_paths, call_count, waiting_samples, tool_starts = set(), 0, [], {}
        job_announced = False
        final_goal = None
        while time.monotonic() - started < GOAL_SECONDS and not expired.is_set():
            path, goal = native_goal(state, goal_id)
            if goal:
                head_paths.add(str(path.relative_to(state)))
                budget = goal.get("budget", {})
                snapshot = {"status": goal["status"], "revision": goal["revision"], "calls_reserved": budget.get("calls_reserved", 0),
                            "slices_started": budget.get("slices_started", 0), "run": goal["run"], "last_settled_run": goal.get("last_settled_run", "")}
                if not snapshots or snapshot != snapshots[-1]:
                    require(not snapshots or snapshot["calls_reserved"] >= snapshots[-1]["calls_reserved"], "Root call budget reset between slices")
                    require(not snapshots or snapshot["slices_started"] >= snapshots[-1]["slices_started"], "Root slice budget reset")
                    require(snapshot["calls_reserved"] <= MAX_CALLS and snapshot["slices_started"] <= MAX_SLICES, "Root budget exceeded")
                    snapshots.append(snapshot)
                require(goal["status"] != "accepted", "Agent silently accepted the Goal")
                if goal["status"] == "awaiting_review":
                    final_goal = goal
                require(goal["status"] not in {"failed", "canceled", "waiting_recovery", "waiting_authorization", "budget_limited"}, "Goal stopped before verified completion")
            job_dir, job = job_snapshot(state)
            if job_announced and job and job.get("state") not in TERMINAL_JOBS:
                waiting_samples.append({"elapsed": round(time.monotonic() - started, 3), "calls": call_count, "state": job["state"]})
            if not controller.pending and not select.select([controller.sock], [], [], .1)[0]:
                continue
            message = controller.receive(timeout=3)
            if message.get("k") != "dc":
                continue
            frame = json.loads(base64.b64decode(message["d"]))
            if frame.get("k") == "out" and frame.get("done") and final_goal and frame.get("id") == final_goal["last_settled_run"]:
                break  # Ordered terminal follows every tool/model receipt for this Run.
            if frame.get("k") != "agent-event":
                continue
            event = frame.get("event", {})
            kind = event.get("type")
            if kind == "model_call":
                _, current_job = job_snapshot(state)
                require(not job_announced or not current_job or current_job.get("state") in TERMINAL_JOBS, "Actual model call polled an unfinished managed Job")
                call_count += 1
                require(call_count <= MAX_CALLS and event.get("usage") == "unknown", "Call reservation exceeded budget or invented usage")
            if kind == "tool_start":
                events.append({"run":frame["id"],"event":event})
                inspect_tool(event, work)
                tool_starts[(frame["id"], event.get("step_id"))] = event
            if kind == "tool_end" and event.get("title") == "job_start" and event.get("status") == "completed":
                job_announced = True
            if kind in {"tool_end", "model_call", "agent_end"}:
                events.append({"run": frame["id"], "event": event})
        require(final_goal is not None, "Goal did not settle within the single bounded trial")
        stage = "validate_actual_receipts"
        require(len(head_paths) == 1, "Slices changed native Session")
        budget = final_goal["budget"]
        require(2 <= budget["slices_started"] <= MAX_SLICES, "No actual cross-slice continuation observed")
        require(0 < budget["calls_reserved"] <= MAX_CALLS and budget["usage_known"] is False, "Root accounting/unknown usage contract missing")
        job_dir, job = job_snapshot(state)
        require(job and job["state"] == "succeeded" and job["exit_code"] == 0 and job["effects_unknown"] is False, "Independent managed Job did not succeed")
        require(job["worker_pid"] and job["finished_at"] is not None, "Independent worker receipt missing")
        request = read_json(job_dir / "request.json")
        require(request["spec"]["language"] == "python" and same_existing_directory(request["spec"]["cwd"], work), "Job escaped fixture language/cwd")
        if args.terminal:
            require(request["spec"].get("terminal") is True and job.get("terminal") is True, "Actual model did not select a native terminal")
            require(job["stderr_bytes"] == 0, "Native terminal output was duplicated")
        report = read_json(work / "report.json")
        require(report == {"sum": 42, "count": 3}, "Actual report artifact differs from computed fixture result")
        require(read_json(work / "input.json") == [6, 12, 24] and {path.name for path in work.iterdir()} == {"AGENTS.md", "input.json", "report.json"}, "Model changed fixture inputs or produced extra effects")
        stdout = (job_dir / "stdout.bin").read_bytes()
        require(b"LIVE_GOAL_REPORT sum=42 count=3" in stdout and hashlib.sha256(stdout).hexdigest() == job["stdout_sha256"], "Job stdout/hash receipt differs")
        ends = [row for row in events if row["event"].get("type") == "tool_end" and row["event"].get("status") == "completed"]
        require(any(row["event"].get("title") == "job_read_log" and "LIVE_GOAL_REPORT sum=42 count=3" in row["event"].get("output", "") for row in ends), "No actual Job stdout read receipt")
        require(any(row["event"].get("title") == "read_file" and "report.json" in tool_starts.get((row["run"], row["event"].get("step_id")), {}).get("detail", "") and "42" in row["event"].get("output", "") for row in ends), "No actual report read receipt")
        require(waiting_samples and waiting_samples[-1]["elapsed"] - waiting_samples[0]["elapsed"] >= .5 and len({row["calls"] for row in waiting_samples}) == 1, "Did not observe model-free Job waiting")
        records = read_json(conf / "goal-leases.json")["records"]
        lease_record = next(row for row in records.values() if row["lease"]["goal_id"] == goal_id)
        runs = set(lease_record["runs"])
        grants = set(hub_capture.grant_runs())
        require(runs <= grants and len(runs) == budget["slices_started"], "Fresh trusted grant receipt missing for a slice")
        require(lease_record["calls"] == budget["calls_reserved"] == call_count, "Hub HTTP ledger, native root budget and observed reservations differ")
        require(lease_record["pin"]["Model"] == MODEL and lease_record["pin"]["Protocol"] == PROTOCOL, "Cross-slice trusted tuple changed")
        result.update({"status": "PASS", "goal": final_goal, "root_http_attempts": lease_record["calls"], "fresh_granted_runs": sorted(runs),
                       "native_session_heads": sorted(head_paths), "job": job, "report": report, "report_sha256": hashlib.sha256((work / "report.json").read_bytes()).hexdigest(),
                       "waiting_samples": waiting_samples, "passed": ["actual_model_job_tool_loop", "same_session_fresh_granted_slices", "model_free_job_wait",
                       "independent_worker_artifact_and_read_receipts", "root_budget_never_reset", "usage_stays_unknown", "awaiting_review_without_accept"]})
    except Exception as error:
        # Never include exception strings, SSH bodies, argv, env, or raw logs.
        result.update({"failure_stage": stage, "error_type": type(error).__name__, "automatic_retry_performed": False})
        if isinstance(error, AssertionError) and str(error) in {
            "Job script differed from the fixed fixture template", "Unexpected Job import", "Unexpected Job function",
            "Unexpected Job operation", "Job file exceeded fixture scope", "Job sleep exceeded fixture duration",
            "Model attempted a tool outside the fixture scope", "No actual cross-slice continuation observed",
            "Did not observe model-free Job waiting", "Goal stopped before verified completion",
        }:
            result["fixture_assertion"] = str(error)  # Fixed public code, never arbitrary exception text.
    finally:
        try:
            cancel_own_jobs(state)
        except Exception:
            result["cleanup_job_cancel_failure"] = True
        for process in reversed(processes):
            try:
                stop(process)
            except Exception:
                result["cleanup_process_failure"] = True
        for capture in captures:
            capture.thread.join(timeout=2)
        if controller:
            try:
                controller.close()
            except OSError:
                pass
        watchdog.cancel()
        if expired.is_set() or result.get("cleanup_process_failure") or result.get("cleanup_job_cancel_failure"):
            result["status"] = "FAIL"
        result["watchdog_triggered"] = expired.is_set()
        result["elapsed_seconds"] = round(time.monotonic() - started, 3)
        result["snapshots"] = snapshots
        result["waiting_samples"] = waiting_samples
        result["trusted_grant_run_ids"] = sorted(set(hub_capture.grant_runs())) if hub_capture else []
        result["native_session_heads"] = sorted(head_paths)
        result["observed_model_call_count"] = call_count
        # Treat captured model/tool text as untrusted; remove all known tokens
        # before saving any structured receipt. Raw process output is never saved.
        payload = json.dumps({"result": result, "events": events}, ensure_ascii=False, indent=2)
        for credential in (gateway_token, root_token, node_token):
            if credential:
                payload = payload.replace(credential, "[REDACTED]")
        document = json.loads(payload)
        (root / "result.json").write_text(json.dumps(document["result"], ensure_ascii=False, indent=2), encoding="utf-8")
        (root / "receipts.json").write_text(json.dumps(document["events"], ensure_ascii=False, indent=2), encoding="utf-8")
        print(json.dumps({"status": result["status"], "failure_stage": result.get("failure_stage"), "evidence": str(root)}, ensure_ascii=False))
    return 0 if result["status"] == "PASS" else 1


if __name__ == "__main__":
    raise SystemExit(main())
