"""Five bounded local-maintenance families through the actual ara Host.

Uses the existing CLI e2e flags, JSONL fixtures and Chat SSE shape, plus the
Windows test environment helper. No build, Git, credentials or live model is
used. The fake upstream listens only on loopback. Each family writes process,
wire, raw Session and artifact receipts; the delivered binary is supplied by
the caller after its module build.
"""
from __future__ import annotations

import argparse
import datetime
import hashlib
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
import json
import os
from pathlib import Path
import queue
import re
import subprocess
import threading
import time
from typing import Any

from verify_backend import windows_test_env

ROOT = Path(__file__).resolve().parents[1]
DEFAULT_OUTPUT = Path("C:/Temp/ara-local-reducers-batch/host")
MODEL = "fake-local-reduction"
MAX_MODEL_CALLS = 3
FAKE_KEY = "local-reducer-fixture-key"
IMAGE = {"type": "image", "mimeType": "image/png", "data":
         "iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAQAAAC1HAwCAAAAC0lEQVR42mP8/x8AAwMCAO+j9b0AAAAASUVORK5CYII="}


def require(condition: bool, message: str) -> None:
    if not condition:
        raise AssertionError(message)


def dump(path: Path, value: Any) -> None:
    path.write_text(json.dumps(value, ensure_ascii=False, indent=2), encoding="utf-8")


def digest(path: Path) -> str:
    return hashlib.sha256(path.read_bytes()).hexdigest()


def source_pins() -> dict[str, str]:
    paths = [ROOT / "Cargo.toml", ROOT / "Cargo.lock", Path(__file__), ROOT / "upstream/omp.lock.json"]
    for package in ("ara-cli", "ara-agent", "ara-session", "ara-tools", "ara-ai"):
        paths.extend((ROOT / "crates" / package / "src").rglob("*.rs"))
    return {str(path.relative_to(ROOT)): digest(path) for path in sorted(set(paths)) if path.is_file()}


def row(kind: str, identity: str, parent: str | None, **fields: Any) -> dict[str, Any]:
    return {"type": kind, "id": identity, "parentId": parent,
            "timestamp": "2026-10-02T00:00:00.000Z", "opaqueReceipt": {"keep": True}, **fields}


def assistant(content: list[dict[str, Any]], reason: str = "stop", known_usage: bool = False) -> dict[str, Any]:
    usage = {"input": 30000, "output": 6000, "cacheRead": 0, "cacheWrite": 0} if known_usage else {}
    return {"role": "assistant", "api": "openai-completions", "provider": "openai-compatible", "model": MODEL,
            "content": content, "stopReason": reason, "usage": usage, "timestamp": 1}


def write_seed(path: Path, work: Path, entries: list[dict[str, Any]]) -> None:
    header = {"type": "session", "version": 3, "id": "local-reducer-host", "cwd": str(work),
              "timestamp": "2026-10-02T00:00:00.000Z"}
    path.write_text("".join(json.dumps(value, ensure_ascii=False) + "\n" for value in [header, *entries]), encoding="utf-8")


def journal(path: Path) -> dict[str, dict[str, Any]]:
    return {value["id"]: value for line in path.read_text(encoding="utf-8").splitlines()
            if (value := json.loads(line)).get("type") not in ("session", "title")}


def preserved(before: dict[str, dict[str, Any]], after: dict[str, dict[str, Any]]) -> None:
    for identity, source in before.items():
        require(identity in after, f"original raw ID disappeared: {identity}")
        for field in ("id", "parentId", "timestamp", "opaqueReceipt"):
            require(after[identity].get(field) == source.get(field), f"original {field} changed: {identity}")


