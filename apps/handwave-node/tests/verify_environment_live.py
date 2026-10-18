"""One bounded two-Session actual-model environment-adaptation trial.

Uses an isolated signed local Hub/Node and the existing read-only SSH Gateway.
The model selects all tools and writes the skill/knowledge itself. No production
device grant, system installation, cloud configuration or service is changed.
Natural tasks are not replaced by mocked tool plans. A FAIL is retained, not retried.
Only the Node receives temporary home-directory variables. This limits accidental
home discovery during the trial; it is not an OS sandbox or access-control claim.
"""
from __future__ import annotations

import argparse
import base64
import hashlib
import http.server
import json
import os
from pathlib import Path
import re
import secrets
import shlex
import subprocess
import threading
import time
import urllib.request

from verify_cloud import read_gateway_credential, wait_tunnel_health
from verify_host import WS, port, require, run_frames, status_rows, stop

MODEL = "deepseek-v4.1-flash"
PROTOCOL = "openai-completions"
KEY = "isolated-environment-budget-gateway-key"
OUTPUT_CAP = 4096
TRIAL_CALLS, TRIAL_SECONDS, TRIAL_RESERVATION = 20, 180, 1_000_000
TOTAL_CALLS, TOTAL_SECONDS, TOTAL_RESERVATION = 28, 300, 1_500_000
RULE_TEXT = """# Fixture JSONL log format

This project's source logs are UTF-8 JSONL. Ignore blank lines. A valid record
is a JSON object with level exactly INFO, WARN or ERROR. Count other nonblank
lines as invalid_records, excluding them from valid_records and level counts.
Never modify source logs. Reports are UTF-8 JSON objects containing
valid_records, invalid_records, and levels with INFO/WARN/ERROR integer counts.
The level string UNKNOWN is deliberately not an ERROR.
Manual rule marker: ENVIRONMENT_PROJECT_RULE_KEEP_20261018.
"""


