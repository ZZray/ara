"""V1-CANCEL real-console check on Windows through ConPTY (manual, not a CI test).

Windows Terminal hosts a shell through a pseudoconsole (ConPTY): a key press
becomes bytes on the pseudoconsole's input pipe, and the console host inside
it turns "\\x03" into a Ctrl+C key event and a CTRL_C_EVENT for every process
attached to that console. This script drives `ara` the same way. It passes no
--repl flag, so the REPL is chosen by `stdin.is_terminal()`. It types lines
and Ctrl+C as bytes and reads the rendered VT output. The model is the
controlled fake upstream: no network, no real model, no credential.

Usage (after `cargo build -p ara-cli --bin ara -p ara-testkit --bins`):
    python scripts/windows_conpty_ctrl_c.py [--target-dir DIR] [--out DIR] [--enable-ctrl-c]

`--enable-ctrl-c` clears an inherited "ignore Ctrl+C" flag before starting
`ara` (see the comment at its use). Exit 0 means every check passed.
"""
from __future__ import annotations

import argparse
import ctypes
import json
import os
import pathlib
import re
import shutil
import subprocess
import sys
import threading
import time

if sys.platform != "win32":
    print("SKIP: ConPTY exists only on Windows")
    sys.exit(0)

from ctypes import wintypes as wt  # noqa: E402

k32 = ctypes.WinDLL("kernel32", use_last_error=True)


class COORD(ctypes.Structure):
    _fields_ = [("X", wt.SHORT), ("Y", wt.SHORT)]


class STARTUPINFOW(ctypes.Structure):
    _fields_ = [("cb", wt.DWORD), ("lpReserved", wt.LPWSTR), ("lpDesktop", wt.LPWSTR), ("lpTitle", wt.LPWSTR),
                ("dwX", wt.DWORD), ("dwY", wt.DWORD), ("dwXSize", wt.DWORD), ("dwYSize", wt.DWORD),
                ("dwXCountChars", wt.DWORD), ("dwYCountChars", wt.DWORD), ("dwFillAttribute", wt.DWORD),
                ("dwFlags", wt.DWORD), ("wShowWindow", wt.WORD), ("cbReserved2", wt.WORD),
                ("lpReserved2", ctypes.c_void_p), ("hStdInput", wt.HANDLE), ("hStdOutput", wt.HANDLE),
                ("hStdError", wt.HANDLE)]


class STARTUPINFOEXW(ctypes.Structure):
    _fields_ = [("StartupInfo", STARTUPINFOW), ("lpAttributeList", ctypes.c_void_p)]


class PROCESS_INFORMATION(ctypes.Structure):
    _fields_ = [("hProcess", wt.HANDLE), ("hThread", wt.HANDLE), ("dwProcessId", wt.DWORD),
                ("dwThreadId", wt.DWORD)]


k32.CreatePseudoConsole.argtypes = [COORD, wt.HANDLE, wt.HANDLE, wt.DWORD, ctypes.POINTER(ctypes.c_void_p)]
k32.CreatePseudoConsole.restype = ctypes.c_long
k32.ClosePseudoConsole.argtypes = [ctypes.c_void_p]
k32.CreatePipe.argtypes = [ctypes.POINTER(wt.HANDLE), ctypes.POINTER(wt.HANDLE), ctypes.c_void_p, wt.DWORD]
k32.InitializeProcThreadAttributeList.argtypes = [ctypes.c_void_p, wt.DWORD, wt.DWORD, ctypes.POINTER(ctypes.c_size_t)]
k32.UpdateProcThreadAttribute.argtypes = [ctypes.c_void_p, wt.DWORD, ctypes.c_size_t, ctypes.c_void_p,
                                          ctypes.c_size_t, ctypes.c_void_p, ctypes.c_void_p]
k32.CreateProcessW.argtypes = [wt.LPCWSTR, wt.LPWSTR, ctypes.c_void_p, ctypes.c_void_p, wt.BOOL, wt.DWORD,
                               ctypes.c_void_p, wt.LPCWSTR, ctypes.c_void_p, ctypes.POINTER(PROCESS_INFORMATION)]
