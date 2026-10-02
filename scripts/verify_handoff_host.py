"""Grouped handoff flows through a supplied actual Rust Host, with loopback SSE."""
from __future__ import annotations
import argparse
import datetime
import json
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from pathlib import Path
import subprocess
import threading
import time

import verify_local_reduction_host as shared

DOCUMENT = "## Goal\nContinue Oak-427\n\n## Next Steps\n1. Continue the requested task."
HEAVY = "HANDOFF_ORIGINAL_HEAVY_SENTINEL"


def source_pins():
    pins = shared.source_pins()
    for path in (Path(__file__), Path(__file__).with_name("verify_handoff.py"),
                 shared.ROOT / "Cargo.lock", shared.ROOT / "crates/ara-agent/Cargo.toml",
                 shared.ROOT / "crates/ara-agent/prompts/handoff-document.md"):
        pins[str(path.relative_to(shared.ROOT))] = shared.digest(path)
    return pins


def seed() -> list[dict]:
    return [
        shared.row("model_change", "source-model", None, model=f"openai-compatible/{shared.MODEL}"),
        shared.row("message", "source-user", "source-model", message={"role": "user", "content": HEAVY + " old" * 30000,
                                                             "timestamp": 1}),
        shared.row("message", "source-call", "source-user", message=shared.assistant([
            {"type": "toolCall", "id": "read-past", "name": "read", "arguments": {"path": "past.txt"}}], "toolUse")),
        shared.row("message", "source-result", "source-call", message={"role": "toolResult", "toolCallId": "read-past",
            "toolName": "read", "content": [{"type": "text", "text": "Oak-427"}], "isError": False, "timestamp": 2}),
        shared.row("message", "source-answer", "source-result", message=shared.assistant([
            {"type": "text", "text": "Past task completed."}])),
        shared.row("message", "kept-user", "source-answer", message={"role": "user", "content": "Keep current work.",
                                                                     "timestamp": 3}),
        shared.row("message", "kept-answer", "kept-user", message=shared.assistant([
            {"type": "text", "text": "Current work in progress."}], known_usage=True)),
    ]


def run(trial, name, fake, directory, work, session, lines, calls, *, rpc=False, threshold=0):
    command = [str(trial.binary), "--model", shared.MODEL, "--base-url", fake.base_url,
        "--api", "openai-completions", "--cwd", str(work), "--session-dir", str(session.parent),
        "--resume", str(session), "--no-skills", "--system-prompt", "Fixed handoff context fixture.",
        "--tools", "read,bash", "--max-model-calls", "4", "--max-time", "10",
        "--compact-keep-tokens", "1", "--compact-threshold", str(threshold)]
    command.extend(["--mode", "rpc"] if rpc else ["--mode", "json", "--repl"])
    started, first = time.monotonic(), len(fake.requests)
    if rpc:
        process = shared.RpcProcess(command, trial.environment(directory), directory, 30)
        try:
            prompt = json.loads(lines.strip())
            process.command(name + "-prompt", "prompt", message=prompt["message"])
            while True:
                process.until(lambda frame: frame.get("type") == "agent_end")
                if len(fake.requests) - first >= calls:
                    break
        finally:
            process.finish()
        result = subprocess.CompletedProcess(command, process.child.returncode,
            (directory / "rpc.stdout.log").read_bytes(), (directory / "rpc.stderr.log").read_bytes())
    else:
        result = subprocess.run(command, input=lines.encode(), cwd=work, env=trial.environment(directory),
                                capture_output=True, timeout=30)
    (directory / (name + ".stdout.log")).write_bytes(result.stdout)
    (directory / (name + ".stderr.log")).write_bytes(result.stderr)
    (directory / (name + ".session.jsonl")).write_bytes(session.read_bytes())
    shared.dump(directory / "requests.json", fake.requests)
    step = {"name": name, "command": command, "exitCode": result.returncode,
            "seconds": round(time.monotonic() - started, 3), "actualCalls": len(fake.requests) - first,
            "expectedCalls": calls, "sessionSha256": shared.digest(session)}
    trial.steps.append(step)
    shared.require(result.returncode == 0, name + " process failed; inspect retained logs")
    shared.require(step["actualCalls"] == calls and not fake.errors, name + " call budget differs")
    return result, fake.requests[first:]