class BudgetGateway:
    """Independent per-trial and aggregate admission accounting before forwarding."""
    def __init__(self, target, token):
        self.target, self.token = target, token
        self.lock = threading.Lock()
        self.requests, self.responses, self.denials = [], [], []
        self.reserved, self.total_deadline, self.trial = 0, None, None
        self.trials = []
        budget = self

        class Handler(http.server.BaseHTTPRequestHandler):
            def log_message(self, *args):
                pass

            def respond(self, code, payload, kind="application/json"):
                self.send_response(code)
                self.send_header("Content-Type", kind)
                self.send_header("Content-Length", str(len(payload)))
                self.end_headers()
                try:
                    self.wfile.write(payload)
                except (BrokenPipeError, ConnectionResetError):
                    pass

            def do_GET(self):
                if self.headers.get("Authorization") != "Bearer " + KEY:
                    self.respond(401, b"{}")
                    return
                if self.path not in ("/v1/models", "/models"):
                    self.respond(404, b"{}")
                    return
                try:
                    request = urllib.request.Request(budget.target + "/v1/models", headers={"Authorization": "Bearer " + budget.token})
                    with urllib.request.build_opener(urllib.request.ProxyHandler({})).open(request, timeout=12) as response:
                        self.respond(response.status, response.read(1024 * 1024))
                except Exception:
                    self.respond(502, b'{"error":"read-only Gateway catalog unavailable"}')

            def do_POST(self):
                if self.headers.get("Authorization") != "Bearer " + KEY:
                    self.respond(401, b"{}")
                    return
                call_index, started = None, time.monotonic()
                try:
                    require(self.path.endswith("/chat/completions"), "Unexpected model route")
                    count = int(self.headers.get("Content-Length", "0"))
                    require(0 < count <= 1024 * 1024, "Invalid model request size")
                    body = json.loads(self.rfile.read(count))
                    require(body.get("model") == MODEL, "Exact model changed")
                    field = "max_completion_tokens" if "max_completion_tokens" in body else "max_tokens"
                    body[field] = min(OUTPUT_CAP, body.get(field, OUTPUT_CAP))
                    data = json.dumps(body, ensure_ascii=False, separators=(",", ":")).encode()
                    reserve = len(data) + OUTPUT_CAP
                    with budget.lock:
                        now, trial = time.monotonic(), budget.trial
                        reason = None
                        if trial is None:
                            reason = "no active trial"
                        elif now >= trial["deadline"]:
                            reason = f"{TRIAL_SECONDS} second per-trial deadline"
                        elif now >= budget.total_deadline:
                            reason = f"{TOTAL_SECONDS} second aggregate deadline"
                        elif trial["calls"] >= TRIAL_CALLS:
                            reason = f"{TRIAL_CALLS} model HTTP calls per trial"
                        elif len(budget.requests) >= TOTAL_CALLS:
                            reason = f"{TOTAL_CALLS} model HTTP calls aggregate"
                        elif trial["reserved"] + reserve > TRIAL_RESERVATION:
                            reason = f"{TRIAL_RESERVATION} conservative UTF8/output reservation per trial"
                        elif budget.reserved + reserve > TOTAL_RESERVATION:
                            reason = f"{TOTAL_RESERVATION} conservative UTF8/output reservation aggregate"
                        if reason:
                            budget.denials.append({"trial": trial["name"] if trial else None, "reason": reason,
                                "request_utf8_bytes": len(data), "candidate_reservation": reserve})
                            self.respond(429, json.dumps({"error": reason}).encode())
                            return
                        budget.reserved += reserve
                        trial["reserved"] += reserve
                        trial["calls"] += 1
                        call_index = len(budget.requests)
                        budget.requests.append({"trial": trial["name"], "request_utf8_bytes": len(data),
                            "reserved_upper_tokens": reserve, "max_output_tokens": body[field], "body": body})
                        remaining = max(0.1, min(trial["deadline"], budget.total_deadline) - now)
                    request = urllib.request.Request(budget.target + "/v1/chat/completions", data=data,
                        headers={"Authorization": "Bearer " + budget.token, "Content-Type": "application/json"})
                    with urllib.request.build_opener(urllib.request.ProxyHandler({})).open(request, timeout=remaining) as response:
                        payload = response.read(2 * 1024 * 1024 + 1)
                        require(len(payload) <= 2 * 1024 * 1024, "Oversized model response")
                        packets = []
                        for line in payload.decode(errors="replace").splitlines():
                            if line.startswith("data: ") and line != "data: [DONE]":
                                packets.append(json.loads(line[6:]))
                        if not packets and payload.startswith(b"{"):
                            packets = [json.loads(payload)]
                        with budget.lock:
                            budget.responses.append({"call_index": call_index, "trial": trial["name"],
                                "elapsed_seconds": round(time.monotonic() - started, 3), "packets": packets})
                        self.respond(response.status, payload, response.headers.get("Content-Type", "text/event-stream"))
                except Exception as error:
                    with budget.lock:
                        budget.denials.append({"call_index": call_index, "reason": "upstream/request failure " + type(error).__name__})
                    self.respond(502, b'{"error":"bounded real-model trial failed; no automatic replay"}')

        self.server = http.server.ThreadingHTTPServer(("127.0.0.1", 0), Handler)
        self.server.daemon_threads = True
        threading.Thread(target=self.server.serve_forever, daemon=True).start()
        self.url = f"http://127.0.0.1:{self.server.server_port}"

    def begin(self, name):
        with self.lock:
            now = time.monotonic()
            if self.total_deadline is None:
                self.total_deadline = now + TOTAL_SECONDS
            require(now < self.total_deadline, "Aggregate trial time exhausted before dispatch")
            self.trial = {"name": name, "calls": 0, "reserved": 0, "started": now,
                          "deadline": min(now + TRIAL_SECONDS, self.total_deadline)}
            self.trials.append(self.trial)


