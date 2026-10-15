"""Bounded actual-model acceptance over an isolated SSH tunnel to existing HandWave.

Only a fresh temporary node identity is approved/revoked. The script never prints
or saves root/enrollment/model credentials and does not modify remote files,
services, authentication settings, or other devices. Run only after authorizing
the cloud trial. --list-models performs a read-only model inventory.
"""
from __future__ import annotations

import argparse
import base64
import hashlib
import json
import os
from pathlib import Path
import secrets
import shlex
import subprocess
import tempfile
import threading
import time
import urllib.error
import urllib.request
import urllib.parse

from verify_host import WS, port, require, run_frames, status_rows, stop


def read_credentials(alias, remote_config):
    receipt = subprocess.run(
        ["ssh", "-o", "BatchMode=yes", "-o", "StrictHostKeyChecking=yes", alias,
         "cat " + shlex.quote(remote_config)], capture_output=True, timeout=15, check=False,
    )
    require(receipt.returncode == 0, "Read-only SSH configuration read failed (credential output suppressed)")
    conf = {}
    for line in receipt.stdout.decode("utf-8").splitlines():
        if not line.strip() or line.lstrip().startswith("#") or "=" not in line:
            continue
        key, value = line.split("=", 1)
        conf[key.strip()] = value.strip()
    require(conf.get("token") and conf.get("node_token"), "Root and enrollment token must exist in the verified remote configuration")
    return conf["token"], conf["node_token"]


def read_gateway_credential(alias, remote_config):
    # SSH stdout is captured privately in this process, never returned or saved.
    source = ("import pathlib,yaml,sys\n"
              f"cfg=yaml.safe_load(pathlib.Path({remote_config!r}).read_text())\n"
              "keys=[t['key'] for t in (cfg.get('auth') or {}).get('tokens',[]) if not t.get('disabled') and t.get('key')]\n"
              "if len(keys)!=1: sys.exit(2)\n"
              "sys.stdout.write(keys[0])\n")
    receipt = subprocess.run(["ssh", "-o", "BatchMode=yes", "-o", "StrictHostKeyChecking=yes", alias, "python3 -"], input=source.encode(), capture_output=True, timeout=15, check=False)
    require(receipt.returncode == 0, "Exactly one authorized enabled Gateway credential must exist (output suppressed)")
    value = receipt.stdout.decode().strip()
    require(bool(value) and "\n" not in value and "\r" not in value, "Invalid Gateway credential (value suppressed)")
    return value


def safe_get(base, path, token):
    request = urllib.request.Request(base + path, headers={"Authorization": "Bearer " + token})
    # The tunnel is local; ignore parent machine proxy configuration.
    opener = urllib.request.build_opener(urllib.request.ProxyHandler({}))
    try:
        with opener.open(request, timeout=15) as response:
            body = json.loads(response.read(1024 * 1024))
            return body, {"path": path, "http_status": response.status, "body_type": type(body).__name__, "top_keys": sorted(body.keys()) if isinstance(body, dict) else [], "row_count": len(body) if isinstance(body, list) else None}
    except urllib.error.HTTPError as error:
        # Classify the known proxy fault without exposing any upstream body.
        body = error.read(65536).lower()
        return None, {"path": path, "http_status": error.code, "upstream_not_configured": b"upstream not configured" in body or b"no upstream" in body}


def models(base, token):
    # Workbench source api.go:312/549 lists the configured aggregate directly.
    # The deployed /v1/models route already returned 503 twice; do not replay it.
    body, receipt = safe_get(base, "/api/models", token)
    if receipt["http_status"] != 200:
        return [], receipt
    rows = body.get("data", []) if isinstance(body, dict) else body
    require(isinstance(rows, list), "Unexpected model inventory shape")
    safe = []
    for row in rows:
        if isinstance(row, dict) and isinstance(row.get("id"), str):
            safe.append({"id": row["id"], "provider": row.get("owned_by", row.get("provider", "")), "protocols": row.get("protocols", [])})
    return safe, receipt


def wait_tunnel_health(base):
    opener = urllib.request.build_opener(urllib.request.ProxyHandler({}))
    end = time.monotonic() + 15
    while time.monotonic() < end:
        try:
            with opener.open(base + "/healthz", timeout=1) as response:
                if response.status == 200:
                    return
        except OSError:
            time.sleep(0.1)
    raise TimeoutError("Read-only tunnel health check failed")