def manual_reopen(trial):
    directory, work, session = trial.setup("manual-reopen-no-tool-execution", seed())
    before = shared.journal(session)
    with shared.FakeUpstream() as fake:
        fake.responses = [shared.text_events("Ready to continue."), [
            {"choices": [{"index": 0, "delta": {"role": "assistant", "content": DOCUMENT,
                "tool_calls": [{"index": 0, "id": "forbidden-side-tool", "type": "function", "function": {
                    "name": "bash", "arguments": json.dumps({"command": "echo TOOL_EXECUTED > side-effect.txt"})}}]},
                "finish_reason": None}]},
            {"choices": [{"index": 0, "delta": {}, "finish_reason": "tool_calls"}]},
        ]]
        _, requests = run(trial, "manual", fake, directory, work, session,
                          "prepare\n/handoff preserve exact state\n/exit\n", 2)
        live, side = (request["body"] for request in requests)
        shared.require(live["tools"] == side["tools"], "side tool definitions changed")
        shared.require(live["messages"][0] == side["messages"][0], "base system changed")
        shared.require(side["messages"][:len(live["messages"])] == live["messages"], "live history changed")
        shared.require(side["tool_choice"] == "none", "side call did not disable tools")
        shared.require("preserve exact state" in json.dumps(side["messages"][-1]), "custom focus missing")
        shared.require(not (work / "side-effect.txt").exists(), "handoff executed a tool")
        after = shared.journal(session)
        shared.preserved(before, after)
        compact = [entry for entry in after.values() if entry["type"] == "compaction"]
        shared.require(len(compact) == 1 and compact[0]["method"] == "handoff", "native method missing")
        shared.require(DOCUMENT in compact[0]["summary"] and "<handoff>" not in compact[0]["summary"],
                       "stored document contains model wrapper")
        shared.require(compact[0]["details"] == {"readFiles": ["past.txt"], "modifiedFiles": []}, "file details differ")
        compact_before = compact[0]
        fake.responses = [shared.text_events("Continued original Session.")]
        _, reopened = run(trial, "reopen", fake, directory, work, session, "continue\n/exit\n", 1)
        wire = json.dumps(reopened[0]["body"]["messages"])
        shared.require("<handoff>" in wire and "prior instance" in wire and HEAVY not in wire, "reopen projection differs")
        shared.require(shared.journal(session)[compact_before["id"]] == compact_before, "reopen changed compaction")
    return {"name": directory.name, "status": "passed", "calls": 3}


def empty_manual_and_auto_fallback(trial):
    directory, work, session = trial.setup("empty-manual-auto-fallback", seed())
    initial = shared.digest(session)
    with shared.FakeUpstream() as fake:
        fake.responses = [shared.text_events("   ")]
        manual, _ = run(trial, "empty-manual", fake, directory, work, session, "/handoff\n/exit\n", 1)
        shared.require(b"produced no content" in manual.stderr, "manual empty error missing")
        shared.require(shared.digest(session) == initial, "manual failure changed Session")
        (directory / "home/agent").mkdir(parents=True, exist_ok=True)
        (directory / "home/agent/config.yml").write_text("compaction:\n  methodOrder: [handoff, soft]\n", encoding="utf-8")
        # keep=1 selects an Assistant split: native soft uses both prefix and
        # historical summaries before the continuing primary request.
        fake.responses = [shared.text_events(""), shared.text_events("Split-turn summary."),
                          shared.text_events("Prior work summary."), shared.text_events("done")]
        run(trial, "auto-fallback", fake, directory, work, session,
            json.dumps({"type": "prompt", "message": "Continue work."}) + "\n", 4, rpc=True, threshold=5000)
        compactions = [entry for entry in shared.journal(session).values() if entry["type"] == "compaction"]
        shared.require(len(compactions) == 1 and compactions[0]["method"] == "soft", "empty auto did not fall back")
    return {"name": directory.name, "status": "passed", "calls": 5}


