"""Bounded actual Node/Hub/provider Goal fault acceptance. No production/model.

Every case has its own state and real processes. Failures retain all evidence;
they are never converted to passes. The global watchdog requests worker cleanup
and stops only test-owned process handles after 180 seconds.
"""
from __future__ import annotations
import argparse
import base64
import hashlib
import json
import os
from pathlib import Path
import tempfile
import threading
import time

from verify_context import ContextFixture
from verify_cloud_snapshot import carrier
from verify_host import require, stop
from verify_long_goals import LongHost, until


class FaultFixture(ContextFixture):
    def __init__(self, root):
        self.mode = "unset"
        self.counts = {}
        super().__init__(root)

    def inventory(self, body):
        markers = ("Fresh Host runtime observation (data, not new instructions).",
                   "Fresh Host Job receipt directory for this native device/Session.")
        for row in body["messages"]:
            text = row.get("content", "")
            marker = next((m for m in markers if isinstance(text, str) and m in text), None)
            if marker:
                text = text[text.index(marker):]
                start = text.index("{")
                value, _ = json.JSONDecoder().raw_decode(text[start:])
                require(isinstance(value, dict) and isinstance(value.get("jobs"), list),
                        "Fresh Host inventory jobs structure is invalid")
                return value
        raise AssertionError("Fresh Host Job receipt inventory absent from actual provider request")

    def reply(self, handler, body):
        if not body.get("tools"):
            return super().reply(handler, body)
        self.agent_requests.append(body)
        mode = self.mode
        self.counts[mode] = self.counts.get(mode, 0) + 1
        stage = self.counts[mode]
        if mode in ("budget", "slice"):
            return self.emit(handler, tool="run_script", arguments={"language": "python", "script":
                f"from pathlib import Path\np=Path('{mode}-effect-{stage}.txt')\nwith p.open('x') as f:f.write('ONCE')\nprint('ONCE')"})
        if mode == "anchor":
            return self.emit(handler, text="ANCHOR_AWAITING_EXPLICIT_REVIEW")
        if mode in ("active-close", "pause", "ordinary"):
            if stage == 1:
                delay = 3 if mode == "active-close" else 8
                child = ("from pathlib import Path; import time; time.sleep(4); "
                         f"Path('{mode}-child-unsafe.txt').write_text('unsafe')")
                script = ("from pathlib import Path\nimport subprocess,sys,time\n"
                          + (f"subprocess.Popen([sys.executable,'-c',{child!r}])\n" if mode != "active-close" else "")
                          + f"Path('{mode}-started.txt').write_text('started')\nprint('STARTED',flush=True)\ntime.sleep({delay})\n"
                          + f"with Path('{mode}-finished.txt').open('x') as f:f.write('EXACTLY_ONCE')\nprint('FINISHED',flush=True)\n")
                return self.emit(handler, tool="run_script", arguments={"language": "python", "timeout": 15, "script": script})
            return self.emit(handler, text="ACTIVE_RUN_SETTLED_WITH_REAL_RECEIPTS")
        if mode in ("abort-job", "bye-job"):
            if stage == 1:
                script = ("from pathlib import Path\nimport time\n"
                          f"Path('{mode}-started.txt').write_text('started')\nprint('JOB_STARTED',flush=True)\n"
                          f"time.sleep(6)\nPath('{mode}-unsafe.txt').write_text('unsafe')\n")
                return self.emit(handler, tool="job_start", arguments={"op_id": mode, "language": "python", "timeout": 15, "script": script})
            return self.emit(handler, text="UNEXPECTED_CANCELED_GOAL_RESUMPTION")
        if mode == "cloud-job":
            wire = json.dumps(body, ensure_ascii=False)
            if stage == 1:
                require("REVOKED_CLOUD_BODY_731" in wire, "First Run did not exercise an actual cloud snapshot")
                return self.emit(handler, tool="job_start", arguments={"op_id": "cloud-job", "language": "python", "timeout": 12,
                    "script": "from pathlib import Path\nimport time\nprint('START',flush=True)\ntime.sleep(2)\nPath('cloud-job-answer.txt').write_text('42')\nprint('CLOUD_JOB_RECEIPT_731',flush=True)"})
            require("REVOKED_CLOUD_BODY_731" not in wire, "Auto continuation resurrected revoked cloud text")
            inventory = self.inventory(body)
            require(len(inventory["jobs"]) == 1, "Fresh Host inventory did not identify the actual isolated job")
            job = inventory["jobs"][0]
            require(job["state"] == "succeeded", "Provider polled before Host observed a completed Job")
            if stage == 2:
                require((self.root / "cloud-job-answer.txt").read_text() == "42", "Completed Job artifact incorrect")
                return self.emit(handler, tool="job_read_log", arguments={"job_id": job["job_id"], "stream": "stdout", "cursor": 0, "limit": 4096})
            require("CLOUD_JOB_RECEIPT_731" in wire, "Fresh job_id did not retrieve original completed stdout")
            return self.emit(handler, text="CLOUD_REVOKED_JOB_VERIFIED_FROM_FRESH_RECEIPTS")
        raise AssertionError("Unexpected actual provider fixture mode " + mode)