def seed_entries() -> list[dict[str, Any]]:
    fence = "```text\nRECOVERY_SENTINEL\nELIDE_LOST_FENCE\n" + "large fence content " * 1100 + "\n```"
    xml = "<context>\nELIDE_LOST_XML\n" + "large XML content " * 1100 + "\n</context>"
    tool_text = "ELIDE_LOST_TOOL\n" + "large tool result " * 1400
    custom = "<host-context>\nELIDE_LOST_CUSTOM\n" + "large custom context " * 1100 + "\n</host-context>"
    return [
        row("model_change", "model", None, model=f"openai-compatible/{MODEL}"),
        row("message", "original-user", "model", message={"role": "user", "timestamp": 1,
            "content": [{"type": "text", "text": fence + "\nnormal text\n" + xml, "opaqueBlock": "keep"}, IMAGE]}),
        row("message", "original-call", "original-user", message=assistant([
            {"type": "thinking", "thinking": "old thought", "thinkingSignature": "keep-until-shake"},
            {"type": "toolCall", "id": "settled-read", "name": "read", "arguments": {"path": "old.txt"}},
        ], "toolUse")),
        row("message", "original-result", "original-call", message={"role": "toolResult", "timestamp": 1,
            "toolCallId": "settled-read", "toolName": "read", "isError": False,
            "content": [{"type": "text", "text": ""}, IMAGE, {"type": "text", "text": tool_text},
                        {"type": "text", "text": "tail fragment"}],
            "details": {"images": [IMAGE, {"opaque": True}], "keep": "original details"}}),
        row("custom_message", "original-custom", "original-result", customType="host-note", display=False,
            attribution="agent", content=[{"type": "text", "text": custom}, IMAGE]),
        row("message", "original-files", "original-custom", message={"role": "fileMention", "timestamp": 1,
            "files": [{"path": "image.png", "image": IMAGE, "opaque": 23}, {"path": "notes.md", "content": "keep file content"}]}),
        row("message", "legacy-summary", "original-files", message={"role": "compactionSummary", "timestamp": 1,
            "summary": "legacy summary", "tokensBefore": 100, "images": [IMAGE]}),
        # The genuine manual protect window is 4000 tokens. This settled tail
        # deliberately exceeds it, leaving all older heavy regions eligible.
        row("message", "usage-anchor", "legacy-summary", message={**assistant([
            {"type": "thinking", "thinking": "recent thinking"}, {"type": "redactedThinking", "data": "opaque"},
            {"type": "text", "text": "recent live tail content " * 1500}, IMAGE,
        ], known_usage=True), "contextSnapshot": {"promptTokens": 30000, "nonMessageTokens": 30,
            "compactionEpoch": 0, "opaqueSnapshot": "keep"}}),
    ]


def text_events(text: str) -> list[dict[str, Any]]:
    return [{"choices": [{"index": 0, "delta": {"role": "assistant", "content": text}, "finish_reason": None}]},
            {"choices": [{"index": 0, "delta": {}, "finish_reason": "stop"}]},
            {"choices": [], "usage": {"prompt_tokens": 30, "completion_tokens": 4,
             "prompt_tokens_details": {"cached_tokens": 0}}}]


def tool_events(identity: str, path: str) -> list[dict[str, Any]]:
    return [{"choices": [{"index": 0, "delta": {"role": "assistant", "tool_calls": [{"index": 0,
             "id": identity, "type": "function", "function": {"name": "read", "arguments": json.dumps({"path": path})}}]},
             "finish_reason": None}]}, {"choices": [{"index": 0, "delta": {}, "finish_reason": "tool_calls"}]}]


class FakeUpstream:
    def __init__(self) -> None:
        self.responses: list[list[dict[str, Any]]] = []
        self.requests: list[dict[str, Any]] = []
        self.errors: list[str] = []
        self.lock = threading.Lock()
        owner = self

        class Handler(BaseHTTPRequestHandler):
            def log_message(self, *_: Any) -> None:
                pass

            def do_POST(self) -> None:
                try:
                    body = json.loads(self.rfile.read(int(self.headers.get("Content-Length", "0"))))
                    with owner.lock:
                        owner.requests.append({"path": self.path, "body": body,
                                               "authorization": "<fixture redacted>"})
                        events = owner.responses.pop(0) if owner.responses else None
                    if events is None:
                        with owner.lock:
                            owner.errors.append("unexpected model call beyond the scripted budget")
                        self.send_response(400)
                        self.end_headers()
                        self.wfile.write(b'{"error":{"message":"unexpected model call"}}')
                        return
                    payload = "".join("data: " + json.dumps(event) + "\n\n" for event in events) + "data: [DONE]\n\n"
                    encoded = payload.encode("utf-8")
                    self.send_response(200)
                    self.send_header("Content-Type", "text/event-stream")
                    self.send_header("Content-Length", str(len(encoded)))
                    self.end_headers()
                    self.wfile.write(encoded)
                except Exception as error:
                    with owner.lock:
                        owner.errors.append(str(error))

        self.server = ThreadingHTTPServer(("127.0.0.1", 0), Handler)
        self.server.daemon_threads = True
        self.thread = threading.Thread(target=self.server.serve_forever, daemon=True)

    def __enter__(self) -> FakeUpstream:
        self.thread.start()
        return self

    def __exit__(self, *_: Any) -> None:
        self.server.shutdown()
        self.server.server_close()
        self.thread.join(timeout=2)

    @property
    def base_url(self) -> str:
        return f"http://127.0.0.1:{self.server.server_port}/v1"