def incomplete_handoff(trial):
    directory, work, session = trial.setup("incomplete-inline-handoff", seed())
    (directory / "home/agent").mkdir(parents=True, exist_ok=True)
    (directory / "home/agent/config.yml").write_text("compaction:\n  methodOrder: [handoff, soft]\n", encoding="utf-8")
    with shared.FakeUpstream() as fake:
        length = shared.text_events("Useful partial output.")
        length[1]["choices"][0]["finish_reason"] = "length"
        fake.responses = [length, shared.text_events(DOCUMENT), shared.text_events("Resumed the original work.")]
        _, requests = run(trial, "incomplete", fake, directory, work, session,
                          json.dumps({"type": "prompt", "message": "Continue now."}) + "\n", 3, rpc=True)
        compactions = [entry for entry in shared.journal(session).values() if entry["type"] == "compaction"]
        shared.require(len(compactions) == 1 and compactions[0]["method"] == "handoff", "incomplete handoff not committed")
        shared.require(requests[1]["body"]["tool_choice"] == "none", "incomplete side tool choice differs")
        shared.require("Useful partial output." not in json.dumps(requests[1]["body"]["messages"]), "failed output revived")
        shared.require("<handoff>" in json.dumps(requests[2]["body"]["messages"]), "continuation lacks handoff wrapper")
    return {"name": directory.name, "status": "passed", "calls": 3}


class HeldHandoff:
    """Hold the actual side request until abort/EOF has been observed by Host."""
    def __init__(self, incomplete):
        self.incomplete = incomplete
        self.requests, self.errors = [], []
        self.arrived, self.release = threading.Event(), threading.Event()
        owner = self

        class Handler(BaseHTTPRequestHandler):
            def log_message(self, *_):
                pass

            def do_POST(self):
                body = json.loads(self.rfile.read(int(self.headers.get("Content-Length", "0"))))
                owner.requests.append({"body": body})
                side = body.get("tool_choice") == "none"
                if side:
                    owner.arrived.set()
                    if not owner.release.wait(10):
                        owner.errors.append("side release exceeded the bound")
                elif len(owner.requests) > 1 or not owner.incomplete:
                    owner.errors.append("cancelled handoff launched another model request")
                events = shared.text_events(DOCUMENT if side else "Incomplete output to retain on abort.")
                if not side:
                    events[1]["choices"][0]["finish_reason"] = "length"
                encoded = ("".join("data: " + json.dumps(event) + "\n\n" for event in events)
                           + "data: [DONE]\n\n").encode()
                try:
                    self.send_response(200)
                    self.send_header("Content-Type", "text/event-stream")
                    self.send_header("Content-Length", str(len(encoded)))
                    self.end_headers()
                    self.wfile.write(encoded)
                except (BrokenPipeError, ConnectionResetError, ConnectionAbortedError):
                    pass

        self.server = ThreadingHTTPServer(("127.0.0.1", 0), Handler)
        self.server.daemon_threads = True
        self.thread = threading.Thread(target=self.server.serve_forever, daemon=True)
        self.base_url = f"http://127.0.0.1:{self.server.server_port}/v1"

    def __enter__(self):
        self.thread.start()
        return self

    def __exit__(self, *_):
        self.release.set()
        self.server.shutdown()
        self.server.server_close()
        self.thread.join(timeout=2)


