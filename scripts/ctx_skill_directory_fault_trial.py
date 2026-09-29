"""Run the GNU directory-I/O shim through the real ARA CLI and Chat/Session path.

This is a fail-closed Ubuntu 24.04 x86_64 trial, not a portable fault adapter.
All three cases and their receipts are retained beneath a fresh --output run.
Optional source arguments name the exact Rust unix.rs and Tokio read_dir.rs files.
"""

from __future__ import annotations

import argparse
import hashlib
import json
import os
from pathlib import Path
import platform
import shutil
import signal
import stat
import subprocess
import sys
import tempfile
import threading
import time
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer


RUST_COMMIT = "8bab26f4f68e0e26f0bb7960be334d5b520ea452"
HOOK_SYMBOLS = {"opendir", "readdir64", "closedir", "statx", "fstatat64"}
DIRECTORIES = ["a-dir", "b-dir"]
FILES = [f"file{index:02}.txt" for index in range(9)] + ["probe-type.txt"]
EXPECTED_LISTING = "\n".join([name + "/" for name in DIRECTORIES] + FILES)
READ_URL = "skill://probe/references:raw"


def require(condition, message):
    if not condition:
        raise RuntimeError(message)


def write_json(path, value):
    path.write_text(json.dumps(value, indent=2) + "\n", encoding="utf-8")


def file_hash(path):
    digest = hashlib.sha256()
    with path.open("rb") as source:
        while chunk := source.read(1024 * 1024):
            digest.update(chunk)
    return digest.hexdigest()


def artifact_hashes(directory):
    return {
        str(path.relative_to(directory)): {"bytes": path.stat().st_size, "sha256": file_hash(path)}
        for path in sorted(directory.rglob("*"))
        if path.is_file() and path != directory / "summary.json"
    }


def clean_environment(home):
    credentials = {
        "OPENAI_API_KEY", "OPENAI_BASE_URL", "OPENAI_ORG_ID", "OPENAI_ORGANIZATION", "OPENAI_PROJECT_ID",
        "OPENROUTER_API_KEY", "OPENROUTER_BASE_URL", "ANTHROPIC_API_KEY", "ANTHROPIC_AUTH_TOKEN",
        "ANTHROPIC_BASE_URL", "GOOGLE_API_KEY", "GEMINI_API_KEY", "GOOGLE_APPLICATION_CREDENTIALS",
        "CLAUDE_CONFIG_DIR", "COPILOT_HOME", "COPILOT_CUSTOM_INSTRUCTIONS_DIRS", "PI_CODING_AGENT_DIR",
        "LD_PRELOAD", "LD_LIBRARY_PATH", "HOME", "USERPROFILE", "XDG_CONFIG_HOME", "XDG_DATA_HOME",
        "XDG_CACHE_HOME", "XDG_STATE_HOME", "HTTP_PROXY", "HTTPS_PROXY", "ALL_PROXY", "NO_PROXY",
        "http_proxy", "https_proxy", "all_proxy", "no_proxy", "WSL_DISTRO_NAME", "WSL_INTEROP",
    }
    env = {key: value for key, value in os.environ.items() if not key.startswith("ARA_") and key not in credentials}
    env.update({
        "HOME": str(home), "USERPROFILE": str(home), "ARA_HOME": str(home),
        "XDG_CONFIG_HOME": str(home / "config"), "XDG_DATA_HOME": str(home / "data"),
        "XDG_CACHE_HOME": str(home / "cache"), "XDG_STATE_HOME": str(home / "state"),
        "NO_PROXY": "127.0.0.1,localhost", "no_proxy": "127.0.0.1,localhost",
    })
    return env


