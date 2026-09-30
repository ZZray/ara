#!/usr/bin/env python3
"""Exercise the real RPC reader/writer through a child process's OS pipes.

The probe is a transport-only executable; this runner neither dispatches Agent
commands nor contacts a model or product host. Receipts contain byte hashes and
observable ordering, without embedding multi-megabyte payloads.
"""

from __future__ import annotations

import argparse
import base64
import hashlib
import json
import math
import os
from pathlib import Path
import queue
import subprocess
import sys
import threading
import time
from typing import Any, Callable, TypeVar


MAX_FRAME_BYTES = 1024 * 1024
CHUNK_PAYLOAD_BYTES = 256 * 1024
T = TypeVar("T")


class HarnessError(Exception):
    pass


def require(condition: bool, message: str) -> None:
    if not condition:
        raise HarnessError(message)


def digest(data: bytes) -> dict[str, Any]:
    return {"byteLength": len(data), "sha256": hashlib.sha256(data).hexdigest()}


def file_digest(path: Path) -> dict[str, Any]:
    hash_state = hashlib.sha256()
    length = 0
    with path.open("rb") as source:
        for block in iter(lambda: source.read(1024 * 1024), b""):
            length += len(block)
            hash_state.update(block)
    return {"byteLength": length, "sha256": hash_state.hexdigest()}


def compact_json(value: Any) -> bytes:
    return json.dumps(value, ensure_ascii=False, separators=(",", ":"), allow_nan=False).encode("utf-8")


def sampled_peak_memory(process: subprocess.Popen[bytes]) -> dict[str, Any] | None:
    """Read an OS peak counter while the process is live, when available."""
    if process.poll() is not None:
        return None
    if sys.platform.startswith("linux"):
        try:
            for line in Path(f"/proc/{process.pid}/status").read_text(encoding="ascii").splitlines():
                if line.startswith("VmHWM:"):
                    return {"source": "proc:VmHWM", "bytes": int(line.split()[1]) * 1024}
        except (OSError, ValueError, IndexError):
            return None
        return None
    if os.name == "nt":
        import ctypes
        from ctypes import wintypes

        class ProcessMemoryCounters(ctypes.Structure):
            _fields_ = [
                ("cb", wintypes.DWORD),
                ("page_fault_count", wintypes.DWORD),
                ("peak_working_set_size", ctypes.c_size_t),
                ("working_set_size", ctypes.c_size_t),
                ("quota_peak_paged_pool_usage", ctypes.c_size_t),
                ("quota_paged_pool_usage", ctypes.c_size_t),
                ("quota_peak_nonpaged_pool_usage", ctypes.c_size_t),
                ("quota_nonpaged_pool_usage", ctypes.c_size_t),
                ("pagefile_usage", ctypes.c_size_t),
                ("peak_pagefile_usage", ctypes.c_size_t),
            ]

        kernel32 = ctypes.WinDLL("kernel32", use_last_error=True)
        psapi = ctypes.WinDLL("psapi", use_last_error=True)
        kernel32.OpenProcess.argtypes = [wintypes.DWORD, wintypes.BOOL, wintypes.DWORD]
        kernel32.OpenProcess.restype = wintypes.HANDLE
        kernel32.CloseHandle.argtypes = [wintypes.HANDLE]
        psapi.GetProcessMemoryInfo.argtypes = [wintypes.HANDLE, ctypes.POINTER(ProcessMemoryCounters), wintypes.DWORD]
        psapi.GetProcessMemoryInfo.restype = wintypes.BOOL
        handle = kernel32.OpenProcess(0x0410, False, process.pid)
        if not handle:
            return None
        try:
            counters = ProcessMemoryCounters()
            counters.cb = ctypes.sizeof(counters)
            if psapi.GetProcessMemoryInfo(handle, ctypes.byref(counters), counters.cb):
                return {"source": "psapi:PeakWorkingSetSize", "bytes": counters.peak_working_set_size}
        finally:
            kernel32.CloseHandle(handle)
    return None


