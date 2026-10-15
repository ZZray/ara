"""Actual Rust executable with controlled signaling/HTTP fault paths.

No production access. Delay grant beyond the actual 12s node deadline, deliver
late/canceled grants, and verify real wire terminal ordering and zero effects.
Delayed collector drainage is exercised; OS send-buffer saturation is not claimed.
"""
import argparse
import base64
import hashlib
import json
import os
from pathlib import Path
import socketserver
import struct
import subprocess
import tempfile
import threading
import time

from verify_host import WS, require, stop, validate_loop_events


class ServerWire(WS):
    def send_raw(self, data, opcode=1):
        size = len(data)
        header = bytes([0x80 | opcode])
        header += bytes([size]) if size < 126 else bytes([126]) + struct.pack("!H", size) if size < 65536 else bytes([127]) + struct.pack("!Q", size)
        self.sock.sendall(header + data)


class GrantHub:
    def __init__(self):
        self.ready, self.release = threading.Event(), threading.Event()
        self.http, self.receipts = [], []
        hub = self
        class Handler(socketserver.BaseRequestHandler):
            def handle(self):
                data = bytearray()
                while b"\r\n\r\n" not in data:
                    chunk = self.request.recv(4096)
                    if not chunk: return
                    data.extend(chunk)
                    require(len(data) < 65536, "oversized signaling HTTP headers")
                head, rest = bytes(data).split(b"\r\n\r\n", 1)
                lines = head.decode().split("\r\n")
                method, path, _ = lines[0].split(" ", 2)
                if path.startswith("/ws?"):
                    headers = dict(line.split(":", 1) for line in lines[1:] if ":" in line)
                    key = next(value.strip() for name, value in headers.items() if name.lower() == "sec-websocket-key")
                    accept = base64.b64encode(hashlib.sha1((key + "258EAFA5-E914-47DA-95CA-C5AB0DC85B11").encode()).digest()).decode()
                    self.request.sendall(("HTTP/1.1 101 Switching Protocols\r\nUpgrade: websocket\r\nConnection: Upgrade\r\nSec-WebSocket-Accept: " + accept + "\r\nSec-WebSocket-Protocol: handwave\r\n\r\n").encode())
                    hub.wire = object.__new__(ServerWire)
                    hub.wire.sock, hub.wire.pending = self.request, bytearray(rest)
                    hub.ready.set()
                    hub.release.wait(90)
                else:
                    hub.http.append({"method": method, "path": path})
                    self.request.sendall(b"HTTP/1.1 503 Service Unavailable\r\nContent-Type: application/json\r\nContent-Length: 2\r\nConnection: close\r\n\r\n{}")
        class Server(socketserver.ThreadingTCPServer):
            daemon_threads = True
            allow_reuse_address = True
        self.server = Server(("127.0.0.1", 0), Handler)
        threading.Thread(target=self.server.serve_forever, daemon=True).start()

    def receive(self, timeout=15):
        packet = self.wire.receive(timeout)
        if packet.get("k") == "dc":
            packet = json.loads(base64.b64decode(packet["d"]))
        self.receipts.append(packet)
        return packet

    def grant_request(self, run_id):
        while True:
            packet = self.receive()
            if packet.get("k") == "model-grant" and packet.get("id") == run_id:
                return packet

    def terminal(self, run_id, expected):
        events = []
        while True:
            packet = self.receive()
            if packet.get("id") != run_id: continue
            if packet.get("k") == "agent-event": events.append(packet["event"])
            if packet.get("k") == "out" and packet.get("done"):
                require(packet.get("code") == -1 and packet.get("err"), "rejection was reported as success")
                validate_loop_events(events, run_id, packet)
                require(events[-1]["status"] == expected, "incorrect early terminal status")
                return events

    def close(self):
        self.release.set()
        self.server.shutdown()
        self.server.server_close()


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--binary", type=Path, required=True)
    parser.add_argument("--output", type=Path)
    args = parser.parse_args()
    root = args.output or Path(tempfile.mkdtemp(prefix="handwave-grant-terminal-"))
    root.mkdir(parents=True, exist_ok=True)
    require(not (root / "result.json").exists(), "Use fresh evidence directory")
    work, state = root / "work", root / "state"
    work.mkdir(); state.mkdir()
    hub = GrantHub()
    env = os.environ.copy()
    for key in list(env):
        if key.lower() in ("http_proxy", "https_proxy", "all_proxy", "no_proxy", "hw_proxy", "hw_hub", "hw_token", "hw_node_state"):
            del env[key]
    env.update(HW_TOKEN="controlled-enrollment", NO_PROXY="127.0.0.1,localhost")
    log = (root / "node.log").open("wb")
    node = None
    passed = []
    result = {"scope": "actual Rust process; controlled signaling, grant and HTTP; no production/model effects", "passed": passed}
    started = time.monotonic()
    try:
        node = subprocess.Popen([str(args.binary.resolve()), "--hub", f"http://127.0.0.1:{hub.server.server_address[1]}", "--state-dir", str(state.resolve()), "--cwd", str(work.resolve()), "--once", "--max-calls", "2", "--max-seconds", "20"], env=env, stdout=log, stderr=log)
        require(hub.ready.wait(10), "Node signaling connection missing")
        require(hub.receive().get("k") == "join", "No actual join")
        hub.wire.send({"k": "pair-req", "ctlfp": "controlled-grant-owner"})
        require(hub.receive().get("k") == "node-hello", "No actual paired node hello")
        for run_id, message in [("empty-objective", {"msg": ""}), ("smart-conflict", {"msg": "NO_EFFECT", "smart": True, "model": "fixture-model"})]:
            hub.wire.frame({"k": "agent-loop", "id": run_id, **message})
            hub.terminal(run_id, "failed")
            require(not hub.http, "Early admission rejection contacted model HTTP")
            passed.append(run_id + "_local_end_before_out")
        for run_id in ("grant-denied", "grant-selection-missing"):
            hub.wire.frame({"k": "agent-loop", "id": run_id, "msg": "NO_EFFECT", "model": "default"})
            hub.grant_request(run_id)
            grant = {"k": "model-grant", "id": run_id, "d": "controlled-grant", "ttl": 120}
            if run_id == "grant-denied": grant.update(err="controlled denial", d="")
            hub.wire.send(grant)
            hub.terminal(run_id, "failed")
            require(not hub.http, "Denied/incomplete grant caused model HTTP")
            passed.append(run_id + "_local_end_before_out_no_effect")
        run_id = "grant-timeout-late"
        hub.wire.frame({"k": "agent-loop", "id": run_id, "msg": "NO_EFFECT", "model": "fixture-model"})
        hub.grant_request(run_id)
        # Hold collector drainage beyond the actual grant timer, then deliver a
        # valid late grant before reading queued terminal packets.
        time.sleep(13.25)
        hub.wire.send({"k": "model-grant", "id": run_id, "d": "controlled-grant", "ttl": 120, "default_model": "fixture-model", "default_protocol": "openai-completions"})
        hub.terminal(run_id, "failed")
        require(not hub.http and not list(work.iterdir()), "Late valid grant admitted model/tool effects")
        passed.append("timeout_delayed_terminal_drain_and_late_grant_no_effect")
        run_id = "grant-cancel-pending"
        hub.wire.frame({"k": "agent-loop", "id": run_id, "msg": "NO_EFFECT", "model": "fixture-model"})
        hub.grant_request(run_id)
        hub.wire.frame({"k": "agent-abort", "id": run_id})
        hub.wire.send({"k": "model-grant", "id": run_id, "d": "controlled-grant", "ttl": 120, "default_model": "fixture-model", "default_protocol": "openai-completions"})
        hub.terminal(run_id, "canceled")
        require(not hub.http and not list(work.iterdir()), "Canceled pending grant admitted model/tool effects")
        passed.append("cancel_pending_valid_grant_local_end_before_out_no_effect")
        run_id = "execute-catalog-failure"
        hub.wire.frame({"k": "agent-loop", "id": run_id, "msg": "NO_EFFECT", "model": "fixture-model"})
        hub.grant_request(run_id)
        hub.wire.send({"k": "model-grant", "id": run_id, "d": "controlled-grant", "ttl": 120, "default_model": "fixture-model", "default_protocol": "openai-completions"})
        hub.terminal(run_id, "failed")
        require(hub.http == [{"method": "GET", "path": "/v1/models"}] and not list(work.iterdir()), "Execute failure triggered model/tool effects or lost catalog receipt")
        passed.append("execute_catalog_error_local_end_before_out_no_model_tool_effect")
        result["status"] = "PASS"
    except Exception as error:
        result.update(status="FAIL", error=f"{type(error).__name__}: {error}")
        raise
    finally:
        stop(node); hub.close(); log.close()
        result.update(binary_sha256=hashlib.sha256(args.binary.read_bytes()).hexdigest(), elapsed_seconds=round(time.monotonic()-started,3), http_receipts=hub.http, coverage_gap="Collector drainage delayed; kernel-level send-buffer saturation and successful-end/duplicate collision timing are not deterministically forced here")
        (root / "controller.json").write_text(json.dumps(hub.receipts,indent=2,ensure_ascii=False),encoding="utf-8")
        (root / "result.json").write_text(json.dumps(result,indent=2),encoding="utf-8")
        print(json.dumps(result,indent=2))


if __name__ == "__main__": main()