def environment_evidence(run_dir, args):
    directory = run_dir / "environment"
    directory.mkdir()
    home = directory / "home"
    home.mkdir()
    env = clean_environment(home)
    # rustc may be a rustup proxy whose installed toolchain is located using
    # the original home. These probes are read-only; CLI cases remain isolated.
    if "HOME" in os.environ:
        env["HOME"] = os.environ["HOME"]
    receipts = []

    def capture(name, command):
        try:
            result = subprocess.run(command, env=env, capture_output=True, timeout=20, check=False)
        except subprocess.TimeoutExpired as error:
            (directory / f"{name}.stdout.txt").write_bytes(error.stdout or b"")
            (directory / f"{name}.stderr.txt").write_bytes(error.stderr or b"")
            receipts.append({"name": name, "command": command, "exitCode": None, "timedOut": True})
            write_json(directory / "commands.json", receipts)
            raise RuntimeError(f"Environment prerequisite timed out: {name}") from error
        (directory / f"{name}.stdout.txt").write_bytes(result.stdout)
        (directory / f"{name}.stderr.txt").write_bytes(result.stderr)
        receipts.append({"name": name, "command": command, "exitCode": result.returncode})
        write_json(directory / "commands.json", receipts)
        require(result.returncode == 0, f"Environment prerequisite failed: {name} ({result.returncode})")
        return result.stdout.decode("utf-8", errors="replace")

    require(sys.platform == "linux" and platform.machine() == "x86_64", "Requires Linux x86_64")
    os_release = Path("/etc/os-release").read_bytes()
    (directory / "os-release.txt").write_bytes(os_release)
    release = {}
    for line in os_release.decode().splitlines():
        if "=" in line:
            key, value = line.split("=", 1)
            release[key] = value.strip('"')
    require(release.get("ID") == "ubuntu" and release.get("VERSION_ID") == "24.04", "Requires Ubuntu 24.04")
    capture("uname", ["uname", "-a"])
    libc = capture("glibc", ["getconf", "GNU_LIBC_VERSION"]).strip()
    require(libc.startswith("glibc 2.39"), f"Requires Ubuntu 24.04 GNU libc 2.39; got {libc}")
    rust = capture("rustc", ["rustc", "-vV"])
    rust_fields = dict(line.split(": ", 1) for line in rust.splitlines() if ": " in line)
    require(rust_fields.get("release") == "1.97.1", "Requires rustc release 1.97.1")
    require(rust_fields.get("commit-hash") == RUST_COMMIT, "Rust std commit differs from the reviewed source")
    require(rust_fields.get("host") == "x86_64-unknown-linux-gnu", "Requires the reviewed Rust GNU host")
    capture("cc", ["cc", "--version"])
    capture("ldd-version", ["ldd", "--version"])
    for name, target in (("binary", args.binary), ("shim", args.shim)):
        require(target.is_file(), f"Missing {name}: {target}")
        with target.open("rb") as source:
            require(source.read(4) == b"\x7fELF", f"{name} is not an ELF artifact")
        header = capture(f"{name}-elf-header", ["readelf", "--wide", "--file-header", str(target)])
        require("Advanced Micro Devices X86-64" in header, f"{name} is not x86_64 ELF")
        capture(f"{name}-dynamic-symbols", ["readelf", "--wide", "--dyn-syms", str(target)])
        symbols = capture(f"{name}-nm", ["nm", "-D", str(target)])
        capture(f"{name}-ldd", ["ldd", str(target)])
        if name == "shim":
            exports = {line.split()[-1].split("@", 1)[0] for line in symbols.splitlines() if len(line.split()) >= 3}
            require(HOOK_SYMBOLS <= exports, f"Shim exports missing: {sorted(HOOK_SYMBOLS - exports)}")
    sources = []
    for name, source in (("std-unix", args.std_source), ("tokio-read-dir", args.tokio_source)):
        if source is not None:
            require(source.is_file(), f"{name} must name a source file")
            target = directory / f"{name}.rs"
            shutil.copyfile(source, target)
            require(source.read_bytes() == target.read_bytes(), f"{name} copy differs")
            sources.append({"name": name, "inputPath": str(source), "artifact": target.name, "sha256": file_hash(target)})
    repository = Path(__file__).resolve().parents[1]
    commit = capture("git-head", ["git", "-C", str(repository), "rev-parse", "HEAD"]).strip()
    snapshot_paths = [
        "crates/ara-tools/src/read.rs", "crates/ara-cli/src/main.rs", "Cargo.lock",
        "scripts/ctx_skill_directory_fault.c", "scripts/ctx_skill_directory_fault_trial.py",
        ".github/workflows/ctx-skill-directory-fault.yml",
    ]
    snapshot = {"commit": commit, "files": {path: file_hash(repository / path) for path in snapshot_paths}}
    write_json(directory / "source-snapshot.json", snapshot)
    summary = {
        "status": "PASS", "rustCommit": RUST_COMMIT, "glibc": libc, "sources": sources,
        "binarySha256": file_hash(args.binary), "shimSha256": file_hash(args.shim),
        "scriptSha256": file_hash(Path(__file__)), "sourceSnapshot": snapshot,
        "compilerProbeHome": "original HOME for read-only rustup/compiler probes",
        "artifacts": artifact_hashes(directory),
    }
    write_json(directory / "summary.json", summary)
    return summary