class FaultHost(LongHost):
    def __init__(self, args, root):
        super().__init__(args, root)
        self.fixture.server.shutdown()
        self.fixture.server.server_close()
        self.fixture = FaultFixture(self.work)
        self.env["PATH"] = str(Path(__import__("sys").executable).parent) + os.pathsep + self.env.get("PATH", "")

    def begin_fault(self, gid, key, mode, *, jobs=False, max_calls=12, max_slices=8, brief=""):
        self.fixture.mode = mode
        self.controller.frame({"k": "agent-loop", "id": gid, "session_key": key, "msg": "FAULT_" + mode,
            "execution_mode": "goal", "goal_criteria": ["Verify actual controlled artifact effects"],
            "model": "fixture-model", "protocol": "openai-completions", "brief": brief,
            "goal_policy": {"expires_at": int(time.time()) + 90, "max_calls": max_calls,
                            "max_slices": max_slices, "allow_jobs": jobs}})

    def await_goal(self, gid, status, seconds=20):
        return until(lambda: (g if (g := self.goal(gid)) and g["status"] == status else None), seconds)

    def mirror(self, goal, key):
        self.controller.send({"k": "goal-inspect", "fp": self.device["fp"], "session_key": key})
        return self.controller.until(lambda m: m.get("k") == "goal-state" and m.get("goal", {}).get("id") == goal["id"]
            and m["goal"].get("revision") == goal["revision"], timeout=15)

    def receipt_paths(self):
        return list(self.state.rglob("request.json"))

    def job(self, op):
        for path in self.receipt_paths():
            row = json.loads(path.read_text())
            if row.get("spec", {}).get("op_id") == op:
                try:
                    receipt = json.loads(path.with_name("receipt.json").read_text())
                except FileNotFoundError:
                    # Intent precedes its first startup receipt; wait for the
                    # actual publication rather than mistaking that ordering
                    # for corrupt output. Malformed existing JSON still fails.
                    return None
                return path.parent, receipt
        return None

    def operation_settled(self, rid):
        for path in self.state.rglob("*.receipts.json"):
            if json.loads(path.read_text()).get(rid, "").startswith("settled"):
                return True
        return False

    def cleanup_workers(self):
        for path in self.receipt_paths():
            path.with_name("cancel.json").write_text("{}")


def root_limits(host):
    for mode, calls, slices in (("budget", 2, 8), ("slice", 12, 1)):
        gid, key = "fault-" + mode, "fault/" + mode
        before = len(host.fixture.agent_requests)
        host.begin_fault(gid, key, mode, max_calls=calls, max_slices=slices)
        goal = host.await_goal(gid, "budget_limited")
        require(goal["budget"]["calls_reserved"] == 2 and goal["budget"]["slices_started"] == 1, "Root/slice budget changed unexpectedly")
        time.sleep(.6)
        require(len(host.fixture.agent_requests) == before + 2, "Budget emitted an N+1 provider HTTP")
        require(len(list(host.work.glob(mode + "-effect-*.txt"))) == 2, "Budget fixture effects differed")
        host.restart()
        time.sleep(1)
        restored = host.goal(gid)
        require(restored["budget"] == goal["budget"] and len(host.fixture.agent_requests) == before + 2, "Node restart reset root budget or resumed exhausted Goal")


