"""Actual Rust Node + Go Hub + Python child, with a bounded fake provider.

Proves same-process input, private controller path, EOF, reconnect and no replay.
No production account, real credentials or external model is used.
"""
import argparse
import base64
import json
import os
from pathlib import Path
import tempfile
import threading
import time

from verify_context import ContextFixture, native_state
from verify_host import require, stop
from verify_long_goals import LongHost, until


class InteractiveFixture(ContextFixture):
    def reply(self, handler, body):
        if not body.get("tools"):
            return super().reply(handler, body)
        self.agent_requests.append(body)
        messages = body["messages"]
        tools = {t["function"]["name"] for t in body["tools"]}
        require("job_write_input" in tools and "job_input_status" in tools, "Input tools missing from actual provider schema")
        if "MODEL_FOLLOWUP_24" in json.dumps(messages):
            jobs = [json.loads(m["content"]) for m in messages if m.get("role") == "tool"
                    and isinstance(m.get("content"), str) and '"worker_pid"' in m["content"]]
            if "goal_update" not in tools:
                require("job_start" not in tools, "Execute follow-up gained background spawn authority")
                wrote = any(t["function"]["name"] == "job_write_input" for m in messages if m.get("role") == "assistant" for t in m.get("tool_calls", []))
                if not wrote:
                    self.emit(handler, tool="job_write_input", arguments={"job_id": jobs[0]["job_id"], "input_id":"follow-up-once", "text":"MODEL_FOLLOWUP_VALUE\n"})
                else:
                    until(lambda: (self.root/"followup.txt").exists(), 5)
                    self.emit(handler, text="FOLLOWUP_INPUT_ARTIFACT_VERIFIED")
            elif not jobs:
                self.emit(handler, tool="job_start", arguments={"op_id":"followup-job", "language":"python", "interactive":True, "timeout":45,
                    "script":"from pathlib import Path\nimport os\nprint('FOLLOWUP_READY:'+str(os.getpid()),flush=True)\na=input()\nPath('followup.txt').write_text(a,encoding='utf-8')\n"})
            else:
                require((self.root/"followup.txt").read_text()=="MODEL_FOLLOWUP_VALUE", "Wrong follow-up effect")
                self.emit(handler, text="FOLLOWUP_GOAL_VERIFIED")
            return
        receipts = [json.loads(m["content"]) for m in messages if m.get("role") == "tool"
                    and isinstance(m.get("content"), str) and '"job_id"' in m["content"] and '"worker_pid"' in m["content"]]
        if not receipts:
            self.emit(handler, tool="job_start", arguments={"op_id": "interactive-once", "language": "python", "interactive": True,
                "timeout": 90, "script": "import os,sys,json\nfrom pathlib import Path\nprint('READY_ONE:'+str(os.getpid()),flush=True)\na=input()\nprint('READY_TWO',flush=True)\nb=input()\nprint('READY_EOF',flush=True)\nrest=sys.stdin.read()\nPath('interactive-result.json').write_text(json.dumps({'pid':os.getpid(),'a':a,'b':b,'tail':rest}),encoding='utf-8')\nprint('INPUT_DONE',flush=True)"})
        else:
            require((self.root / "interactive-result.json").exists(), "Provider polled before child completion")
            require(all("worker_nonce" not in r for r in receipts), "Worker capability exposed to model")
            self.emit(handler, text="INTERACTIVE_ARTIFACT_VERIFIED")


