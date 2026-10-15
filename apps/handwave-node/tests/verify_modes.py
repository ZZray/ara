"""Actual Rust Node + Go Hub + deterministic model: modes and goal lifecycle.

Only isolated fixture files are changed. This does not test model intelligence,
production installation or a background goal worker.
"""
from __future__ import annotations

import argparse
import base64
import json
import os
from pathlib import Path
import sys
import tempfile
import threading
import time

from verify_context import ContextFixture, Host
from verify_host import require, stop, validate_loop_events


READ_TOOLS = {"read_file", "search_files", "list_dir", "find_files", "system_info",
              "reference_read", "reference_search", "experience_search"}


class ModesFixture(ContextFixture):
    def __init__(self, root):
        self.hold_started = threading.Event()
        self.hold_release = threading.Event()
        super().__init__(root)

    def reply(self, handler, body):
        require(body.get("tools"), "Unexpected summary call in bounded modes fixture")
        self.agent_requests.append(body)
        messages = body["messages"]
        last_user = max(i for i, row in enumerate(messages) if row.get("role") == "user")
        objective = messages[last_user].get("content", "")
        require(isinstance(objective, str), "Unexpected objective representation")
        tools = [row for row in messages[last_user + 1:] if row.get("role") == "tool"]
        declared = {row["function"]["name"] for row in body["tools"]}
        stage = len(tools)
        if "MODES_HOLD_" in objective or "MODES_FOLLOW_" in objective:
            plan = "_PLAN" in objective
            require(declared == READ_TOOLS if plan else "goal_update" in declared, "Busy follow changed actual mode authority")
            if "MODES_HOLD_" in objective:
                self.hold_started.set()
                require(self.hold_release.wait(10), "Busy follow did not reach transport queue")
            self.emit(handler, text="MODES_BUSY_FOLLOW_VERIFIED")
        elif "MODES_PLAN" in objective:
            require(declared == READ_TOOLS, "Plan mode advertised an effectful or unexpected tool")
            if not tools:
                self.emit(handler, tool="system_info", arguments={})
            else:
                require(stage == 1 and "computer_ui" in tools[0]["content"], "Plan did not observe actual system metadata")
                self.emit(handler, text="MODES_PLAN_VERIFIED")
        elif "MODES_FORGED_" in objective:
            require(declared == READ_TOOLS, "Forged-call trial was not actually restricted")
            forbidden = objective.split("MODES_FORGED_", 1)[1].split()[0]
            if not tools:
                args = {"path": str(self.root / "forbidden.txt"), "content": "ILLEGAL"}
                if forbidden == "exec":
                    args = {"cmd": "echo ILLEGAL"}
                elif forbidden == "run_script":
                    args = {"language": "python", "script": "from pathlib import Path\nPath('forbidden.txt').write_text('ILLEGAL')"}
                elif forbidden == "memory_propose":
                    args = {"kind": "fact", "text": "ILLEGAL", "source_ids": ["ref-1"]}
                self.emit(handler, tool=forbidden, arguments=args)
            else:
                require(stage == 1 and not (self.root / "forbidden.txt").exists(), "Plan executed forbidden file effects")
                self.emit(handler, text="MODES_FORGED_TOOL_REJECTED")
        elif "MODES_GOAL" in objective:
            require("goal_update" in declared and "write_file" in declared, "Goal tools absent")
            steps = [{"name": "Write and read fixture", "status": "in_progress", "evidence": "Not yet verified"}]
            if stage == 0:
                self.emit(handler, tool="goal_update", arguments={"steps": steps})
            elif stage == 1:
                self.emit(handler, tool="write_file", arguments={"path": str(self.root / "goal-result.txt"), "content": "GOAL_ARTIFACT_VERIFIED_73"})
            elif stage == 2:
                self.emit(handler, tool="read_file", arguments={"path": str(self.root / "goal-result.txt")})
            elif stage == 3:
                require("GOAL_ARTIFACT_VERIFIED_73" in tools[2]["content"], "Actual goal artifact missing from read receipt")
                steps[0].update(status="done", evidence="Actual read_file returned GOAL_ARTIFACT_VERIFIED_73")
                self.emit(handler, tool="goal_update", arguments={"steps": steps})
            else:
                require(stage == 4, "Unexpected goal replay")
                self.emit(handler, text="MODES_GOAL_AWAITS_USER_REVIEW")
        elif "MODES_CANCEL" in objective:
            require(stage == 0, "Canceled goal continued provider loop")
            self.emit(handler, tool="run_script", arguments={"language": "python", "timeout": 15,
                "script": "from pathlib import Path\nimport time\nPath('goal-cancel-started.txt').write_text('started')\ntime.sleep(8)\nPath('goal-cancel-survived.txt').write_text('UNSAFE')\n"})
        else:
            require("MODES_LEGACY" in objective, "Unknown modes scenario")
            require("exec" in declared and "run_script" in declared and "goal_update" not in declared, "Legacy execute tools changed")
            self.emit(handler, text="MODES_LEGACY_VERIFIED")