def queued_stdout_bytes(process: subprocess.Popen[bytes]) -> int | None:
    """Peek at queued pipe bytes without draining stdout, where supported."""
    if process.stdout is None or process.stdout.closed:
        return None
    if sys.platform.startswith("linux"):
        import array
        import fcntl
        import termios

        count = array.array("i", [0])
        try:
            fcntl.ioctl(process.stdout.fileno(), termios.FIONREAD, count, True)
            return count[0]
        except OSError:
            return None
    if os.name == "nt":
        import ctypes
        import msvcrt
        from ctypes import wintypes

        kernel32 = ctypes.WinDLL("kernel32", use_last_error=True)
        kernel32.PeekNamedPipe.argtypes = [
            wintypes.HANDLE,
            ctypes.c_void_p,
            wintypes.DWORD,
            ctypes.POINTER(wintypes.DWORD),
            ctypes.POINTER(wintypes.DWORD),
            ctypes.POINTER(wintypes.DWORD),
        ]
        kernel32.PeekNamedPipe.restype = wintypes.BOOL
        available = wintypes.DWORD()
        handle = wintypes.HANDLE(msvcrt.get_osfhandle(process.stdout.fileno()))
        if kernel32.PeekNamedPipe(handle, None, 0, None, ctypes.byref(available), None):
            return available.value
    return None