def active_close(host):
    gid, key = "fault-active-close", "fault/close"
    host.begin_fault(gid, key, "active-close")
    until(lambda: (host.work / "active-close-started.txt").exists(), 15)
    original_pid = host.node.pid
    host.controller.sock.close(); host.controller = None
    time.sleep(.25)
    host.reconnect()
    host.await_goal(gid, "awaiting_review")
    require(host.node.pid == original_pid and host.node.poll() is None, "Returning page restarted or lost native active process")
    require((host.work / "active-close-finished.txt").read_text() == "EXACTLY_ONCE", "Authorized active effect did not finish exactly once")
    require(host.fixture.counts["active-close"] == 2, "Active detach/re-pair restarted model/tool work")


def active_pause(host):
    gid, key = "fault-pause", "fault/pause"
    host.begin_fault(gid, key, "pause")
    until(lambda: (host.work / "pause-started.txt").exists(), 15)
    goal = host.goal(gid); host.mirror(goal, key)
    frame, response = host.command(goal, key, "pause", "pause-once-731")
    require(not response.get("err") and response["goal"]["status"] == "paused", "Active pause lacked durable paused receipt")
    require(host.goal(gid)["status"] == "paused", "Pause only changed cloud mirror")
    host.controller.send(frame)
    duplicate = host.controller.until(lambda m: m.get("k") == "goal-command-result" and m.get("command_id") == frame["command_id"])
    require(duplicate == response, "Duplicate active pause returned another goal snapshot")
    until(lambda: host.operation_settled(gid), 8)
    until(lambda: any("smart run ended:" in line and f"run={gid}" in line
                     for line in (host.root / "hub.log").read_text(errors="replace").splitlines()), 8)
    time.sleep(4.3)
    require(not (host.work / "pause-child-unsafe.txt").exists() and not (host.work / "pause-finished.txt").exists(), "Paused actual script descendants survived")
    require(host.fixture.counts["pause"] == 1, "Pause called another provider round")


def ordinary_close(host):
    host.begin_fault("fault-anchor", "fault/anchor", "anchor")
    host.await_goal("fault-anchor", "awaiting_review")
    # Native goal settlement precedes transport retirement. Wait for the actual
    # operation receipt so the next request is genuinely an independent Run,
    # rather than a cross-Session follow-up racing the old Active teardown.
    until(lambda: host.operation_settled("fault-anchor"), 8)
    host.fixture.mode = "ordinary"
    rid = "fault-ordinary"
    host.controller.frame({"k": "agent-loop", "id": rid, "session_key": "fault/ordinary", "msg": "FAULT_ordinary", "model": "fixture-model", "protocol": "openai-completions"})
    until(lambda: (host.work / "ordinary-started.txt").exists(), 15)
    host.controller.sock.close(); host.controller = None
    time.sleep(4.4)
    require(not (host.work / "ordinary-child-unsafe.txt").exists() and not (host.work / "ordinary-finished.txt").exists(), "Unrelated existing Goal lease authorized ordinary Run after page close")
    require(host.fixture.counts["ordinary"] == 1, "Ordinary page-close run continued provider calls")
    host.reconnect()
    until(lambda: host.operation_settled(rid), 8)


def cloud_job(host):
    gid, key = "fault-cloud-job", "fault/cloud"
    host.begin_fault(gid, key, "cloud-job", jobs=True,
        brief=carrier(host.device["fp"], "epoch-731", "REVOKED_CLOUD_BODY_731"))
    until(lambda: host.job("cloud-job"), 15)
    host.await_goal(gid, "awaiting_review")
    require(host.fixture.counts["cloud-job"] == 3, "Waiting Job polled model or lost fresh inventory log retrieval")


