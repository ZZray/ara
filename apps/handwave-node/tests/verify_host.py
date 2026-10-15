"""Bounded real-process test: Rust node -> actual Go Hub -> controlled OpenAI fixture.

No third-party Python dependencies. All configuration, credentials, effects and logs
are isolated beneath --output (a fresh temporary directory by default). This is
protocol/host acceptance, not production cloud or real-model acceptance.
"""
from __future__ import annotations

import argparse
import base64
import hashlib
import http.server
import json
import os
from pathlib import Path
import secrets
import select
import shlex
import socket
import socketserver
import ssl
import struct
import subprocess
import tempfile
import threading
import time
import urllib.request
import urllib.parse


def require(condition, message):
    if not condition:
        raise AssertionError(message)


def status_rows(message):
    rows = json.loads(message.get("d", "[]"))
    # Go buildStatusLocked marshals its initial nil slice as null before a
    # device has joined. Treat that normal empty inventory as an empty list.
    if rows is None:
        return []
    require(isinstance(rows, list), "Unexpected Hub device inventory shape")
    return rows


def port():
    with socket.socket() as sock:
        sock.bind(("127.0.0.1", 0))
        return sock.getsockname()[1]


class WS:
    """Minimal RFC6455 client for the local real Hub, including masking/ping."""
    def __init__(self, host_port, role, token, host="127.0.0.1", path_prefix="", secure=False):
        self.sock = socket.create_connection((host, host_port), timeout=15)
        if secure:
            self.sock = ssl.create_default_context().wrap_socket(self.sock, server_hostname=host)
        self.pending = bytearray()
        key = base64.b64encode(secrets.token_bytes(16)).decode()
        request = (f"GET {path_prefix.rstrip('/')}/ws?role={role} HTTP/1.1\r\nHost: {host}:{host_port}\r\n"
                   f"Upgrade: websocket\r\nConnection: Upgrade\r\nSec-WebSocket-Key: {key}\r\n"
                   f"Sec-WebSocket-Version: 13\r\nSec-WebSocket-Protocol: handwave\r\n"
                   f"Authorization: Bearer {token}\r\n\r\n")
        self.sock.sendall(request.encode())
        data = bytearray()
        while b"\r\n\r\n" not in data:
            chunk = self.sock.recv(4096)
            require(chunk, "Hub closed during WebSocket handshake")
            data.extend(chunk)
        headers, rest = bytes(data).split(b"\r\n\r\n", 1)
        require(headers.startswith(b"HTTP/1.1 101"), "WebSocket handshake failed")
        expected = base64.b64encode(hashlib.sha1((key + "258EAFA5-E914-47DA-95CA-C5AB0DC85B11").encode()).digest())
        require(expected.lower() in headers.lower(), "Invalid WebSocket accept")
        self.pending.extend(rest)

    def send_raw(self, data, opcode=1):
        mask = secrets.token_bytes(4)
        count = len(data)
        header = bytes([0x80 | opcode])
        if count < 126:
            header += bytes([0x80 | count])
        elif count < 65536:
            header += bytes([0xFE]) + struct.pack("!H", count)
        else:
            header += bytes([0xFF]) + struct.pack("!Q", count)
        self.sock.sendall(header + mask + bytes(v ^ mask[i % 4] for i, v in enumerate(data)))

    def send(self, message):
        self.send_raw(json.dumps(message, ensure_ascii=False).encode())

    def frame(self, value):
        self.send({"k": "dc", "d": base64.b64encode(json.dumps(value).encode()).decode()})

    def read_exact(self, size):
        while len(self.pending) < size:
            data = self.sock.recv(max(4096, size - len(self.pending)))
            if not data:
                raise ConnectionError("WebSocket closed")
            self.pending.extend(data)
        out = bytes(self.pending[:size])
        del self.pending[:size]
        return out

    def receive(self, timeout=15):
        end = time.monotonic() + timeout
        chunks = bytearray()
        while True:
            self.sock.settimeout(max(0.01, end - time.monotonic()))
            h = self.read_exact(2)
            opcode, length = h[0] & 15, h[1] & 127
            if length == 126:
                length = struct.unpack("!H", self.read_exact(2))[0]
            elif length == 127:
                length = struct.unpack("!Q", self.read_exact(8))[0]
            require(length <= 8 * 1024 * 1024, "Oversized fixture WS frame")
            mask = self.read_exact(4) if h[1] & 128 else None
            data = self.read_exact(length)
            if mask:
                data = bytes(v ^ mask[i % 4] for i, v in enumerate(data))
            if opcode == 9:
                self.send_raw(data, 10)
                continue
            if opcode == 10:
                continue
            if opcode == 8:
                raise ConnectionError("WebSocket close frame")
            require(opcode in (0, 1, 2), "Unexpected WS opcode")
            chunks.extend(data)
            if h[0] & 128:
                return json.loads(chunks)

    def until(self, predicate, timeout=15):
        end = time.monotonic() + timeout
        while time.monotonic() < end:
            message = self.receive(end - time.monotonic())
            if predicate(message):
                return message
        raise TimeoutError("Expected Hub message did not arrive")

    def close(self):
        self.sock.close()