class ProbeSession:
    def __init__(self, executable: Path, timeout: float):
        self.process = subprocess.Popen(
            [str(executable)], stdin=subprocess.PIPE, stdout=subprocess.PIPE, stderr=subprocess.PIPE, bufsize=0
        )
        self.deadline = time.monotonic() + timeout
        self.stdout_hash = hashlib.sha256()
        self.stdout_bytes = 0
        self.lines: list[dict[str, Any]] = []
        self.stderr: bytes | None = None
        self.exit_code: int | None = None

    def __enter__(self) -> ProbeSession:
        return self

    def __exit__(self, _kind: object, _value: object, _traceback: object) -> None:
        if self.process.poll() is None:
            self.process.kill()
        try:
            self.process.wait(timeout=3)
        except subprocess.TimeoutExpired:
            pass
        for pipe in (self.process.stdin, self.process.stdout, self.process.stderr):
            if pipe is not None and not pipe.closed:
                pipe.close()

    def remaining(self) -> float:
        remaining = self.deadline - time.monotonic()
        if remaining <= 0:
            raise HarnessError("case deadline expired")
        return remaining

    def bounded(self, label: str, operation: Callable[[], T]) -> T:
        """Watch a blocking pipe operation without relying on select (Windows pipes)."""
        completed: queue.Queue[tuple[bool, object]] = queue.Queue(maxsize=1)

        def worker() -> None:
            try:
                completed.put((True, operation()))
            except BaseException as error:  # Propagate pipe errors to the case.
                completed.put((False, error))

        thread = threading.Thread(target=worker, name=f"rpc-{label}", daemon=True)
        thread.start()
        try:
            success, result = completed.get(timeout=self.remaining())
        except queue.Empty as error:
            self.process.kill()
            raise HarnessError(f"timed out waiting for {label}") from error
        if not success:
            raise HarnessError(f"{label} failed: {result}")
        return result  # type: ignore[return-value]

    def write(self, data: bytes) -> None:
        require(self.process.stdin is not None and not self.process.stdin.closed, "stdin is closed")

        def write_all() -> None:
            position = 0
            while position < len(data):
                written = self.process.stdin.write(data[position:])  # type: ignore[union-attr]
                if written is None or written == 0:
                    raise OSError("stdin write returned zero")
                position += written

        self.bounded("stdin write", write_all)

    def command(self, value: Any, *, ending: bytes = b"\n") -> None:
        self.write(compact_json(value) + ending)

    def close_stdin(self) -> None:
        if self.process.stdin is not None and not self.process.stdin.closed:
            self.bounded("stdin close", self.process.stdin.close)

    def read_line(self, expected_kind: str | None = None) -> tuple[bytes, Any]:
        require(self.process.stdout is not None and not self.process.stdout.closed, "stdout is closed")
        line = self.bounded("stdout line", self.process.stdout.readline)
        require(isinstance(line, bytes) and line.endswith(b"\n"), "stdout ended before a complete JSONL frame")
        require(len(line) <= MAX_FRAME_BYTES, "physical frame exceeds one MiB")
        try:
            value = json.loads(line)
        except (UnicodeDecodeError, json.JSONDecodeError) as error:
            raise HarnessError(f"invalid stdout JSONL: {error}") from error
        require(isinstance(value, dict), "probe output must be an object")
        if expected_kind is not None:
            require(value.get("type") == expected_kind, f"expected {expected_kind}, saw {value.get('type')!r}")
        self.stdout_hash.update(line)
        self.stdout_bytes += len(line)
        receipt: dict[str, Any] = {"ordinal": len(self.lines), **digest(line), "type": value.get("type")}
        if value.get("type") == "rpc_chunk":
            receipt.update({"chunkId": value.get("chunkId"), "index": value.get("index"), "count": value.get("count")})
        self.lines.append(receipt)
        return line, value

    def ready(self) -> None:
        expected = (
            b'{"type":"ready","protocolVersion":1,"supportedProtocolVersions":[1,2],'
            b'"maxFrameBytes":1048576,"maxReassembledFrameBytes":67108864}\n'
        )
        line, _value = self.read_line("ready")
        require(line == expected, "initial ready frame bytes differ")

    def negotiate_v2(self) -> None:
        self.command({"op": "negotiate", "version": 2})
        line, _value = self.read_line("response")
        expected = b'{"type":"response","command":"negotiate_protocol","success":true,"data":{"protocolVersion":2}}\n'
        require(line == expected, "negotiate response did not precede v2 output as an exact v1 line")

    def wait_exit(self, expected_zero: bool) -> int:
        try:
            code = self.process.wait(timeout=self.remaining())
        except subprocess.TimeoutExpired as error:
            self.process.kill()
            raise HarnessError("child did not exit within case deadline") from error
        require((code == 0) == expected_zero, f"unexpected child exit code {code}")
        self.exit_code = code
        if self.process.stdout is not None and not self.process.stdout.closed:
            trailing = self.process.stdout.read()
            require(trailing == b"", f"unexpected trailing stdout after expected frames: {digest(trailing)}")
        require(self.process.stderr is not None, "stderr pipe missing")
        self.stderr = self.process.stderr.read()
        return code

    def receipt(self) -> dict[str, Any]:
        return {
            "pid": self.process.pid,
            "exitCode": self.exit_code,
            "stdout": {"byteLength": self.stdout_bytes, "sha256": self.stdout_hash.hexdigest()},
            "physicalLines": self.lines,
            "stderr": digest(self.stderr if self.stderr is not None else b""),
        }


def expect_echo(session: ProbeSession, value: Any) -> None:
    line, parsed = session.read_line("probe_echo")
    expected = b'{"type":"probe_echo","value":' + compact_json(value) + b"}\n"
    require(line == expected, "echo bytes or field order differ")
    require(parsed == {"type": "probe_echo", "value": value}, "echo value differs")


def input_recovery(executable: Path) -> dict[str, Any]:
    with ProbeSession(executable, 30) as session:
        session.ready()
        emoji = b'{"op":"echo","value":"\xf0\x9f\x98\x80"}\n'
        start = emoji.index(b"\xf0")
        for fragment in (emoji[: start + 1], emoji[start + 1 : start + 3], emoji[start + 3 :]):
            session.write(fragment)
        expect_echo(session, "😀")

        session.write(b'bad\n{"op":"echo","value":"after-error"}\n')
        _line, error_frame = session.read_line("response")
        require(error_frame.get("command") == "parse" and error_frame.get("success") is False, "malformed line was not reported")
        require(str(error_frame.get("error", "")).startswith("Failed to parse command:"), "parse error lost OMP prefix")
        expect_echo(session, "after-error")

        session.write("\ufeff\u00a0".encode("utf-8") + b'{"op":"echo","value":"trimmed"}' + "\u2029".encode("utf-8") + b"\r\n")
        expect_echo(session, "trimmed")
        session.command({"op": "echo", "value": "eof-tail"}, ending=b"")
        session.close_stdin()
        expect_echo(session, "eof-tail")
        session.wait_exit(expected_zero=True)
        result = session.receipt()
        result["checks"] = ["fragmented UTF-8", "malformed recovery", "CRLF and JavaScript trim", "unterminated EOF tail"]
        return result