k32.ReadFile.argtypes = [wt.HANDLE, ctypes.c_void_p, wt.DWORD, ctypes.POINTER(wt.DWORD), ctypes.c_void_p]
k32.WriteFile.argtypes = [wt.HANDLE, ctypes.c_void_p, wt.DWORD, ctypes.POINTER(wt.DWORD), ctypes.c_void_p]
k32.WaitForSingleObject.argtypes = [wt.HANDLE, wt.DWORD]
k32.GetExitCodeProcess.argtypes = [wt.HANDLE, ctypes.POINTER(wt.DWORD)]
k32.TerminateProcess.argtypes = [wt.HANDLE, wt.UINT]
k32.CloseHandle.argtypes = [wt.HANDLE]

EXTENDED_STARTUPINFO_PRESENT = 0x00080000
CREATE_UNICODE_ENVIRONMENT = 0x00000400
PROC_THREAD_ATTRIBUTE_PSEUDOCONSOLE = 0x00020016
STARTF_USESTDHANDLES = 0x00000100
ANSI = re.compile(r"\x1b\[[0-9;?]*[ -/]*[@-~]|\x1b\][^\x07\x1b]*(\x07|\x1b\\)|\x1b[()][0-9A-Za-z]|\x1b[=>]")


class Pty:
    def __init__(self, cmdline, cwd, env):
        in_r, self.in_w, out_r, out_w = wt.HANDLE(), wt.HANDLE(), wt.HANDLE(), wt.HANDLE()
        assert k32.CreatePipe(ctypes.byref(in_r), ctypes.byref(self.in_w), None, 0)
        assert k32.CreatePipe(ctypes.byref(out_r), ctypes.byref(out_w), None, 0)
        self.out_r = out_r
        self.hpc = ctypes.c_void_p()
        hr = k32.CreatePseudoConsole(COORD(160, 50), in_r, out_w, 0, ctypes.byref(self.hpc))
        assert hr == 0, f"CreatePseudoConsole hr={hr:#x}"
        # The pseudoconsole owns duplicates now.
        k32.CloseHandle(in_r)
        k32.CloseHandle(out_w)
        size = ctypes.c_size_t()
        k32.InitializeProcThreadAttributeList(None, 1, 0, ctypes.byref(size))
        self.attrs = ctypes.create_string_buffer(size.value)
        assert k32.InitializeProcThreadAttributeList(self.attrs, 1, 0, ctypes.byref(size))
        # lpValue is the HPCON value itself, as in the Microsoft sample.
        assert k32.UpdateProcThreadAttribute(self.attrs, 0, PROC_THREAD_ATTRIBUTE_PSEUDOCONSOLE, self.hpc,
                                             ctypes.sizeof(ctypes.c_void_p), None, None), ctypes.get_last_error()
        si = STARTUPINFOEXW()
        si.StartupInfo.cb = ctypes.sizeof(STARTUPINFOEXW)
        # The harness's own stdio are pipes; null std handles make the child use
        # the pseudoconsole instead of inheriting them.
        si.StartupInfo.dwFlags = STARTF_USESTDHANDLES
        si.lpAttributeList = ctypes.cast(self.attrs, ctypes.c_void_p)
        block = "".join(f"{k}={v}\0" for k, v in env.items()) + "\0"
        envbuf = ctypes.create_unicode_buffer(block, len(block))
        self.pi = PROCESS_INFORMATION()
        cmd = ctypes.create_unicode_buffer(cmdline)
        ok = k32.CreateProcessW(None, cmd, None, None, False,
                                EXTENDED_STARTUPINFO_PRESENT | CREATE_UNICODE_ENVIRONMENT, envbuf, cwd,
                                ctypes.byref(si), ctypes.byref(self.pi))
        assert ok, f"CreateProcessW error {ctypes.get_last_error()}"
        self.raw = bytearray()
        self.lock = threading.Lock()
        self.reader = threading.Thread(target=self._read, daemon=True)
        self.reader.start()

    def _read(self):
        buf = ctypes.create_string_buffer(8192)
        n = wt.DWORD()
        while k32.ReadFile(self.out_r, buf, 8192, ctypes.byref(n), None) and n.value:
            with self.lock:
                self.raw += buf.raw[:n.value]

    def text(self):
        with self.lock:
            s = self.raw.decode("utf-8", "replace")
        return ANSI.sub("", s).replace("\r", "")

    def send(self, data: bytes):
        n = wt.DWORD()
        assert k32.WriteFile(self.in_w, data, len(data), ctypes.byref(n), None) and n.value == len(data)

    def wait_text(self, needle, timeout, start=0):
        end = time.monotonic() + timeout
        while time.monotonic() < end:
            t = self.text()
            i = t.find(needle, start)
            if i >= 0:
                return i
            time.sleep(0.05)
        return -1

    def exit_code(self, timeout):
        if k32.WaitForSingleObject(self.pi.hProcess, int(timeout * 1000)) != 0:
            return None
        code = wt.DWORD()
        k32.GetExitCodeProcess(self.pi.hProcess, ctypes.byref(code))
        return code.value

    def close(self):
        if self.exit_code(0) is None:
            k32.TerminateProcess(self.pi.hProcess, 99)
        k32.ClosePseudoConsole(self.hpc)
        k32.CloseHandle(self.in_w)
        self.reader.join(timeout=5)
        k32.CloseHandle(self.out_r)
        k32.CloseHandle(self.pi.hProcess)
        k32.CloseHandle(self.pi.hThread)


