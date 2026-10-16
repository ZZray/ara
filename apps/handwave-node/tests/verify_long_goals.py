"""Real Node/Hub/provider processes: leased slices, detach, review, and Jobs.

All effects are isolated fixtures. This proves orchestration, not model intelligence.
"""
import argparse
import base64
import json
import os
from pathlib import Path
import sys
import tempfile
import threading
import time

from verify_context import Host, ContextFixture, native_state
from verify_host import WS, require, stop


def until(fn, seconds=30):
    end = time.monotonic() + seconds
    while time.monotonic() < end:
        result = fn()
        if result:
            return result
        time.sleep(.05)
    raise AssertionError("Bounded long-goal observation did not occur")


class LongFixture(ContextFixture):
    def __init__(self, root, logical_model="fixture-model"):
        self.logical_model = logical_model
        super().__init__(root)
        self.expected_model = logical_model

    def handler(self):
        base, fixture = super().handler(), self

        class Handler(base):
            def do_GET(self):
                if self.path != "/v1/models" or fixture.logical_model != "auto":
                    return super().do_GET()
                require(self.headers.get("Authorization") == "Bearer fixture-key", "Untrusted directory credential")
                fixture.catalog_requests += 1
                data = json.dumps({"data": [{"id": "auto", "protocols": ["openai-completions"], "context_window": 128000,
                    "max_tokens": 4096, "omp": {"supportsTools": True, "input": ["text"]}}]}).encode()
                self.send_response(200)
                self.send_header("Content-Type", "application/json")
                self.send_header("Content-Length", str(len(data)))
                self.end_headers()
                self.wfile.write(data)

        return Handler

    def reply(self, handler, body):
        if not body.get("tools"):
            return super().reply(handler, body)
        self.agent_requests.append(body)
        text = json.dumps(body["messages"], ensure_ascii=False)
        schemas = {r["function"]["name"] for r in body["tools"]}
        require("goal_update" in schemas, "Long Goal lost its native mode")
        if "LONG_GOAL_JOB_73" in text:
            require("job_start" in schemas, "Explicit Job permission not connected")
            calls = [m for m in body["messages"] if m.get("role") == "assistant" for t in m.get("tool_calls", [])
                     if t["function"]["name"] == "job_start"]
            logs = [m for m in body["messages"] if m.get("role") == "tool" and "JOB_FINISHED_73" in str(m.get("content", ""))]
            if not calls:
                self.emit(handler, tool="job_start", arguments={"op_id": "one-managed-build", "language": "python", "timeout": 12,
                    "script": "from pathlib import Path\nimport time\nprint('JOB_STARTED_73',flush=True)\nPath('job-started.txt').write_text('once')\ntime.sleep(3)\nPath('job-finished.txt').write_text('JOB_FINISHED_73')\nprint('JOB_FINISHED_73',flush=True)"})
            elif not logs:
                results = [m for m in body["messages"] if m.get("role") == "tool"]
                receipt = next(json.loads(m["content"]) for m in results if '"job_id"' in m.get("content", ""))
                require((self.root / "job-finished.txt").exists(), "Model polled before Host Job completion")
                self.emit(handler, tool="job_read_log", arguments={"job_id": receipt["job_id"], "stream": "stdout", "cursor": 0, "limit": 4096})
            else:
                self.emit(handler, text="LONG_JOB_ARTIFACT_AND_LOG_VERIFIED")
        else:
            require("LONG_GOAL_SLICES_73" in text, "Wrong native Session on continuation")
            count = len(list(self.root.glob("slice-effect-*.txt")))
            if count < 5:
                self.emit(handler, tool="run_script", arguments={"language": "python", "script":
                    f"from pathlib import Path\np=Path('slice-effect-{count}.txt')\nwith p.open('x') as f:f.write('ONCE_{count}')\nprint(p.read_text())"})
            else:
                self.emit(handler, text="LONG_SLICES_ARTIFACTS_VERIFIED_AWAIT_USER")