def write_fixtures(work):
    knowledge = work / "docs" / "knowledge"
    knowledge.mkdir(parents=True)
    (knowledge / "INDEX.md").write_text("# Project knowledge\n\n- [JSONL log format](log-format.md)\n\nHuman-maintained index must remain intact.\n", encoding="utf-8")
    (knowledge / "log-format.md").write_text(RULE_TEXT, encoding="utf-8")
    rows_a = ["INFO", "ERROR", "INFO", "WARN", "ERROR", "INFO"]
    rows_b = ["ERROR", "WARN", "INFO", "ERROR", "WARN", "INFO", "ERROR"]
    for name, rows, extra in [("first.jsonl", rows_a, ["not-json", '{"level":"UNKNOWN"}']),
                               ("second.jsonl", rows_b, ["{broken", '{"level":"UNKNOWN"}', "42"])]:
        lines = [json.dumps({"timestamp": "2026-10-18T12:00:00Z", "level": level, "message": "fixture event " + str(i)}, ensure_ascii=False)
                 for i, level in enumerate(rows)]
        (work / name).write_text("\n".join(lines[:2] + [""] + lines[2:] + extra) + "\n", encoding="utf-8")
    return {"first": {"valid_records": 6, "invalid_records": 2, "levels": {"INFO": 3, "WARN": 1, "ERROR": 2}},
            "second": {"valid_records": 7, "invalid_records": 3, "levels": {"INFO": 2, "WARN": 2, "ERROR": 3}}}


def native_calls(requests, trial):
    calls = {}
    for request in requests:
        if request["trial"] != trial:
            continue
        for row in request["body"].get("messages", []):
            for call in row.get("tool_calls", []):
                function = call.get("function", {})
                args = function.get("arguments", "{}")
                try:
                    args = json.loads(args) if isinstance(args, str) else args
                except json.JSONDecodeError:
                    args = {"unparsed": args}
                calls[call["id"]] = {"name": function.get("name"), "arguments": args}
    return calls


def bundle_hash(directory):
    digest = hashlib.sha256()
    for path in sorted((p for p in directory.rglob("*") if p.is_file() and p.relative_to(directory).as_posix() != ".handwave-skill.json"),
                       key=lambda p: p.relative_to(directory).as_posix()):
        name, content = path.relative_to(directory).as_posix().encode(), path.read_bytes()
        digest.update(len(name).to_bytes(8, "big")); digest.update(name)
        digest.update(len(content).to_bytes(8, "big")); digest.update(content)
    return digest.hexdigest()


def skill_snapshot(work):
    result = []
    base = work / ".handwave" / "skills"
    if base.exists():
        for path in sorted(base.iterdir()):
            if path.is_dir() and not path.name.startswith(".") and (path / "SKILL.md").is_file():
                meta = json.loads((path / ".handwave-skill.json").read_text(encoding="utf-8")) if (path / ".handwave-skill.json").is_file() else {}
                result.append({"name": path.name, "directory": str(path), "content_hash": bundle_hash(path), "metadata": meta,
                               "resources": [p.relative_to(path).as_posix() for p in sorted(path.rglob("*")) if p.is_file()]})
    return result


def resolved_path(value, cwd):
    value = str(value).strip("'\"")
    if value.startswith("\\\\?\\"):
        value = value[4:]
    path = Path(value)
    return (path if path.is_absolute() else cwd / path).resolve()