def json_lines(path):
    return [json.loads(line) for line in path.read_text(encoding="utf-8").splitlines() if line.strip()]


def check_text(mode, text):
    require(isinstance(text, str), "Tool text is not a string")
    if mode == "observe":
        require(text == EXPECTED_LISTING, "Control did not return the exact complete listing")
    else:
        require(text.startswith("Cannot list"), f"{mode} did not surface Cannot list")
        require("Input/output error" in text or "os error 5" in text, f"{mode} did not surface EIO")
        require(text != EXPECTED_LISTING, f"{mode} returned a successful listing")


def check_hooks(mode, entries, references, arm, log_fd):
    require(entries and all(row.get("mode") == mode for row in entries), "Missing hooks or mismatched hook mode")
    require(not any(row.get("event") in {"scope_mismatch", "fatal"} for row in entries), "Shim scope/config failure")
    expected_events = {"init", "resolved", "opendir", "entry", "dot_entry", "dtype_unknown", "enumeration_eio", "metadata_eio", "closedir", "eof"}
    require(all(row.get("event") in expected_events for row in entries), "Unexpected shim phase or native directory error")
    init = [row for row in entries if row.get("event") == "init"]
    require(len(init) == 1, "Expected one shim initialization")
    require(init[0].get("path") == str(references) and init[0].get("arm") == str(arm), "Shim configured a different path/arm")
    require(init[0].get("name") == "probe-type.txt" and init[0].get("log_fd") == log_fd, "Shim configured a different basename/fd")
    require(init[0].get("initial_armed") is False, "Shim started with a pre-existing arm file")
    resolved = [row for row in entries if row.get("event") == "resolved"]
    require(len(resolved) == 5 and {row.get("symbol") for row in resolved} == HOOK_SYMBOLS, "Five original functions were not resolved")
    for row in resolved:
        address = row.get("address")
        if isinstance(address, str):
            address = int(address, 0)
        require(type(address) is int and address != 0, "Unresolved original function address")
    opens = [row for row in entries if row.get("event") == "opendir"]
    require(len(opens) == 1, "Expected exactly one registered target opendir")
    opened = opens[0]
    require(opened.get("path") == str(references) and opened.get("armed") is True and opened.get("target") is True, "Target opendir occurred outside the armed scope")
    generation, dirfd = opened.get("generation"), opened.get("dirfd")
    require(isinstance(generation, int) and generation > 0 and isinstance(dirfd, int) and dirfd >= 0, "Invalid target handle identity")
    phase_events = {"entry", "dtype_unknown", "enumeration_eio", "metadata_eio", "closedir", "eof"}
    phases = [row for row in entries if row.get("event") in phase_events]
    require(all(row.get("generation") == generation and row.get("dirfd") == dirfd for row in phases), "Hook hit did not match the registered handle")
    require(all(row.get("path") == str(references) and row.get("armed") is True and row.get("target") is True for row in phases), "Hook hit was outside the armed target path")
    actual = [row for row in phases if row.get("event") == "entry" and row.get("name") not in {".", ".."}]
    require(actual and all(row.get("name") in DIRECTORIES + FILES for row in actual), "No real fixture entries or unexpected basename")
    require([row.get("non_dot_count") for row in actual] == list(range(1, len(actual) + 1)), "Non-dot entry counts are incomplete")
    require(any(row.get("event") == "closedir" for row in phases), "Registered directory was not closed")
    enumeration = [row for row in phases if row.get("event") == "enumeration_eio"]
    metadata = [row for row in phases if row.get("event") == "metadata_eio"]
    unknown = [row for row in phases if row.get("event") == "dtype_unknown"]
    if mode == "observe":
        require(not enumeration and not metadata and not unknown, "Control injected a fault")
        require(len(actual) == 12 and {row["name"] for row in actual} == set(DIRECTORIES + FILES), "Control did not enumerate all twelve actual entries")
        require(any(row.get("event") == "eof" and row.get("non_dot_count") == 12 and row.get("errno") == 0 for row in phases), "Control did not observe complete EOF")
    elif mode == "enumeration":
        require(enumeration and not metadata and not unknown, "Enumeration fault hits are missing or mixed")
        require(all(row.get("errno") == 5 and row.get("non_dot_count", 0) >= 1 for row in enumeration), "Enumeration EIO preceded all real entries")
        require(entries.index(actual[0]) < entries.index(enumeration[0]), "Enumeration fault was not post-entry")
        require([row.get("injection_count") for row in enumeration] == list(range(1, len(enumeration) + 1)), "Enumeration injection counts differ")
    else:
        require(not enumeration and len(unknown) == 1 and len(metadata) >= 2, "Type fault needs one DT_UNKNOWN and repeated metadata EIO")
        require(unknown[0].get("name") == "probe-type.txt" and unknown[0].get("entry_type") == 0 and unknown[0].get("actual_entry_type") == 8, "Type fault did not force the real regular entry to DT_UNKNOWN")
        require(all(row.get("name") == "probe-type.txt" and row.get("errno") == 5 and row.get("symbol") in {"statx", "fstatat64"} for row in metadata), "Metadata EIO did not match target dirfd/basename")
        require(entries.index(unknown[0]) < entries.index(metadata[0]), "Metadata EIO preceded DT_UNKNOWN")
        require([row.get("metadata_injection_count") for row in metadata] == list(range(1, len(metadata) + 1)), "Metadata EIO was not persistent")
    return {"generation": generation, "dirfd": dirfd, "actualEntries": len(actual), "enumerationHits": len(enumeration), "metadataHits": len(metadata), "dtypeUnknownHits": len(unknown)}