class Trial:
    def __init__(self, binary: Path, output: Path, timeout: float) -> None:
        self.binary, self.output, self.timeout = binary, output, timeout
        self.steps: list[dict[str, Any]] = []
        self.base_env = windows_test_env() or os.environ.copy()

    def setup(self, name: str, entries: list[dict[str, Any]] | None = None) -> tuple[Path, Path, Path]:
        directory = self.output / name
        work, home, sessions = (directory / value for value in ("work", "home", "sessions"))
        for path in (work, home, sessions):
            path.mkdir(parents=True)
        session = sessions / "seed.jsonl"
        write_seed(session, work, seed_entries() if entries is None else entries)
        dump(directory / "before.json", journal(session))
        return directory, work, session

    def run(self, name: str, fake: FakeUpstream, directory: Path, work: Path, session: Path,
            lines: str, expected_calls: int, continuation: bool = False) -> dict[str, Any]:
        env = self.environment(directory)
        command = [str(self.binary), "--model", MODEL, "--base-url", fake.base_url, "--api", "openai-completions",
                   "--cwd", str(work), "--session-dir", str(session.parent), "--repl", "--no-skills",
                   "--system-prompt", "Controlled local maintenance fixture.", "--tools", "read,bash",
                   "--max-model-calls", str(MAX_MODEL_CALLS), "--max-time", "10", "--compact-threshold", "0"]
        command.extend(["--continue"] if continuation else ["--resume", str(session)])
        start, calls = time.monotonic(), len(fake.requests)
        timed_out = False
        try:
            result = subprocess.run(command, input=lines.encode("utf-8"), cwd=work, env=env, capture_output=True, timeout=self.timeout)
        except subprocess.TimeoutExpired as error:
            timed_out = True
            result = subprocess.CompletedProcess(command, 124, error.stdout or b"", error.stderr or b"")
        (directory / f"{name}.stdout.log").write_bytes(result.stdout)
        (directory / f"{name}.stderr.log").write_bytes(result.stderr)
        (directory / f"{name}.session.jsonl").write_bytes(session.read_bytes())
        with fake.lock:
            actual = len(fake.requests) - calls
            dump(directory / "requests.json", fake.requests)
            dump(directory / "upstream-errors.json", fake.errors)
        step = {"name": name, "exitCode": result.returncode, "seconds": round(time.monotonic() - start, 3),
                "modelCalls": actual, "expectedModelCalls": expected_calls, "maxModelCallsPerPrompt": MAX_MODEL_CALLS,
                "processTimeoutSeconds": self.timeout, "timedOut": timed_out, "command": command, "sessionSha256": digest(session)}
        self.steps.append(step)
        dump(self.output / "steps.json", self.steps)
        print(json.dumps({key: value for key, value in step.items() if key != "command"}), flush=True)
        require(not timed_out, f"{name}: process timed out; inspect partial process and Session receipts")
        require(result.returncode == 0, f"{name}: process failed; inspect stderr receipt")
        require(actual == expected_calls, f"{name}: expected {expected_calls} model calls, observed {actual}")
        require(FAKE_KEY.encode() not in result.stdout + result.stderr, "fixture auth appeared in process output")
        require(not fake.errors, f"{name}: fake upstream rejected a request: {fake.errors}")
        return {"stdout": result.stdout.decode("utf-8", errors="replace"), "stderr": result.stderr.decode("utf-8", errors="replace")}

    def environment(self, directory: Path) -> dict[str, str]:
        env = {key: value for key, value in self.base_env.items() if not key.startswith(
            ("ARA_", "OPENAI_", "OPENROUTER_", "ANTHROPIC_", "CAS_", "CLAUDE_", "COPILOT_"))
            and key not in ("WSL_DISTRO_NAME", "WSL_INTEROP")}
        env.update({"HOME": str(directory / "home"), "USERPROFILE": str(directory / "home"),
                    "ARA_HOME": str(directory / "home"), "ARA_API_KEY": FAKE_KEY})
        return env