def path_evidence(work, calls, events, snapshots):
    """Bounded recognition of this fixture's direct Python invocations, not a shell parser."""
    successful = {event["step_id"]: event for event in events if event.get("type") == "tool_end"
                  and event.get("status") == "completed" and event.get("step_id")}
    resources, entries = {}, {}
    for skill in snapshots:
        directory = Path(skill["directory"])
        entries[resolved_path(directory / "SKILL.md", work)] = skill
        for relative in skill["resources"]:
            if relative.startswith("scripts/") and relative.endswith(".py"):
                resources[resolved_path(directory / relative, work)] = skill
    index = (work / "docs" / "knowledge" / "INDEX.md").read_text(encoding="utf-8")
    manifest = re.search(r"<!-- manifest:(.*?) -->", index, re.DOTALL)
    notes = {}
    for entry in json.loads(manifest[1]) if manifest else []:
        path = resolved_path(entry["path"], work / "docs" / "knowledge")
        if entry.get("status") == "observed" and path.is_file() and hashlib.sha256(path.read_bytes()).hexdigest() == entry["sha256"]:
            notes[path] = entry["sha256"]
    reads, invocations = [], []
    for call_id, call in calls.items():
        event = successful.get(call_id)
        if event is None or event.get("title") != call["name"]:
            continue
        args = call["arguments"]
        cwd = resolved_path(args.get("cwd", str(work)), work)
        read_path = None
        if call["name"] == "read_file" and isinstance(args.get("path"), str):
            read_path = resolved_path(args["path"], work)
        elif call["name"] == "skill_read" and args.get("scope") == "project":
            read_path = resolved_path(work / ".handwave" / "skills" / args.get("name", "") / args.get("path", "SKILL.md"), work)
        elif call["name"] == "knowledge_read" and args.get("scope") == "project":
            read_path = resolved_path(args.get("path", "INDEX.md"), work / "docs" / "knowledge")
        if read_path in entries or read_path in notes:
            reads.append({"call_id": call_id, "tool": call["name"], "path": str(read_path),
                          "kind": "current_skill_entry" if read_path in entries else "current_indexed_note",
                          "content_hash": entries[read_path]["content_hash"] if read_path in entries else notes[read_path],
                          "receipt_status": "completed"})
        if call["name"] not in ("exec", "run_script"):
            continue
        command = args.get("cmd" if call["name"] == "exec" else "script", "")
        if not isinstance(command, str):
            continue
        assignments = {match[1].lower(): match[3] for match in re.finditer(r"(\$[A-Za-z_]\w*)\s*=\s*(['\"])(.*?)\2", command)}
        for statement in re.split(r"[;\r\n]", command):
            try:
                tokens = shlex.split(statement.strip().lstrip("&").strip(), posix=False)
            except ValueError:
                continue
            if not tokens:
                continue
            executable = assignments.get(tokens[0].lower(), tokens[0]).strip("'\"")
            if not re.fullmatch(r"(?:python(?:3(?:\.\d+)?)?|py)(?:\.exe)?", executable.replace("\\", "/").rsplit("/", 1)[-1], re.IGNORECASE):
                continue
            i = 1
            while i < len(tokens) and tokens[i] in ("-u", "-B", "-E", "-s", "-Xutf8", "-X"):
                i += 2 if tokens[i] == "-X" else 1
            if i >= len(tokens):
                continue
            resource = resolved_path(assignments.get(tokens[i].lower(), tokens[i]), cwd)
            if resource in resources:
                invocations.append({"call_id": call_id, "tool": call["name"], "cwd": str(cwd), "resource": str(resource),
                    "skill": resources[resource]["name"], "content_hash": resources[resource]["content_hash"],
                    "interpreter": executable, "command_or_script": command, "receipt_status": "completed",
                    "review_boundary": "Candidate direct Python invocation resolved to an existing current bundle script. Root/reviewer must inspect the command and actual report; this is not a general shell parser."})
    return {"reads": reads, "invocations": invocations}


