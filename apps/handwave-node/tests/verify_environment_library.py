"""Isolated actual Node/Hub acceptance for environment knowledge and skill reuse.

The model is deterministic: assertions inspect actual schemas, receipts, files and
process effects. This proves host wiring, not autonomous real-model intelligence.
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

from verify_host import Fixture, WS, port, require, run_frames, run_budget, RUN_BUDGET_PREFIX, status_rows, stop, wait_health

MARKER = "ENVIRONMENT_SKILL_EXECUTED_91"
SCRIPT = ("from pathlib import Path\n"
          "p = Path('skill-result.txt')\n"
          "p.write_text((p.read_text(encoding='utf-8') if p.exists() else '') + '" + MARKER + "\\n', encoding='utf-8')\n"
          "print('" + MARKER + "')\n")
MUTATIONS = {"knowledge_write", "knowledge_revoke", "skill_create", "skill_install", "skill_verify", "skill_remove",
             "exec", "run_script", "download_file", "write_file", "edit_file"}


class EnvironmentFixture(Fixture):
    def __init__(self, root, state, imported, plan_work, plan_state):
        self.state, self.imported = state, imported
        self.plan_work, self.plan_state = plan_work, plan_state
        self.accepted, self.receipts, self.saved = [], [], {}
        super().__init__(root, modern=True)

    def reply(self, handler, body):
        messages = body.get("messages", [])
        last_user = max(i for i, row in enumerate(messages) if row.get("role") == "user")
        prompt = messages[last_user].get("content", "")
        scenario = next((name for name in ("ENV_CREATE", "ENV_INSTALL", "ENV_REUSE", "ENV_RETIRE", "ENV_PLAN") if name in prompt), None)
        require(scenario, "Unexpected environment fixture scenario")
        receipts = [row for row in messages[last_user + 1:] if row.get("role") == "tool"]
        calls = {call["id"]: call["function"]["name"] for row in messages[last_user + 1:] for call in row.get("tool_calls", [])}
        schemas = {row["function"]["name"]: row["function"]["parameters"] for row in body.get("tools", [])}
        require({"environment_catalog", "library_evidence", "skill_read", "knowledge_read", "knowledge_search"} <= schemas.keys(), "Actual model request lacks environment tools")
        for receipt in receipts:
            require(receipt.get("tool_call_id") in calls and isinstance(receipt.get("content"), str), "Receipt lacks native preceding call")
        stage = len(receipts)
        def value(at):
            try:
                return json.loads(receipts[at]["content"])
            except json.JSONDecodeError as error:
                raise AssertionError(f"Expected successful JSON receipt for {calls[receipts[at]['tool_call_id']]}: "
                                     + receipts[at]["content"][:512]) from error
        def expect_error(at, fragment):
            require(fragment in receipts[at]["content"].lower(), "Expected error receipt: " + fragment)
        tool, arguments = None, None
        if scenario == "ENV_CREATE":
            require(MUTATIONS <= schemas.keys(), "Execute schemas lack environment mutations")
            if stage == 0:
                require((self.state / "library/knowledge/INDEX.md").is_file(), "Global INDEX not initialized")
                require((self.root / "docs/knowledge/INDEX.md").read_text(encoding="utf-8").startswith("# Human project knowledge"), "Human index replaced")
                tool, arguments = "environment_catalog", {}
            elif stage == 1:
                catalog = value(0)
                names = {row["name"] for row in catalog["skills"]["skills"]}
                require({"skill-creator", "skill-installer"} <= names, "Builtin metadata not discovered")
                require("SCRIPT_BODY_NOT_IN_CATALOG_91" not in receipts[0]["content"], "Catalog eagerly loaded script body")
                tool, arguments = "skill_read", {"scope": "builtin", "name": "skill-creator", "max_bytes": 64}
            elif stage == 2:
                page = value(1)
                require(page["truncated"] is True and page["next_offset"] > 0 and len(page["content"].encode()) <= 64, "Builtin progressive page failed")
                tool, arguments = "skill_read", {"scope": "builtin", "name": "skill-creator", "offset": page["next_offset"], "max_bytes": 128}
            elif stage == 3:
                require(value(2)["offset"] == value(1)["next_offset"], "Builtin pagination offset lost")
                tool, arguments = "skill_read", {"scope": "builtin", "name": "skill-installer", "max_bytes": 256}
            elif stage == 4:
                require(value(3)["status"] == "builtin", "Installer builtin read failed")
                tool, arguments = "skill_create", {"scope": "project", "name": "marker-tool", "description": "Isolated marker script for verified environment reuse", "instructions": "Read scripts/marker.py then execute it explicitly using run_script. SCRIPT_BODY_NOT_IN_CATALOG_91", "resources": {"scripts/marker.py": SCRIPT, "LICENSE": "MIT fixture license"}}
            elif stage == 5:
                created = value(4)
                require(created["status"] == "unverified" and not (self.root / "skill-result.txt").exists(), "Create executed resource or falsely verified")
                require(Path(created["bundle_dir"]).samefile(self.root / ".handwave/skills/marker-tool")
                        and (Path(created["bundle_dir"]) / "scripts/marker.py").read_text(encoding="utf-8") == SCRIPT,
                        "Create receipt does not identify actual final bundle")
                self.saved["skill_hash"] = created["content_hash"]
                tool, arguments = "skill_read", {"scope": "project", "name": "marker-tool", "path": "scripts/marker.py"}
            elif stage == 6:
                resource = value(5)
                require(Path(resource["file_path"]).read_text(encoding="utf-8") == SCRIPT, "Resource receipt path differs from actual file")
                require(resource["content_hash"] == self.saved["skill_hash"] and not (self.root / "skill-result.txt").exists(), "Read executed script or changed bundle")
                self.saved["script_path"] = resource["file_path"]
                tool, arguments = "run_script", {"language": "python", "script": "import runpy\nrunpy.run_path(" + repr(resource["file_path"]) + ", run_name='__main__')", "cwd": str(self.root), "timeout": 15, "output_limit": 4096}
            elif stage == 7:
                require(MARKER in receipts[6]["content"] and (self.root / "skill-result.txt").read_text(encoding="utf-8") == MARKER + "\n", "Explicit script effect not proven")
                tool, arguments = "library_evidence", {}
            elif stage == 8:
                directory = value(7)
                require(directory["source"] == "native_current_run", "Evidence provenance absent")
                receipt = next(row for row in directory["receipts"] if row["tool_name"] == "run_script")
                require(receipt["tool_call_id"] == receipts[6]["tool_call_id"] and receipt["is_error"] is False and len(receipt["output_hash"]) == 64, "Evidence not bound to actual native script receipt")
                self.saved["evidence_id"] = receipt["tool_call_id"]
                tool, arguments = "skill_verify", {"scope": "project", "name": "marker-tool", "expected_hash": self.saved["skill_hash"], "evidence_ids": [receipt["tool_call_id"]]}
            elif stage in (9, 10):
                if stage == 9:
                    require(value(8)["status"] == "execution_evidence_attached", "Exact skill execution evidence was not attached")
                else:
                    require(value(9)["entry"]["status"] == "observed", "Global technical note lacks host observation")
                    self.saved["global_note"] = value(9)["entry"]
                tool, arguments = "knowledge_write", {"scope": "global" if stage == 9 else "project", "id": "marker-runtime", "title": "Verified Python marker runtime", "content": "The marker-tool resource was executed explicitly and its isolated marker file was checked. This is an environment observation, not task acceptance.", "evidence_ids": [self.saved["evidence_id"]]}
            else:
                require(stage == 11 and value(10)["entry"]["status"] == "observed", "Project note not observed")
                self.saved["project_note"] = value(10)["entry"]
                self.accepted.append(scenario)
        elif scenario == "ENV_INSTALL":
            if stage == 0:
                tool, arguments = "skill_install", {"scope": "global", "source_dir": str(self.imported)}
            elif stage == 1:
                result = value(0)
                require(result["status"] == "unverified" and result["source"].startswith("local:"), "Installed provenance/status absent")
                require(Path(result["bundle_dir"]).samefile(self.state / "library/skills/imported-marker")
                        and (Path(result["bundle_dir"]) / "LICENSE").read_bytes() == (self.imported / "LICENSE").read_bytes(),
                        "Install receipt does not identify final preserved bundle")
                require(not (self.root / "import-executed.txt").exists(), "Install executed resource")
                self.saved["import_hash"] = result["content_hash"]
                tool, arguments = "skill_install", {"scope": "global", "source_dir": str(self.imported)}
            elif stage == 2:
                expect_error(1, "already exists")
                tool, arguments = "skill_read", {"scope": "global", "name": "imported-marker", "path": "LICENSE"}
            elif stage == 3:
                license_page = value(2)
                source_license = (self.imported / "LICENSE").read_bytes()
                require(source_license.replace(b"\r\n", b"\n") == b"MIT local fixture license\n", "License fixture changed")
                require(license_page["content"].encode("utf-8") == source_license
                        and Path(license_page["file_path"]).read_bytes() == source_license,
                        "Installer did not preserve exact source license bytes")
                tool, arguments = "skill_read", {"scope": "global", "name": "imported-marker", "path": "scripts/unused.py"}
            else:
                require(stage == 4 and "import-executed.txt" in value(3)["content"] and not (self.root / "import-executed.txt").exists(), "Resource copy/read executed code")
                resource_page = value(3)
                source_resource = (self.imported / "scripts/unused.py").read_bytes()
                require(resource_page["content"].encode("utf-8") == source_resource
                        and Path(resource_page["file_path"]).read_bytes() == source_resource,
                        "Installer did not preserve exact source resource bytes")
                require((self.imported / "LICENSE").is_file(), "Install mutated source")
                self.accepted.append(scenario)
        elif scenario == "ENV_REUSE":
            if stage == 0:
                tool, arguments = "environment_catalog", {}
            elif stage == 1:
                current = next(row for row in value(0)["skills"]["skills"] if row["name"] == "marker-tool")
                require(current["status"] == "execution_evidence_attached" and current["content_hash"] == self.saved["skill_hash"], "Restart/new-session rediscovery lost exact evidence association")
                tool, arguments = "skill_read", {"scope": "project", "name": "marker-tool", "path": "scripts/marker.py"}
            elif stage == 2:
                resource = value(1)
                require(resource["file_path"] == self.saved["script_path"] and resource["status"] == "execution_evidence_attached", "Reuse moved/reinstalled skill or lost receipt association")
                tool, arguments = "run_script", {"language": "python", "script": "import runpy\nrunpy.run_path(" + repr(resource["file_path"]) + ", run_name='__main__')", "cwd": str(self.root), "timeout": 15, "output_limit": 4096}
            elif stage == 3:
                require((self.root / "skill-result.txt").read_text(encoding="utf-8") == (MARKER + "\n") * 2 and MARKER in receipts[2]["content"], "Reused script was not executed a second time")
                tool, arguments = "knowledge_read", {"scope": "project", "path": self.saved["project_note"]["path"]}
            else:
                require(stage == 4 and value(3)["status"] == "observed" and "environment observation" in value(3)["text"], "New-session knowledge reuse failed")
                require(not any(name in {"skill_install", "skill_create"} for name in calls.values()), "Reuse installed or recreated skill")
                self.accepted.append(scenario)
        elif scenario == "ENV_RETIRE":
            note = self.saved["project_note"]
            if stage == 0:
                tool, arguments = "knowledge_revoke", {"scope": "project", "id": note["id"], "expected_hash": note["sha256"]}
            elif stage == 1:
                require(value(0)["entry"]["status"] == "revoked", "Knowledge not revoked")
                self.saved["revoked_index"] = (self.root / "docs/knowledge/INDEX.md").read_bytes().hex()
                tool, arguments = "knowledge_write", {"scope": "project", "id": note["id"], "title": "Stale writer", "content": "Must not reactivate", "expected_hash": note["sha256"]}
            elif stage == 2:
                expect_error(1, "cannot be reactivated")
                require((self.root / "docs/knowledge/INDEX.md").read_bytes().hex() == self.saved["revoked_index"], "Stale write changed index")
                tool, arguments = "knowledge_read", {"scope": "project", "path": note["path"]}
            elif stage == 3:
                expect_error(2, "revoked")
                tool, arguments = "skill_remove", {"scope": "project", "name": "marker-tool", "expected_hash": self.saved["skill_hash"]}
            elif stage == 4:
                removed = value(3)
                require(removed["status"] == "retired" and Path(removed["preserved_at"]).joinpath("scripts/marker.py").is_file(), "Retired skill source not preserved")
                require((self.root / "docs/knowledge" / note["path"]).is_file(), "Revoked audit file lost")
                tool, arguments = "environment_catalog", {}
            else:
                require(stage == 5 and not any(row["name"] == "marker-tool" for row in value(4)["skills"]["skills"]), "Removed skill still discovered")
                self.accepted.append(scenario)
        else:
            require(not MUTATIONS & schemas.keys(), "Plan advertises mutations")
            require(not (self.plan_work / "docs/knowledge").exists() and not (self.plan_state / "library").exists(), "Plan initialized a library")
            if stage == 0:
                tool, arguments = "environment_catalog", {}
            elif stage == 1:
                require(all(row["status"] == "missing" for row in value(0)["knowledge"]["indexes"]), "Plan missing roots not reported")
                tool, arguments = "skill_read", {"scope": "builtin", "name": "skill-creator", "max_bytes": 128}
            elif stage == 2:
                require(value(1)["status"] == "builtin", "Plan builtin unavailable")
                tool, arguments = "skill_create", {"scope": "project", "name": "forbidden", "description": "Forbidden Plan mutation", "instructions": "Never create"}
            else:
                require(stage == 3, "Plan forged tool replayed")
                self.accepted.append(scenario)
        if receipts:
            self.receipts.append({"scenario": scenario, "stage": stage, "receipts": receipts})
        if tool and not (scenario == "ENV_PLAN" and tool == "skill_create"):
            require(tool in schemas and arguments.keys() <= schemas[tool].get("properties", {}).keys(), "Actual advertised schema mismatch: " + tool)
        handler.send_response(200)
        handler.send_header("Content-Type", "text/event-stream")
        handler.send_header("Connection", "close")
        handler.end_headers()
        handler.close_connection = True
        def delta(value, finish=None):
            chunk = {"id": "environment-fixture", "object": "chat.completion.chunk", "choices": [{"index": 0, "delta": value, "finish_reason": finish}]}
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
    args = parser.parse_args()
    root = args.output.resolve() if args.output else Path(tempfile.mkdtemp(prefix="handwave-environment-host-"))
    root.mkdir(parents=True, exist_ok=True)
    require(not any(root.iterdir()), "Use a fresh empty evidence directory")
    conf, state, work, imported, plan_work, plan_state = (root / name for name in ("hub-conf", "node-state", "work", "import-source", "plan-work", "plan-state"))
    for directory in (conf, state, work, imported, plan_work, plan_state):
        directory.mkdir()
    (work / "docs/knowledge").mkdir(parents=True)
    (work / "docs/knowledge/INDEX.md").write_text("# Human project knowledge\n\n- [Existing](existing.md)\n", encoding="utf-8")
    (work / "docs/knowledge/existing.md").write_text("Human content must remain.\n", encoding="utf-8")
    (imported / "scripts").mkdir()
    (imported / "SKILL.md").write_text("---\nname: imported-marker\ndescription: Local installation fixture\n---\n\nRead resources before explicit execution.\n", encoding="utf-8")
    (imported / "LICENSE").write_text("MIT local fixture license\n", encoding="utf-8")
    (imported / "scripts/unused.py").write_text("from pathlib import Path\nPath('import-executed.txt').write_text('must not execute')\n", encoding="utf-8")
    fixture = EnvironmentFixture(work, state, imported, plan_work, plan_state)
    root_token, node_token = secrets.token_hex(24), secrets.token_hex(24)
    hub_port = port()
    hub_url = f"http://127.0.0.1:{hub_port}"
    env = os.environ.copy()
    for key in list(env):
        if key.lower() in ("http_proxy", "https_proxy", "all_proxy", "no_proxy") or key.startswith("HANDWAVE_") or key in ("HW_PROXY", "HW_GATEWAY_TOKEN", "HW_GLOBAL_KNOWLEDGE", "HW_KNOWLEDGE_DIR", "HW_NODE_STATE"):
            del env[key]
    env.update(HW_CONF=str(conf), HW_TEST_GATEWAY_TOKEN="fixture-key", HW_TOKEN=node_token, NO_PROXY="127.0.0.1,localhost")
    node_env = env.copy()
    node_env.pop("HW_TEST_GATEWAY_TOKEN")
    node_env["PATH"] = str(Path(sys.executable).parent) + os.pathsep + node_env.get("PATH", "")
    hub_log, node_log = (root / "hub.log").open("wb"), (root / "node.log").open("wb")
    hub = node = controller = watchdog = None
    transcript, passed = [], []
    result = {"scope": "actual isolated signed Go Hub + Rust Node; deterministic model; real file/process effects", "passed": passed, "not_run": ["real-model intelligence", "production deployment", "system package installation"]}
    started = time.monotonic()
    def start_node(node_state, cwd):
        return subprocess.Popen([str(args.binary.resolve()), "--hub", hub_url, "--state-dir", str(node_state), "--cwd", str(cwd), "--max-calls", "16", "--max-seconds", "60"], env=node_env, cwd=cwd, stdout=node_log, stderr=node_log)
    def hello(approve):
        nonlocal controller
        controller = WS(hub_port, "ctl", root_token)
        controller.send({"k": "hello", "fp": "environment-host-controller", "relay": True})
        if approve:
            inventory = controller.until(lambda message: message.get("k") == "status" and any(row.get("state") == "pending" for row in status_rows(message)))
            device = next(row for row in status_rows(inventory) if row.get("state") == "pending")
            controller.send({"k": "approve", "nid": device["nid"], "relay": True})
            controller.until(lambda message: message.get("k") == "paired")
        controller.until(lambda message: message.get("k") == "dc" and json.loads(base64.b64decode(message["d"])).get("k") == "node-hello")
    def run(scenario, mode="execute"):
        first_request = len(fixture.requests)
        original_frame = controller.frame
        controller.frame = lambda value: original_frame(dict(value, execution_mode=mode, session_key=f"environment-host/{scenario.lower()}") if value.get("k") == "agent-loop" else value)
        try:
            terminal, events, _ = run_frames(controller, scenario.lower(), scenario, transcript, timeout=65)
        finally:
            controller.frame = original_frame
        require(not terminal.get("err") and terminal.get("code", 0) == 0, "Run failed: " + scenario)
        require(scenario in fixture.accepted and (scenario + "_VALIDATED").encode() in base64.b64decode(terminal["b64"]), "Actual receipts not accepted: " + scenario)
        for index, body in enumerate(fixture.requests[first_request:]):
            observation = run_budget(body["messages"][-1])
            require(observation is not None and observation["reserved_calls"] == index
                    and observation["call_limit"] == 16 and observation["calls_remaining_including_next"] == 16 - index
                    and 0 < observation["milliseconds_remaining"] <= 60000,
                    "Actual provider request lacks fresh shared budget")
            require(sum(run_budget(message) is not None for message in body["messages"]) == 1,
                    "Budget carrier accumulated across calls")
        if scenario in ("ENV_INSTALL", "ENV_RETIRE", "ENV_PLAN"):
            require(any(event.get("type") == "tool_end" and event.get("status") == "failed" for event in events), "Expected rejection tool event absent: " + scenario)
        passed.append(scenario)
    try:
        hub = subprocess.Popen([str(args.hub_binary.resolve()), "hub", "-listen", f"127.0.0.1:{hub_port}", "-token", root_token, "-node_token", node_token, "-gateway", f"http://127.0.0.1:{fixture.server.server_port}", "-gateway_token_env", "HW_TEST_GATEWAY_TOKEN", "-model", "fixture-model", "-protocol", "openai-completions", "-allow_loopback"], env=env, cwd=conf, stdout=hub_log, stderr=hub_log)
        wait_health(hub_url)
        node = start_node(state, work)
        watchdog = threading.Timer(240, lambda: (stop(node), stop(hub)))
        watchdog.daemon = True
        watchdog.start()
        hello(True)
        run("ENV_CREATE")
        run("ENV_INSTALL")
        stop(node)
        controller.close()
        controller = None
        node = start_node(state, work)
        hello(False)
        run("ENV_REUSE")
        run("ENV_RETIRE")
        stop(node)
        controller.close()
        controller = None
        node = start_node(plan_state, plan_work)
        hello(True)
        run("ENV_PLAN", "plan")
        for directory in (state, plan_state):
            require(all(RUN_BUDGET_PREFIX.splitlines()[0].encode() not in path.read_bytes()
                        for path in directory.rglob("*") if path.is_file() and path.suffix in (".json", ".jsonl")),
                    "Provider-only budget leaked into durable state")
        require(not fixture.errors, "Controlled model assertion errors")
        require((work / "docs/knowledge/existing.md").read_text(encoding="utf-8") == "Human content must remain.\n", "Human page overwritten")
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
        for name, value in (("result", result), ("controller", transcript), ("model-receipts", fixture.receipts), ("saved-receipt-values", fixture.saved)):
            (root / f"{name}.json").write_text(json.dumps(value, ensure_ascii=False, indent=2), encoding="utf-8")
        print(json.dumps(result, ensure_ascii=False, indent=2))


if __name__ == "__main__":
    main()