def no_resurrection(request: dict[str, Any]) -> None:
    require("ELIDE_LOST_" not in json.dumps(request["body"]["messages"]), "elided original text revived in provider context")


def content_text(content: Any) -> str:
    if isinstance(content, str):
        return content
    if isinstance(content, list):
        return "\n".join(block.get("text", "") for block in content if isinstance(block, dict))
    return ""


def elide_recovery_restart(trial: Trial) -> dict[str, Any]:
    directory, work, session = trial.setup("elide-recovery-restart")
    before = journal(session)
    with FakeUpstream() as fake:
        trial.run("shake-default-elide", fake, directory, work, session, "/shake\n/exit\n", 0)
        after = journal(session)
        preserved(before, after)
        for identity in ("original-user", "original-result", "original-custom"):
            require("ELIDE_LOST_" not in json.dumps(after[identity]), f"{identity}: native elide did not remove heavy regions")
        require(after["original-result"]["message"]["content"][0] == IMAGE, "elide lost non-text tool blocks/order")
        anchor = after["usage-anchor"]["message"]
        require(anchor["usage"] == before["usage-anchor"]["message"]["usage"], "anchor usage was overwritten")
        require(anchor["contextSnapshot"].get("historyRewriteTokensRemoved", 0) > 0, "pre-anchor savings were not persisted")
        require(anchor["contextSnapshot"]["opaqueSnapshot"] == "keep", "anchor opaque snapshot fields were lost")
        match = re.search(r"artifact://([0-9]+) \(region [1-9][0-9]*\)", json.dumps(after))
        require(match is not None, "persistent elide lacks an artifact recovery placeholder")
        identity = match.group(1)
        artifacts = list(session.with_suffix("").glob(f"{identity}.*.log"))
        require(len(artifacts) == 1, "artifact did not publish to the selected Session store")
        artifact = artifacts[0].read_text(encoding="utf-8")
        require("RECOVERY_SENTINEL" in artifact and "ELIDE_LOST_TOOL" in artifact, "artifact lacks original removable text")
        fake.responses.extend([tool_events("recover-artifact", f"artifact://{identity}:raw:3-4"), text_events("recovery read accepted")])
        trial.run("artifact-read-selector", fake, directory, work, session, "recover selected artifact lines\n/exit\n", 2)
        no_resurrection(fake.requests[0])
        tool = next((message for message in fake.requests[1]["body"]["messages"]
                     if message.get("role") == "tool" and message.get("tool_call_id") == "recover-artifact"), None)
        require(tool is not None and "RECOVERY_SENTINEL" in json.dumps(tool["content"]), "actual read tool did not recover selected artifact lines")
        require("ELIDE_LOST_" not in json.dumps(tool["content"]), "artifact selector read beyond its selected lines")
        fake.responses.append(text_events("restart accepted"))
        trial.run("continue-restart", fake, directory, work, session, "after restart\n/exit\n", 1, continuation=True)
        no_resurrection(fake.requests[2])
        final = journal(session)
        for identity in before:
            require(final[identity] == after[identity], f"restart or recovery rewrote original source {identity}")
        dump(directory / "requests.json", fake.requests)
        dump(directory / "after.json", final)
        return {"name": "elide-recovery-restart", "artifact": str(artifacts[0]), "artifactSha256": digest(artifacts[0]), "modelCalls": len(fake.requests)}