def public_node_preflight(base, node_token):
    parsed = urllib.parse.urlsplit(base)
    require(parsed.scheme in ("http", "https") and parsed.hostname and not parsed.username and not parsed.query and not parsed.fragment, "Public Hub must be an HTTP/HTTPS root without credentials/query")
    wait_tunnel_health(base)
    ws = WS(parsed.port or (443 if parsed.scheme == "https" else 80), "node", node_token, host=parsed.hostname, path_prefix=parsed.path, secure=parsed.scheme == "https")
    try:
        ws.send({"k": "join", "fp": "ingress-probe-" + secrets.token_hex(8), "info": {"hostname": "handwave-ingress-test", "user": "temporary-probe", "os": "test", "arch": "test", "version": "read-only-probe"}})
        ws.send({"k": "model-grant", "id": "unapproved-ingress-probe"})
        message = ws.until(lambda m: m.get("k") == "model-grant")
        require(message.get("err") and not message.get("d"), "Public unapproved probe unexpectedly received a model grant")
        return {"health_status": 200, "websocket_handshake": 101, "unapproved_grant_denied": True}
    finally:
        ws.close()


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--binary", type=Path)
    parser.add_argument("--ssh-alias", default="ray-dev")
    parser.add_argument("--remote-config", default="/opt/handwave/hub.conf")
    parser.add_argument("--remote-port", type=int, help="Defaults to public Hub 18080, or existing Gateway 18799 in isolated-hub mode")
    parser.add_argument("--isolated-hub", action="store_true", help="Keep production HandWave unchanged: start a temporary modern local Hub behind SSH-L to the existing Gateway")
    parser.add_argument("--hub-binary", type=Path)
    parser.add_argument("--gateway-config", default="/opt/omp-gateway/gateway.yaml")
    parser.add_argument("--protocol", default="openai-completions")
    parser.add_argument("--public-hub", help="Route the actual node WS/model HTTP through this public Hub root; keep root-authenticated control private through SSH")
    parser.add_argument("--model")
    parser.add_argument("--list-models", action="store_true")
    parser.add_argument("--output", type=Path)
    parser.add_argument("--max-calls", type=int, default=12, help="Total upper bound across both model runs")
    parser.add_argument("--max-seconds", type=int, default=120, help="Total trial deadline, including model runs")
    args = parser.parse_args()
    require(args.list_models or (args.binary and args.binary.is_file() and args.model), "Provide --binary and an exact verified --model, or use --list-models")
    require(2 <= args.max_calls <= 12 and 1 <= args.max_seconds <= 120, "Trial budgets cannot exceed 12 calls or 120 seconds")
    require(not args.isolated_hub or (args.hub_binary and args.hub_binary.is_file() and args.model), "Isolated mode needs the modern --hub-binary and exact --model")
    require(not (args.public_hub and args.isolated_hub), "Public ingress and isolated-local-Hub trials have different acceptance scopes")
    if args.isolated_hub:
        gateway_token = read_gateway_credential(args.ssh_alias, args.gateway_config)
        root_token, node_token = secrets.token_hex(24), secrets.token_hex(24)
    else:
        root_token, node_token = read_credentials(args.ssh_alias, args.remote_config)
    root = args.output.resolve() if args.output else Path(tempfile.mkdtemp(prefix="handwave-cloud-"))
    root.mkdir(parents=True, exist_ok=True)
    require(not (root / "result.json").exists(), "Use a fresh evidence directory")
    tunnel_log = (root / "ssh-tunnel.log").open("wb")
    node_log = hub_log = None
    tunnel = hub = node = controller = watchdog = None
    device_fp = ""
    cleanup_revoke = False
    result = {"scope": "temporary Rust node + temporary modern local Hub + SSH-L to existing cloud Gateway + actual model; production public HandWave NOT ACCEPTED" if args.isolated_hub else "temporary Rust node + existing cloud HandWave + actual configured model; no remote configuration/service mutations", "status": "FAIL"}
    transcript = []
    started = time.monotonic()
    try:
        local_port = port()
        base = f"http://127.0.0.1:{local_port}"
        remote_port = args.remote_port or (18799 if args.isolated_hub else 18080)
        tunnel = subprocess.Popen(["ssh", "-N", "-o", "BatchMode=yes", "-o", "StrictHostKeyChecking=yes", "-o", "ExitOnForwardFailure=yes", "-L", f"127.0.0.1:{local_port}:127.0.0.1:{remote_port}", args.ssh_alias], stdout=subprocess.DEVNULL, stderr=tunnel_log)
        wait_tunnel_health(base)
        if args.isolated_hub:
            conf = root / "hub-conf"
            conf.mkdir()
            hub_env = os.environ.copy()
            for key in list(hub_env):
                if key.lower() in ("http_proxy", "https_proxy", "all_proxy", "no_proxy"):
                    del hub_env[key]
            hub_env.update({"HW_CONF": str(conf), "HW_TEST_GATEWAY_TOKEN": gateway_token, "NO_PROXY": "127.0.0.1,localhost"})
            hub_port = port()
            hub_log = (root / "local-hub.log").open("wb")
            hub = subprocess.Popen([str(args.hub_binary.resolve()), "hub", "-listen", f"127.0.0.1:{hub_port}", "-token", root_token, "-node_token", node_token, "-gateway", base, "-gateway_token_env", "HW_TEST_GATEWAY_TOKEN", "-model", args.model, "-protocol", args.protocol, "-allow_loopback"], env=hub_env, cwd=conf, stdout=hub_log, stderr=hub_log)
            base = f"http://127.0.0.1:{hub_port}"
            local_port = hub_port
            wait_tunnel_health(base)
            result["hub_binary_sha256"] = hashlib.sha256(args.hub_binary.read_bytes()).hexdigest()
        available, inventory_receipt = models(base, root_token)
        result["model_inventory_receipt"] = inventory_receipt
        require(inventory_receipt["http_status"] == 200, "Read-only workbench model inventory failed")
        if args.list_models:
            result.update({"status": "READ_ONLY", "models": available})
            print(json.dumps({"models": available, "inventory_receipt": inventory_receipt}, ensure_ascii=False, indent=2))
            return
        require(any(row["id"] == args.model for row in available), "Requested model ID is absent from the current cloud inventory")
        node_base = base
        if args.public_hub:
            result["public_ingress_preflight"] = public_node_preflight(args.public_hub, node_token)
            node_base = args.public_hub.rstrip("/")
            result["scope"] = "temporary Rust node WS/model HTTP through actual public HandWave ingress + private SSH controller + configured cloud Gateway + actual model"
            result["public_node_hub"] = node_base
        work, state = root / "work", root / "state"
        work.mkdir()
        state.mkdir()
        env = os.environ.copy()
        for key in list(env):
            if key.lower() in ("http_proxy", "https_proxy", "all_proxy", "no_proxy", "hw_proxy"):
                del env[key]
        env.update({"HW_HUB": node_base, "HW_TOKEN": node_token, "NO_PROXY": "127.0.0.1,localhost"})
        per_run_calls = args.max_calls // 2
        node_log = (root / "node.log").open("wb")
        expected_fp = subprocess.run([str(args.binary.resolve()), "--state-dir", str(state), "--cwd", str(work), "--identity"], env=env, capture_output=True, timeout=15, check=False)
        require(expected_fp.returncode == 0, "Failed to query this temporary node's public identity")
        # CLI prints '... | device FP'. It contains no root/enrollment credential.
        device_fp = expected_fp.stdout.decode().split("device ", 1)[-1].strip().splitlines()[0]
        require(device_fp and " " not in device_fp, "Unexpected temporary identity output")
        node = subprocess.Popen([str(args.binary.resolve()), "--hub", node_base, "--cwd", str(work), "--state-dir", str(state), "--max-calls", str(per_run_calls), "--max-seconds", str(args.max_seconds)], env=env, cwd=work, stdout=node_log, stderr=node_log)
        controller = WS(local_port, "ctl", root_token)
        owner = "cloud-trial-" + secrets.token_hex(8)
        controller.send({"k": "hello", "fp": owner, "relay": True})
        message = controller.until(lambda m: m.get("k") == "status" and any(row.get("fp") == device_fp and row.get("nid", "").startswith("n") for row in status_rows(m)))
        device = next(row for row in json.loads(message["d"]) if row.get("fp") == device_fp)
        require(device.get("state") == "pending", "Fresh temporary identity unexpectedly already approved")
        controller.send({"k": "approve", "nid": device["nid"], "relay": True, "ttl": args.max_seconds + 30})
        cleanup_revoke = True
        controller.until(lambda m: m.get("k") == "paired")
        controller.until(lambda m: m.get("k") == "dc" and json.loads(base64.b64decode(m["d"])).get("k") == "node-hello")
        marker = "CLOUD_RUST_ARTIFACT_" + secrets.token_hex(8)
        receipt = "COMPILED_CLOUD_RECEIPT_" + secrets.token_hex(8)
        source = f'fn main() {{ std::fs::write("cloud-hello.log", "{marker}\\n").unwrap(); println!("{receipt}"); }}'
        exe_name = "cloud-hello.exe" if os.name == "nt" else "cloud-hello"
        prompt = (f"Perform this bounded real task in working directory {work}. Use write_file to create cloud-hello.rs with this exact Rust source:\n{source}\n"
                  f"Use exec to compile it with rustc --crate-name cloud_hello cloud-hello.rs -o {exe_name}, then actually execute it. Use the current OS shell (PowerShell on Windows), check compiler/exit errors. "
                  "Read cloud-hello.log using read_file to verify the actual artifact. Do not install tools, use Cargo, access files outside this task directory, or launch background processes. Report actual stdout and the verified log marker.")
        result.update({"model_id": args.model, "protocol": args.protocol, "max_calls_total": args.max_calls, "max_calls_per_run": per_run_calls, "max_seconds_total": args.max_seconds, "temporary_device_fp": device_fp})
        watchdog = threading.Timer(max(1, args.max_seconds - (time.monotonic() - started)), lambda: stop(node))
        watchdog.daemon = True
        watchdog.start()
        remaining = max(1, args.max_seconds - (time.monotonic() - started))
        out, events, live = run_frames(controller, "cloud-compile", prompt, transcript, timeout=remaining, model=args.model, protocol=args.protocol)
        require(not out.get("err") and out.get("code", 0) == 0, "Actual cloud/model compilation task did not succeed")
        require(live, "No live cloud text event before terminal result")
        require((work / "cloud-hello.log").read_text().strip() == marker, "Compiled program log artifact missing or wrong")
        require((work / exe_name).is_file(), "Compiled executable is missing")
        require(any(event.get("type") == "tool_start" and event.get("title") == "exec" and "rustc" in event.get("detail", "") for event in events), "No actual compiler command receipt")
        require(any(event.get("type") == "tool_end" and receipt in event.get("output", "") for event in events), "Actual compiled program stdout receipt missing")
        continuation = ("Continue this same native session. Recall which Rust program you just compiled. Read cloud-hello.log with read_file, then write continuation.txt containing its exact log marker plus CONTINUITY_OK. "
                        "Use only these temporary working-directory files. Report the file you compiled and the log content; do not rerun or recompile the executable.")
        remaining = args.max_seconds - (time.monotonic() - started)
        require(remaining > 1, "Total cloud trial deadline exhausted before continuation")
        out, events, _ = run_frames(controller, "cloud-continuation", continuation, transcript, timeout=remaining, model=args.model, protocol=args.protocol)
        require(not out.get("err") and out.get("code", 0) == 0, "Native cloud conversation continuation failed")
        saved = (work / "continuation.txt").read_text()
        require(marker in saved and "CONTINUITY_OK" in saved, "Continuation artifact does not match actual prior log")
        result.update({"status": "PASS", "passed": ["new_temporary_device_approval", "actual_cloud_model_tool_loop", "actual_rust_compiler_and_executable", "actual_log_readback", "live_stream_event", "same_native_session_continuation"], "compiled_binary_sha256": hashlib.sha256((work / exe_name).read_bytes()).hexdigest()})
    except Exception as error:
        # Do not propagate HTTP/SSH response bodies that may include credentials.
        result["error"] = f"{type(error).__name__} status={getattr(error, 'code', None)}: cloud trial failed; inspect safe node/controller receipts"
        raise RuntimeError(result["error"]) from None
    finally:
        if watchdog:
            watchdog.cancel()
        stop(node)
        if controller and cleanup_revoke and device_fp:
            try:
                controller.send({"k": "revoke", "fp": device_fp})
                controller.until(lambda m: m.get("k") == "status" and not any(row.get("fp") == device_fp and row.get("state") in ("approved", "paired", "approved-offline") for row in status_rows(m)), timeout=5)
                result["own_temporary_approval_revoked"] = True
            except Exception:
                result["own_temporary_approval_revoked"] = False
        if controller:
            controller.close()
        stop(hub)
        stop(tunnel)
        tunnel_log.close()
        if node_log:
            node_log.close()
        if hub_log:
            hub_log.close()
        result["elapsed_seconds"] = round(time.monotonic() - started, 3)
        if args.binary and args.binary.is_file():
            result["binary_sha256"] = hashlib.sha256(args.binary.read_bytes()).hexdigest()
        # Agent events concern only our trial. Status messages with other devices
        # are deliberately never recorded in the controller transcript.
        (root / "result.json").write_text(json.dumps(result, indent=2), encoding="utf-8")
        (root / "controller.json").write_text(json.dumps(transcript, indent=2), encoding="utf-8")
        print(json.dumps({"status": result["status"], "evidence": str(root)}, indent=2))


if __name__ == "__main__":
    main()