class ModesHost(Host):
    def __init__(self, args, root):
        super().__init__(args, root, owner="modes-owner", max_calls=8)
        self.fixture.server.shutdown()
        self.fixture.server.server_close()
        self.fixture = ModesFixture(self.work)
        self.env["PATH"] = str(Path(sys.executable).parent) + os.pathsep + self.env.get("PATH", "")

    def run_mode(self, objective, *, mode=None, criteria=None, goal_id=None, success=True, cancel=False):
        run_id = "mode-" + str(time.time_ns())
        frame = {"k": "agent-loop", "id": run_id, "msg": objective, "model": "default"}
        if mode is not None:
            frame["execution_mode"] = mode
        if criteria is not None:
            frame["goal_criteria"] = criteria
        if goal_id is not None:
            frame["goal_id"] = goal_id
        self.controller.frame(frame)
        if cancel:
            def abort():
                end = time.monotonic() + 10
                while time.monotonic() < end:
                    if (self.work / "goal-cancel-started.txt").exists():
                        self.controller.frame({"k": "agent-abort", "id": run_id})
                        return
                    time.sleep(0.03)
            threading.Thread(target=abort, daemon=True).start()
        events, output, end = [], bytearray(), time.monotonic() + 30
        while time.monotonic() < end:
            packet = self.controller.receive(end - time.monotonic())
            if packet.get("k") != "dc":
                continue
            row = json.loads(base64.b64decode(packet["d"]))
            self.transcript.append(row)
            if row.get("id") != run_id:
                continue
            if row.get("k") == "agent-event":
                events.append(row["event"])
            if row.get("k") == "out":
                output.extend(base64.b64decode(row.get("b64", "")))
                if row.get("done"):
                    validate_loop_events(events, run_id, row)
                    require((not row.get("err") and row.get("code", 0) == 0) == success, "Unexpected modes terminal outcome: " + str(row))
                    return run_id, events, output.decode(errors="replace")
        raise TimeoutError("Modes Run lacked terminal receipt")

    def native(self):
        return json.loads(self.raw_path().read_text(encoding="utf-8"))

    def busy_pair(self, mode):
        self.fixture.hold_started.clear()
        self.fixture.hold_release.clear()
        first, follow = ("busy-first-" + str(time.time_ns()), "busy-follow-" + str(time.time_ns()))
        criteria = ["Original bounded goal criterion"] if mode == "goal" else []
        self.controller.frame({"k": "agent-loop", "id": first, "msg": "MODES_HOLD_" + mode.upper(),
                               "model": "default", "execution_mode": mode, "goal_criteria": criteria})
        require(self.fixture.hold_started.wait(10), "First busy Run never reached actual provider")
        # Real workbench follow is another agent-loop frame, not agent-follow.
        self.controller.frame({"k": "agent-loop", "id": follow, "msg": "MODES_FOLLOW_" + mode.upper(),
                               "model": "default", "execution_mode": "execute", "goal_id": "forged-other-goal",
                               "goal_criteria": ["Forged replacement criterion"]})
        events, terminals = {first: [], follow: []}, {}
        queued = False
        end = time.monotonic() + 30
        while len(terminals) < 2 and time.monotonic() < end:
            packet = self.controller.receive(end - time.monotonic())
            if packet.get("k") != "dc":
                continue
            row = json.loads(base64.b64decode(packet["d"]))
            self.transcript.append(row)
            rid = row.get("id")
            if rid not in events:
                continue
            if row.get("k") == "agent-event":
                event = row["event"]
                events[rid].append(event)
                if rid == first and event.get("type") == "steer" and event.get("status") == "queued":
                    queued = True
                    self.fixture.hold_release.set()
            if row.get("k") == "out" and row.get("done"):
                require(not row.get("err") and row.get("code", 0) == 0, "Busy Run failed: " + str(row))
                terminals[rid] = row
        require(queued and len(terminals) == 2, "Busy follow not queued/settled using original controller IDs")
        for rid, terminal in terminals.items():
            validate_loop_events(events[rid], rid, terminal)
        if mode == "goal":
            goal = self.native()["goal"]
            require(goal["id"] == first and goal["run"] == follow and goal["criteria"] == criteria,
                    "Busy Goal follow replaced goal identity or criteria")
            require(goal["status"] == "awaiting_review", "Busy Goal follow autoaccepted")


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--binary", type=Path, required=True)
    parser.add_argument("--hub-binary", type=Path, required=True)
    parser.add_argument("--output", type=Path)
    args = parser.parse_args()
    root = args.output.resolve() if args.output else Path(tempfile.mkdtemp(prefix="handwave-modes-"))
    root.mkdir(parents=True, exist_ok=True)
    require(not any(root.iterdir()), "Use a fresh empty evidence directory")
    host = ModesHost(args, root)
    result = {"scope": "actual Rust Node + actual Go Hub; controlled model; isolated files", "passed": [],
              "not_run": ["real model reasoning", "production installation", "background goal continuation"]}
    watchdog = threading.Timer(90, lambda: (stop(host.node), stop(host.hub)))
    watchdog.daemon = True
    watchdog.start()
    try:
        host.start()
        host.run_mode("MODES_PLAN inspect only", mode="plan")
        for tool in ("exec", "run_script", "write_file", "memory_propose"):
            _, events, _ = host.run_mode("MODES_FORGED_" + tool, mode="plan")
            require(any(e.get("type") == "tool_end" and e.get("status") == "failed" for e in events), "Forged tool did not produce failed receipt")
        require(not (host.work / "forbidden.txt").exists(), "Plan caused forbidden file write")
        result["passed"].append("plan_exact_read_schemas_and_forged_effect_tools_rejected")
        count = len(host.fixture.requests)
        host.run_mode("/memory accept mem-1", mode="plan", success=False)
        host.run_mode("MODES_LEGACY", mode="unknown", success=False)
        require(len(host.fixture.requests) == count, "Invalid mode/plan write management reached provider")
        result["passed"].append("unknown_mode_and_plan_management_rejected_without_model_call")
        host.busy_pair("plan")
        host.busy_pair("goal")
        result["passed"].append("actual_agent_loop_busy_follow_cannot_raise_plan_permission_or_replace_goal_identity")
        host.run_mode("MODES_LEGACY missing mode keeps original tools")
        goal_id, events, _ = host.run_mode("MODES_GOAL write verified artifact", mode="goal", criteria=["Read exact artifact text"])
        goal = host.native()["goal"]
        require(goal["id"] == goal_id and goal["status"] == "awaiting_review", "Goal autoaccepted or lost identity")
        require(goal["steps"][0]["status"] == "done" and (host.work / "goal-result.txt").read_text() == "GOAL_ARTIFACT_VERIFIED_73", "Goal progress/artifact wrong")
        require(any(e.get("type") == "goal_update" and e.get("goal", {}).get("status") == "awaiting_review" for e in events), "Final goal state missing from controller")
        host.restart()
        count = len(host.fixture.requests)
        host.run_mode("/goal status")
        host.run_mode("/goal accept", goal_id="stale-goal", success=False)
        host.run_mode("/goal pause " + goal_id)
        require(host.native()["goal"]["status"] == "paused", "Pause not durable")
        host.run_mode("/goal resume " + goal_id)
        require(host.native()["goal"]["status"] == "running", "Resume status not durable")
        host.run_mode("/goal accept", goal_id=goal_id, success=False)
        require(len(host.fixture.requests) == count, "Goal management contacted model")
        host.run_mode("MODES_GOAL continue original goal", mode="goal", goal_id=goal_id)
        host.run_mode("/goal accept", goal_id=goal_id)
        require(host.native()["goal"]["status"] == "accepted" and host.native()["goal"]["id"] == goal_id, "Explicit adoption lost same native goal")
        result["passed"].append("goal_actual_artifact_steps_restart_pause_resume_review_accept_zero_model_commands")
        _, events, _ = host.run_mode("MODES_CANCEL actual child process", mode="goal", criteria=["Must remain unaccepted on cancel"], cancel=True, success=False)
        require(host.native()["goal"]["status"] == "canceled", "Canceled goal did not retain status")
        require(any(g["id"] == goal_id and g["status"] == "accepted" for g in host.native()["goal_history"]), "New goal discarded prior audit")
        time.sleep(0.15)
        require(not (host.work / "goal-cancel-survived.txt").exists(), "Canceled script completed unsafe tail")
        result["passed"].append("goal_cancel_settles_and_preserves_prior_goal_history")
        require(not host.fixture.errors, "Fixture assertions failed: " + str(host.fixture.errors))
    finally:
        watchdog.cancel()
        host.close()
        (root / "result.json").write_text(json.dumps(result, indent=2), encoding="utf-8")
    print(json.dumps(result, ensure_ascii=False))


if __name__ == "__main__":
    main()
