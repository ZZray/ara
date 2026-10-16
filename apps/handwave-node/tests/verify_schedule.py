"""Bounded actual Rust Node + Hub process acceptance for once-only Goal resumes.

Controlled local model only. No production credentials, account grants, or paid
model calls. All native journals, config, effects, and receipts live under output.
"""
from __future__ import annotations

import argparse
import hashlib
import json
import os
from pathlib import Path
import sys
import tempfile
import threading
import time

from verify_context import ContextFixture, Host
from verify_host import require, stop
from verify_long_goals import LongHost, until


class ScheduleFixture(ContextFixture):
    def __init__(self, root):
        self.phase = "initial"
        super().__init__(root)

    def reply(self, handler, body):
        require(body.get("tools"), "Scheduling fixture unexpectedly requested compaction")
        self.agent_requests.append(body)
        schemas = {row["function"]["name"] for row in body["tools"]}
        require("goal_update" in schemas and "run_script" in schemas, "Scheduled continuation lost Goal tools")
        require("SCHEDULE_GOAL_91" in json.dumps(body["messages"]), "Scheduled continuation changed native Session")
        if not (self.root / "initial-effect.txt").exists():
            self.emit(handler, tool="run_script", arguments={"language": "python", "script":
                "from pathlib import Path\nwith Path('initial-effect.txt').open('x') as f:f.write('INITIAL_ONCE_91')\nprint('INITIAL_ONCE_91')"})
        elif self.phase == "initial":
            self.emit(handler, text="INITIAL_SLICE_VERIFIED_READY_FOR_EXPLICIT_PAUSE_91")
        elif not (self.root / "scheduled-effect.json").exists():
            self.emit(handler, tool="run_script", arguments={"language": "python", "script":
                "from pathlib import Path\nimport json\ndata={'sum':sum([11,13,18]),'count':3}\nwith Path('scheduled-effect.json').open('x') as f:json.dump(data,f)\nprint(json.dumps(data))"})
        else:
            self.emit(handler, text="SCHEDULED_SLICE_VERIFIED_SUM42_COUNT3_AWAIT_USER_REVIEW_91")