def images_thinking_restart(trial: Trial) -> dict[str, Any]:
    directory, work, session = trial.setup("images-thinking-restart")
    before = journal(session)
    with FakeUpstream() as fake:
        trial.run("shake-images-thinking", fake, directory, work, session, "/shake images\n/shake thinking\n/exit\n", 0)
        after = journal(session)
        preserved(before, after)
        require(after["original-user"]["message"]["content"] == before["original-user"]["message"]["content"][:1], "user image removal changed original text")
        require(after["original-result"]["message"]["details"]["images"] == [{"opaque": True}], "details.images removal lost opaque values")
        require("image" not in after["original-files"]["message"]["files"][0], "FileMention image remains")
        for identity in ("original-call", "usage-anchor"):
            wanted = [block for block in before[identity]["message"]["content"] if block["type"] not in ("thinking", "redactedThinking")]
            require(after[identity]["message"]["content"] == wanted, "thinking shake changed non-thinking blocks")
        require(after["legacy-summary"] == before["legacy-summary"], "image shake changed an excluded native legacy summary")
        fake.responses.append(text_events("image/thinking restart accepted"))
        trial.run("continue-native-files", fake, directory, work, session, "after image rewrite\n/exit\n", 1, continuation=True)
        file_message = next((message for message in fake.requests[0]["body"]["messages"]
                             if '<file path="image.png">' in content_text(message.get("content"))), None)
        # The default Chat compatibility policy maps canonical Developer to
        # User when supports_developer_role=false. Session's module family
        # separately proves its exact native Developer fragment.
        require(file_message is not None and file_message.get("role") in ("developer", "user"), "rewritten FileMention disappeared from the compatible wire projection")
        require("image_url" not in json.dumps(file_message.get("content")), "FileMention wire fragment retained its removed image")
        for identity in before:
            require(journal(session)[identity] == after[identity], f"restart resurrected a raw field: {identity}")
        dump(directory / "requests.json", fake.requests)
        dump(directory / "after.json", journal(session))
        return {"name": "images-thinking-restart", "modelCalls": len(fake.requests),
                "fileMentionWireRole": file_message["role"], "canonicalDeveloperCoverage": "Session module family"}


def artifact_failure(trial: Trial) -> dict[str, Any]:
    directory, work, session = trial.setup("artifact-disk-failure")
    blocker = session.with_suffix("")
    blocker.write_text("controlled artifact directory obstruction", encoding="utf-8")
    before = journal(session)
    with FakeUpstream() as fake:
        trial.run("shake-bare-fallback", fake, directory, work, session, "/shake elide\n/exit\n", 0)
        after = journal(session)
        preserved(before, after)
        rewritten = json.dumps([after[identity] for identity in ("original-user", "original-result", "original-custom")])
        require("ELIDE_LOST_" not in rewritten, "artifact failure prevented the native local rewrite")
        require("[shaken ~" in rewritten and "artifact://" not in rewritten, "artifact failure did not use native bare placeholders")
        require(blocker.read_text(encoding="utf-8") == "controlled artifact directory obstruction", "artifact failure changed the blocking original file")
        fake.responses.append(text_events("bare fallback restart accepted"))
        trial.run("continue-bare-fallback", fake, directory, work, session, "after failed artifact storage\n/exit\n", 1, continuation=True)
        no_resurrection(fake.requests[0])
        dump(directory / "requests.json", fake.requests)
        dump(directory / "after.json", journal(session))
        return {"name": "artifact-disk-failure", "modelCalls": len(fake.requests), "failure": "artifact directory is a regular file"}


