"""Actual same-EXE managed worker acceptance; no model or production service.

Writes isolated Host-contract fixtures and invokes the earliest hidden worker
entry point. Validates independent-process lifetime, outputs, process-tree
cancellation, deadline/quota and no replay. This does not substitute for the
controller -> Hub signed Goal lease -> AgentTool integration acceptance.
"""
from __future__ import annotations

import argparse
import hashlib
import json
import os
from pathlib import Path
import secrets
import subprocess
import sys
import tempfile
import time


TERMINAL = {"succeeded", "failed", "cancelled", "timed_out", "lease_expired", "log_limit", "interrupted"}


def require(condition, message):
    if not condition:
        raise AssertionError(message)


def compact(value):
    return json.dumps(value, ensure_ascii=False, separators=(",", ":"))


def sha(value):
    return hashlib.sha256(value).hexdigest()


def atomic_json(path, value):
    temp = path.with_suffix(".test-tmp")
    with temp.open("wb") as output:
        output.write(compact(value).encode())
        output.flush()
        os.fsync(output.fileno())
    os.replace(temp, path)


def read_receipt(directory):
    return json.loads((directory / "receipt.json").read_text(encoding="utf-8"))


def until(predicate, seconds=12):
    deadline = time.monotonic() + seconds
    while time.monotonic() < deadline:
        value = predicate()
        if value:
            return value
        time.sleep(0.04)
    raise AssertionError("Actual process condition did not occur within bounded deadline")