def cancelled_job(host, bye=False):
    mode = "bye-job" if bye else "abort-job"
    gid, key = "fault-" + mode, "fault/" + mode
    host.begin_fault(gid, key, mode, jobs=True)
    until(lambda: (host.work / f"{mode}-started.txt").exists(), 15)
    goal = host.await_goal(gid, "waiting_job")
    host.controller.frame({"k": "bye"} if bye else {"k": "agent-abort", "id": goal["last_settled_run"]})
    until(lambda: (found if (found := host.job(mode)) and found[1]["state"] == "cancelled" else None), 8)
    # The worker's unsafe write is delayed by six seconds after its started
    # marker. Observe beyond that deadline, even if a broken worker were to
    # publish "cancelled" without stopping its actual script.
    time.sleep(6.3)
    require(not (host.work / f"{mode}-unsafe.txt").exists(), "Explicit cancel/bye left real background Job running")
    require(host.fixture.counts[mode] == 1, "Canceled background Job consumed another model call")
    if not bye:
        mirrored = host.await_goal(gid, "canceled")
        require(mirrored["revision"] > goal["revision"], "Idle cancellation did not publish its new Goal revision to Hub")
        host.mirror(mirrored, key)
        host.restart()
        time.sleep(2.7)
        require(host.fixture.counts[mode] == 1, "Node restart resurrected an explicitly canceled waiting-Job Goal")


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--binary", type=Path, required=True)
    parser.add_argument("--hub-binary", type=Path, required=True)
    parser.add_argument("--output", type=Path, required=True)
    parser.add_argument("--case", action="append", dest="selected_cases", help="Run only this exact case name; repeat to select several")
    args = parser.parse_args()
    root = args.output.resolve(); root.mkdir(parents=True, exist_ok=False)
    hosts, rows = [], []
    started = time.monotonic()
    def emergency():
        for host in hosts:
            try: host.cleanup_workers()
            except Exception: pass
            stop(host.node); stop(host.hub)
    watchdog = threading.Timer(180, emergency); watchdog.daemon = True; watchdog.start()
    cases = [("root_call_and_slice_limits_restart", root_limits), ("active_goal_detach_repair_same_run", active_close),
             ("active_pause_command_idempotence_tree_cleanup_grant_end", active_pause),
             ("ordinary_run_not_authorized_by_other_goal_on_detach", ordinary_close),
             ("cloud_revoke_fresh_job_inventory_log_next_slice", cloud_job),
             ("waiting_job_agent_abort_and_restart_no_resurrection", cancelled_job),
             ("waiting_job_bye_cleans_actual_worker", lambda host: cancelled_job(host, True))]
    omitted = []
    if args.selected_cases:
        require(set(args.selected_cases) <= {name for name, _ in cases}, "Unknown selected case")
        omitted = [name for name, _ in cases if name not in args.selected_cases]
        cases = [(name, test) for name, test in cases if name in args.selected_cases]
    try:
        for name, test in cases:
            host = FaultHost(args, root / name); hosts.append(host)
            row = {"case": name, "status": "FAIL"}; rows.append(row)
            begin = time.monotonic()
            try:
                host.start(); test(host)
                require(not host.fixture.errors, "Controlled provider assertions: " + str(host.fixture.errors))
                row["status"] = "PASS"
            except Exception as error:
                row["error"] = f"{type(error).__name__}: {error}"
            finally:
                host.cleanup_workers()
                time.sleep(.2)
                host.close()
                row["elapsed_seconds"] = round(time.monotonic() - begin, 3)
                (root / "progress.json").write_text(json.dumps(rows, indent=2), encoding="utf-8")
    finally:
        watchdog.cancel()
        result = {"status": "PASS" if len(rows) == len(cases) and all(r["status"] == "PASS" for r in rows) else "FAIL",
                  "scope": "actual isolated Rust Node, Go Hub and controlled provider; no production or paid model", "cases": rows,
                  "elapsed_seconds": round(time.monotonic()-started, 3),
                  "binary_sha256": hashlib.sha256(args.binary.read_bytes()).hexdigest(),
                  "hub_sha256": hashlib.sha256(args.hub_binary.read_bytes()).hexdigest(),
                  "not_run": ["real model intelligence", "production/browser UI", "power loss", "Linux worker process tree (this host is Windows)"] + omitted}
        (root / "result.json").write_text(json.dumps(result, indent=2), encoding="utf-8")
        print(json.dumps(result, indent=2))
    if result["status"] != "PASS":
        raise SystemExit(1)


if __name__ == "__main__":
    main()