def read_chunks(session: ProbeSession, frame: dict[str, Any], first: tuple[bytes, Any] | None = None) -> dict[str, Any]:
    expected = compact_json(frame)
    _first_line, first_chunk = first if first is not None else session.read_line("rpc_chunk")
    require(first_chunk.get("type") == "rpc_chunk", "first physical line is not a chunk")
    count = first_chunk.get("count")
    require(isinstance(count, int) and count > 1, "large v2 frame was not chunked")
    require(count == math.ceil(len(expected) / CHUNK_PAYLOAD_BYTES), "chunk count differs from payload length")
    chunk_id = first_chunk.get("chunkId")
    require(isinstance(chunk_id, str) and chunk_id.startswith("rpc-"), "invalid chunk ID")
    payload = bytearray()
    for ordinal in range(count):
        chunk = first_chunk if ordinal == 0 else session.read_line("rpc_chunk")[1]
        require(chunk.get("type") == "rpc_chunk", "non-chunk interleaved within logical frame")
        require(chunk.get("chunkId") == chunk_id and chunk.get("index") == ordinal, "chunk ID or order differs")
        require(chunk.get("count") == count and chunk.get("byteLength") == len(expected), "chunk metadata differs")
        try:
            piece = base64.b64decode(chunk["data"], validate=True)
        except (KeyError, ValueError, base64.binascii.Error) as error:
            raise HarnessError(f"invalid chunk base64: {error}") from error
        require(len(piece) == min(CHUNK_PAYLOAD_BYTES, len(expected) - ordinal * CHUNK_PAYLOAD_BYTES), "chunk payload size differs")
        payload.extend(piece)
    require(bytes(payload) == expected, "reassembled physical chunks differ from the logical frame bytes")
    return {"chunkId": chunk_id, "count": count, "logicalFrame": digest(expected)}


def large_response(size: int) -> dict[str, Any]:
    return {"type": "response", "id": "pipe", "command": "get_state", "success": True, "data": {"payload": "x" * size}}


def streaming_before_eof(executable: Path) -> dict[str, Any]:
    with ProbeSession(executable, 45) as session:
        session.ready()
        session.negotiate_v2()
        frame = large_response(MAX_FRAME_BYTES + 12345)
        session.command({"op": "emit", "frame": frame})
        first = session.read_line("rpc_chunk")
        require(session.process.stdin is not None and not session.process.stdin.closed, "stdin closed before first output")
        require(session.process.poll() is None, "child exited before stdin EOF")
        memory = sampled_peak_memory(session.process)
        chunks = read_chunks(session, frame, first)
        session.command({"op": "echo", "value": "after-chunks"})
        expect_echo(session, "after-chunks")
        session.command({"op": "exit"})
        session.close_stdin()
        session.wait_exit(expected_zero=True)
        result = session.receipt()
        result["streaming"] = {"firstChunkBeforeStdinEof": True, "sampledPeakMemory": memory, **chunks}
        return result


