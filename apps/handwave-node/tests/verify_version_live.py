"""Bounded actual model context/memory acceptance through an isolated local Hub.
Production is read only: one authenticated Gateway route is reused via SSH. All
Node identities, approvals, history, files and memory changes stay in new D paths.
At most five model Runs with three provider calls each, plus effect-free native
management commands; no automatic rerun of a failed task.
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
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from urllib.error import HTTPError
from urllib.request import Request, urlopen

from verify_cloud import read_gateway_credential, read_credentials, models, wait_tunnel_health
from verify_host import WS, port, require, run_frames, status_rows, stop


class IsolatedGateway(ThreadingHTTPServer):
    """Narrow only this trial's advertised capacity; retain the actual model wire."""
    daemon_threads = True

    def __init__(self, upstream, model, deadline):
        super().__init__(("127.0.0.1", 0), GatewayForwarder)
        self.upstream, self.model, self.deadline = upstream, model, deadline
        self.guard = threading.Lock()
        self.calls = 0
        self.capacity_receipts = []


class GatewayForwarder(BaseHTTPRequestHandler):
    protocol_version = "HTTP/1.1"

    def log_message(self, *_):
        pass  # Never persist authorization headers or request bodies.

    def do_GET(self):
        self.forward()

    def do_POST(self):
        self.forward()

    def forward(self):
        proxy = self.server
        remaining = proxy.deadline - time.monotonic()
        if remaining <= 0:
            self.send_error(504, "Isolated trial deadline exhausted"); return
        if self.command == "POST":
            if self.path.split("?", 1)[0] != "/v1/chat/completions":
                self.send_error(403, "Only bounded actual model calls are allowed"); return
            with proxy.guard:
                if proxy.calls >= 15:
                    self.send_error(429, "Isolated trial provider call budget exhausted"); return
                proxy.calls += 1  # Reserve before upstream HTTP, including failures.
        elif self.path.split("?", 1)[0] not in ("/healthz", "/health", "/v1/models"):
            self.send_error(403, "Only health and model metadata reads are allowed"); return
        length = int(self.headers.get("Content-Length", "0"))
        if length < 0 or length > 2 << 20:
            self.send_error(413, "Isolated request budget exceeded"); return
        body = self.rfile.read(length) if length else None
        headers = {k: v for k, v in self.headers.items()
                   if k.lower() not in ("host", "connection", "content-length", "transfer-encoding")}
        request = Request(proxy.upstream + self.path, data=body, headers=headers, method=self.command)
        try:
            response = urlopen(request, timeout=max(.1, min(30, remaining)))
        except HTTPError as error:
            response = error
        except Exception:
            self.send_error(502, "Isolated upstream request failed"); return
        try:
            if self.command == "GET" and self.path.split("?", 1)[0] == "/v1/models" and response.status == 200:
                raw = response.read((2 << 20) + 1)
                require(len(raw) <= 2 << 20, "Live directory exceeds fixture bound")
                value = json.loads(raw)
                def narrow(row):
                    if isinstance(row, dict):
                        if row.get("id") == proxy.model:
                            original = json.loads(json.dumps({k: row.get(k) for k in ("context_window", "contextWindow", "omp")}))
                            caps = [row.get("context_window"), row.get("omp", {}).get("contextWindow")
                                    if isinstance(row.get("omp"), dict) else None]
                            require(all(v is None or type(v) is int and v >= 0 for v in caps),
                                    "Live model capacity declaration is malformed")
                            capacity = min([262144] + [v for v in caps if isinstance(v, int) and not isinstance(v, bool) and v > 0])
                            row["context_window"] = capacity
                            if isinstance(row.get("omp"), dict):
                                row["omp"]["contextWindow"] = capacity
                            with proxy.guard:
                                proxy.capacity_receipts.append({"model": proxy.model, "original": original,
                                                                "isolated_context_window": capacity})
                        for child in list(row.values()):
                            narrow(child)
                    elif isinstance(row, list):
                        for child in row: narrow(child)
                narrow(value)
                raw = json.dumps(value, ensure_ascii=False, separators=(",", ":")).encode()
                self.send_response(200); self.send_header("Content-Type", "application/json")
                self.send_header("Content-Length", str(len(raw))); self.end_headers(); self.wfile.write(raw)
                return
            self.send_response(response.status)
            for key, value in response.headers.items():
                if key.lower() not in ("connection", "content-length", "transfer-encoding"):
                    self.send_header(key, value)
            self.send_header("Connection", "close"); self.end_headers(); self.close_connection = True
            while time.monotonic() < proxy.deadline:
                chunk = response.read1(65536)
                if not chunk: break
                self.wfile.write(chunk); self.wfile.flush()
        except (BrokenPipeError, ConnectionResetError):
            pass
        finally:
            response.close()


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--binary", type=Path, required=True)
    parser.add_argument("--hub-binary", type=Path, required=True)
    parser.add_argument("--output", type=Path, required=True)
    parser.add_argument("--ssh-alias", default="ray-dev")
    parser.add_argument("--remote-config", default="/opt/handwave/hub.conf")
    parser.add_argument("--model", required=True)
    args = parser.parse_args()
    root = args.output.resolve()
    root.mkdir(parents=True, exist_ok=False)
    work, state = root / "work", root / "state"
    work.mkdir(); state.mkdir()
    root_token, node_token = secrets.token_hex(24), secrets.token_hex(24)
    gateway_token = read_gateway_credential(args.ssh_alias, "/opt/omp-gateway/gateway.yaml")
    endpoint_port = port()
    logs = [(root / "tunnel.log").open("wb"), (root / "node.log").open("wb")]
    tunnel = node = hub = controller = watchdog = proxy = None
    device = None
    transcript, passed = [], []
    model_runs = 0
    started = time.monotonic()
    result = {"status": "FAIL", "scope": "actual Rust Node + isolated local Hub + actual cloud model; no production device or memory mutations",
              "max_model_runs": 5, "max_provider_calls_per_run": 3, "max_provider_calls_total": 15,
              "deadline_seconds": 240, "model": args.model, "protocol": "openai-completions", "passed": passed}
    try:
        gateway_port = port()
        tunnel = subprocess.Popen(["ssh", "-N", "-o", "BatchMode=yes", "-o", "StrictHostKeyChecking=yes",
                    "-o", "ExitOnForwardFailure=yes", "-L", f"127.0.0.1:{gateway_port}:127.0.0.1:18799", args.ssh_alias],
                    stdout=subprocess.DEVNULL, stderr=logs[0])
        gateway=f"http://127.0.0.1:{gateway_port}";wait_tunnel_health(gateway)
        proxy = IsolatedGateway(gateway, args.model, started + 240)
        threading.Thread(target=proxy.serve_forever, daemon=True).start()
        isolated_gateway = f"http://127.0.0.1:{proxy.server_address[1]}"
        base = f"http://127.0.0.1:{endpoint_port}"
        conf=root/"hub-conf";conf.mkdir();logs.append((root/"hub-private.log").open("wb"))
        hub_env=os.environ.copy();hub_env.update(HW_CONF=str(conf),HW_VERSION_GATEWAY_TOKEN=gateway_token)
        hub=subprocess.Popen([str(args.hub_binary.resolve()),"hub","-listen",f"127.0.0.1:{endpoint_port}","-token",root_token,
            "-node_token",node_token,"-gateway",isolated_gateway,"-gateway_token_env","HW_VERSION_GATEWAY_TOKEN",
            "-model",args.model,"-protocol","openai-completions","-allow_loopback"],env=hub_env,stdout=logs[2],stderr=logs[2])
        args.public_hub=base
        wait_tunnel_health(base)
        available, receipt = models(base, root_token)
        require(receipt["http_status"] == 200 and any(r["id"] == args.model for r in available), "Model absent from live inventory")
        result["inventory"] = receipt
        require(proxy.capacity_receipts, "Isolated model capacity was not applied")
        require(all(r["isolated_context_window"] == 262144 for r in proxy.capacity_receipts),
                "Selected actual model is too small for this fixed-input live trial")
        result["isolated_capacity_boundary"] = "Only local fixture model metadata is narrowed to at most 262144; production metadata, actual model ID/protocol and completions are unchanged. This is a constrained-capacity real-model task, not a full1M capacity trial."
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
        watchdog = threading.Timer(max(1, 240 - (time.monotonic() - started)), lambda: stop(node)); watchdog.daemon = True; watchdog.start()
        controller = WS(endpoint_port, "ctl", root_token)
        controller.send({"k": "hello", "fp": "context-live-" + secrets.token_hex(8), "relay": True})
        status = controller.until(lambda m: m.get("k") == "status" and any(r.get("fp") == fp and r.get("nid", "").startswith("n") for r in status_rows(m)))
        device = next(r for r in status_rows(status) if r["fp"] == fp)
        require(device.get("state") == "pending", "Temporary node unexpectedly already approved")
        controller.send({"k": "approve", "nid": device["nid"], "relay": True, "ttl": 240})
        controller.until(lambda m: m.get("k") == "paired")
        controller.until(lambda m: m.get("k") == "dc" and json.loads(base64.b64decode(m["d"])).get("k") == "node-hello")
        marker = "REFERENCE_" + secrets.token_hex(6)
        rule = "RULE_" + secrets.token_hex(6)
        (work / "AGENTS.md").write_text("For this trial, final answers must contain " + rule + ".\n", encoding="utf-8")
        # Two bounded tool outputs exceed the isolated ~180 KiB projection, while
        # each current Run remains below it. The real1M declaration would not
        # trigger on these ~220 KiB receipts. No injected
        # session JSON or synthetic summary is used in this live-model trial.
        for name in ("first.txt", "second.txt"):
            (work / name).write_text(("ordinary log context padding 0123456789 abcdef\n" * 2500) + marker + "\n", encoding="utf-8")

        def run(name, prompt):
            nonlocal model_runs
            remaining = 240 - (time.monotonic() - started)
            require(remaining > 1, "Live trial deadline exhausted")
            if not prompt.startswith(("/memory ", "/reference ")):
                require(model_runs < 5, "Live trial model Run budget exhausted")
                model_runs += 1
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

        events, _ = run("context-live-propose", "Using the independent user reference directory, find my project marker from the earlier user request. Call memory_propose exactly once to propose that marker as a fact, citing its real source ID. You may use reference_read once first if needed. After memory_propose returns success, immediately give a short final answer; this completes this task. Do not verify the candidate with any other tools, do not call history_search or history_read, and do not adopt it. There are only three model calls available including your final answer.")
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
        source = refs[0]["id"]; memory_id=candidates[0]["id"]
        run("context-live-revoke-memory","/memory revoke "+memory_id)
        run("context-live-revoke-reference","/reference revoke "+source)
        require(next(m for m in store()["memories"] if m["id"]==memory_id)["status"]=="revoked","Memory revocation not durable")
        require(next(r for r in store()["references"] if r["id"]==source)["status"]=="revoked","Reference revocation not durable")
        revoked_events,_=run("context-live-revoked-probe", "Call reference_read exactly once with id "+source+". Report that source availability only. Do not read files or call other tools.")
        revoked_starts=[e for e in revoked_events if e.get("type")=="tool_start"]
        denied=[e for e in revoked_events if e.get("type")=="tool_end" and e.get("title")=="reference_read"]
        require(len(revoked_starts)==1 and revoked_starts[0].get("title")=="reference_read"
                and len(denied)==1 and denied[0].get("status")=="failed"
                and all(marker not in e.get("output", "") for e in revoked_events),
                "Revoked source leaked, another tool ran, or source denial was not actually checked")
        passed.append("actual_model_revoked_source_denial_and_memory_revocation")
        summary_events=[f.get("event",{}) for f in transcript if f.get("k")=="agent-event"]
        require(any(e.get("type")=="compact" and e.get("status")=="completed" for e in summary_events),"Actual compaction success receipt absent")
        result["provider_call_receipts"]=sum(e.get("type")=="model_call" for e in summary_events)
        require(result["provider_call_receipts"]<=15,"Global trial call budget exceeded")
        require(proxy.calls == result["provider_call_receipts"], "Actual upstream reservations differ from Node receipts")
        result["usage_receipts"]=[{k:e.get(k) for k in ("phase","call","usage","usage_metrics")} for e in summary_events if e.get("type")=="model_result"]
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
        stop(hub)
        if proxy:
            proxy.shutdown(); proxy.server_close()
            result["actual_provider_http_reservations"] = proxy.calls
            result["isolated_capacity_receipts"] = proxy.capacity_receipts
        result["model_runs_started"] = model_runs
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