def ev_text(s):
    return {"data": {"id": "fake-1", "choices": [{"index": 0, "delta": {"content": s}}]}}


def ev_tool(i, cid, name, args):
    return {"data": {"id": "fake-1", "choices": [{"index": 0, "delta": {"tool_calls": [
        {"index": i, "id": cid, "type": "function", "function": {"name": name, "arguments": args}}]}}]}}


def ev_finish(r):
    return {"data": {"id": "fake-1", "choices": [{"index": 0, "delta": {}, "finish_reason": r}]}}


DONE = {"done": True}


def main() -> int:
    ap = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    ap.add_argument("--target-dir", default=os.environ.get("CARGO_TARGET_DIR", "target"))
    ap.add_argument("--out", default=os.path.join(os.environ.get("TEMP", "."), "ara-v1-conpty"))
    ap.add_argument("--enable-ctrl-c", action="store_true")
    opts = ap.parse_args()
    out = pathlib.Path(opts.out)
    ara_exe = pathlib.Path(opts.target_dir) / "debug" / "ara.exe"
    fake_exe = pathlib.Path(opts.target_dir) / "debug" / "ara-fake-upstream.exe"
    for exe in (ara_exe, fake_exe):
        if not exe.exists():
            print(f"FAIL: missing {exe}")
            return 2
    shutil.rmtree(out, ignore_errors=True)
    for d in ("sessions", "work", "home"):
        (out / d).mkdir(parents=True)
    script = out / "script.json"
    script.write_text(json.dumps({"responses": [
        {"events": [ev_tool(0, "call_s", "bash", json.dumps({"command": "echo started; sleep 30"})),
                    ev_finish("tool_calls"), DONE]},
        {"events": [ev_text("After the abort."), ev_finish("stop"), DONE]},
        {"events": [ev_text("must not be requested"), ev_finish("stop"), DONE]},
    ]}), encoding="utf-8")
    record, port_file = out / "requests.jsonl", out / "port.txt"
    fake = subprocess.Popen([str(fake_exe), "--script", str(script), "--record", str(record), "--port-file", str(port_file)],
                            stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
    results = {}
    pty = None
    try:
        for _ in range(100):
            if port_file.exists() and port_file.read_text().strip():
                break
            time.sleep(0.1)
        base = port_file.read_text().strip()
        env = {k: v for k, v in os.environ.items()
               if k not in ("OPENROUTER_API_KEY", "BAI_API_KEY", "ARA_API_KEY", "ARA_TEST_API_KEY", "ARA_MODEL",
                            "ARA_BASE_URL", "ANTHROPIC_API_KEY")}
        env.update({"CONPTY_TEST_KEY": "test", "HOME": str(out / "home"), "ARA_HOME": str(out / "home")})
        cmdline = subprocess.list2cmdline([str(ara_exe), "--model", "fake-model", "--base-url", base, "--api",
                                           "openai-completions", "--api-key-env", "CONPTY_TEST_KEY",
                                           "--session-dir", str(out / "sessions"), "--cwd", str(out / "work")])
        # A process created with CREATE_NEW_PROCESS_GROUP has Ctrl+C disabled,
        # and children inherit that flag. A shell in Windows Terminal has it
        # enabled; `--enable-ctrl-c` clears it here so `ara` inherits the same.
        if opts.enable_ctrl_c:
            k32.SetConsoleCtrlHandler.argtypes = [ctypes.c_void_p, wt.BOOL]
            results["cleared_ignore_flag"] = bool(k32.SetConsoleCtrlHandler(None, False))
            results["clear_last_error"] = ctypes.get_last_error()
        t0 = time.monotonic()
        pty = Pty(cmdline, str(out / "work"), env)
        results["pid"] = pty.pi.dwProcessId
        results["repl_banner"] = pty.wait_text("interactive session", 20) >= 0
        results["first_prompt"] = pty.wait_text("> ", 20) >= 0
        time.sleep(0.3)

        # Turn 1: type a line; the fake model calls bash `sleep 30`.
        pty.send(b"run something slow\r")
        mark = pty.wait_text("ara: tool bash", 20)
        results["tool_started"] = mark >= 0
        time.sleep(1.0)
        t_int = time.monotonic()
        pty.send(b"\x03")  # Ctrl+C as Windows Terminal sends it
        results["interrupt_seen"] = pty.wait_text("interrupt received", 10, max(mark, 0)) >= 0
        i = pty.wait_text("turn 1 cancelled; session kept", 15, max(mark, 0))
        results["turn_cancelled"] = i >= 0
        results["cancel_latency_s"] = round(time.monotonic() - t_int, 2)
        results["alive_after_turn_interrupt"] = pty.exit_code(0) is None
        p2 = pty.wait_text("> ", 10, max(i, 0))
        results["prompt_after_cancel"] = p2 >= 0
        time.sleep(0.5)

        # Turn 2 in the same process and Session.
        pty.send(b"after\r")
        a = pty.wait_text("After the abort.", 20, max(p2, 0))
        results["second_turn_answered"] = a >= 0
        p3 = pty.wait_text("> ", 10, max(a, 0))
        results["prompt_after_second_turn"] = p3 >= 0
        time.sleep(0.8)

        # Idle prompt: ReadConsole is pending; Ctrl+C must exit 130.
        t_idle = time.monotonic()
        pty.send(b"\x03")
        results["idle_exit_code"] = pty.exit_code(10)
        results["idle_exit_latency_s"] = round(time.monotonic() - t_idle, 2)
        results["total_s"] = round(time.monotonic() - t0, 2)
    finally:
        if pty:
            screen = pty.text()
            pty.close()
            (out / "screen.txt").write_text(screen, encoding="utf-8")
        fake.kill()
        fake.wait()

    reqs = [json.loads(l) for l in record.read_text(encoding="utf-8").splitlines()] if record.exists() else []
    results["model_calls"] = len(reqs)
    files = sorted((out / "sessions").rglob("*.jsonl"))
    results["session_files"] = len(files)
    if files:
        entries = [json.loads(l) for l in files[0].read_text(encoding="utf-8").splitlines()]
        roles = [e.get("message", {}).get("role", e.get("type")) for e in entries[1:]]
        results["journal_roles"] = roles
        results["stop_reasons"] = [e["message"].get("stopReason") for e in entries[1:]
                                   if e.get("message", {}).get("role") == "assistant"]
        tr = [e for e in entries if e.get("message", {}).get("role") == "toolResult"]
        if tr:
            c = tr[0]["message"]["content"]
            results["tool_result_tail"] = (c[0].get("text", "") if c else "")[-160:]
            results["tool_result_is_error"] = tr[0]["message"].get("isError")
    checks = {
        "REPL chosen by a console stdin": results.get("repl_banner") and results.get("first_prompt"),
        "bash tool started": results.get("tool_started"),
        "turn interrupt reported": results.get("interrupt_seen"),
        "turn cancelled, process kept": results.get("turn_cancelled") and results.get("alive_after_turn_interrupt"),
        "prompt shown again": results.get("prompt_after_cancel"),
        "next turn answered": results.get("second_turn_answered") and results.get("prompt_after_second_turn"),
        "idle Ctrl+C exits 130": results.get("idle_exit_code") == 130,
        "two model calls": results.get("model_calls") == 2,
        "journal roles": results.get("journal_roles") == ["session", "model_change", "user", "assistant",
                                                          "toolResult", "assistant", "user", "assistant"],
        "aborted assistant recorded": results.get("stop_reasons") == ["toolUse", "aborted", "stop"],
        "tool result says aborted": str(results.get("tool_result_tail", "")).endswith("[Command aborted]"),
    }
    results["checks"] = {k: bool(v) for k, v in checks.items()}
    (out / "results.json").write_text(json.dumps(results, indent=2), encoding="utf-8")
    print(json.dumps(results, indent=2))
    failed = [k for k, v in checks.items() if not v]
    print("PASS" if not failed else f"FAIL: {failed}")
    return 0 if not failed else 1


if __name__ == "__main__":
    sys.exit(main())