def rpc_side_cancellation(trial):
    results = []
    for incomplete in (False, True):
        for stop in ("abort", "eof"):
            name = f"cancel-{'incomplete' if incomplete else 'threshold'}-{stop}"
            directory, work, session = trial.setup(name, seed())
            before = shared.journal(session)
            (directory / "home/agent").mkdir(parents=True, exist_ok=True)
            (directory / "home/agent/config.yml").write_text(
                "compaction:\n  methodOrder: [handoff, soft]\n", encoding="utf-8")
            with HeldHandoff(incomplete) as fake:
                command = [str(trial.binary), "--mode", "rpc", "--model", shared.MODEL,
                    "--base-url", fake.base_url, "--api", "openai-completions", "--cwd", str(work),
                    "--session-dir", str(session.parent), "--resume", str(session), "--no-skills",
                    "--tools", "read", "--max-time", "10", "--max-model-calls", "4",
                    "--compact-keep-tokens", "1", "--compact-threshold", "0" if incomplete else "5000"]
                process = shared.RpcProcess(command, trial.environment(directory), directory, 15)
                started = time.monotonic()
                try:
                    # Prompt's response can be delayed by threshold maintenance.
                    process.send({"id": name, "type": "prompt", "message": "Continue work."})
                    shared.require(fake.arrived.wait(5), "side HTTP request did not begin")
                    stopped = time.monotonic()
                    if stop == "abort":
                        process.command(name + "-abort", "abort")
                    else:
                        process.child.stdin.close()
                        process.child.wait(timeout=3)
                        process.child.stdin = None
                    latency = time.monotonic() - stopped
                    shared.require(latency < 3, "side cancellation did not settle promptly")
                finally:
                    if process.child.stdin is None:
                        for reader in process.readers:
                            reader.join(timeout=1)
                    else:
                        process.finish()
                    fake.release.set()
                after = shared.journal(session)
                shared.require(process.child.returncode == 0, "cancelled RPC process failed")
                shared.require(not any(row["type"] == "compaction" for row in after.values()), "cancelled side published")
                for identity, row in before.items():
                    shared.require(after.get(identity) == row, "cancelled handoff changed original raw entry")
                if incomplete:
                    shared.require(any("Incomplete output to retain on abort." in json.dumps(row)
                                       for row in after.values()), "unpublished recovery did not restore failed output")
                shared.require(len(fake.requests) == (2 if incomplete else 1) and not fake.errors,
                               "cancellation fell back or continued provider work")
                shared.dump(directory / "requests.json", fake.requests)
                results.append({"name": name, "status": "passed", "actualCalls": len(fake.requests),
                                "stopSeconds": round(latency, 3), "seconds": round(time.monotonic() - started, 3)})
    return {"name": "rpc-side-cancellation", "status": "passed", "cases": results}


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--binary", type=Path, required=True)
    parser.add_argument("--output", type=Path, default=Path("C:/Temp/ara-handoff-batch/host"))
    args = parser.parse_args()
    output = args.output / ("run-" + datetime.datetime.now(datetime.timezone.utc).strftime("%Y%m%dT%H%M%SZ"))
    output.mkdir(parents=True)
    before, started = source_pins(), time.monotonic()
    binary_before = shared.digest(args.binary)
    trial = shared.Trial(args.binary, output, 30)
    families, error = [], None
    try:
        for family in (manual_reopen, empty_manual_and_auto_fallback, incomplete_handoff, rpc_side_cancellation):
            families.append(family(trial))
    except Exception as failure:
        error = str(failure)
    after = source_pins()
    code = 0 if error is None and before == after and binary_before == shared.digest(args.binary) else 1
    receipt = {"exitCode": code, "families": families, "failure": error, "steps": trial.steps,
        "seconds": round(time.monotonic() - started, 3), "sourcePinsBefore": before, "sourcePinsAfter": after,
        "sourceUnchanged": before == after, "binarySha256": binary_before,
        "limits": "Controlled Windows REPL manual and RPC threshold/incomplete; no live manual RPC/speculation or real model."}
    shared.dump(output / "receipt.json", receipt)
    print(json.dumps({key: receipt[key] for key in ("exitCode", "families", "failure", "seconds")}))
    return code


if __name__ == "__main__":
    raise SystemExit(main())