class Fixture:
    def __init__(self, root, modern=False, *, context_window=128000, input_modalities=None):
        self.root = root
        self.modern = modern
        self.expected_model = "fixture-model"
        self.context_window = context_window
        self.input_modalities = ["text"] if input_modalities is None else input_modalities
        self.requests = []
        self.downloads = []
        self.catalog_requests = 0
        self.route_requests = []
        self.errors = []
        self.lock = threading.Lock()
        self.partial_path = root / "partial-must-not-exist.txt"
        self.live_sent = threading.Event()
        self.server = http.server.ThreadingHTTPServer(("127.0.0.1", 0), self.handler())
        self.server.daemon_threads = True
        threading.Thread(target=self.server.serve_forever, daemon=True).start()

    def handler(self):
        fixture = self

        class Handler(http.server.BaseHTTPRequestHandler):
            protocol_version = "HTTP/1.1"

            def log_message(self, *args):
                pass

            def do_GET(self):
                if self.path == "/v1/models":
                    require(self.headers.get("Authorization") == "Bearer fixture-key", "Hub directory did not use its gateway credential")
                    fixture.catalog_requests += 1
                    data = json.dumps({"object": "list", "data": [
                        {"id": "fixture-model", "object": "model", "protocols": ["openai-completions"], "context_window": fixture.context_window, "max_tokens": 4096, "omp": {"name": "Fixture", "input": fixture.input_modalities, "supportsTools": True, "reasoning": False}},
                        {"id": "fixture-other-protocol", "object": "model", "protocols": ["anthropic-messages"], "context_window": 128000, "max_tokens": 8192, "omp": {"name": "Unsupported fixture", "input": ["text"], "supportsTools": True}}
                        , {"id": "fixture-ambiguous", "object": "model", "protocols": ["openai-completions", "anthropic-messages"], "context_window": 128000, "max_tokens": 8192, "omp": {"name": "Ambiguous fixture", "input": ["text"], "supportsTools": True}}
                        , {"id": "fixture-legacy-null", "protocols": None, "context_window": 128000, "max_tokens": 4096, "omp": {"supportsTools": True}}
                        , {"id": "fixture-legacy-empty", "protocols": [], "context_window": 128000, "max_tokens": 4096, "omp": {"supportsTools": True}}
                        , {"id": "fixture-legacy-absent", "context_window": 128000, "max_tokens": 4096, "omp": {"supportsTools": True}}
                        , {"id": "fixture-protocol-dupes", "protocols": [" openai-completions ", "openai-completions"], "context_window": 128000, "max_tokens": 4096, "omp": {"supportsTools": True}}
                    ]}).encode()
                    self.send_response(200)
                    self.send_header("Content-Type", "application/json")
                    self.send_header("Content-Length", str(len(data)))
                    self.end_headers()
                    self.wfile.write(data)
                    return
                if self.path != "/fixture-tool.txt":
                    self.send_error(404)
                    return
                fixture.downloads.append(self.path)
                data = b"DOWNLOADED_TOOL_ARTIFACT_73\n"
                self.send_response(200)
                self.send_header("Content-Type", "application/octet-stream")
                self.send_header("Content-Length", str(len(data)))
                self.end_headers()
                self.wfile.write(data)

            def do_POST(self):
                try:
                    size = int(self.headers.get("Content-Length", "0"))
                    require(0 < size < 8 * 1024 * 1024, "Invalid model request size")
                    body = json.loads(self.rfile.read(size))
                    require(self.headers.get("Authorization") == "Bearer fixture-key", "Hub did not replace grant with upstream credential")
                    if self.path == "/v1/route/resolve":
                        with fixture.lock:
                            fixture.route_requests.append(body)
                        data = json.dumps({"decision": {"decision_id": "fixture-route-decision", "model": "fixture-model", "protocol": "openai-completions", "pool": "fixture-pool", "profile_hash": "fixture-profile", "policy_version": "fixture-policy-v1", "catalog_version": "fixture-catalog-v1", "source": "controlled-fixture", "reason_codes": ["fixture"], "cost_estimate": None}, "expires_at": time.strftime("%Y-%m-%dT%H:%M:%SZ", time.gmtime(time.time() + 300)), "eligible_count": 1, "catalog_version": "fixture-catalog-v1"}).encode()
                        self.send_response(200)
                        self.send_header("Content-Type", "application/json")
                        self.send_header("Content-Length", str(len(data)))
                        self.end_headers()
                        self.wfile.write(data)
                        return
                    require(self.path.endswith("/chat/completions"), "Unexpected model route")
                    if fixture.modern:
                        require(body.get("model") == fixture.expected_model, "Modern default/model sentinel leaked to the actual model request")
                        limit = body.get("max_tokens", body.get("max_completion_tokens"))
                        require(isinstance(limit, int) and 0 < limit <= 4096, "Actual model output limit exceeded the directory's 4096-token cap")
                    with fixture.lock:
                        fixture.requests.append(body)
                    fixture.reply(self, body)
                except (BrokenPipeError, ConnectionResetError):
                    pass  # Expected when canceled or disconnected.
                except Exception as error:
                    with fixture.lock:
                        fixture.errors.append(str(error))
                    self.send_error(500, "fixture assertion failed")

        return Handler

    def reply(self, handler, body):
        messages = body.get("messages", [])
        latest = next((m for m in reversed(messages) if m.get("role") == "user"), {})
        text = latest.get("content", "")
        if not isinstance(text, str):
            text = json.dumps(text)
        last_user = max((i for i, m in enumerate(messages) if m.get("role") == "user"), default=-1)
        tools = [m for m in messages[last_user + 1:] if m.get("role") == "tool"]
        declared = {t["function"]["name"] for t in body.get("tools", [])}
        tool_name = None
        arguments = None
        scenario = ("init" if "INIT_ARTIFACT" in text else "download" if "DOWNLOAD_TOOL" in text else "compile" if "CONTROLLED_COMPILE" in text else "partial" if "PARTIAL_TOOL" in text else "abort" if "ABORT_PROCESS" in text else "disconnect")
        tool_id = f"fixture-{scenario}-{len(tools)}"
        partial = False
        if "READ_SELECTION_TOOL" in text:
            schemas = {t["function"]["name"]: t["function"]["parameters"] for t in body.get("tools", [])}
            require(all(name in schemas["read_file"]["properties"] for name in ("offset", "limit", "tail")), "Read pagination/tail schema absent from actual model request")
            tool_id = f"fixture-read-selection-{len(tools)}"
            if not tools:
                tool_name = "read_file"
                arguments = {"path": str(self.root.parent / "outside-log.txt"), "offset": 40000, "limit": 2}
            elif len(tools) == 1:
                tool_name = "read_file"
                arguments = {"path": str(self.root.parent / "outside-log.txt"), "tail": 2}
            else:
                receipt = json.loads(tools[0]["content"])
                require(receipt["lines"][0]["line"] == 40000 and receipt["lines"][1]["text"] == "READ_TOOLS_EOF_73", "Actual range did not select expected numbered log lines")
                tail = json.loads(tools[1]["content"])
                require(tail["lines"][0]["line"] is None and tail["lines"][1]["line_from_end"] == 1 and tail["lines"][1]["text"] == "READ_TOOLS_EOF_73", "Actual tail receipt did not come from EOF")
                require(receipt["coverage"]["snapshot_file_bytes"] > 1024 * 1024, "Read range fixture was not a large log")
        elif "NATIVE_SEARCH_TOOL" in text:
            schemas = {t["function"]["name"]: t["function"]["parameters"] for t in body.get("tools", [])}
            require(all(name in schemas["search_files"]["properties"] for name in ("query", "path", "max_results", "case_sensitive")), "Search schema absent from actual model request")
            tool_id = f"fixture-native-search-{len(tools)}"
            if not tools:
                tool_name = "search_files"
                arguments = {"query": "native needle.*", "path": str(self.root / "native-search"), "max_results": 5, "case_sensitive": False}
            else:
                receipt = json.loads(tools[0]["content"])
                require(len(receipt["matches"]) == 1 and receipt["matches"][0]["line"] == 2 and "NATIVE NEEDLE.*" in receipt["matches"][0]["excerpt"], "Actual literal search location/content missing")
                require(receipt["coverage"]["skipped_generated_dirs"] == 1 and receipt["coverage"]["complete"] is False, "Skipped generated directory was not reported as partial")
        elif "INIT_ARTIFACT" in text:
            if len(tools) == 0:
                tool_name = "write_file" if "write_file" in declared else "write"
                arguments = {"path": str(fixture_path(self.root)), "content": "HANDWAVE_HOST_ARTIFACT_73\n"}
            elif len(tools) == 1:
                tool_name = "read_file" if "read_file" in declared else "read"
                arguments = {"path": str(fixture_path(self.root))}
            else:
                require("HANDWAVE_HOST_ARTIFACT_73" in json.dumps(tools), "Read tool receipt missing actual file content")
        elif "CHECK_CONTINUITY" in text:
            require("INIT_ARTIFACT" in json.dumps(messages[:-1]), "Native session history lost")
            require("HANDWAVE_HOST_ARTIFACT_73" in json.dumps(messages[:-1]), "Native session tool evidence lost")
        elif "PARTIAL_TOOL" in text:
            tool_name = "write_file" if "write_file" in declared else "write"
            arguments = {"path": str(self.partial_path), "content": "unsafe partial"}
            partial = True
        elif "ACTIVE_ID_COLLISION" in text:
            pass  # Stream first, then hold the real run active during bad packets.
        elif "CATALOG_CASE" in text:
            pass
        elif "DOWNLOAD_TOOL" in text:
            if not tools:
                tool_name = "download_file"
                arguments = {"url": f"http://127.0.0.1:{self.server.server_port}/fixture-tool.txt", "path": str(self.root.parent / "downloaded-tool.txt")}
            else:
                require((self.root.parent / "downloaded-tool.txt").read_text().strip() == "DOWNLOADED_TOOL_ARTIFACT_73", "Downloaded file did not match real HTTP body")
        elif "CONTROLLED_COMPILE" in text:
            if not tools:
                tool_name = "exec" if "exec" in declared else "bash"
                source = 'fn main() { std::fs::write("compiled-marker.txt", "ACTUAL_RUST_COMPILE_73").unwrap(); println!("COMPILED_EXEC_RECEIPT_73"); }'
                source_path = self.root / "fixture-hello.rs"
                binary_path = self.root / ("fixture-hello.exe" if os.name == "nt" else "fixture-hello")
                if os.name == "nt":
                    quoted_source = source.replace("'", "''")
                    ps_source = str(source_path).replace("'", "''")
                    ps_binary = str(binary_path).replace("'", "''")
                    cmd = f"Set-Content -LiteralPath '{ps_source}' -Value '{quoted_source}' -Encoding utf8; & rustc --crate-name fixture_hello '{ps_source}' -o '{ps_binary}'; if ($LASTEXITCODE -ne 0) {{ throw 'rustc failed' }}; & '{ps_binary}'; if ($LASTEXITCODE -ne 0) {{ throw 'compiled executable failed' }}"
                else:
                    cmd = f"printf '%s' {shlex.quote(source)} > {shlex.quote(str(source_path))} && rustc --crate-name fixture_hello {shlex.quote(str(source_path))} -o {shlex.quote(str(binary_path))} && {shlex.quote(str(binary_path))}"
                arguments = {"cmd" if tool_name == "exec" else "command": cmd, "cwd": str(self.root), "timeout": 120}
            else:
                require("COMPILED_EXEC_RECEIPT_73" in json.dumps(tools), "Actual compiled executable stdout was lost")
                require((self.root / "compiled-marker.txt").read_text() == "ACTUAL_RUST_COMPILE_73", "Actual compiled executable did not write expected artifact")
        elif "ABORT_PROCESS" in text or "DISCONNECT_PROCESS" in text:
            if not tools:
                tool_name = "exec" if "exec" in declared else "bash"
                # Parent+child must be canceled; child writes after a delay if it survives.
                marker = self.root / "cancel-after.txt"
                started = self.root / "cancel-started.txt"
                escaped_marker = str(marker).replace("'", "''")
                escaped_started = str(started).replace("'", "''")
                if os.name == "nt":
                    child = f"Start-Sleep -Seconds 4; Set-Content -LiteralPath '{escaped_marker}' -Value survived"
                    encoded = base64.b64encode(child.encode("utf-16-le")).decode()
                    cmd = f"Set-Content -LiteralPath '{escaped_started}' -Value started; & powershell -NoProfile -EncodedCommand {encoded}"
                else:
                    child = f"sleep 4; printf survived > {shlex.quote(str(marker))}"
                    cmd = f"printf started > {shlex.quote(str(started))}; sh -c {shlex.quote(child)}"
                arguments = {"cmd" if tool_name == "exec" else "command": cmd, "cwd": str(self.root)}
        else:
            raise AssertionError("Unknown fixture prompt")
        if tool_name:
            require(tool_name in declared, "Node failed to expose required local tool")
        handler.send_response(200)
        handler.send_header("Content-Type", "text/event-stream")
        handler.send_header("Connection", "close")
        handler.end_headers()
        handler.close_connection = True

        def delta(value, finish=None):
            data = {"id": "fixture", "object": "chat.completion.chunk", "choices": [{"index": 0, "delta": value, "finish_reason": finish}]}
            handler.wfile.write(("data: " + json.dumps(data) + "\n\n").encode())
            handler.wfile.flush()

        if tool_name:
            encoded = json.dumps(arguments)
            if partial:
                encoded = encoded[:len(encoded) // 2]
            delta({"tool_calls": [{"index": 0, "id": tool_id, "type": "function", "function": {"name": tool_name, "arguments": encoded}}]})
            if partial:
                return
            delta({}, "tool_calls")
        else:
            delta({"role": "assistant", "content": "LIVE_FIXTURE_DELTA "})
            self.live_sent.set()
            time.sleep(1.5 if "ACTIVE_ID_COLLISION" in text else 0.5)
            delta({"content": "CONTINUITY_OK" if "CHECK_CONTINUITY" in text else "ARTIFACT_OK"})
            delta({}, "stop")
        handler.wfile.write(b"data: [DONE]\n\n")
        handler.wfile.flush()


def fixture_path(root):
    # Deliberately outside --cwd: cwd is a relative-path base, not an OS sandbox.
    return root.parent / "artifact.txt"


class Proxy:
    """Loopback-only controlled HTTP CONNECT/forward proxy, with safe receipts."""
    def __init__(self):
        self.receipts = []
        self.errors = []
        self.active = []
        proxy = self

        class Handler(socketserver.BaseRequestHandler):
            def handle(self):
                upstream = None
                try:
                    self.request.settimeout(15)
                    headers = bytearray()
                    while b"\r\n\r\n" not in headers:
                        data = self.request.recv(4096)
                        require(data, "Proxy client closed during headers")
                        headers.extend(data)
                        require(len(headers) <= 65536, "Proxy oversized request headers")
                    head, remaining = bytes(headers).split(b"\r\n\r\n", 1)
                    lines = head.split(b"\r\n")
                    method, target, version = lines[0].decode().split(" ", 2)
                    if method == "CONNECT":
                        host, dest_port = target.rsplit(":", 1)
                        path = ""
                    else:
                        parsed = urllib.parse.urlsplit(target)
                        host, dest_port = parsed.hostname, parsed.port or 80
                        path = parsed.path or "/"
                        if parsed.query:
                            path += "?" + parsed.query
                    require(host == "127.0.0.1", "Proxy refused non-fixture destination")
                    proxy.receipts.append({"method": method, "target": target})
                    upstream = socket.create_connection((host, int(dest_port)), timeout=15)
                    proxy.active.extend([self.request, upstream])
                    if method == "CONNECT":
                        self.request.sendall(b"HTTP/1.1 200 Connection Established\r\n\r\n")
                        if remaining:
                            upstream.sendall(remaining)
                    else:
                        lines[0] = f"{method} {path} {version}".encode()
                        # This fixture records one HTTP request per connection.
                        # Close forwarded HTTP connections so provider POSTs
                        # cannot reuse a catalog TCP stream without a receipt.
                        # CONNECT remains a transparent, persistent tunnel.
                        lines = [lines[0]] + [line for line in lines[1:]
                            if line.split(b":", 1)[0].strip().lower()
                            not in (b"connection", b"proxy-connection")]
                        lines.append(b"Connection: close")
                        upstream.sendall(b"\r\n".join(lines) + b"\r\n\r\n" + remaining)
                    end = time.monotonic() + 300
                    while time.monotonic() < end:
                        readable, _, _ = select.select([self.request, upstream], [], [], 1)
                        for source in readable:
                            data = source.recv(65536)
                            if not data:
                                return
                            (upstream if source is self.request else self.request).sendall(data)
                except (BrokenPipeError, ConnectionResetError, ConnectionAbortedError, OSError):
                    pass  # Cancellation and cleanup close active tunnels.
                except Exception as error:
                    proxy.errors.append(str(error))
                finally:
                    if upstream:
                        upstream.close()

        class Server(socketserver.ThreadingTCPServer):
            daemon_threads = True
            allow_reuse_address = True

        self.server = Server(("127.0.0.1", 0), Handler)
        threading.Thread(target=self.server.serve_forever, daemon=True).start()

    @property
    def url(self):
        return f"http://127.0.0.1:{self.server.server_address[1]}"

    def close(self):
        for connection in self.active:
            try:
                connection.shutdown(socket.SHUT_RDWR)
                connection.close()
            except OSError:
                pass
        self.server.shutdown()
        self.server.server_close()


def stop(proc):
    if proc is None or proc.poll() is not None:
        return
    if os.name == "nt":
        subprocess.run(["taskkill", "/PID", str(proc.pid), "/T", "/F"], stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL, timeout=15, check=False)
    else:
        proc.terminate()
    try:
        proc.wait(timeout=15)
    except subprocess.TimeoutExpired:
        proc.kill()
        proc.wait(timeout=5)


def wait_health(url):
    end = time.monotonic() + 15
    while time.monotonic() < end:
        try:
            with urllib.request.urlopen(url + "/healthz", timeout=1) as response:
                if response.status == 200:
                    return
        except OSError:
            time.sleep(0.1)
    raise TimeoutError("Isolated Go Hub did not become healthy")


def validate_loop_events(events, run_id, terminal):
    require(bool(events), "Agent loop emitted no lifecycle events: " + run_id)
    ids = []
    for event in events:
        require(event.get("agent_id") == "local" and event.get("agent_name") == "本机"
                and event.get("agent_role") == "operator", "Local card identity changed: " + run_id)
        require(event.get("task_id") == run_id and event.get("id") and event.get("ts"),
                "Wire event lost fresh identity/task/timestamp: " + run_id)
        ids.append(event["id"])
    require(len(ids) == len(set(ids)), "Wire event IDs were reused: " + run_id)
    ends = [e for e in events if e.get("type") == "agent_end"]
    require(len(ends) == 1, "Run must emit exactly one local end before terminal out: " + run_id)
    expected = ("failed", "canceled") if terminal.get("err") or terminal.get("code", 0) != 0 else ("completed",)
    require(ends[0].get("status") in expected, "Local card terminal disagrees with actual out: " + run_id)
    require(events[-1].get("type") == "agent_end", "Progress followed terminal local end: " + run_id)


def run_frames(ws, run_id, prompt, transcript, cancel=False, timeout=15, model="fixture-model", protocol=None, smart=False):
    request = {"k": "agent-loop", "id": run_id, "msg": prompt, "model": model, "brief": "Real local host acceptance fixture."}
    if protocol:
        request["protocol"] = protocol
    if smart:
        request["smart"] = True
    ws.frame(request)
    events = []
    output = bytearray()
    live = False
    end = time.monotonic() + timeout
    while time.monotonic() < end:
        message = ws.receive(end - time.monotonic())
        if message.get("k") != "dc":
            continue
        frame = json.loads(base64.b64decode(message["d"]))
        transcript.append(frame)
        if frame.get("id") != run_id:
            continue
        if frame.get("k") == "agent-event":
            event = frame.get("event", {})
            events.append(event)
            if event.get("type") == "text_delta" and event.get("output"):
                live = True
            if cancel and event.get("type") == "tool_start":
                # Wait for real process receipt rather than cancel before process spawn.
                time.sleep(0.8)
                ws.frame({"k": "agent-abort", "id": run_id})
                cancel = False
        if frame.get("k") == "out":
            output.extend(base64.b64decode(frame.get("b64", "")))
            if frame.get("done"):
                validate_loop_events(events, run_id, frame)
                # Keep the raw terminal packet in controller.json. Go's contract
                # sends body chunks with done:false and an empty final packet.
                terminal = dict(frame)
                terminal["raw_terminal_b64"] = frame.get("b64", "")
                terminal["b64"] = base64.b64encode(output).decode()
                return terminal, events, live
    raise TimeoutError("Node run did not produce terminal out")


def active_id_collision(ws, root, transcript):
    run_id = "active-collision"
    ws.frame({"k": "agent-loop", "id": run_id, "msg": "ACTIVE_ID_COLLISION", "model": "fixture-model"})
    injected, completed, errors, ends = False, False, 0, 0
    events = []
    output = bytearray()
    end = time.monotonic() + 15
    while time.monotonic() < end:
        message = ws.receive(end - time.monotonic())
        if message.get("k") != "dc":
            continue
        frame = json.loads(base64.b64decode(message["d"]))
        transcript.append(frame)
        if frame.get("id") != run_id:
            continue
        event = frame.get("event", {})
        if frame.get("k") == "agent-event":
            events.append(event)
        if event.get("type") == "text_delta" and not injected:
            injected = True
            # These responses reuse X but must not settle/cancel the active X.
            ws.frame({"k": "put", "id": run_id, "path": str(root / "collision-must-not-exist.txt"), "size": 0})
            ws.frame({"k": "chunk", "id": run_id, "b64": "eA==", "done": True})
            ws.frame({"k": "unsupported-test-packet", "id": run_id})
            ws.frame({"k": "agent-loop", "id": run_id, "msg": "INIT_ARTIFACT", "model": "fixture-model"})
        if event.get("type") == "agent_end":
            ends += 1
            require(event.get("status") == "completed", "Malformed packet canceled or corrupted active run")
            completed = True
        if frame.get("k") == "out":
            if frame.get("done") and (frame.get("err") or frame.get("code", 0) != 0):
                require(not completed, "Active run failed after malformed packets")
                errors += 1
                continue
            output.extend(base64.b64decode(frame.get("b64", "")))
            if frame.get("done"):
                require(injected and completed and errors == 3, "Missing collision denials or active completion")
                require(ends == 1 and any(e.get("type") == "retry" for e in events), "Same-ID packet rejection prematurely ended the local card")
                validate_loop_events(events, run_id, frame)
                require(b"ARTIFACT_OK" in output, "Active stream/body was lost after malformed packets")
                require(not (root / "collision-must-not-exist.txt").exists(), "Duplicate put caused a file effect")
                return
    raise TimeoutError("Active collision run did not complete")


def catalog_variants(controller, hub_port, root_token, fp, fixture, transcript, passed):
    cases = ("fixture-legacy-null", "fixture-legacy-empty", "fixture-legacy-absent", "fixture-protocol-dupes")
    for index, model in enumerate(cases):
        controller.close()
        controller = WS(hub_port, "ctl", root_token)
        # Each owner gets fresh native state: selection is exercised without a
        # prior model-binding mismatch masking the actual catalog behavior.
        owner = f"catalog-fixture-{index}"
        controller.send({"k": "hello", "fp": owner, "relay": True})
        message = controller.until(lambda m: m.get("k") == "status" and any(row.get("fp") == fp and row.get("nid", "").startswith("n") and row.get("state") != "paired" for row in status_rows(m)))
        row = next(row for row in status_rows(message) if row.get("fp") == fp)
        controller.send({"k": "approve", "nid": row["nid"], "relay": True})
        controller.until(lambda m: m.get("k") == "paired")
        controller.until(lambda m: m.get("k") == "dc" and json.loads(base64.b64decode(m["d"])).get("k") == "node-hello")
        fixture.expected_model = model
        before = len(fixture.requests)
        out, _, _ = run_frames(controller, model, "CATALOG_CASE", transcript, model=model)
        require(not out.get("err") and out.get("code", 0) == 0, "Fresh catalog compatibility case failed: " + model)
        require(len(fixture.requests) == before + 1, "Catalog compatibility did not execute exactly one actual model request")
        passed.append(model + "_fresh_native_state")
    return controller


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--binary", type=Path, required=True)
    parser.add_argument("--hub-binary", type=Path, required=True)
    parser.add_argument("--output", type=Path)
    parser.add_argument("--proxy", action="store_true", help="Exercise an actual isolated HTTP CONNECT/forward proxy through node --proxy")
    parser.add_argument("--modern-hub", action="store_true", help="Use current E:/repos/ara/apps/handwave gateway contract instead of the legacy direct-upstream Hub")
    parser.add_argument("--catalog-variants", action="store_true", help="After modern host checks, exercise null/empty/absent/duplicate protocol directories with fresh native owner states")
    parser.add_argument("--max-seconds-total", type=int, default=600, help="Whole local process-chain watchdog (maximum 600 seconds)")
    args = parser.parse_args()
    require(1 <= args.max_seconds_total <= 600, "Local test watchdog must be between 1 and 600 seconds")
    require(not args.catalog_variants or args.modern_hub, "Catalog variants require --modern-hub")
    require(args.binary.is_file() and args.hub_binary.is_file(), "Node/Hub binary missing")
    root = args.output.resolve() if args.output else Path(tempfile.mkdtemp(prefix="handwave-host-"))
    root.mkdir(parents=True, exist_ok=True)
    require(not (root / "result.json").exists(), "Use a fresh evidence directory")
    conf, state, work = root / "hub-conf", root / "node-state", root / "work"
    for path in (conf, state, work):
        path.mkdir()
    fixture = Fixture(work, modern=args.modern_hub)
    proxy = Proxy() if args.proxy else None
    root_token, node_token = secrets.token_hex(24), secrets.token_hex(24)
    hub_port = port()
    hub_url = f"http://127.0.0.1:{hub_port}"
    env = os.environ.copy()
    env["HW_CONF"] = str(conf)
    env["HW_TOKEN"] = node_token
    env["HW_HUB"] = hub_url
    env.pop("HW_PROXY", None)
    # Keep parent machine proxy settings out of isolated Hub/model tests.
    for key in list(env):
        if key.lower() in ("http_proxy", "https_proxy", "all_proxy", "no_proxy"):
            del env[key]
    env["NO_PROXY"] = "127.0.0.1,localhost"
    hub_log, node_log = (root / "hub.log").open("wb"), (root / "node.log").open("wb")
    hub = node = controller = watchdog = None
    transcript, passed = [], []
    started = time.monotonic()
    result = {"scope": "real Rust executable + real local Go Hub + controlled model; production cloud/model NOT RUN", "passed": passed}
    try:
        hub_args = [str(args.hub_binary.resolve()), "hub", "-listen", f"127.0.0.1:{hub_port}", "-token", root_token, "-node_token", node_token]
        if args.modern_hub:
            hub_args.extend(["-gateway", f"http://127.0.0.1:{fixture.server.server_port}", "-gateway_token_env", "HW_TEST_GATEWAY_TOKEN", "-model", "fixture-model", "-protocol", "openai-completions", "-allow_loopback"])
            env["HW_TEST_GATEWAY_TOKEN"] = "fixture-key"
        else:
            hub_args.extend(["-upstream", f"http://127.0.0.1:{fixture.server.server_port}/v1", "-upkey", "fixture-key"])
        hub = subprocess.Popen(hub_args, env=env, stdout=hub_log, stderr=hub_log, cwd=conf)
        wait_health(hub_url)
        node_args = [str(args.binary.resolve()), "--hub", hub_url, "--cwd", str(work), "--state-dir", str(state), "--max-seconds", "300", "--max-calls", "12"]
        if proxy:
            node_args.extend(["--proxy", proxy.url])
        node_env = env.copy()
        node_env.pop("HW_TEST_GATEWAY_TOKEN", None)
        node_env.pop("HW_GATEWAY_TOKEN", None)
        if proxy:
            node_env["NO_PROXY"] = ""
        node = subprocess.Popen(node_args, env=node_env, stdout=node_log, stderr=node_log, cwd=work)
        watchdog = threading.Timer(args.max_seconds_total, lambda: (stop(node), stop(hub)))
        watchdog.daemon = True
        watchdog.start()
        controller = WS(hub_port, "ctl", root_token)
        controller.send({"k": "hello", "fp": "fixture-controller", "relay": True})
        message = controller.until(lambda m: m.get("k") == "status" and any(row.get("state") == "pending" for row in status_rows(m)))
        device = next(row for row in json.loads(message["d"]) if row["state"] == "pending")
        passed.append("join_actual_hub")
        controller.frame({"k": "agent-loop", "id": "unauthorized", "msg": "INIT_ARTIFACT", "model": "fixture-model"})
        time.sleep(0.4)
        require(not fixture.requests and not fixture_path(work).exists(), "Unapproved controller caused tool/model effects")
        passed.append("unapproved_has_no_effect")
        controller.send({"k": "approve", "nid": device["nid"], "relay": True})
        controller.until(lambda m: m.get("k") == "paired")
        controller.until(lambda m: m.get("k") == "dc" and json.loads(base64.b64decode(m["d"])).get("k") == "node-hello")
        passed.append("approve_pair_node_hello")
        out, events, live = run_frames(controller, "init", "INIT_ARTIFACT", transcript, model="default" if args.modern_hub else "fixture-model")
        require(not out.get("err") and out.get("code", 0) == 0, "Initial run failed")
        require(fixture_path(work).read_text().strip() == "HANDWAVE_HOST_ARTIFACT_73", "Actual write artifact missing")
        require(live, "No streaming text_delta before terminal out")
        require(sum(e.get("type") == "tool_end" for e in events) >= 2, "Write/read tool receipts missing")
        require(any(e.get("type") == "agent_end" and e.get("status") == "completed" for e in events), "Completed run event missing")
        passed.extend(["write_read_real_artifact", "live_event_before_terminal"])
        passed.append("all_native_loop_events_keep_local_card_and_terminal_order")
        controller.frame({"k": "agent-task", "id": "wire-task-identity", "task": {
            "objective": "actual task identity fixture", "steps": [
                {"id": "task-write", "kind": "write_file", "title": "write task artifact", "path": "wire-task-artifact.txt", "content": "WIRE_TASK_IDENTITY_73"},
                {"id": "task-read", "kind": "read_file", "title": "read task artifact", "path": "wire-task-artifact.txt"}]}})
        task_events = []
        while True:
            packet = controller.until(lambda m: m.get("k") == "dc")
            frame = json.loads(base64.b64decode(packet["d"]))
            transcript.append(frame)
            if frame.get("id") != "wire-task-identity":
                continue
            if frame.get("k") == "agent-event":
                event = frame.get("event", {})
                require(event.get("task_id") == "wire-task-identity" and event.get("agent_id") != "local", "agent-task identity was overwritten by local agent-loop card")
                task_events.append(event)
            if frame.get("k") == "out" and frame.get("done"):
                require(not frame.get("err") and frame.get("code", 0) == 0, "Actual wire agent-task failed")
                break
        require((work / "wire-task-artifact.txt").read_text() == "WIRE_TASK_IDENTITY_73"
                and task_events[-1].get("type") == "task_finish"
                and task_events[-1].get("status") == "completed", "Task artifact/lifecycle missing")
        passed.append("agent_task_preserves_distinct_task_identity_and_real_effect")
        large_log = root / "outside-log.txt"
        large_log.write_text("".join(f"record {i:05} 中文日志内容\r\n" for i in range(1, 40001)) + "READ_TOOLS_EOF_73", encoding="utf-8")
        search_root = work / "native-search"
        (search_root / "nested").mkdir(parents=True)
        (search_root / "nested" / "match.txt").write_text("first\nNATIVE NEEDLE.* exact\n", encoding="utf-8")
        (search_root / "target").mkdir()
        (search_root / "target" / "excluded.txt").write_text("native needle.*", encoding="utf-8")
        readonly_hash = hashlib.sha256(large_log.read_bytes()).hexdigest()
        out, _, _ = run_frames(controller, "read-selection", "READ_SELECTION_TOOL", transcript)
        require(not out.get("err") and out.get("code", 0) == 0, "Actual large-log pagination/tail task failed")
        require(hashlib.sha256(large_log.read_bytes()).hexdigest() == readonly_hash, "Read selection modified source file")
        passed.append("actual_model_read_file_range_and_tail_large_log")
        out, _, _ = run_frames(controller, "native-search", "NATIVE_SEARCH_TOOL", transcript)
        require(not out.get("err") and out.get("code", 0) == 0, "Actual native literal search task failed")
        require((search_root / "nested" / "match.txt").read_text() == "first\nNATIVE NEEDLE.* exact\n", "Search modified source file")
        passed.append("actual_model_native_search_schema_effects_and_partial_coverage")
        if args.modern_hub:
            require(fixture.catalog_requests > 0, "Modern node never read the actual Gateway catalog")
            passed.append("modern_default_resolved_from_grant_and_directory")
            before = len(fixture.requests)
            out, _, _ = run_frames(controller, "wrong-protocol", "CHECK_CONTINUITY", transcript, protocol="anthropic-messages")
            require(out.get("err") or out.get("code", 0) != 0, "Unadvertised/unsupported protocol was silently accepted")
            require(len(fixture.requests) == before, "Unsupported protocol silently fell back to the chat wire")
            passed.append("unsupported_protocol_rejected_without_model_call")
            for case_id, selected in (("missing-model", "fixture-missing"), ("ambiguous-model", "fixture-ambiguous")):
                before = len(fixture.requests)
                out, _, _ = run_frames(controller, case_id, "CHECK_CONTINUITY", transcript, model=selected)
                require(out.get("err") or out.get("code", 0) != 0, "Missing/ambiguous model was silently accepted")
                require(len(fixture.requests) == before, "Invalid selection silently fell back to a model request")
                passed.append(case_id + "_rejected_without_model_call")
        out, _, _ = run_frames(controller, "download", "DOWNLOAD_TOOL", transcript)
        require(not out.get("err") and out.get("code", 0) == 0 and fixture.downloads, "Real HTTP download failed")
        passed.append("actual_file_download_outside_cwd")
        out, _, _ = run_frames(controller, "compile", "CONTROLLED_COMPILE", transcript, timeout=90)
        require(not out.get("err") and out.get("code", 0) == 0, "Actual Rust compilation/executable task failed")
        passed.append("actual_rust_compilation_and_executable_artifact")
        out, _, _ = run_frames(controller, "continue", "CHECK_CONTINUITY", transcript)
        require(not out.get("err") and b"CONTINUITY_OK" in base64.b64decode(out.get("b64", "")), "Same-process continuation failed")
        passed.append("same_process_native_continuation")
        # Reconnect controller after restart: Hub node join auto-pair has relay=false,
        # while controller hello preserves relay=true for the standalone relay client.
        controller.close()
        controller = None
        stop(node)
        node = subprocess.Popen(node_args, env=node_env, stdout=node_log, stderr=node_log, cwd=work)
        controller = WS(hub_port, "ctl", root_token)
        controller.send({"k": "status"})
        message = controller.until(lambda m: m.get("k") == "status" and any(row.get("fp") == device["fp"] and row.get("nid", "").startswith("n") for row in status_rows(m)))
        controller.send({"k": "hello", "fp": "fixture-controller", "relay": True})
        controller.until(lambda m: m.get("k") == "paired")
        controller.until(lambda m: m.get("k") == "dc" and json.loads(base64.b64decode(m["d"])).get("k") == "node-hello")
        out, _, _ = run_frames(controller, "restart", "CHECK_CONTINUITY", transcript)
        require(not out.get("err") and b"CONTINUITY_OK" in base64.b64decode(out.get("b64", "")), "Restart lost native session")
        passed.append("restart_native_continuation")
        before = len(fixture.requests)
        out, _, _ = run_frames(controller, "init", "INIT_ARTIFACT", transcript)
        require(out.get("err") or out.get("code", 0) != 0, "Repeated run ID was accepted")
        require(len(fixture.requests) == before, "Repeated run ID replayed the model/tool loop")
        passed.append("duplicate_run_rejected_without_replay")
        active_id_collision(controller, work, transcript)
        passed.append("same_id_bad_packets_do_not_settle_active_run")
        out, events, _ = run_frames(controller, "abort", "ABORT_PROCESS", transcript, cancel=True)
        require(out.get("err") or out.get("code", 0) != 0, "Canceled run reported success")
        require((work / "cancel-started.txt").exists(), "Cancellation test did not spawn real command")
        time.sleep(4.5)
        require(not (work / "cancel-after.txt").exists(), "Canceled child process survived")
        passed.append("abort_kills_real_process_tree")
        (work / "cancel-started.txt").unlink()
        controller.frame({"k": "agent-loop", "id": "disconnect", "msg": "DISCONNECT_PROCESS", "model": "fixture-model"})
        message = controller.until(lambda m: m.get("k") == "dc" and (lambda f: f.get("id") == "disconnect" and f.get("event", {}).get("type") == "tool_start")(json.loads(base64.b64decode(m["d"]))))
        transcript.append(json.loads(base64.b64decode(message["d"])))
        deadline = time.monotonic() + 3
        while not (work / "cancel-started.txt").exists() and time.monotonic() < deadline:
            time.sleep(0.05)
        require((work / "cancel-started.txt").exists(), "Disconnect test did not spawn actual process")
        before = len(fixture.requests)
        controller.close()
        controller = None
        time.sleep(4.5)
        require(not (work / "cancel-after.txt").exists(), "Controller disconnect left child process alive")
        require(len(fixture.requests) == before, "Disconnected run automatically replayed model work")
        controller = WS(hub_port, "ctl", root_token)
        controller.send({"k": "hello", "fp": "fixture-controller", "relay": True})
        controller.until(lambda m: m.get("k") == "paired")
        controller.until(lambda m: m.get("k") == "dc" and json.loads(base64.b64decode(m["d"])).get("k") == "node-hello")
        out, _, _ = run_frames(controller, "after-disconnect", "CHECK_CONTINUITY", transcript)
        require(not out.get("err") and b"CONTINUITY_OK" in base64.b64decode(out.get("b64", "")), "Settled disconnected run lost native session")
        passed.append("disconnect_cancels_without_replay_and_session_survives")
        out, _, _ = run_frames(controller, "partial", "PARTIAL_TOOL", transcript)
        require(out.get("err") or out.get("code", 0) != 0, "Partial provider stream reported success")
        require(not fixture.partial_path.exists(), "Partial tool arguments caused file effect")
        passed.append("partial_stream_rejected_without_tool_effect")
        if args.catalog_variants:
            controller = catalog_variants(controller, hub_port, root_token, device["fp"], fixture, transcript, passed)
        if proxy:
            require(not proxy.errors, "Proxy fixture errors: " + "; ".join(proxy.errors))
            require(any(r["method"] == "CONNECT" for r in proxy.receipts), "WebSocket never traversed actual CONNECT proxy")
            require(any(r["method"] == "POST" and r["target"].endswith("/v1/chat/completions") for r in proxy.receipts), "Model HTTP never traversed actual forward proxy")
            require(any(r["method"] == "GET" and r["target"].endswith("/fixture-tool.txt") for r in proxy.receipts), "File download never traversed actual forward proxy")
            passed.append("actual_proxy_for_websocket_and_model_http")
        require(not fixture.errors, "Controlled upstream assertions failed: " + "; ".join(fixture.errors))
        result["status"] = "PASS"
    except Exception as error:
        result["status"] = "FAIL"
        result["error"] = f"{type(error).__name__}: {error}"
        raise
    finally:
        if watchdog:
            watchdog.cancel()
        if controller:
            controller.close()
        stop(node)
        stop(hub)
        fixture.server.shutdown()
        if proxy:
            proxy.close()
            (root / "proxy-receipts.json").write_text(json.dumps(proxy.receipts, indent=2), encoding="utf-8")
        hub_log.close()
        node_log.close()
        result.update({"elapsed_seconds": round(time.monotonic() - started, 3), "model_requests": len(fixture.requests), "catalog_requests": fixture.catalog_requests, "modern_hub": args.modern_hub, "binary_sha256": hashlib.sha256(args.binary.read_bytes()).hexdigest(), "hub_sha256": hashlib.sha256(args.hub_binary.read_bytes()).hexdigest(), "fixture_errors": fixture.errors})
        (root / "result.json").write_text(json.dumps(result, indent=2), encoding="utf-8")
        (root / "controller.json").write_text(json.dumps(transcript, indent=2), encoding="utf-8")
        (root / "model-requests.json").write_text(json.dumps(fixture.requests, indent=2), encoding="utf-8")
        (root / "route-requests.json").write_text(json.dumps(fixture.route_requests, indent=2), encoding="utf-8")
        print(json.dumps({"status": result.get("status"), "passed": passed, "evidence": str(root)}, indent=2))


if __name__ == "__main__":
    main()