def paused_stdout_backpressure(executable: Path) -> dict[str, Any]:
    with ProbeSession(executable, 60) as session:
        session.ready()
        session.negotiate_v2()
        frame = large_response(4 * MAX_FRAME_BYTES)
        session.command({"op": "emit", "frame": frame})
        session.command({"op": "echo", "value": "after-drain"})
        session.command({"op": "exit"})
        session.close_stdin()
        first = session.read_line("rpc_chunk")
        pause_seconds = 0.75
        require(session.remaining() > pause_seconds, "no time for backpressure observation")
        time.sleep(pause_seconds)
        alive_during_pause = session.process.poll() is None
        require(alive_during_pause, "child exited while large stdout remained undrained")
        queued_during_pause = queued_stdout_bytes(session.process)
        if queued_during_pause is not None:
            require(queued_during_pause > 0, "stdout pipe did not contain pending bytes during pause")
        memory = sampled_peak_memory(session.process)
        chunks = read_chunks(session, frame, first)
        expect_echo(session, "after-drain")
        session.wait_exit(expected_zero=True)
        result = session.receipt()
        result["backpressure"] = {
            "firstChunkObservedBeforePause": True,
            "pauseSeconds": pause_seconds,
            "childAliveWithStdoutUndrained": alive_during_pause,
            "queuedStdoutBytesDuringPause": queued_during_pause,
            "echoObservedOnlyAfterChunkDrain": True,
            "sampledPeakMemory": memory,
            **chunks,
        }
        return result


def broken_output_pipe(executable: Path) -> dict[str, Any]:
    with ProbeSession(executable, 15) as session:
        session.ready()
        require(session.process.stdout is not None, "stdout pipe missing")
        session.process.stdout.close()
        session.command({"op": "echo", "value": "broken-output"})
        session.close_stdin()
        session.wait_exit(expected_zero=False)
        require(session.stderr is not None and b"BrokenPipe" in session.stderr, "child did not report BrokenPipe")
        result = session.receipt()
        result["brokenPipe"] = {"stdoutClosedBeforeEcho": True, "boundedNonzeroExit": True,
                                "diagnostic": session.stderr.decode("utf-8", "replace")[:1024]}
        return result


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--probe", type=Path, required=True, help="built pipe_probe executable")
    parser.add_argument("--output", type=Path, required=True, help="directory for results.json")
    arguments = parser.parse_args()
    executable = arguments.probe.resolve(strict=True)
    require(executable.is_file(), "--probe must name an executable file")
    arguments.output.mkdir(parents=True, exist_ok=True)

    cases: list[dict[str, Any]] = []
    for name, operation in [
        ("input-recovery", input_recovery),
        ("streaming-before-eof", streaming_before_eof),
        ("paused-stdout-backpressure", paused_stdout_backpressure),
        ("broken-output-pipe", broken_output_pipe),
    ]:
        started = time.monotonic()
        try:
            observation = operation(executable)
            cases.append({"id": name, "status": "passed", "elapsedSeconds": round(time.monotonic() - started, 3), **observation})
        except (HarnessError, OSError, ValueError) as error:
            cases.append({"id": name, "status": "failed", "elapsedSeconds": round(time.monotonic() - started, 3), "error": str(error)})

    summary = {
        "runner": {"path": str(Path(__file__).resolve()), **file_digest(Path(__file__).resolve())},
        "probe": {"path": str(executable), **file_digest(executable)},
        "platform": sys.platform,
        "python": sys.version.split()[0],
        "executionBoundary": (
            "Rust pipe_probe uses the production RpcInputReader and awaits RpcOutput.write_frame, "
            "including its flush, before exit. This does not exercise Bun's stdoutQueue/EOF exit behavior. "
            "Sampled OS peak memory is observational, not a total memory bound."
        ),
        "cases": cases,
        "summary": {"passed": sum(case["status"] == "passed" for case in cases), "failed": sum(case["status"] == "failed" for case in cases)},
    }
    output_file = arguments.output / "results.json"
    output_file.write_text(json.dumps(summary, ensure_ascii=False, indent=2) + "\n", encoding="utf-8")
    print(json.dumps({"results": str(output_file), **summary["summary"]}))
    return 0 if summary["summary"]["failed"] == 0 else 1


if __name__ == "__main__":
    try:
        raise SystemExit(main())
    except (HarnessError, OSError) as error:
        print(f"RPC process harness setup failed: {error}", file=sys.stderr)
        raise SystemExit(2) from error