class ScheduleHost(LongHost):
    def __init__(self, args, root):
        Host.__init__(self, args, root, owner="schedule-owner-91", max_calls=2)
        self.fixture.server.shutdown()
        self.fixture.server.server_close()
        self.fixture = ScheduleFixture(self.work)
        self.env["PATH"] = str(Path(sys.executable).parent) + os.pathsep + self.env.get("PATH", "")

    def mirror(self, gid, key, status):
        goal = until(lambda: g if (g := self.goal(gid)) and g["status"] == status else None)
        self.controller.send({"k": "goal-inspect", "fp": self.device["fp"], "session_key": key})
        packet = self.controller.until(lambda m: m.get("k") == "goal-state" and m.get("goal", {}).get("id") == gid
            and m["goal"].get("revision") == goal["revision"] and m["goal"].get("status") == status)
        self.transcript.append(packet)
        return packet

    def schedule(self, operation, request=None, expect_error=False):
        correlation = "schedule-request-" + str(time.time_ns())
        self.controller.send({"k": operation, "id": correlation, **({"schedule": request} if request is not None else {})})
        packet = self.controller.until(lambda m: m.get("k") == "schedule-result" and m.get("id") == correlation)
        self.transcript.append(packet)
        require(bool(packet.get("err")) == expect_error, "Unexpected schedule reply: " + str(packet))
        return packet

    def schedule_disk(self, schedule_id):
        try:
            book = json.loads((self.root / "hub-conf" / "schedules.json").read_text(encoding="utf-8"))
            return book["records"].get(self.owner + "\0" + schedule_id)
        except (OSError, ValueError):
            return None


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--binary", required=True, type=Path)
    parser.add_argument("--hub-binary", required=True, type=Path)
    parser.add_argument("--output", type=Path)
    args = parser.parse_args()
    root = args.output.resolve() if args.output else Path(tempfile.mkdtemp(prefix="schedule-process-"))
    root.mkdir(parents=True, exist_ok=True)
    require(not any(root.iterdir()), "Use a fresh isolated evidence directory")
    host = ScheduleHost(args, root)
    started = time.monotonic()
    digest = lambda path: hashlib.sha256(Path(path).read_bytes()).hexdigest()
    result = {"ok": False, "passed": [], "node_sha256": digest(args.binary), "hub_sha256": digest(args.hub_binary),
        "script_sha256": digest(__file__), "limits": ["controlled provider, not real model intelligence",
        "no production access or authorization changes", "no browser UI or power-loss acceptance",
        "restart-intent faults are covered separately by Go tests"]}
    watchdog = threading.Timer(170, lambda: (stop(host.node), stop(host.hub)))
    watchdog.daemon = True
    watchdog.start()
    try:
        host.start()
        key, gid = "goals/schedule", "schedule-goal-91"
        host.begin(gid, key, "SCHEDULE_GOAL_91 verify an exclusive initial artifact; later continue the same Goal and verify a second artifact")
        initial = host.mirror(gid, key, "awaiting_review")["goal"]
        require((host.work / "initial-effect.txt").read_text() == "INITIAL_ONCE_91", "Initial artifact missing")
        require(len(host.fixture.agent_requests) == 2, "Initial slice did not use bounded two-call policy")
        _, paused = host.command(initial, key, "pause", "schedule-pause-91")
        require(not paused.get("err") and paused["goal"]["status"] == "paused", "Actual Rust pause failed")
        mirror = host.mirror(gid, key, "paused")
        due = int(time.time()) + 5
        request = {"id": "resume-once-91", "device": host.device["fp"], "goal_id": gid, "session": key,
            "expected_revision": mirror["goal"]["revision"], "writer_epoch": mirror["writer_epoch"], "due_at": due}
        saved = host.schedule("schedule-save", request)
        require(next(row for row in saved["schedule"]["items"] if row["id"] == request["id"])["status"] == "scheduled", "Save did not return persisted plan")
        host.fixture.phase = "resumed"
        before = len(host.fixture.agent_requests)
        host.controller.sock.close()
        host.controller = None
        time.sleep(.75)
        require(int(time.time()) < due and len(host.fixture.agent_requests) == before and host.goal(gid)["status"] == "paused", "Schedule ran before due")
        goal = until(lambda: g if (g := host.goal(gid)) and g["status"] == "awaiting_review" else None, seconds=25)
        schedule = until(lambda: r if (r := host.schedule_disk(request["id"])) and r["status"] == "triggered" else None)
        require(json.loads((host.work / "scheduled-effect.json").read_text()) == {"sum": 42, "count": 3}, "Scheduled Python effect incorrect")
        require(len(host.fixture.agent_requests) == before + 2, "Scheduled continuation replayed or polled provider")
        require(goal["budget"]["calls_reserved"] == 4 and goal["budget"]["slices_started"] == 2, "Resume reset native Goal accounting")
        require(schedule["receipt"]["acknowledged"] is True and schedule["receipt"]["command_id"] == schedule["command_id"], "Missing native resume acknowledgement")
        result["passed"].append("actual_rust_pause_cloud_save_detach_clock_resume_native_slice_and_exclusive_python_effect")
        host.reconnect()
        current = host.mirror(gid, key, "awaiting_review")["goal"]
        listed = host.schedule("schedule-list")
        require(next(row for row in listed["schedule"]["items"] if row["id"] == request["id"])["status"] == "triggered" and current["status"] == "awaiting_review", "Schedule silently accepted Goal completion")
        result["passed"].append("reconnect_list_triggered_is_resume_ack_goal_remains_awaiting_review")
        _, paused = host.command(current, key, "pause", "cancel-target-pause-91")
        require(not paused.get("err"), "Could not pause same Goal for negative cases")
        mirror = host.mirror(gid, key, "paused")
        canceled = dict(request, id="cancel-once-91", expected_revision=mirror["goal"]["revision"], due_at=int(time.time()) + 4)
        host.schedule("schedule-save", canceled)
        host.schedule("schedule-cancel", {"id": canceled["id"]})
        calls = len(host.fixture.agent_requests)
        until(lambda: time.time() > canceled["due_at"] + 1, seconds=8)
        require(host.schedule_disk(canceled["id"])["status"] == "canceled" and host.goal(gid)["status"] == "paused" and len(host.fixture.agent_requests) == calls, "Canceled plan called provider")
        wrong = dict(canceled, id="wrong-epoch-91", due_at=int(time.time()) + 10, writer_epoch="different-writer-epoch")
        host.schedule("schedule-save", wrong, expect_error=True)
        expired = dict(canceled, id="expiry-boundary-91", due_at=mirror["goal_lease"]["expires_at"])
        host.schedule("schedule-save", expired, expect_error=True)
        require(len(host.fixture.agent_requests) == calls, "Rejected schedule called provider")
        result["passed"].append("cancel_future_schedule_and_reject_changed_writer_or_expiry_without_model")
        require(not host.fixture.route_requests and not host.fixture.errors, "Unexpected route or fixture failure")
        result.update(ok=True, model_calls=len(host.fixture.agent_requests), final_goal_status=host.goal(gid)["status"],
            artifact_sha256=digest(host.work / "scheduled-effect.json"), native_goal_budget=goal["budget"])
    except Exception as error:
        result["error"] = {"type": type(error).__name__, "message": str(error)}
        raise
    finally:
        watchdog.cancel()
        host.close()
        result["elapsed_seconds"] = round(time.monotonic() - started, 3)
        (root / "result.json").write_text(json.dumps(result, indent=2), encoding="utf-8")
    print(json.dumps(result))


if __name__ == "__main__":
    main()
