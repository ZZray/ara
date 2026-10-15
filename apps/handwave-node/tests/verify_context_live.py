"""Bounded real-model context trial through public HandWave, private controller.

At most four model runs, three provider calls per run, including compaction.
Only a fresh temporary device is approved/revoked. No service/config mutation.
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
import threading
import time

from verify_cloud import read_credentials, models, wait_tunnel_health
from verify_host import WS, port, require, run_frames, status_rows, stop


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--binary", type=Path, required=True)
    parser.add_argument("--output", type=Path, required=True)
    parser.add_argument("--ssh-alias", default="ray-dev")
    parser.add_argument("--remote-config", default="/opt/handwave/hub.conf")
    parser.add_argument("--model", required=True)
    parser.add_argument("--public-hub", default="https://118.25.103.226/handwave")
    args = parser.parse_args()
    root = args.output.resolve()
    root.mkdir(parents=True, exist_ok=False)
    work, state = root / "work", root / "state"
    work.mkdir(); state.mkdir()
    root_token, node_token = read_credentials(args.ssh_alias, args.remote_config)
    endpoint_port = port()
    logs = [(root / "tunnel.log").open("wb"), (root / "node.log").open("wb")]
    tunnel = node = controller = watchdog = None
    device = None
    transcript, passed = [], []
    started = time.monotonic()
    result = {"status": "FAIL", "scope": "actual public HandWave + actual cloud model + temporary Rust node",
              "max_model_runs": 4, "max_provider_calls_per_run": 3, "max_provider_calls_total": 12,
              "deadline_seconds": 180, "model": args.model, "protocol": "openai-completions", "passed": passed}
    try:
        tunnel = subprocess.Popen(["ssh", "-N", "-o", "BatchMode=yes", "-o", "StrictHostKeyChecking=yes",
                    "-o", "ExitOnForwardFailure=yes", "-L", f"127.0.0.1:{endpoint_port}:127.0.0.1:18080", args.ssh_alias],
                    stdout=subprocess.DEVNULL, stderr=logs[0])
        base = f"http://127.0.0.1:{endpoint_port}"
        wait_tunnel_health(base)
        available, receipt = models(base, root_token)
        require(receipt["http_status"] == 200 and any(r["id"] == args.model for r in available), "Model absent from live inventory")
        result["inventory"] = receipt
        env = os.environ.copy()
        for key in list(env):
            if key.lower() in ("http_proxy", "https_proxy", "all_proxy", "no_proxy", "hw_proxy", "hw_node_state", "hw_hub", "hw_token"):
                del env[key]
        env.update(HW_TOKEN=node_token, NO_PROXY="127.0.0.1,localhost")
        node_args = [str(args.binary.resolve()), "--hub", args.public_hub, "--state-dir", str(state),
                     "--cwd", str(work), "--max-calls", "3", "--max-seconds", "150"]
        identity = subprocess.run(node_args + ["--identity"], env=env, capture_output=True, timeout=15)
        require(identity.returncode == 0, "Identity query failed")
        fp = identity.stdout.decode().split("device ", 1)[-1].strip().splitlines()[0]
        node = subprocess.Popen(node_args, env=env, cwd=work, stdout=logs[1], stderr=logs[1])
        watchdog = threading.Timer(max(1, 180 - (time.monotonic() - started)), lambda: stop(node)); watchdog.daemon = True; watchdog.start()
        controller = WS(endpoint_port, "ctl", root_token)
        controller.send({"k": "hello", "fp": "context-live-" + secrets.token_hex(8), "relay": True})
        status = controller.until(lambda m: m.get("k") == "status" and any(r.get("fp") == fp and r.get("nid", "").startswith("n") for r in status_rows(m)))
        device = next(r for r in status_rows(status) if r["fp"] == fp)
        require(device.get("state") == "pending", "Temporary node unexpectedly already approved")
        controller.send({"k": "approve", "nid": device["nid"], "relay": True, "ttl": 210})
        controller.until(lambda m: m.get("k") == "paired")
        controller.until(lambda m: m.get("k") == "dc" and json.loads(base64.b64decode(m["d"])).get("k") == "node-hello")
        marker = "REFERENCE_" + secrets.token_hex(6)
        rule = "RULE_" + secrets.token_hex(6)
        (work / "AGENTS.md").write_text("For this trial, final answers must contain " + rule + ".\n", encoding="utf-8")
        # Two bounded tool outputs exercise actual context compaction. No injected
        # session JSON or synthetic summary is used in this live-model trial.
        for name in ("first.txt", "second.txt"):
            (work / name).write_text(("ordinary log context padding 0123456789 abcdef\n" * 2500) + marker + "\n", encoding="utf-8")

        def run(name, prompt):
            remaining = 180 - (time.monotonic() - started)
            require(remaining > 1, "Live trial deadline exhausted")
            out, events, live = run_frames(controller, name, prompt, transcript, timeout=remaining,
                                           model=args.model, protocol="openai-completions")
            require(not out.get("err") and out.get("code", 0) == 0, "Live context task failed: " + name)
            return events, live

        first = f"My project marker is {marker}. This is user reference data for this project. Read first.txt completely with read_file (path only). Then report the last line and the project rule marker. Use no other tools."
        events, _ = run("context-live-first", first)
        require(any(e.get("type") == "tool_end" and marker in e.get("output", "") for e in events), "First real read receipt missing")
        require(any(e.get("type") == "text" and rule in e.get("output", "") for e in events), "Actual model did not follow project guidance")
        passed.append("real_model_project_guidance_and_reference_read")
        run("context-live-second", "Read second.txt completely with read_file (path only), then report its last line. Use no other tools.")

        def store():
            paths = list((state / "sessions").rglob("*.context.json"))
            require(len(paths) == 1, "Exactly one context sidecar required")
            return json.loads(paths[0].read_text(encoding="utf-8"))

        events, _ = run("context-live-propose", "Using the independent user reference directory, find my project marker from the earlier user request. Call memory_propose exactly once to propose that marker as a fact, citing its real source ID. Do not adopt it; then give a short final answer.")
        proposal_receipts = [e for e in events if e.get("type") == "tool_end" and e.get("title") == "memory_propose"]
        require(len(proposal_receipts) == 1 and proposal_receipts[0].get("status") == "completed", "Exactly one successful memory_propose receipt required")
        memory_state = store()
        require(memory_state.get("summary"), "Actual provider did not produce a persisted summary")
        refs = [r for r in memory_state["references"] if r["text"] == first]
        require(len(refs) == 1 and refs[0]["hash"] == hashlib.sha256(first.encode()).hexdigest(), "Original reference changed during compaction")
        candidates = [m for m in memory_state["memories"] if m["status"] == "candidate" and marker in m["text"]
                      and m["run"] == "context-live-propose" and {"id": refs[0]["id"], "hash": refs[0]["hash"]} in m["sources"]]
        require(candidates, "Actual model did not propose marker memory citing the original user source")
        passed.extend(["actual_model_summary_and_source_retention", "actual_model_sourced_candidate_not_autoaccepted"])
        run("context-live-accept", "/memory accept " + candidates[0]["id"])
        require(next(m for m in store()["memories"] if m["id"] == candidates[0]["id"])["status"] == "accepted", "Trusted adoption failed")
        events, _ = run("context-live-recall", "Recall the adopted project marker. Use write_file exactly once to write only that marker into recalled.txt. Do not use any other tools, including reading back recalled.txt for verification. Do not reread first.txt or second.txt. Then report completion.")
        starts = [e for e in events if e.get("type") == "tool_start"]
        ends = [e for e in events if e.get("type") == "tool_end"]
        require(len(starts) == 1 and starts[0].get("title") == "write_file"
                and len(ends) == 1 and ends[0].get("title") == "write_file" and ends[0].get("status") == "completed",
                "Recall must use only one successful write_file, without rereading original files")
        require((work / "recalled.txt").read_text().strip() == marker, "Post-compaction adopted-memory artifact is wrong")
        passed.append("actual_post_compaction_recall_to_file")
        result["status"] = "PASS"
        result["usage_boundary"] = "Inspect call receipts; missing supplier usage is unknown, not zero. Actual HTTP requests are not captured at the public ingress."
    except Exception as error:
        result["failure"] = type(error).__name__ + ": " + str(error)[:300]
    finally:
        if watchdog: watchdog.cancel()
        stop(node)
        if controller and device:
            try:
                controller.send({"k": "revoke", "fp": device["fp"]})
                controller.until(lambda m: m.get("k") == "status" and not any(r.get("fp") == device["fp"] and r.get("state") in ("approved", "paired", "approved-offline") for r in status_rows(m)), timeout=5)
                result["temporary_approval_revoked"] = True
            except Exception:
                result["temporary_approval_revoked"] = False
        if controller: controller.close()
        stop(tunnel)
        for log in logs: log.close()
        if device and not result.get("temporary_approval_revoked"):
            result["status"] = "FAIL"
            result["cleanup_failure"] = "Temporary approval revocation unconfirmed; bounded approval TTL remains."
        result.update(elapsed_seconds=round(time.monotonic()-started, 3), binary_sha256=hashlib.sha256(args.binary.read_bytes()).hexdigest())
        (root / "result.json").write_text(json.dumps(result, indent=2), encoding="utf-8")
        (root / "controller.json").write_text(json.dumps(transcript, indent=2), encoding="utf-8")
        print(json.dumps(result, indent=2))
    require(result["status"] == "PASS", "Live context trial failed; retained evidence")


if __name__ == "__main__":
    main()