def unknown_effect_and_rejected_mode(trial: Trial) -> dict[str, Any]:
    unknown = [row("model_change", "model", None, model=f"openai-compatible/{MODEL}"),
               row("message", "user", "model", message={"role": "user", "timestamp": 1, "content": "inspect effects before retry"}),
               row("message", "unknown-call", "user", message=assistant([
                   {"type": "toolCall", "id": "unknown-effect", "name": "bash",
                    "arguments": {"command": "printf REPLAYED >> replay-guard.txt"}},
               ], "toolUse"))]
    directory, work, session = trial.setup("unknown-effect-rejected-mode", unknown)
    guard = work / "replay-guard.txt"
    guard.write_text("external effect already observed\n", encoding="utf-8")
    with FakeUpstream() as fake:
        fake.responses.append(text_events("unknown effects retained"))
        trial.run("resume-unknown-effects", fake, directory, work, session, "continue without replay\n/exit\n", 1)
        require(guard.read_text(encoding="utf-8") == "external effect already observed\n", "an unknown-effect tool was replayed")
        entries = journal(session)
        receipts = [entry["message"] for entry in entries.values() if entry.get("type") == "message"
                    and entry.get("message", {}).get("toolCallId") == "unknown-effect"]
        require(len(receipts) == 1 and receipts[0].get("details", {}).get("source") == "interrupted_unknown_effect", "resume lacks its original unknown-effect receipt")
        before = session.read_bytes()
        result = trial.run("reject-unsupported-shake", fake, directory, work, session, "/shake unsupported\n/exit\n", 0)
        require(session.read_bytes() == before, "unsupported mode rewrote the Session")
        require("shake" in result["stderr"].lower(), "unsupported shake did not report its refusal")
        require(guard.read_text(encoding="utf-8") == "external effect already observed\n", "unsupported mode replayed an unknown tool")
        dump(directory / "requests.json", fake.requests)
        dump(directory / "after.json", journal(session))
        return {"name": "unknown-effect-rejected-mode", "modelCalls": len(fake.requests), "effectSha256": digest(guard)}


class RpcProcess:
    """One child and one deadline for the concentrated artifact routing flow."""
    def __init__(self, command: list[str], env: dict[str, str], directory: Path, timeout: float) -> None:
        self.deadline = time.monotonic() + timeout
        self.frames: queue.Queue[Any] = queue.Queue()
        self.seen: list[dict[str, Any]] = []
        self.child = subprocess.Popen(command, env=env, cwd=directory, stdin=subprocess.PIPE,
                                      stdout=subprocess.PIPE, stderr=subprocess.PIPE)

        def output_reader() -> None:
            with (directory / "rpc.stdout.log").open("wb") as log:
                for line in self.child.stdout:
                    log.write(line)
                    log.flush()
                    try:
                        self.frames.put(json.loads(line))
                    except json.JSONDecodeError as error:
                        self.frames.put(error)
                self.frames.put(None)

        def error_reader() -> None:
            with (directory / "rpc.stderr.log").open("wb") as log:
                while block := self.child.stderr.read(4096):
                    log.write(block)
                    log.flush()

        self.readers = [threading.Thread(target=output_reader, daemon=True), threading.Thread(target=error_reader, daemon=True)]
        for reader in self.readers:
            reader.start()

    def send(self, value: dict[str, Any]) -> None:
        self.child.stdin.write((json.dumps(value) + "\n").encode("utf-8"))
        self.child.stdin.flush()

    def until(self, matches: Any) -> dict[str, Any]:
        while True:
            remaining = self.deadline - time.monotonic()
            require(remaining > 0, "RPC artifact family exceeded its shared deadline")
            try:
                frame = self.frames.get(timeout=remaining)
            except queue.Empty:
                raise AssertionError("RPC artifact family timed out; inspect retained process receipts") from None
            require(isinstance(frame, dict), "RPC stdout ended or contained invalid JSONL")
            self.seen.append(frame)
            if matches(frame):
                return frame

    def command(self, identity: str, kind: str, **fields: Any) -> dict[str, Any]:
        self.send({"id": identity, "type": kind, **fields})
        response = self.until(lambda frame: frame.get("type") == "response" and frame.get("id") == identity)
        require(response.get("success") is True, f"RPC command failed: {identity}")
        return response.get("data", {})

    def finish(self) -> None:
        self.child.stdin.close()
        try:
            self.child.wait(timeout=max(0.05, self.deadline - time.monotonic()))
        except subprocess.TimeoutExpired:
            self.child.kill()
            self.child.wait(timeout=2)
        for reader in self.readers:
            reader.join(timeout=1)


