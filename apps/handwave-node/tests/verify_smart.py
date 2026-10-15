"""Actual modern Go Hub smart grants and Rust node with a controlled Gateway.

Independent temporary identities/configuration; no production changes.
"""
import argparse
import base64
import hashlib
import json
import os
from pathlib import Path
import secrets
import subprocess
import tempfile
import threading
import time

from verify_host import Fixture, WS, fixture_path, port, require, run_frames, status_rows, stop, wait_health


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--binary", type=Path, required=True)
    parser.add_argument("--hub-binary", type=Path, required=True)
    parser.add_argument("--output", type=Path)
    args = parser.parse_args()
    root = args.output.resolve() if args.output else Path(tempfile.mkdtemp(prefix="handwave-smart-"))
    root.mkdir(parents=True, exist_ok=True)
    require(not (root / "result.json").exists(), "Use a fresh evidence directory")
    conf, state, work = (root / name for name in ("hub-conf", "node-state", "work"))
    for directory in (conf, state, work):
        directory.mkdir()
    fixture = Fixture(work, modern=True)
    root_token, node_token = secrets.token_hex(24), secrets.token_hex(24)
    hub_port = port()
    hub_url = f"http://127.0.0.1:{hub_port}"
    config = {"gateway_root": f"http://127.0.0.1:{fixture.server.server_port}", "gateway_token_env": "HW_TEST_GATEWAY_TOKEN", "default_model": "fixture-model", "default_protocol": "openai-completions", "allow_loopback": True, "smart_pool": "fixture-pool"}
    (conf / "gateway.json").write_text(json.dumps(config), encoding="utf-8")
    env = os.environ.copy()
    for key in list(env):
        if key.lower() in ("http_proxy", "https_proxy", "all_proxy", "no_proxy") or key in ("HW_PROXY", "HW_GATEWAY_TOKEN"):
            del env[key]
    env.update(HW_CONF=str(conf), HW_TEST_GATEWAY_TOKEN="fixture-key", HW_TOKEN=node_token, NO_PROXY="127.0.0.1,localhost")
    node_env = env.copy()
    node_env.pop("HW_TEST_GATEWAY_TOKEN")
    hub_log, node_log = (root / "hub.log").open("wb"), (root / "node.log").open("wb")
    hub = node = controller = watchdog = None
    passed, transcript = [], []
    result = {"scope": "actual modern Go Hub smart grants + Rust node; controlled model/Gateway", "passed": passed}
    started = time.monotonic()
    node_args = [str(args.binary.resolve()), "--hub", hub_url, "--state-dir", str(state), "--cwd", str(work), "--max-calls", "6", "--max-seconds", "60"]
    try:
        hub = subprocess.Popen([str(args.hub_binary.resolve()), "hub", "-listen", f"127.0.0.1:{hub_port}", "-token", root_token, "-node_token", node_token], env=env, cwd=conf, stdout=hub_log, stderr=hub_log)
        wait_health(hub_url)
        node = subprocess.Popen(node_args, env=node_env, cwd=work, stdout=node_log, stderr=node_log)
        watchdog = threading.Timer(120, lambda: (stop(node), stop(hub)))
        watchdog.daemon = True
        watchdog.start()
        controller = WS(hub_port, "ctl", root_token)
        controller.send({"k": "hello", "fp": "smart-controller", "relay": True})
        inventory = controller.until(lambda m: m.get("k") == "status" and any(row.get("state") == "pending" for row in status_rows(m)))
        device = next(row for row in status_rows(inventory) if row.get("state") == "pending")
        controller.send({"k": "approve", "nid": device["nid"], "relay": True})
        controller.until(lambda m: m.get("k") == "paired")
        controller.until(lambda m: m.get("k") == "dc" and json.loads(base64.b64decode(m["d"])).get("k") == "node-hello")
        out, _, _ = run_frames(controller, "smart-init", "INIT_ARTIFACT", transcript, smart=True, model="", timeout=30)
        require(not out.get("err") and out.get("code", 0) == 0, "Smart initial actual tool task failed")
        require(fixture_path(work).read_text().strip() == "HANDWAVE_HOST_ARTIFACT_73", "Smart task did not write/read real file")
        require(len(fixture.route_requests) == 1, "First smart task did not resolve exactly once")
        passed.append("smart_bound_headers_pass_actual_hub_gate_and_real_tools")
        # Native history and wire replay receipts live beside each other.
        # Select only the session checkpoint, not transport or context sidecars.
        checkpoints = [p for p in (state / "sessions" / "smart-controller").glob("*.json")
                       if not p.name.endswith((".receipts.json", ".context.json"))]
        require(len(checkpoints) == 1, "Expected exactly one native smart session checkpoint")
        checkpoint = checkpoints[0]
        saved = json.loads(checkpoint.read_text(encoding="utf-8"))
        require(saved["route"]["binding"] and saved["route"]["decision"] == "fixture-route-decision", "Smart identity was not persisted")
        before = checkpoint.read_bytes()
        count = len(fixture.requests)
        for run_id, smart, model in (("explicit-takeover", False, "fixture-model"), ("smart-conflict", True, "fixture-model")):
            out, _, _ = run_frames(controller, run_id, "CHECK_CONTINUITY", transcript, smart=smart, model=model)
            require(out.get("err") or out.get("code", 0) != 0, "Invalid smart mode combination was accepted")
            require(len(fixture.requests) == count and checkpoint.read_bytes() == before, "Rejected selection changed history or called model")
        passed.append("mode_and_explicit_conflicts_preserve_original_history")
        out, _, _ = run_frames(controller, "smart-follow", "CHECK_CONTINUITY", transcript, smart=True, model="")
        require(not out.get("err") and b"CONTINUITY_OK" in base64.b64decode(out.get("b64", "")), "Smart continuation lost native history")
        require(len(fixture.route_requests) == 1, "Same smart session re-resolved selection")
        passed.append("same_binding_continuation_without_reresolve")
        stop(node)
        node = None
        controller.until(lambda m: m.get("k") == "unpaired")
        node = subprocess.Popen(node_args, env=node_env, cwd=work, stdout=node_log, stderr=node_log)
        controller.until(lambda m: m.get("k") == "paired")
        controller.until(lambda m: m.get("k") == "dc" and json.loads(base64.b64decode(m["d"])).get("k") == "node-hello")
        saved_before_restart = checkpoint.read_bytes()
        requests_before_restart = len(fixture.requests)
        out, _, _ = run_frames(controller, "smart-restart", "CHECK_CONTINUITY", transcript, smart=True, model="")
        # Current Hub dropLocked/model_binding.dropFP intentionally revoke pins on
        # device disconnect. A node must preserve history and fail visibly; it
        # cannot turn that lost authority into a fresh conversation or resolve.
        require(out.get("err") or out.get("code", 0) != 0, "Disconnected smart pin silently gained new authority")
        require(checkpoint.read_bytes() == saved_before_restart, "Rejected smart restart erased/replaced native history")
        require(len(fixture.requests) == requests_before_restart and len(fixture.route_requests) == 1, "Rejected smart restart re-resolved or replayed model effects")
        require(not fixture.errors, "Controlled Gateway assertions failed")
        passed.append("hub_revoked_pin_after_disconnect_is_rejected_with_history_retained")
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
        result.update(elapsed_seconds=round(time.monotonic() - started, 3), binary_sha256=hashlib.sha256(args.binary.read_bytes()).hexdigest(), hub_sha256=hashlib.sha256(args.hub_binary.read_bytes()).hexdigest(), model_requests=len(fixture.requests), route_resolves=len(fixture.route_requests), fixture_errors=fixture.errors)
        (root / "result.json").write_text(json.dumps(result, indent=2), encoding="utf-8")
        (root / "controller.json").write_text(json.dumps(transcript, indent=2), encoding="utf-8")
        print(json.dumps(result, indent=2))


if __name__ == "__main__":
    main()
