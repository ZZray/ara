"""Actual Node reconnection faults over loopback, no model or production access.

Uses real 15s/15s heartbeat and 20s handshake deadlines; no test-only runtime
timings. Each process has a fresh private state directory. Cases run concurrently
and have a 70-second watchdog. This is not power-loss or physical-network proof.
"""
from __future__ import annotations
import argparse
import base64
import concurrent.futures
import hashlib
import json
import os
from pathlib import Path
import re
import socket
import socketserver
import struct
import subprocess
import tempfile
import threading
import time

from verify_host import require, stop


def read_exact(sock, count):
    result = bytearray()
    while len(result) < count:
        part = sock.recv(count - len(result))
        if not part:
            raise EOFError()
        result.extend(part)
    return bytes(result)


class FaultHub:
    def __init__(self, mode):
        self.mode, self.started = mode, time.monotonic()
        self.stopping = threading.Event()
        self.lock = threading.Lock()
        self.attempts, self.joins, self.pings, self.errors, self.unexpected_http = [], [], [], [], []
        hub = self
        class Handler(socketserver.BaseRequestHandler):
            def handle(self):
                try:
                    self.request.settimeout(60)
                    header = bytearray()
                    while not header.endswith(b"\r\n\r\n"):
                        header.extend(read_exact(self.request, 1))
                        require(len(header) < 65536, "oversized fixture headers")
                    path = header.split(b"\r\n", 1)[0].split()[1].decode()
                    if not path.startswith('/ws?'):
                        hub.unexpected_http.append(path)
                        return
                    with hub.lock:
                        hub.attempts.append(time.monotonic() - hub.started)
                        number = len(hub.attempts)
                    if hub.mode == 'handshake_stall':
                        hub.stopping.wait(65)
                        return
                    if hub.mode == 'denied_once':
                        self.request.sendall(b'HTTP/1.1 401 Unauthorized\r\nContent-Length: 0\r\nConnection: close\r\n\r\n')
                        return
                    lines = header.decode().split('\r\n')
                    key = next(line.split(':', 1)[1].strip() for line in lines if line.lower().startswith('sec-websocket-key:'))
                    accept = base64.b64encode(hashlib.sha1((key + '258EAFA5-E914-47DA-95CA-C5AB0DC85B11').encode()).digest()).decode()
                    self.request.sendall(('HTTP/1.1 101 Switching Protocols\r\nUpgrade: websocket\r\nConnection: Upgrade\r\nSec-WebSocket-Protocol: handwave\r\nSec-WebSocket-Accept: ' + accept + '\r\n\r\n').encode())
                    while not hub.stopping.is_set():
                        head = read_exact(self.request, 2)
                        opcode, size = head[0] & 15, head[1] & 127
                        if size == 126: size = struct.unpack('!H', read_exact(self.request, 2))[0]
                        elif size == 127: size = struct.unpack('!Q', read_exact(self.request, 8))[0]
                        require(size <= 1024 * 1024, 'unexpected large fixture frame')
                        mask = read_exact(self.request, 4) if head[1] & 128 else None
                        payload = read_exact(self.request, size)
                        if mask: payload = bytes(value ^ mask[i % 4] for i, value in enumerate(payload))
                        if opcode == 8: return
                        if opcode == 9:
                            hub.pings.append({'connection': number, 'at': time.monotonic() - hub.started})
                            if hub.mode in ('healthy', 'abrupt'):
                                self.request.sendall(bytes([0x8A, len(payload)]) + payload)
                            elif hub.mode == 'wrong_pong':
                                self.request.sendall(b'\x8a\x05wrong')
                        elif opcode == 1:
                            message = json.loads(payload)
                            require(message.get('k') == 'join', 'unexpected application replay: ' + str(message.get('k')))
                            hub.joins.append({'connection': number, 'at': time.monotonic() - hub.started})
                            if hub.mode == 'abrupt' and number == 1:
                                return
                except (EOFError, ConnectionResetError, BrokenPipeError, socket.timeout):
                    pass
                except Exception as error:
                    hub.errors.append(type(error).__name__ + ': ' + str(error))
        class Server(socketserver.ThreadingTCPServer):
            allow_reuse_address, daemon_threads = True, True
        self.server = Server(('127.0.0.1', 0), Handler)
        threading.Thread(target=self.server.serve_forever, daemon=True).start()

    def close(self):
        self.stopping.set()
        self.server.shutdown()
        self.server.server_close()