class InteractiveHost(LongHost):
    def __init__(self, args, root):
        super().__init__(args, root)
        self.fixture.server.shutdown()
        self.fixture.server.server_close()
        self.fixture = InteractiveFixture(self.work)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--binary", required=True, type=Path)
    parser.add_argument("--hub-binary", required=True, type=Path)
    parser.add_argument("--output", type=Path)
    args = parser.parse_args()
    root = args.output.resolve() if args.output else Path(tempfile.mkdtemp(prefix="hw-interactive-"))
    root.mkdir(parents=True, exist_ok=True)
    require(not any(root.iterdir()), "Use a fresh evidence directory")
    host = InteractiveHost(args, root)
    result = {"passed": [], "scope": "actual Node/Hub/child and controlled provider; no production/model-intelligence acceptance"}
    watchdog = threading.Timer(100, lambda: (stop(host.node), stop(host.hub)))
    watchdog.daemon = True
    watchdog.start()
    seq = 0
    key, gid = "interactive/session", "interactive-goal-24"

    def command(action, **fields):
        nonlocal seq
        seq += 1
        packet = {"k": "job-command", "command_id": f"console-{seq}", "goal_id": gid, "session_key": key,
                  "fp": host.device["fp"], "job_action": action, **fields}
        host.controller.send(packet)
        return host.controller.until(lambda m: m.get("k") == "job-command-result" and m.get("command_id") == packet["command_id"])

    def ok(action, **fields):
        response = command(action, **fields)
        require(response.get("job_ok") is True and not response.get("err"), f"{action} rejected: {response}")
        return response["job_data"]

    try:
        host.start()
        host.begin(gid, key, "INTERACTIVE_PROCESS_24 maintain one Python process for two user inputs and EOF", jobs=True)
        until(lambda: (g if (g := host.goal(gid)) and g["status"] == "waiting_job" else None))
        host.controller.send({"k": "goal-inspect", "fp": host.device["fp"], "session_key": key})
        host.controller.until(lambda m: m.get("k") == "goal-state" and m.get("goal", {}).get("status") == "waiting_job")
        jobs = ok("list")["jobs"]
        require(len(jobs) == 1 and jobs[0]["interactive"] and not jobs[0]["stdin_closed"], "Interactive receipt not discoverable")
        require("worker_nonce" not in jobs[0], "Worker IPC capability leaked to controller")
        job, worker = jobs[0]["job_id"], jobs[0]["worker_pid"]
        first = until(lambda: (r if (r := ok("log", job_id=job, job_stream="stdout", job_limit=8192)).get("text", "").startswith("READY_ONE:") else None))
        calls = len(host.fixture.agent_requests)
        a, b = "PRIVATE_INPUT_中文_24", "SECOND_INPUT_24"
        sent = ok("input", job_id=job, job_input_id="input-a", job_text=a + "\n")
        require(sent["state"] == "written" and not sent["effects_unknown"], "First input not acknowledged")
        duplicate = ok("input", job_id=job, job_input_id="input-a", job_text=a + "\n")
        require(duplicate == sent, "Repeated input did not return the original ACK")
        conflict = command("input", job_id=job, job_input_id="input-a", job_text="conflict\n")
        require(conflict.get("err") and a not in json.dumps(conflict), "Input conflict was admitted or leaked body")
        require(ok("input_status", job_id=job, job_input_id="input-a") == sent, "Read-only input receipt changed")
        until(lambda: "READY_TWO" in ok("log", job_id=job, job_stream="stdout", job_cursor=first["next_cursor"], job_limit=8192).get("text", ""))
        require(len(host.fixture.agent_requests) == calls, "Waiting/input used provider polling")
        result["passed"].append("same_live_process_incremental_prompt_input_duplicate_conflict_and_private_receipt_no_model_poll")

        host.controller.sock.close()
        host.controller = None
        host.reconnect()
        after = ok("list")["jobs"]
        require(len(after) == 1 and after[0]["job_id"] == job and after[0]["worker_pid"] == worker, "Reconnect replaced process")
        scoped = command("log", job_id=job, job_stream="stdout", session_key="other/session")
        require(scoped.get("err") and not scoped.get("job_data"), "Cross-Session console output leaked")
        ok("input", job_id=job, job_input_id="input-b", job_text=b + "\n")
        until(lambda: "READY_EOF" in ok("log", job_id=job, job_stream="stdout", job_limit=8192).get("text", ""))
        # EOF has no job_text field: exercise the real Go omitempty contract.
        close = ok("input", job_id=job, job_input_id="input-eof", job_eof=True)
        require(close["state"] == "written" and close["eof"], "EOF did not close the pipe")
        until(lambda: (g if (g := host.goal(gid)) and g["status"] == "awaiting_review" else None))
        artifact = json.loads((host.work / "interactive-result.json").read_text(encoding="utf-8"))
        require(artifact["a"] == a and artifact["b"] == b and artifact["tail"] == "", "Input duplicated, reordered or EOF lost")
        status = ok("status", job_id=job)
        require(status["state"] == "succeeded" and status["stdin_closed"], "Terminal process input remained writable")
        require(command("input", job_id=job, job_input_id="late-input", job_text="late\n").get("err"), "Terminal/closed process accepted input")
        result["passed"].append("controller_reconnect_same_worker_cross_session_denial_second_input_eof_and_normal_goal_resume")
        for path in host.state.rglob("*"):
            if path.is_file() and path.suffix in (".json", ".jsonl", ".bin"):
                # Atomic Windows checkpoint publication can briefly deny opens
                # or remove the transient pending filename. Retry only those
                # publication errors; malformed/content assertions still fail.
                deadline = time.monotonic() + 1
                while True:
                    try:
                        data = path.read_bytes()
                        break
                    except FileNotFoundError:
                        data = b""
                        break
                    except PermissionError:
                        if time.monotonic() >= deadline:
                            raise
                        time.sleep(.02)
                require(a.encode() not in data and b.encode() not in data, f"Private direct input persisted in Host state/log: {path.name}")
        require(a not in json.dumps(host.fixture.requests, ensure_ascii=False), "Private input entered model context")
        require(not host.fixture.errors, "Provider fixture failed: " + str(host.fixture.errors))
        result["passed"].append("direct_private_input_absent_from_native_journal_job_metadata_logs_and_provider_requests")
        key, gid = "interactive/followup", "followup-goal-24"
        host.begin(gid,key,"MODEL_FOLLOWUP_24 maintain a process for the next ordinary user message",jobs=True)
        until(lambda: (g if (g:=host.goal(gid)) and g["status"]=="waiting_job" else None))
        host.controller.send({"k":"goal-inspect","fp":host.device["fp"],"session_key":key})
        host.controller.until(lambda m:m.get("k")=="goal-state" and m.get("goal",{}).get("id")==gid and m["goal"].get("status")=="waiting_job")
        follow_job = ok("list")["jobs"][0]
        host.controller.frame({"k":"agent-loop","id":"normal-followup-24","session_key":key,"msg":"MODEL_PIPE_REPLY_24 send my non-sensitive follow-up to the existing process","execution_mode":"execute","smart":True})
        until(lambda: (host.work/"followup.txt").exists())
        until(lambda: (g if (g:=host.goal(gid)) and g["status"]=="awaiting_review" else None))
        require((host.work/"followup.txt").read_text()=="MODEL_FOLLOWUP_VALUE", "Ordinary message did not reach the same process")
        require(ok("status",job_id=follow_job["job_id"])["worker_pid"]==follow_job["worker_pid"],"Model follow-up restarted worker")
        require(not host.fixture.errors,"Follow-up provider errors: "+str(host.fixture.errors))
        result["passed"].append("ordinary_execute_followup_can_write_existing_goal_process_without_spawn_or_lease_renewal")
        result.update({"status": "PASS", "provider_calls": len(host.fixture.agent_requests), "same_child_pid": artifact["pid"]})
    finally:
        watchdog.cancel()
        host.close()
        (root / "result.json").write_text(json.dumps(result, indent=2), encoding="utf-8")
    print(json.dumps(result))


if __name__ == "__main__":
    main()