class Jobs:
    def __init__(self, binary, root):
        self.binary = binary.resolve()
        self.root = root
        self.processes = []
        self.receipts = []
        self.env = os.environ.copy()
        self.env["PATH"] = str(Path(sys.executable).parent) + os.pathsep + self.env.get("PATH", "")
        self.env["HW_TOKEN"] = "MUST_NOT_REACH_USER_SCRIPT"
        self.owner = {"state_path_hash": sha(str(root).encode()), "device": "isolated-job-device", "session": "isolated-job-session"}

    def prepare(self, op_id, script, timeout=12, lease_seconds=30, quota=8192):
        job_id = sha((sha(compact(self.owner).encode()) + ":" + op_id).encode())
        directory = self.root / job_id
        directory.mkdir()
        spec = {"op_id": op_id, "language": "python", "script": script, "cwd": str(self.root.resolve()), "timeout_seconds": timeout}
        nonce = secrets.token_hex(16)
        request = {"version": 1, "job_id": job_id, "owner": self.owner, "input_hash": sha(compact(spec).encode()), "worker_nonce": nonce,
                   "created_at": int(time.time()), "lease_expires_at": int(time.time()) + lease_seconds, "max_log_bytes": quota, "spec": spec}
        receipt = {"job_id": job_id, "state": "starting", "worker_nonce": nonce, "worker_pid": None, "started_at": None,
                   "finished_at": None, "exit_code": None, "reason": "isolated Host fixture durable intent", "effects_unknown": True,
                   "stdout_bytes": 0, "stderr_bytes": 0, "stdout_sha256": None, "stderr_sha256": None, "lease_expires_at": request["lease_expires_at"]}
        atomic_json(directory / "request.json", request)
        atomic_json(directory / "receipt.json", receipt)
        return directory

    def spawn(self, directory):
        options = {"creationflags": 0x08000000} if os.name == "nt" else {"start_new_session": True}
        process = subprocess.Popen([str(self.binary), "--managed-job-worker", str(directory)], env=self.env,
                                   stdin=subprocess.DEVNULL, stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL, **options)
        self.processes.append(process)
        return process

    def terminal(self, directory, seconds=15):
        receipt = until(lambda: (lambda row: row if row["state"] in TERMINAL else None)(read_receipt(directory)), seconds)
        require(receipt["finished_at"] is not None, "Terminal receipt was not durably settled")
        self.receipts.append(receipt)
        for stream in ("stdout", "stderr"):
            artifact = directory / f"{stream}.bin"
            if artifact.exists():
                data = artifact.read_bytes()
                require(len(data) == receipt[f"{stream}_bytes"], "Log receipt byte count mismatch")
                require(sha(data) == receipt[f"{stream}_sha256"], "Log receipt digest mismatch")
        return receipt

    def close(self):
        # Request cooperative cleanup before terminating any test-owned worker.
        # Never target a process solely using an old PID read from a receipt.
        for directory in self.root.iterdir():
            if directory.is_dir() and (directory / "request.json").exists():
                atomic_json(directory / "cancel.json", {"requested_at": int(time.time())})
        for process in self.processes:
            try:
                process.wait(timeout=6)
            except subprocess.TimeoutExpired:
                process.terminate()
                process.wait(timeout=5)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--binary", type=Path, required=True)
    parser.add_argument("--output", type=Path)
    args = parser.parse_args()
    root = args.output.resolve() if args.output else Path(tempfile.mkdtemp(prefix="hw-managed-jobs-"))
    root.mkdir(parents=True, exist_ok=True)
    require(not any(root.iterdir()), "Use a fresh empty evidence directory")
    jobs = Jobs(args.binary, root)
    passed, result = [], {"scope": "actual hidden worker process and isolated contract fixtures", "status": "FAIL"}
    started = time.monotonic()
    try:
        script = "from pathlib import Path\nimport os,sys\nassert 'HW_TOKEN' not in os.environ\nPath('complete.txt').write_text('42',encoding='utf-8')\nprint('中文🙂OUT',flush=True)\nsys.stderr.write('错误🙂ERR\\n')\n"
        directory = jobs.prepare("complete", script)
        process = jobs.spawn(directory)
        receipt = jobs.terminal(directory)
        require(receipt["state"] == "succeeded" and receipt["exit_code"] == 0, "Successful process did not produce successful receipt")
        require((root / "complete.txt").read_text() == "42", "Actual job artifact missing")
        require("中文🙂OUT" in (directory / "stdout.bin").read_text(encoding="utf-8"), "Complete Unicode stdout missing")
        require("错误🙂ERR" in (directory / "stderr.bin").read_text(encoding="utf-8"), "Complete stderr missing")
        process.wait(timeout=5)
        passed.append("actual_artifacts_utf8_stderr_and_enrollment_env_removed")

        # A second same-EXE invocation after terminal settlement cannot replay a
        # side effect, even though the original worker lock has been released.
        immutable_receipt = (directory / "receipt.json").read_bytes()
        replay = jobs.spawn(directory)
        require(replay.wait(timeout=5) != 0, "Duplicate worker invocation was admitted")
        require((directory / "receipt.json").read_bytes() == immutable_receipt, "Duplicate invocation altered settled receipt")
        passed.append("terminal_worker_invocation_never_replays")

        script = "from pathlib import Path\nimport time\nPath('detached-started.txt').write_text('started')\ntime.sleep(2)\nPath('detached-done.txt').write_text('done')\n"
        directory = jobs.prepare("parent-exit", script)
        launcher_code = "import subprocess,sys; subprocess.Popen([sys.argv[1], '--managed-job-worker', sys.argv[2]], stdin=subprocess.DEVNULL, stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL, **({'creationflags':0x08000008} if sys.platform=='win32' else {'start_new_session':True}))"
        launcher = subprocess.Popen([sys.executable, "-c", launcher_code, str(args.binary.resolve()), str(directory)], env=jobs.env)
        require(launcher.wait(timeout=5) == 0, "Isolated parent launcher did not exit")
        until(lambda: (root / "detached-started.txt").exists())
        require(jobs.terminal(directory)["state"] == "succeeded" and (root / "detached-done.txt").exists(), "Job did not survive parent process exit")
        passed.append("worker_outlives_parent_launcher_exit")

        child_script = "from pathlib import Path; import time; time.sleep(4); Path('cancel-child-survived.txt').write_text('unsafe')"
        script = f"from pathlib import Path\nimport subprocess,sys,time\nsubprocess.Popen([sys.executable,'-c',{child_script!r}])\nPath('cancel-started.txt').write_text('started')\nprint('before cancel',flush=True)\ntime.sleep(8)\nPath('cancel-root-survived.txt').write_text('unsafe')\n"
        directory = jobs.prepare("cancel-tree", script)
        jobs.spawn(directory)
        until(lambda: (root / "cancel-started.txt").exists())
        atomic_json(directory / "cancel.json", {"requested_at": int(time.time())})
        receipt = jobs.terminal(directory)
        require(receipt["state"] == "cancelled" and receipt["effects_unknown"], "Cancellation hid remaining-effect uncertainty")
        time.sleep(4.3)
        require(not (root / "cancel-child-survived.txt").exists() and not (root / "cancel-root-survived.txt").exists(), "Cancellation failed to clean actual descendant processes")
        require(b"before cancel" in (directory / "stdout.bin").read_bytes(), "Cancellation lost partial output")
        passed.append("cancel_cleans_root_and_descendant_without_pid_recovery")

        directory = jobs.prepare("timeout", "import time\nprint('start',flush=True)\ntime.sleep(8)\n", timeout=1)
        jobs.spawn(directory)
        require(jobs.terminal(directory)["state"] == "timed_out", "Timeout converted to success")
        passed.append("timeout_terminal_and_partial_log")

        directory = jobs.prepare("lease-expiry", "import time\nprint('lease',flush=True)\ntime.sleep(8)\n", timeout=10, lease_seconds=2)
        jobs.spawn(directory)
        require(jobs.terminal(directory)["state"] == "lease_expired", "Job contract expiry was not enforced")
        passed.append("explicit_lease_deadline_not_self_extended")

        directory = jobs.prepare("expired-before-dispatch", "from pathlib import Path\nPath('expired-unsafe.txt').write_text('unsafe')\n", lease_seconds=-1)
        jobs.spawn(directory)
        receipt = jobs.terminal(directory)
        require(receipt["state"] == "lease_expired" and not receipt["effects_unknown"] and not (root / "expired-unsafe.txt").exists(), "Expired contract dispatched a process")
        passed.append("expired_contract_no_process_dispatch")

        directory = jobs.prepare("quota", "import sys,time\nsys.stdout.buffer.write(b'X'*65536)\nsys.stdout.flush()\ntime.sleep(5)\n", quota=4096)
        jobs.spawn(directory)
        receipt = jobs.terminal(directory)
        require(receipt["state"] == "log_limit", "Output quota did not stop process explicitly")
        require(receipt["stdout_bytes"] + receipt["stderr_bytes"] <= 4096, "Combined disk log quota exceeded")
        passed.append("combined_output_disk_quota_stops_job")

        directory = jobs.prepare("exit-seven", "import sys\nprint('partial work')\nsys.exit(7)\n")
        jobs.spawn(directory)
        receipt = jobs.terminal(directory)
        require(receipt["state"] == "failed" and receipt["exit_code"] == 7, "Nonzero exit was lost")
        passed.append("exit_code_seven_is_failed_not_goal_complete")
        result["status"] = "PASS"
    except Exception as error:
        result["error"] = f"{type(error).__name__}: {error}"
        raise
    finally:
        jobs.close()
        result.update(passed=passed, elapsed_seconds=round(time.monotonic()-started, 3), binary_sha256=sha(args.binary.read_bytes()), platform=sys.platform,
                      not_run=["controller-Hub signed lease and model tool integration", "actual Node process restart (parent-launcher exit covered)", "power-loss durability", "malicious setsid/process escape", "Unix worker SIGKILL descendant cleanup; interrupted effects remain unknown"])
        (root / "worker-receipts.json").write_text(json.dumps(jobs.receipts, ensure_ascii=False, indent=2), encoding="utf-8")
        (root / "result.json").write_text(json.dumps(result, ensure_ascii=False, indent=2), encoding="utf-8")
        print(json.dumps(result, ensure_ascii=False, indent=2))


if __name__ == "__main__":
    main()