def large_artifact_session_routing(trial: Trial) -> dict[str, Any]:
    entries = [row("model_change", "model", None, model=f"openai-compatible/{MODEL}"),
               row("message", "user", "model", message={"role": "user", "timestamp": 1, "content": "artifact routing source"}),
               row("message", "answer", "user", message=assistant([{"type": "text", "text": "Ready for routing."}]))]
    directory, work, first = trial.setup("large-artifact-session-routing", entries)
    second = first.with_name("second.jsonl")
    write_seed(second, work, entries)
    values = [json.loads(line) for line in second.read_text(encoding="utf-8").splitlines()]
    values[0]["id"] = "local-reducer-host-second"
    second.write_text("".join(json.dumps(value) + "\n" for value in values), encoding="utf-8")
    artifacts = []
    for session, label in ((first, "SESSION_A_ARTIFACT"), (second, "SESSION_B_ARTIFACT")):
        session.with_suffix("").mkdir()
        artifact = session.with_suffix("") / "0.routing.log"
        artifact.write_bytes((label + "\nSELECTED_LINE_TWO\n").encode() + b"UNSELECTED_LARGE_FILLER\n" * 410_000)
        require(artifact.stat().st_size > 8 * 1024 * 1024, "routing artifact is not above the inline limit")
        artifacts.append({"path": str(artifact), "bytes": artifact.stat().st_size, "sha256": digest(artifact)})
    dump(directory / "artifacts.json", artifacts)
    started, rpc = time.monotonic(), None
    with FakeUpstream() as fake:
        command = [str(trial.binary), "--mode", "rpc", "--model", MODEL, "--base-url", fake.base_url,
                   "--api", "openai-completions", "--cwd", str(work), "--session-dir", str(first.parent),
                   "--resume", str(first), "--no-skills", "--tools", "read", "--compact-threshold", "0",
                   "--max-model-calls", str(MAX_MODEL_CALLS), "--max-time", "10"]
        try:
            rpc = RpcProcess(command, trial.environment(directory), directory, trial.timeout)
            rpc.until(lambda frame: frame.get("type") == "ready")
            rpc.until(lambda frame: frame.get("type") == "available_commands_update")
            original = rpc.command("state-a", "get_state")

            def read_case(identity: str, path: str, expected: str, is_error: bool = False) -> None:
                with fake.lock:
                    fake.responses.extend([tool_events(identity, path), text_events("routing receipt inspected")])
                begin = len(rpc.seen)
                rpc.command("prompt-" + identity, "prompt", message="inspect the requested artifact routing receipt")
                rpc.until(lambda frame: frame.get("type") == "agent_end")
                frames = rpc.seen[begin:]
                end = next((frame for frame in frames if frame.get("type") == "tool_execution_end" and frame.get("toolCallId") == identity), None)
                require(end is not None and end.get("isError") is is_error, f"artifact read outcome was wrong: {identity}")
                text = content_text(end["result"]["content"])
                require(expected in text, f"artifact routing resolved the wrong source: {identity}")
                require("UNSELECTED_LARGE_FILLER" not in text, "selector admitted the unselected oversized body")
                require(not any(frame.get("type") == "host_uri_request" for frame in frames), "native/Removed route dispatched to the Host peer")

            read_case("full-inline-block", "artifact://0", "full internal resolution is blocked", True)
            read_case("selected-a", "artifact://0:raw:1-2", "SESSION_A_ARTIFACT")
            adopted = rpc.command("switch-b", "switch_session", sessionPath=str(second))
            require(not adopted.get("cancelled", False), "switch to second Session was cancelled")
            changed = rpc.command("state-b", "get_state")
            require(changed["sessionId"] != original["sessionId"], "RPC did not adopt the second Session")
            read_case("selected-b", "artifact://0:raw:1-2", "SESSION_B_ARTIFACT")
            rpc.command("switch-a", "switch_session", sessionPath=str(first))
            require(rpc.command("restored-a", "get_state")["sessionId"] == original["sessionId"], "RPC did not restore the first Session")
            read_case("selected-a-again", "artifact://0:raw:1-2", "SESSION_A_ARTIFACT")
            registered = rpc.command("override-artifact", "set_host_uri_schemes", schemes=[{"scheme": "artifact"}])
            require(registered.get("schemes") == ["artifact"], "explicit artifact override was not registered")
            removed = rpc.command("remove-artifact", "set_host_uri_schemes", schemes=[])
            require(removed.get("schemes") == [], "explicit artifact override was not removed")
            read_case("removed-no-fallback", "artifact://0:raw:1-2", "Unknown protocol: artifact://", True)
            require(len(fake.requests) == 10 and not fake.errors, "routing flow exceeded its ten scripted model calls")
            require(all(digest(Path(item["path"])) == item["sha256"] for item in artifacts), "artifact routing mutated immutable files")
            return {"name": "large-artifact-session-routing", "modelCalls": len(fake.requests),
                    "artifactBytes": [item["bytes"] for item in artifacts], "sessionSwitches": 2,
                    "removedRoute": "explicit registered artifact override removed; native fallback refused"}
        finally:
            if rpc is not None:
                rpc.finish()
                dump(directory / "rpc-frames.json", rpc.seen)
                step = {"name": "large-artifact-session-routing", "exitCode": rpc.child.returncode,
                        "seconds": round(time.monotonic() - started, 3), "modelCalls": len(fake.requests),
                        "expectedModelCalls": 10, "processTimeoutSeconds": trial.timeout, "command": command}
                trial.steps.append(step)
                dump(trial.output / "steps.json", trial.steps)
                print(json.dumps({key: value for key, value in step.items() if key != "command"}), flush=True)
                for path in (first, second):
                    (directory / (path.stem + ".after.jsonl")).write_bytes(path.read_bytes())
            dump(directory / "requests.json", fake.requests)
            dump(directory / "upstream-errors.json", fake.errors)
            if rpc is not None:
                require(rpc.child.returncode == 0, "RPC artifact child failed or exceeded deadline")


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--binary", type=Path, default=ROOT / "target/debug" / ("ara.exe" if os.name == "nt" else "ara"))
    parser.add_argument("--output", type=Path, default=DEFAULT_OUTPUT)
    parser.add_argument("--process-timeout", type=float, default=35.0)
    args = parser.parse_args()
    require(args.binary.is_file(), "supplied ara binary is unavailable; caller owns its build")
    require(0 < args.process_timeout <= 60, "process timeout must be between zero and 60 seconds")
    stamp = datetime.datetime.now(datetime.timezone.utc).strftime("%Y%m%dT%H%M%S.%fZ")
    output = args.output / ("run-" + stamp)
    output.mkdir(parents=True, exist_ok=False)
    pins, binary_pin, start = source_pins(), digest(args.binary), time.monotonic()
    trial = Trial(args.binary.resolve(), output.resolve(), args.process_timeout)
    families: list[dict[str, Any]] = []
    failure = None
    code = 0
    for family in (elide_recovery_restart, images_thinking_restart, artifact_failure, unknown_effect_and_rejected_mode,
                   large_artifact_session_routing):
        try:
            result = family(trial)
            families.append({**result, "status": "passed"})
            print(json.dumps(families[-1]), flush=True)
        except Exception as error:
            failure, code = f"{type(error).__name__}: {error}", 1
            families.append({"name": family.__name__, "status": "failed", "error": failure})
            break
    after = source_pins()
    unchanged = pins == after and digest(args.binary) == binary_pin
    if not unchanged:
        code = 125
    receipt = {"startedAt": stamp, "seconds": round(time.monotonic() - start, 3), "exitCode": code,
               "binary": str(args.binary.resolve()), "binarySha256": binary_pin, "sourceUnchanged": unchanged,
               "sourcePinsBefore": pins, "sourcePinsAfter": after, "families": families, "steps": trial.steps,
               "failure": failure, "coverage": "controlled fake upstream with actual ara process and Host file effects",
               "liveModelTrial": "not run; this runner never reads local provider credentials"}
    dump(output / "receipt.json", receipt)
    print(json.dumps({"receipt": str(output / "receipt.json"), "exitCode": code,
                      "seconds": receipt["seconds"], "sourceUnchanged": unchanged}), flush=True)
    return code


if __name__ == "__main__":
    raise SystemExit(main())