def run_case(binary, root, mode):
    directory = root / mode
    directory.mkdir()
    hub = FaultHub(mode)
    env = os.environ.copy()
    for key in list(env):
        if key.lower() in ('http_proxy', 'https_proxy', 'all_proxy', 'no_proxy', 'hw_proxy', 'hw_hub', 'hw_token', 'hw_node_state'):
            del env[key]
    env['NO_PROXY'] = '*'
    logpath = directory / 'stderr.log'
    result = {'mode': mode}
    started = time.monotonic()
    process = None
    try:
        with logpath.open('wb') as stderr, (directory / 'stdout.log').open('wb') as stdout:
            args = [str(binary), '--hub', 'http://127.0.0.1:' + str(hub.server.server_address[1]), '--token', 'loopback-reconnect-fixture', '--state-dir', str(directory / 'state'), '--cwd', str(directory)]
            if mode == 'denied_once': args.append('--once')
            process = subprocess.Popen(args, env=env, stdout=stdout, stderr=stderr)
            while time.monotonic() - started < 70:
                log = logpath.read_text(encoding='utf-8', errors='replace')
                delays = [float(x) for x in re.findall(r'reconnecting in ([0-9.]+)s', log)]
                if mode == 'denied_once' and process.poll() is not None:
                    require(process.returncode != 0, '--once hid failed authentication as success')
                    require(len(hub.attempts) == 1 and not delays, '--once unexpectedly reconnected')
                    break
                require(process.poll() is None, 'Node exited unexpectedly: ' + log[-1500:])
                if mode in ('blackhole', 'wrong_pong') and len(hub.joins) >= 2:
                    require('heartbeat timed out' in log and hub.pings, 'half-open was not detected by heartbeat')
                    require(delays and 1 <= delays[0] <= 1.25, 'unproven connection reset backoff')
                    break
                if mode == 'handshake_stall' and len(delays) >= 2:
                    require(1 <= delays[0] <= 1.25 and 2 <= delays[1] <= 2.5, 'handshake timeout incorrectly reset exponential backoff')
                    require(not hub.joins, 'stalled handshake was considered connected')
                    break
                if mode == 'healthy' and time.monotonic() - started >= 36:
                    require(len(hub.joins) == 1 and len(hub.pings) >= 2, 'healthy Pong connection was lost or never probed')
                    require(not delays, 'healthy connection spuriously reconnected')
                    break
                if mode == 'abrupt' and len(hub.joins) >= 2:
                    require(delays and 1 <= delays[0] <= 1.25, 'abrupt disconnect did not back off and reconnect')
                    break
                time.sleep(.05)
            else:
                raise TimeoutError('bounded reconnect fixture did not complete')
            require(not hub.errors and not hub.unexpected_http, 'fixture error or unexpected HTTP effect')
            result.update(ok=True, attempts=hub.attempts, joins=hub.joins, pings=hub.pings, backoff_seconds=delays, model_http_effects=0)
    except Exception as error:
        result.update(ok=False, error_type=type(error).__name__, error=str(error))
    finally:
        if process: stop(process)
        hub.close()
        result['seconds'] = round(time.monotonic() - started, 3)
        (directory / 'result.json').write_text(json.dumps(result, indent=2), encoding='utf-8')
    return result


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--binary', type=Path, required=True)
    parser.add_argument('--output', type=Path)
    args = parser.parse_args()
    root = args.output or Path(tempfile.mkdtemp(prefix='handwave-reconnect-'))
    root.mkdir(parents=True, exist_ok=True)
    require(not list(root.iterdir()), 'use a fresh empty evidence directory')
    modes = ['blackhole', 'wrong_pong', 'healthy', 'handshake_stall', 'abrupt', 'denied_once']
    with concurrent.futures.ThreadPoolExecutor(max_workers=len(modes)) as pool:
        results = list(pool.map(lambda mode: run_case(args.binary.resolve(), root, mode), modes))
    result = {'ok': all(r['ok'] for r in results), 'binary_sha256': hashlib.sha256(args.binary.read_bytes()).hexdigest(), 'cases': results, 'limits': ['loopback fault server; not production, real models or physical networks', 'no claim of OS send-buffer saturation or physical power-loss recovery']}
    (root / 'result.json').write_text(json.dumps(result, indent=2), encoding='utf-8')
    print(json.dumps(result))
    require(result['ok'], 'reconnect cases failed; inspect retained results')


if __name__ == '__main__': main()