def validate_trial(trial, work, state, expected, calls, events, text, before_skills):
    report = work / ("first-summary.json" if trial == "create" else "second-summary.json")
    require(report.is_file(), "Actual model omitted report artifact")
    actual = json.loads(report.read_text(encoding="utf-8-sig"))
    require(all(actual.get(key) == value for key, value in expected.items()), "Actual log counts differ from independent fixture expectation")
    require(("ERROR" in text.upper() or "错误" in text) and str(expected["invalid_records"]) in text, "Final reply omitted actual counts")
    names = [call["name"] for call in calls.values()]
    ended = [event for event in events if event.get("type") == "tool_end" and event.get("status") == "completed"]
    require(any(event.get("title") in ("exec", "run_script") for event in ended), "No successful actual process receipt")
    snapshots = skill_snapshot(work)
    require(snapshots, "No actual reusable project skill bundle")
    audit = path_evidence(work, calls, events, snapshots)
    (work.parent / (trial + "-path-evidence.json")).write_text(json.dumps(audit, ensure_ascii=False, indent=2), encoding="utf-8")
    if trial == "create":
        ordered = list(calls.values())
        rule_read = next((i for i, call in enumerate(ordered) if call["name"] in ("knowledge_read", "read_file") and "log-format.md" in json.dumps(call["arguments"])), None)
        first_process = next((i for i, call in enumerate(ordered) if call["name"] in ("exec", "run_script")), None)
        require(rule_read is not None and first_process is not None and rule_read < first_process, "Project rule was not read before executing the workflow")
        require("skill_create" in names, "Actual model did not use native skill creation")
        require("skill_verify" in names, "Actual model did not attach native execution evidence")
        require("knowledge_write" in names, "Actual model did not persist native indexed knowledge")
        for scope, base in [("project", work / "docs" / "knowledge")]:
            index = (base / "INDEX.md").read_text(encoding="utf-8")
            require("<!-- handwave-knowledge-v1:begin -->" in index, "Missing native managed knowledge index in " + scope)
            notes = list((base / "notes").glob("*.md")) if (base / "notes").exists() else []
            require(notes and any("Status: observed" in p.read_text(encoding="utf-8") for p in notes), "No observed native technical content in " + scope)
        require((work / "docs" / "knowledge" / "log-format.md").read_text(encoding="utf-8") == RULE_TEXT, "Human project source rule changed")
        require("Human-maintained index must remain intact." in (work / "docs" / "knowledge" / "INDEX.md").read_text(encoding="utf-8"), "Human project index was replaced")
    else:
        require("skill_create" not in names and "skill_install" not in names, "New Session recreated/installed instead of reusing")
        require(any(read["kind"] == "current_skill_entry" for read in audit["reads"]), "New Session did not load the current discovered skill entry")
        require(any(read["kind"] == "current_indexed_note" for read in audit["reads"]), "New Session did not retrieve prior current indexed content")
        require({s["name"]: s["content_hash"] for s in snapshots} == {s["name"]: s["content_hash"] for s in before_skills}, "Reuse unexpectedly changed skill bundle content")
    exercised = list(dict.fromkeys(row["call_id"] for row in audit["invocations"]))
    require(exercised, "No actual process invocation referenced the reusable installed bundle")
    if trial == "create":
        require(any(skill["metadata"].get("verified_hash") == skill["content_hash"]
                    and skill["metadata"].get("evidence", {}).get("source") == "native_current_run"
                    and any(row.get("tool_call_id") in exercised for row in skill["metadata"].get("evidence", {}).get("receipts", []))
                    for skill in snapshots), "Skill execution-evidence association lacks exact-hash actual invocation evidence")
    return {"actual_report": actual, "skills": snapshots, "exercised_call_ids": exercised, "path_evidence": audit}


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--binary", type=Path, required=True)
    parser.add_argument("--expected-sha256", required=True)
    parser.add_argument("--hub-binary", type=Path, required=True)
    parser.add_argument("--output", type=Path, required=True)
    parser.add_argument("--ssh-alias", default="ray-dev")
    parser.add_argument("--gateway-config", default="/opt/omp-gateway/gateway.yaml")
    args = parser.parse_args()
    require(hashlib.sha256(args.binary.read_bytes()).hexdigest() == args.expected_sha256.lower(), "Candidate binary SHA256 does not match delivered image")
    root = args.output.resolve()
    root.mkdir(parents=True, exist_ok=False)
    conf, work, state = (root / name for name in ("hub-conf", "work", "state"))
    for path in (conf, work, state):
        path.mkdir()
    # Config::load merges executable-adjacent node.conf and then state/node.conf.
    # Override a possible adjacent proxy without reading or copying its secrets;
    # the explicit --knowledge-dir below also overrides adjacent knowledge roots.
    (state / "node.conf").write_text("# Isolated acceptance configuration\nproxy=\n", encoding="utf-8")
    expected = write_fixtures(work)
    source_hashes = {p.name: hashlib.sha256(p.read_bytes()).hexdigest() for p in work.glob("*.jsonl")}
    logs = [(root / name).open("wb") for name in ("ssh.log", "hub.log", "node.log")]
    root_token, node_token = secrets.token_hex(24), secrets.token_hex(24)
    tunnel = hub = node = controller = gateway = watchdog = None
    transcript = []
    result = {"status": "FAIL", "model": MODEL, "protocol": PROTOCOL, "trials": [],
        "scope": "selected project task: actual model creates/tests a project skill and indexed project knowledge, then a fresh Session reuses both; no production authorization changes",
        "not_run": {"global_knowledge_real_model": "Not part of this representative project task; global scope is covered separately by actual controlled-host process acceptance."},
        "environment_isolation": "Node-only HOME/USERPROFILE/APPDATA/LOCALAPPDATA point to temporary directories; parent SSH environment and PATH unchanged. This is not an OS sandbox.",
        "budgets": {"per_trial": {"calls": TRIAL_CALLS, "seconds": TRIAL_SECONDS, "reserved_upper_tokens": TRIAL_RESERVATION},
                    "aggregate": {"calls": TOTAL_CALLS, "seconds": TOTAL_SECONDS, "reserved_upper_tokens": TOTAL_RESERVATION}, "output_per_call": OUTPUT_CAP},
        "usage_boundary": f"Conservative request UTF8 bytes plus {OUTPUT_CAP} output tokens reserved before each forwarded HTTP call. Actual provider usage recorded separately; cache hits/costs not inferred."}
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
        require(selected and PROTOCOL in selected.get("protocols", []), "Exact model/protocol absent from live catalog")
        result["model_catalog"] = selected
        gateway = BudgetGateway(target, credential)
        env = os.environ.copy()
        for key in list(env):
            if key.lower() in ("http_proxy", "https_proxy", "all_proxy", "no_proxy", "hw_proxy", "hw_gateway_token",
                               "hw_hub", "hw_token", "hw_conf", "hw_test_gateway_token", "hw_node_state", "hw_knowledge_dir",
                               "handwave_search_url", "handwave_global_knowledge"):
                del env[key]
        env.update(HW_CONF=str(conf), HW_TEST_GATEWAY_TOKEN=KEY, HW_TOKEN=node_token, NO_PROXY="127.0.0.1,localhost")
        hub_port = port()
        hub_url = f"http://127.0.0.1:{hub_port}"
        hub = subprocess.Popen([str(args.hub_binary.resolve()), "hub", "-listen", f"127.0.0.1:{hub_port}", "-token", root_token,
            "-node_token", node_token, "-gateway", gateway.url, "-gateway_token_env", "HW_TEST_GATEWAY_TOKEN", "-model", MODEL,
            "-protocol", PROTOCOL, "-allow_loopback"], env=env, cwd=conf, stdout=logs[1], stderr=logs[1])
        wait_tunnel_health(hub_url)
        node_env = env.copy(); node_env.pop("HW_TEST_GATEWAY_TOKEN")
        isolated_home = root / "isolated-home"
        for name in ("home", "appdata", "localappdata"):
            (isolated_home / name).mkdir(parents=True)
        node_env.update(HOME=str(isolated_home / "home"), USERPROFILE=str(isolated_home / "home"),
                        APPDATA=str(isolated_home / "appdata"), LOCALAPPDATA=str(isolated_home / "localappdata"))
        node = subprocess.Popen([str(args.binary.resolve()), "--hub", hub_url, "--state-dir", str(state), "--cwd", str(work),
            "--knowledge-dir", str(state / "library" / "knowledge"),
            "--max-calls", str(TRIAL_CALLS), "--max-seconds", str(TRIAL_SECONDS)], env=node_env, cwd=work, stdout=logs[2], stderr=logs[2])
        controller = WS(hub_port, "ctl", root_token)
        controller.send({"k": "hello", "fp": "environment-live-trial", "relay": True})
        inventory = controller.until(lambda m: m.get("k") == "status" and any(row.get("state") == "pending" for row in status_rows(m)))
        device = next(row for row in status_rows(inventory) if row.get("state") == "pending")
        controller.send({"k": "approve", "nid": device["nid"], "relay": True})
        controller.until(lambda m: m.get("k") == "paired")
        controller.until(lambda m: m.get("k") == "dc" and json.loads(base64.b64decode(m["d"])).get("k") == "node-hello")
        prompts = [
            ("create", "first", "请处理这个项目的日志分析任务：先查阅项目已有日志规范，然后分析 first.jsonl，"
             "把统计结果保存为 first-summary.json，最后简洁汇报每类记录数量、有效和无效记录数。"
             "请把这次解决办法做成一个带可复用 Python 脚本的项目技能，在当前电脑实际运行验证，保留可查验的测试记录；"
             "把项目日志规则和使用该技能的方法沉淀到项目知识库；本次无需额外探测OS或Python环境，也无需写全局知识。"
             "沿用索引加内容页，保留现有人工资料。脚本应能接收另一份输入日志和输出路径，供以后新会话直接复用。"
             "每次命令只输出完成当前步骤所需的简洁版本、路径或统计，最多约2KB；不要递归打印目录、日志或整个对象。"
             "本任务只允许读写这个临时项目及这台临时被控端自己的技能/知识目录；不要联网、下载、安装系统程序或修改系统配置。"),
            ("reuse", "second", "这是一个新的日志分析任务：请先查找这个项目已经沉淀的知识及技能，"
             "按照原项目日志规范，复用已经存在并验证的 Python 日志分析技能处理 second.jsonl，"
             "把结果保存为 second-summary.json，最后简洁汇报每类记录数量、有效和无效记录数，并指出实际复用了哪个技能。"
             "请实际调用已经保存的脚本，不要重写脚本、重新创建或安装技能。不得修改任何原始日志或人工知识资料；"
             "每次命令只输出当前步骤必要的简洁路径或统计，最多约2KB；不要递归打印目录、日志或整个对象。"
             "不要联网、安装软件或修改系统配置。本任务只允许这个临时项目及本临时被控端自己的技能/知识目录。")]
        before_skills = []
        for trial_name, fixture_name, prompt in prompts:
            require(node.poll() is None, "Candidate Node exited before next trial")
            gateway.begin(trial_name)
            watchdog = threading.Timer(TRIAL_SECONDS + 1, lambda: stop(node)); watchdog.daemon = True; watchdog.start()
            trial = {"name": trial_name, "session_key": "environment-live/" + trial_name, "status": "FAIL", "prompt": prompt}
            trial_started = time.monotonic()
            try:
                terminal, events, _ = run_frames(controller, "environment-live-" + trial_name, prompt, transcript,
                    timeout=min(TRIAL_SECONDS, max(1, gateway.total_deadline - time.monotonic())), model=MODEL, protocol=PROTOCOL, session_key=trial["session_key"])
                text = base64.b64decode(terminal["b64"]).decode(errors="replace")
                trial.update(terminal=terminal, final_text=text, events=events)
                require(not terminal.get("err") and terminal.get("code", 0) == 0, "Actual model trial did not settle successfully")
                calls = native_calls(gateway.requests, trial_name)
                trial["actual_native_calls"] = calls
                trial.update(validate_trial(trial_name, work, state, expected[fixture_name], calls, events, text, before_skills))
                first_request = next(row["body"] for row in gateway.requests if row["trial"] == trial_name)
                schemas = {row["function"]["name"] for row in first_request.get("tools", [])}
                require({"skill_read", "skill_create", "skill_install", "skill_verify", "library_evidence", "knowledge_write", "knowledge_read"} <= schemas,
                        "Actual model request omitted native capability schemas")
                trial["initial_request_tool_schemas"] = sorted(schemas)
                if trial_name == "reuse":
                    serialized = json.dumps(first_request, ensure_ascii=False)
                    require(prompts[0][2] not in serialized, "New Session inherited first Session conversation")
                    require(any(skill["name"] in serialized and skill["content_hash"] in serialized for skill in before_skills),
                            "Fresh second Session directory omitted persisted skill metadata/hash")
                    require("notes/" in serialized and "log-format.md" in serialized, "Fresh second Session omitted current indexed knowledge")
                trial["status"] = "PASS"
            except Exception as error:
                trial["error"] = type(error).__name__ + ": " + str(error) if isinstance(error, AssertionError) else type(error).__name__ + ": bounded trial failed; inspect receipts"
            finally:
                watchdog.cancel(); watchdog = None
                trial["elapsed_seconds"] = round(time.monotonic() - trial_started, 3)
                trial["model_http_calls"] = sum(row["trial"] == trial_name for row in gateway.requests)
                trial["conservative_reserved_upper_tokens"] = sum(row["reserved_upper_tokens"] for row in gateway.requests if row["trial"] == trial_name)
                result["trials"].append(trial)
                (root / (trial_name + "-result.json")).write_text(json.dumps(trial, ensure_ascii=False, indent=2), encoding="utf-8")
                if trial["status"] == "PASS":
                    before_skills = skill_snapshot(work)
            if trial_name == "create" and trial["status"] != "PASS":
                skipped = {"name": "reuse", "session_key": "environment-live/reuse", "status": "NOT_RUN",
                           "reason": "dependency_create_failed; no reuse task dispatched and no model call spent on a missing prerequisite"}
                result["trials"].append(skipped)
                (root / "reuse-result.json").write_text(json.dumps(skipped, ensure_ascii=False, indent=2), encoding="utf-8")
                break
        require(all(hashlib.sha256((work / name).read_bytes()).hexdigest() == value for name, value in source_hashes.items()), "Source logs changed")
        require(len(result["trials"]) == 2 and all(trial["status"] == "PASS" for trial in result["trials"]), "One or both actual-model trials failed; no retry")
        require(not gateway.denials, "Budget/upstream denial occurred")
        result["status"] = "PASS"
    except Exception as error:
        result["error"] = type(error).__name__ + ": " + str(error) if isinstance(error, AssertionError) else type(error).__name__ + ": bounded trial failed; inspect receipts"
    finally:
        if watchdog:
            watchdog.cancel()
        if controller:
            controller.close()
        stop(node); stop(hub); stop(tunnel)
        if gateway:
            gateway.server.shutdown()
            result.update(model_http_calls=len(gateway.requests), conservative_reserved_upper_tokens=gateway.reserved,
                budget_or_upstream_denials=gateway.denials,
                provider_usage=[packet["usage"] for response in gateway.responses for packet in response["packets"] if packet.get("usage") is not None])
            (root / "provider-requests.json").write_text(json.dumps(gateway.requests, ensure_ascii=False, indent=2), encoding="utf-8")
            (root / "provider-sse.json").write_text(json.dumps(gateway.responses, ensure_ascii=False, indent=2), encoding="utf-8")
        for log in logs:
            log.close()
        result.update(elapsed_seconds=round(time.monotonic() - started, 3), binary_sha256=hashlib.sha256(args.binary.read_bytes()).hexdigest(),
            hub_sha256=hashlib.sha256(args.hub_binary.read_bytes()).hexdigest(), source_log_sha256=source_hashes)
        (root / "result.json").write_text(json.dumps(result, ensure_ascii=False, indent=2), encoding="utf-8")
        (root / "controller.json").write_text(json.dumps(transcript, ensure_ascii=False, indent=2), encoding="utf-8")
        print(json.dumps({key: result[key] for key in ("status", "error", "model_http_calls", "conservative_reserved_upper_tokens", "elapsed_seconds") if key in result} | {"evidence": str(root)}, ensure_ascii=False, indent=2))
    if result["status"] != "PASS":
        raise SystemExit(1)


if __name__ == "__main__":
    main()