def terminate_group(process):
    try:
        os.killpg(process.pid, signal.SIGTERM)
    except ProcessLookupError:
        pass
    try:
        return process.communicate(timeout=2)
    except subprocess.TimeoutExpired:
        try:
            os.killpg(process.pid, signal.SIGKILL)
        except ProcessLookupError:
            pass
        return process.communicate(timeout=3)


def run_case(run_dir, mode, args):
    directory = run_dir / mode
    directory.mkdir()
    summary = {"status": "FAIL", "mode": mode}
    requests, upstream_errors = [], []
    stdout = stderr = b""
    server = thread = process = None
    hook_fd = None
    started = time.monotonic()
    try:
        home, work, sessions = (directory / name for name in ("home", "work", "sessions"))
        home.mkdir()
        skill = work / ".ara/skills/probe"
        references = skill / "references"
        references.mkdir(parents=True)
        (skill / "SKILL.md").write_text("---\ndescription: Controlled directory fault fixture\n---\nRead the references listing.\n", encoding="utf-8")
        for name in DIRECTORIES:
            (references / name).mkdir()
        for name in FILES:
            (references / name).write_text(f"fixture {name}\n", encoding="ascii")
        write_json(directory / "fixture-manifest.json", {
            "targetPath": str(references), "readUrl": READ_URL, "expectedListing": EXPECTED_LISTING,
            "directories": DIRECTORIES, "files": [{"name": name, "sha256": file_hash(references / name)} for name in FILES],
            "skillSha256": file_hash(skill / "SKILL.md"),
        })
        arm = directory / "armed"
        hook_path = directory / "hook.jsonl"
        hook_fd = os.open(hook_path, os.O_WRONLY | os.O_CREAT | os.O_EXCL, 0o600)
        require(stat.S_ISREG(os.fstat(hook_fd).st_mode), "Hook fd is not a regular file")
        call_id = f"call_directory_{mode}"
        lock = threading.Lock()

        class Upstream(BaseHTTPRequestHandler):
            def log_message(self, *unused):
                pass

            def do_POST(self):
                try:
                    length = int(self.headers["Content-Length"])
                    require(0 < length <= 2 * 1024 * 1024, "Unexpected upstream body size")
                    body = json.loads(self.rfile.read(length))
                    with lock:
                        index = len(requests)
                        requests.append(body)
                    require(index < 2, "Model request limit exceeded")
                    if index == 0:
                        require(not arm.exists(), "Arm file existed before the first model request")
                        arm.write_bytes(b"armed by first controlled model request\n")
                        delta = {"role": "assistant", "tool_calls": [{"index": 0, "id": call_id, "type": "function", "function": {"name": "read", "arguments": json.dumps({"path": READ_URL})}}]}
                        reason = "tool_calls"
                    else:
                        tool_messages = [message for message in body["messages"] if message.get("role") == "tool"]
                        require(len(tool_messages) == 1 and tool_messages[0].get("tool_call_id") == call_id, "Second model request did not contain exactly the matching tool result")
                        require("isError" not in tool_messages[0], "Chat projection unexpectedly contains isError")
                        check_text(mode, tool_messages[0].get("content"))
                        delta = {"role": "assistant", "content": f"Directory {mode} trial verified."}
                        reason = "stop"
                    chunks = [{"choices": [{"index": 0, "delta": delta, "finish_reason": None}]}, {"choices": [{"index": 0, "delta": {}, "finish_reason": reason}]}]
                    payload = ("".join(f"data: {json.dumps(chunk)}\n\n" for chunk in chunks) + "data: [DONE]\n\n").encode()
                    self.send_response(200)
                    self.send_header("Content-Type", "text/event-stream")
                    self.send_header("Content-Length", str(len(payload)))
                    self.end_headers()
                    self.wfile.write(payload)
                except Exception as error:
                    with lock:
                        upstream_errors.append(str(error))
                    self.send_error(500, "controlled trial assertion failed")

        server = ThreadingHTTPServer(("127.0.0.1", 0), Upstream)
        server.daemon_threads = True
        thread = threading.Thread(target=server.serve_forever, daemon=True)
        thread.start()
        env = clean_environment(home)
        env.update({
            "ARA_API_KEY": "controlled-trial-key", "LD_PRELOAD": str(args.shim),
            "ARA_CTX_FAULT_PATH": str(references), "ARA_CTX_FAULT_ARM": str(arm),
            "ARA_CTX_FAULT_MODE": mode, "ARA_CTX_FAULT_FD": str(hook_fd), "ARA_CTX_FAULT_NAME": "probe-type.txt",
        })
        command = [str(args.binary), "--api", "openai-completions", "--model", "controlled-directory-model", "--base-url", f"http://127.0.0.1:{server.server_port}/v1", "--cwd", str(work), "--session-dir", str(sessions), "--mode", "json", "--tools", "read", "--max-model-calls", "2", "--max-time", "30", "--max-tokens", "128", "Read skill://probe/references:raw once and report the result"]
        write_json(directory / "command.json", command)
        process = subprocess.Popen(command, env=env, stdin=subprocess.DEVNULL, stdout=subprocess.PIPE, stderr=subprocess.PIPE, start_new_session=True, pass_fds=(hook_fd,))
        try:
            stdout, stderr = process.communicate(timeout=40)
        except subprocess.TimeoutExpired as error:
            stdout, stderr = error.stdout or b"", error.stderr or b""
            stdout, stderr = terminate_group(process)
            raise RuntimeError("CLI exceeded the 40-second process bound; its process group was terminated")
        summary["exitCode"] = process.returncode
        require(process.returncode == 0, f"CLI failed ({process.returncode})")
        require(not upstream_errors and len(requests) == 2, f"Upstream request verification failed: {upstream_errors}; calls={len(requests)}")
        journals = list(sessions.glob("*.jsonl"))
        require(len(journals) == 1, f"Expected one Session journal; got {len(journals)}")
        shutil.copyfile(journals[0], directory / "journal.jsonl")
        receipts = [entry["message"] for entry in json_lines(journals[0]) if entry.get("message", {}).get("role") == "toolResult"]
        require(len(receipts) == 1, f"Expected one toolResult Session receipt; got {len(receipts)}")
        receipt = receipts[0]
        require(receipt.get("toolCallId") == call_id and receipt.get("toolName") == "read", "Session receipt does not match the requested read")
        require(receipt.get("isError", False) is (mode != "observe"), "Session isError differs from the injected mode")
        require(len(receipt.get("content", [])) == 1 and receipt["content"][0].get("type") == "text", "Session receipt is not one text result")
        text = receipt["content"][0]["text"]
        check_text(mode, text)
        wire_text = next(message["content"] for message in requests[1]["messages"] if message.get("role") == "tool")
        require(wire_text == text, "Chat projection and Session text differ")
        events = [json.loads(line) for line in stdout.decode("utf-8").splitlines() if line.strip()]
        ends = [event for event in events if event.get("type") == "tool_execution_end"]
        require(len(ends) == 1 and ends[0].get("toolCallId") == call_id and ends[0].get("toolName") == "read", "CLI execution events do not match the one read")
        require(ends[0].get("isError") is receipt.get("isError", False), "CLI event and Session isError differ")
        require(ends[0].get("result", {}).get("content") == receipt["content"], "CLI event and Session text differ")
        if mode == "observe":
            require(receipt.get("details", {}).get("totalLines") == 12, "Control Session totalLines is not twelve")
        hooks = check_hooks(mode, json_lines(hook_path), references, arm, hook_fd)
        summary.update(status="PASS", requests=2, toolReceipts=1, isError=mode != "observe", toolText=text, hooks=hooks)
    except Exception as error:
        summary["error"] = str(error)
    finally:
        if process is not None and process.poll() is None:
            try:
                stdout, stderr = terminate_group(process)
            except Exception as error:
                summary["terminationError"] = str(error)
        if hook_fd is not None:
            os.close(hook_fd)
        if server is not None:
            server.shutdown()
            server.server_close()
        if thread is not None:
            thread.join(timeout=2)
        (directory / "events.jsonl").write_bytes(stdout)
        (directory / "stderr.txt").write_bytes(stderr)
        write_json(directory / "requests.json", requests)
        write_json(directory / "upstream-errors.json", upstream_errors)
        # Keep every Session created on failed cases too, including partial journals.
        sessions_dir = directory / "sessions"
        journals = list(sessions_dir.glob("*.jsonl")) if sessions_dir.exists() else []
        if len(journals) == 1 and not (directory / "journal.jsonl").exists():
            shutil.copyfile(journals[0], directory / "journal.jsonl")
        summary["elapsedSeconds"] = round(time.monotonic() - started, 3)
        summary["artifacts"] = artifact_hashes(directory)
        write_json(directory / "summary.json", summary)
    return summary


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    for name in ("binary", "shim", "output"):
        parser.add_argument(f"--{name}", type=Path, required=True)
    parser.add_argument("--std-source", type=Path)
    parser.add_argument("--tokio-source", type=Path)
    args = parser.parse_args()
    args.output.mkdir(parents=True, exist_ok=True)
    run_dir = Path(tempfile.mkdtemp(prefix="host-", dir=args.output)).resolve()
    summary = {"status": "FAIL", "runDirectory": str(run_dir), "cases": []}
    try:
        args.binary = args.binary.resolve(strict=True)
        args.shim = args.shim.resolve(strict=True)
        for name in ("std_source", "tokio_source"):
            if getattr(args, name) is not None:
                setattr(args, name, getattr(args, name).resolve(strict=True))
        summary["environment"] = environment_evidence(run_dir, args)
        for mode in ("observe", "enumeration", "type"):
            summary["cases"].append(run_case(run_dir, mode, args))
        require(all(case["status"] == "PASS" for case in summary["cases"]), "One or more directory fault cases failed")
        require(file_hash(args.binary) == summary["environment"]["binarySha256"] and file_hash(args.shim) == summary["environment"]["shimSha256"], "Binary or shim bytes changed during the trial")
        repository = Path(__file__).resolve().parents[1]
        require(all(file_hash(repository / path) == digest for path, digest in summary["environment"]["sourceSnapshot"]["files"].items()), "Source snapshot changed during the trial")
        summary["status"] = "PASS"
    except Exception as error:
        summary["error"] = str(error)
    finally:
        summary["artifactHashes"] = artifact_hashes(run_dir)
        write_json(run_dir / "summary.json", summary)
        print(json.dumps(summary, indent=2))
    return 0 if summary["status"] == "PASS" else 1


if __name__ == "__main__":
    raise SystemExit(main())