class LongHost(Host):
    def __init__(self, args, root):
        super().__init__(args, root, owner="long-goal-owner", max_calls=2)
        self.fixture.server.shutdown()
        self.fixture.server.server_close()
        self.fixture = LongFixture(self.work, getattr(args, "logical_model", "fixture-model"))
        self.env["PATH"] = str(Path(sys.executable).parent) + os.pathsep + self.env.get("PATH", "")

    def goal(self, goal_id):
        for head_path in self.state.rglob("head.json"):
            try:
                head = json.loads(head_path.read_text())["head"]
                goal = head["metadata"].get("goal")
                if goal and goal["id"] == goal_id:
                    return goal
            except (OSError, ValueError):
                pass
        return None

    def begin(self, goal_id, key, objective, jobs=False):
        self.controller.frame({"k": "agent-loop", "id": goal_id, "session_key": key,
            "msg": objective, "execution_mode": "goal", "goal_criteria": ["Actual exclusive file effects and read receipts"],
            "model": getattr(self.args, "logical_model", "fixture-model"), "protocol": "openai-completions",
            "goal_policy": {"expires_at": int(time.time()) + 120, "max_calls": 12, "max_slices": 8, "allow_jobs": jobs}})

    def reconnect(self):
        self.controller = WS(self.hub_port, "ctl", self.root_token)
        require(self.controller.controller_fp == self.owner, "Reconnected controller changed authenticated owner")
        self.controller.send({"k": "hello", "fp": self.owner, "relay": True})
        self.controller.until(lambda m: m.get("k") == "paired")
        self.controller.until(lambda m: m.get("k") == "dc" and json.loads(base64.b64decode(m["d"])).get("k") == "node-hello")

    def command(self, goal, key, command, command_id, revision=None):
        frame = {"k": "goal-command", "command_id": command_id, "goal_id": goal["id"], "session_key": key,
                 "command": command, "fp": self.device["fp"], "expected_revision": goal["revision"] if revision is None else revision,
                 "reviewed_run_id": goal["last_settled_run"]}
        self.controller.send(frame)
        packet = self.controller.until(lambda m: m.get("k") in ("goal-command", "goal-command-result") and m.get("command_id") == command_id)
        return frame, packet


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--binary", required=True, type=Path)
    parser.add_argument("--hub-binary", required=True, type=Path)
    parser.add_argument("--output", type=Path)
    parser.add_argument("--logical-model", choices=["fixture-model", "auto"], default="fixture-model")
    args = parser.parse_args()
    root = args.output.resolve() if args.output else Path(tempfile.mkdtemp(prefix="long-goals-"))
    root.mkdir(parents=True, exist_ok=True)
    require(not any(root.iterdir()), "Use fresh evidence directory")
    host = LongHost(args, root)
    result = {"passed": [], "logical_model": args.logical_model, "limits": ["controlled model", "no physical upstream random-distribution test", "no production or browser UI", "no power-loss simulation"]}
    watchdog = threading.Timer(100, lambda: (stop(host.node), stop(host.hub)))
    watchdog.daemon = True
    watchdog.start()
    try:
        host.start()
        key, gid = "goals/slices", "long-slices-73"
        host.begin(gid, key, "LONG_GOAL_SLICES_73 create exactly five exclusive numbered files, then verify")
        until(lambda: (host.work / "slice-effect-0.txt").exists())
        host.controller.sock.close()
        host.controller = None
        goal = until(lambda: (g if (g := host.goal(gid)) and g["status"] == "awaiting_review" else None))
        require(len(list(host.work.glob("slice-effect-*.txt"))) == 5, "Lost or duplicated real effects")
        require(goal["budget"]["calls_reserved"] == 6 and goal["budget"]["slices_started"] == 3, "Root accounting reset between slices")
        require(len(host.fixture.agent_requests) == 6, "Unexpected provider polling or replay")
        result["passed"].append("three_fresh_granted_slices_same_session_close_browser_root_budget_and_exclusive_effects")
        host.reconnect()
        until(lambda: host.goal(gid)["status"] == "awaiting_review")
        # Allow Hub's terminal mirror to arrive; inspect is a real controller API.
        host.controller.send({"k": "goal-inspect", "fp": host.device["fp"], "session_key": key})
        host.controller.until(lambda m: m.get("k") == "goal-state" and m.get("goal", {}).get("id") == gid
                              and m["goal"].get("revision") == goal["revision"] and m["goal"].get("status") == "awaiting_review")
        _, stale = host.command(goal, key, "accept", "stale-review-73", revision=goal["revision"] - 1)
        require(stale.get("err"), "Old goal card accepted a later result")
        frame, accepted = host.command(goal, key, "accept", "review-current-73")
        (root / "accept-reply.json").write_text(json.dumps({"request":frame,"response":accepted},indent=2))
        require(not accepted.get("err") and accepted["goal"]["status"] == "accepted", "Explicit review did not persist: " + str(accepted))
        count = len(host.fixture.requests)
        host.controller.send(frame)
        duplicate = host.controller.until(lambda m: m.get("k") == "goal-command-result" and m.get("command_id") == frame["command_id"])
        require(duplicate == accepted and len(host.fixture.requests) == count, "Command replay changed result or called model")
        result["passed"].append("stale_revision_denied_exact_review_and_duplicate_command_receipt_without_model")
        key, gid = "goals/jobs", "long-job-73"
        host.begin(gid, key, "LONG_GOAL_JOB_73 run a managed build, then inspect log and result", jobs=True)
        until(lambda: (host.work / "job-started.txt").exists())
        calls = len(host.fixture.agent_requests)
        time.sleep(.5)
        require(len(host.fixture.agent_requests) == calls, "Waiting Job consumed model polling calls")
        host.controller.sock.close()
        host.controller = None
        goal = until(lambda: (g if (g := host.goal(gid)) and g["status"] == "awaiting_review" else None))
        require((host.work / "job-finished.txt").read_text() == "JOB_FINISHED_73", "Managed Job lost artifact")
        require(goal["budget"]["calls_reserved"] == 3 and goal["budget"]["slices_started"] == 2, "Managed wait reset budget or polled")
        result["passed"].append("managed_job_start_host_wait_no_llm_poll_close_page_log_read_next_slice")
        require(not host.fixture.route_requests, "Background Goal called a routing model")
        require(all(request.get("model") == args.logical_model for request in host.fixture.requests), "Logical model changed upstream")
        result["passed"].append("logical_model_preserved_without_route_resolve")
        require(not host.fixture.errors, "Provider fixture assertions: " + str(host.fixture.errors))
    finally:
        watchdog.cancel()
        host.close()
        (root / "result.json").write_text(json.dumps(result, indent=2), encoding="utf-8")
    print(json.dumps(result))


if __name__ == "__main__":
    main()
