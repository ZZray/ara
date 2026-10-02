"""Grouped remote-compaction flows through one supplied actual Rust RPC Host.

Loopback fixtures cover native full history/repeat/reopen, portable fallback,
the two generic calls, threshold/incomplete recovery and reader Abort/EOF.
The caller builds the binary once. This runner does not build, use credentials
from this machine, contact a live provider, or stand for full remote parity.
"""
from __future__ import annotations

import argparse
import datetime
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
import json
from pathlib import Path
import threading
import time

import verify_handoff_host as handoff
import verify_local_reduction_host as shared

PROVIDER = "custom-remote-fixture"
FULL = "Remote full readable summary preserving Oak-427 and current work."
SHORT = "Oak-427: continue current work."
SECOND = "REMOTE_NEW_FULL_HISTORY_SENTINEL"
OPAQUE = "fixture-native-encrypted-history"


def retained(text):
    # A complete retained User turn must exceed the configured keep budget;
    # otherwise the fixed upstream cut correctly selects the first old User.
    return text + " retained" * 600


def source_pins():
    pins = shared.source_pins()
    for path in (Path(__file__), Path(handoff.__file__), Path(shared.__file__)):
        pins[str(path.relative_to(shared.ROOT))] = shared.digest(path)
    return pins


def seed(*, native=True):
    entries = handoff.seed()
    entries[-2]["message"]["content"] = retained("Keep current work.")
    if not native:
        return entries
    entries[0]["model"] = f"{PROVIDER}/{shared.MODEL}"
    for entry in entries:
        message = entry.get("message", {})
        if message.get("role") == "assistant":
            message.update(api="openai-responses", provider=PROVIDER)
    entries[1]["message"]["content"] = [
        {"type": "text", "text": entries[1]["message"]["content"]}, shared.IMAGE]
    reasoning = {"type": "reasoning", "encrypted_content": "source-reasoning", "summary": [], "dt": False}
    entries[2]["message"]["content"].insert(0, {
        "type": "thinking", "thinking": "past thought", "thinkingSignature": json.dumps(reasoning)})
    return entries


def response_events(text, incomplete=False):
    item = {"type": "message", "id": "fixture-message", "role": "assistant", "status": "completed",
            "content": [{"type": "output_text", "text": text}]}
    terminal = {"id": "fixture-response", "status": "incomplete" if incomplete else "completed",
                "usage": {"input_tokens": 70, "output_tokens": 4, "total_tokens": 74}}
    if incomplete:
        terminal["incomplete_details"] = {"reason": "max_output_tokens"}
    return [{"type": "response.output_item.done", "output_index": 0, "item": item},
            {"type": "response.incomplete" if incomplete else "response.completed", "response": terminal}]


class RemoteUpstream:
    def __init__(self, directory, *, incomplete=False, held=False, generic=False):
        self.directory, self.incomplete, self.held, self.generic = directory, incomplete, held, generic
        self.requests, self.errors = [], []
        self.arrived, self.release = threading.Event(), threading.Event()
        self.primary_count = self.compact_count = self.generic_count = 0
        self.lock = threading.Lock()
        owner = self

        class Handler(BaseHTTPRequestHandler):
            def log_message(self, *_):
                pass

            def do_POST(self):
                body = json.loads(self.rfile.read(int(self.headers.get("Content-Length", "0"))))
                native = self.path.endswith("/responses/compact")
                generic = self.path.endswith("/chat/completions") and body.get("stream") is False
                with owner.lock:
                    owner.requests.append({"path": self.path, "body": body})
                    if self.headers.get("Authorization") != "Bearer " + shared.FAKE_KEY:
                        owner.errors.append("request did not use the selected fixture credential")
                    if native:
                        owner.compact_count += 1
                        index = owner.compact_count
                    elif generic:
                        owner.generic_count += 1
                        index = owner.generic_count
                    else:
                        owner.primary_count += 1
                        index = owner.primary_count
                if native and owner.held:
                    owner.arrived.set()
                    if not owner.release.wait(10):
                        owner.errors.append("held request release exceeded its bound")
                if native:
                    payload = {"output": [{"type": "compaction", "encrypted_content": OPAQUE + f"-{index}"}],
                               "usage": {"input_tokens": 1000, "output_tokens": 5}}
                    encoded, content_type = json.dumps(payload).encode(), "application/json"
                elif generic:
                    payload = {"choices": [{"message": {"content": FULL if index == 1 else SHORT}}]}
                    encoded, content_type = json.dumps(payload).encode(), "application/json"
                else:
                    text = "Incomplete output to retain on abort." if owner.incomplete and index == 1 else "Continued work."
                    events = (shared.text_events(text) if owner.generic else
                              response_events(text, owner.incomplete and index == 1))
                    encoded = ("".join("data: " + json.dumps(event) + "\n\n" for event in events)
                               + "data: [DONE]\n\n").encode()
                    content_type = "text/event-stream"
                try:
                    self.send_response(200)
                    self.send_header("Content-Type", content_type)
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
        shared.dump(self.directory / "requests.json", self.requests)
        shared.dump(self.directory / "upstream-errors.json", self.errors)


def configure(directory, fake, *, provider=PROVIDER, disabled=False, generic=False):
    agent = directory / "home/agent"
    agent.mkdir(parents=True, exist_ok=True)
    settings = "compaction:\n  methodOrder: [remote, soft]\n  supersedeReads: false\n  dropUseless: false\n"
    settings += "  remoteEnabled: " + ("false" if disabled else "true") + "\n"
    if generic:
        settings += "  remoteEndpoint: " + fake.base_url + "/chat/completions\n"
    (agent / "config.yml").write_text(settings, encoding="utf-8")
    model = {"id": shared.MODEL, "input": ["text", "image"], "contextWindow": 1000000,
             "remoteCompaction": {"enabled": True, "v2StreamingEnabled": False,
                                  "model": "compact-wire-fixture"}}
    shared.dump(agent / "models.json", {"providers": {provider: {
        "api": "openai-completions" if generic else "openai-responses", "baseUrl": fake.base_url,
        "apiKey": "ARA_API_KEY", "models": [model]}}})
    return agent / "models.json"


def command(trial, directory, work, session, models, *, provider=PROVIDER, threshold=0):
    return [str(trial.binary), "--mode", "rpc", "--provider", provider, "--model", shared.MODEL,
            "--models-config", str(models), "--cwd", str(work), "--session-dir", str(session.parent),
            "--resume", str(session), "--no-skills", "--system-prompt", "Fixed remote Host fixture.",
            "--tools", "read", "--compact-keep-tokens", "1000", "--compact-threshold", str(threshold),
            "--max-model-calls", "4", "--max-time", "10"]


def opened(trial, directory, work, session, models, **options):
    process = shared.RpcProcess(command(trial, directory, work, session, models, **options),
                                trial.environment(models.parent.parent.parent), directory, 20)
    process.until(lambda frame: frame.get("type") == "ready")
    return process


def prompt(process, identity, text, fake, expected_calls):
    first = len(fake.requests)
    process.command(identity, "prompt", message=text)
    while True:
        result = process.until(lambda frame: frame.get("type") == "agent_end"
                               or frame.get("type") == "response" and frame.get("success") is False)
        shared.require(result.get("type") == "agent_end", "prompt failed: " + str(result.get("error")))
        if len(fake.requests) - first >= expected_calls:
            break
    shared.require(len(fake.requests) - first == expected_calls, "unexpected prompt request budget")


def finished(process, directory, session):
    process.finish()
    shared.require(process.child.returncode == 0, "RPC Host process failed; inspect retained stderr")
    shared.require(shared.FAKE_KEY.encode() not in (directory / "rpc.stdout.log").read_bytes()
                   + (directory / "rpc.stderr.log").read_bytes(), "fixture credential leaked into process output")
    (directory / "after.session.jsonl").write_bytes(session.read_bytes())


def native_full_repeat_reopen(trial):
    directory, work, session = trial.setup("native-full-repeat-reopen", seed())
    before = shared.journal(session)
    with RemoteUpstream(directory) as fake:
        models = configure(directory, fake)
        process = opened(trial, directory, work, session, models)
        try:
            result = process.command("compact-1", "compact", customInstructions="Preserve Oak-427.")
            shared.require(result.get("method") == "remote" and result.get("attemptReceipt"), "manual remote receipt missing")
            full = fake.requests[0]["body"]["input"]
            wire = json.dumps(full)
            shared.require(handoff.HEAVY in wire and "Oak-427" in wire, "native request omitted full historical text")
            shared.require("function_call" in wire and "function_call_output" in wire
                           and "source-reasoning" in wire and "input_image" in wire, "native history lost a wire family")
            shared.require(fake.requests[0]["body"]["model"] == "compact-wire-fixture", "configured native model missing")
            prompt(process, "new-heavy", SECOND + " later" * 16000, fake, 1)
            prompt(process, "new-tail", retained("Recent retained work."), fake, 1)
            result = process.command("compact-2", "compact")
            shared.require(result.get("method") == "remote", "repeat compaction did not use native remote")
            repeated = json.dumps(fake.requests[-1]["body"]["input"])
            shared.require(OPAQUE + "-1" in repeated and SECOND in repeated,
                           "repeat omitted prior replacement or new complete history")
            shared.require(handoff.HEAVY not in repeated, "repeat resurrected already absorbed history")
        finally:
            finished(process, directory, session)
        first = len(fake.requests)
        reopened = directory / "reopen"
        reopened.mkdir()
        process = opened(trial, reopened, work, session, models)
        try:
            prompt(process, "reopened", "Continue after restart.", fake, 1)
        finally:
            finished(process, reopened, session)
        resumed = json.dumps(fake.requests[first]["body"]["input"])
        shared.require(OPAQUE + "-2" in resumed and handoff.HEAVY not in resumed and SECOND not in resumed,
                       "cold reopen did not replay the newest replacement exclusively")
        after = shared.journal(session)
        for identity, original in before.items():
            shared.require(after.get(identity) == original, "remote compaction rewrote an original source entry")
        compactions = [entry for entry in after.values() if entry["type"] == "compaction"]
        shared.require(len(compactions) == 2 and all(entry.get("preserveData") for entry in compactions),
                       "repeat native preserveData missing")
        shared.require(not fake.errors, "fixture rejected route authentication")
    return {"name": directory.name, "status": "passed", "calls": len(fake.requests), "session": str(session)}


def portable_disabled_foreign(trial):
    cases = []
    for disabled in (True, False):
        name = "portable-" + ("disabled" if disabled else "foreign")
        directory, work, session = trial.setup(name, seed())
        with RemoteUpstream(directory) as fake:
            models = configure(directory, fake)
            process = opened(trial, directory, work, session, models)
            try:
                process.command("native-first", "compact")
            finally:
                finished(process, directory, session)
            before = shared.journal(session)
            provider = PROVIDER if disabled else "foreign-remote-fixture"
            models = configure(directory, fake, provider=provider, disabled=disabled)
            reopened = directory / "portable"
            reopened.mkdir()
            process = opened(trial, reopened, work, session, models, provider=provider)
            try:
                prompt(process, "portable", "Continue using readable history.", fake, 1)
            finally:
                finished(process, reopened, session)
            wire = json.dumps(fake.requests[-1]["body"]["input"])
            shared.require(handoff.HEAVY in wire and "Oak-427" in wire and OPAQUE not in wire,
                           "disabled or foreign route dropped real history or replayed opaque state")
            after = shared.journal(session)
            for identity, original in before.items():
                shared.require(after.get(identity) == original, "portable projection rewrote original receipts")
            cases.append({"name": name, "calls": len(fake.requests), "status": "passed"})
    return {"name": "portable-disabled-foreign", "status": "passed", "cases": cases}


def generic_two_calls(trial):
    directory, work, session = trial.setup("generic-two-calls", seed(native=False))
    before = shared.journal(session)
    with RemoteUpstream(directory, generic=True) as fake:
        models = configure(directory, fake, generic=True)
        process = opened(trial, directory, work, session, models)
        try:
            result = process.command("generic", "compact", customInstructions="Focus on Oak-427.")
            shared.require(result.get("method") == "remote", "generic method receipt missing")
            shared.require(fake.generic_count == 2 and fake.primary_count == 0 and fake.compact_count == 0,
                           "generic endpoint did not use exactly full plus short summary")
            for request in fake.requests:
                shared.require(request["body"]["model"] == "compact-wire-fixture", "generic model override missing")
                shared.require(request["body"].get("stream") is False and "tools" not in request["body"],
                               "generic request admitted tools or an ordinary stream")
            shared.require(handoff.HEAVY in json.dumps(fake.requests[0]["body"]), "generic summary lost source text")
            shared.require(FULL in json.dumps(fake.requests[1]["body"]), "short summary omitted full result")
            prompt(process, "generic-next", "Continue now.", fake, 1)
        finally:
            finished(process, directory, session)
        after = shared.journal(session)
        for identity, original in before.items():
            shared.require(after.get(identity) == original, "generic remote changed an original entry")
        compact = next(entry for entry in after.values() if entry["type"] == "compaction")
        shared.require(compact["summary"].startswith(FULL) and compact["shortSummary"] == SHORT
                       and "preserveData" not in compact, "generic result was stored as opaque native history")
        wire = json.dumps(fake.requests[-1]["body"]["messages"])
        shared.require(FULL in wire and handoff.HEAVY not in wire, "generic continuation projection differs")
    return {"name": directory.name, "status": "passed", "calls": len(fake.requests)}


def automatic_recovery(trial):
    cases = []
    for incomplete in (False, True):
        name = "automatic-" + ("incomplete" if incomplete else "threshold")
        directory, work, session = trial.setup(name, seed())
        with RemoteUpstream(directory, incomplete=incomplete) as fake:
            models = configure(directory, fake)
            process = opened(trial, directory, work, session, models, threshold=0 if incomplete else 5000)
            try:
                prompt(process, name, "Continue the original task.", fake, 3 if incomplete else 2)
                if incomplete:
                    # The incomplete recovery's User was absorbed into replay.
                    # Its new Assistant still belongs to that checked summary.
                    prompt(process, "recovery-heavy", SECOND + " later" * 16000, fake, 1)
                    prompt(process, "recovery-tail", retained("Recent recovery work."), fake, 1)
                    result = process.command("recovery-repeat", "compact")
                    shared.require(result.get("method") == "remote", "repeat recovery compaction failed")
            finally:
                finished(process, directory, session)
            compactions = [entry for entry in shared.journal(session).values() if entry["type"] == "compaction"]
            shared.require(len(compactions) == (2 if incomplete else 1)
                           and all(entry["method"] == "remote" for entry in compactions), "automatic remote publication missing")
            shared.require(OPAQUE in json.dumps(fake.requests[-1]["body"]["input"]), "automatic continuation omitted native replay")
            if incomplete:
                side = next(request for request in fake.requests if request["path"].endswith("/compact"))
                shared.require("Incomplete output to retain on abort." not in json.dumps(side["body"]),
                               "incomplete recovery revived its failed output")
            shared.require(not fake.errors, "automatic fixture route failed")
            cases.append({"name": name, "status": "passed", "calls": len(fake.requests)})
    return {"name": "automatic-threshold-incomplete", "status": "passed", "cases": cases}


def reader_cancellation(trial):
    cases = []
    for incomplete in (False, True):
        for stop in ("abort", "eof"):
            name = f"cancel-{'incomplete' if incomplete else 'threshold'}-{stop}"
            directory, work, session = trial.setup(name, seed())
            before = shared.journal(session)
            with RemoteUpstream(directory, incomplete=incomplete, held=True) as fake:
                models = configure(directory, fake)
                process = opened(trial, directory, work, session, models, threshold=0 if incomplete else 5000)
                try:
                    process.send({"id": name, "type": "prompt", "message": "Continue work."})
                    shared.require(fake.arrived.wait(5), "native remote side request did not start")
                    started = time.monotonic()
                    if stop == "abort":
                        process.command(name + "-abort", "abort")
                    else:
                        process.child.stdin.close()
                        process.child.wait(timeout=3)
                        process.child.stdin = None
                    elapsed = time.monotonic() - started
                    shared.require(elapsed < 3, "reader cancellation did not settle before held HTTP release")
                finally:
                    if process.child.stdin is not None:
                        finished(process, directory, session)
                    else:
                        for reader in process.readers:
                            reader.join(timeout=1)
                    fake.release.set()
                after = shared.journal(session)
                shared.require(process.child.returncode == 0, "cancelled RPC process failed")
                shared.require(not any(entry["type"] == "compaction" for entry in after.values()), "cancelled remote published")
                for identity, original in before.items():
                    shared.require(after.get(identity) == original, "cancelled remote changed original raw history")
                if incomplete:
                    shared.require(any("Incomplete output to retain on abort." in json.dumps(entry) for entry in after.values()),
                                   "cancelled recovery did not restore the original failed turn")
                shared.require(len(fake.requests) == (2 if incomplete else 1) and not fake.errors,
                               "cancelled remote fell back or started another primary")
                cases.append({"name": name, "status": "passed", "calls": len(fake.requests), "stopSeconds": round(elapsed, 3)})
    return {"name": "reader-abort-eof", "status": "passed", "cases": cases}


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--binary", type=Path, required=True)
    parser.add_argument("--output", type=Path, default=Path("C:/Temp/ara-remote-batch/host"))
    args = parser.parse_args()
    output = args.output / ("run-" + datetime.datetime.now(datetime.timezone.utc).strftime("%Y%m%dT%H%M%SZ"))
    output.mkdir(parents=True)
    before, binary_before, started = source_pins(), shared.digest(args.binary), time.monotonic()
    trial = shared.Trial(args.binary, output, 25)
    families, error = [], None
    try:
        for family in (native_full_repeat_reopen, portable_disabled_foreign, generic_two_calls,
                       automatic_recovery, reader_cancellation):
            families.append(family(trial))
            print(json.dumps(families[-1]), flush=True)
    except Exception as failure:
        error = str(failure)
    after = source_pins()
    code = 0 if error is None and before == after and binary_before == shared.digest(args.binary) else 1
    receipt = {"exitCode": code, "families": families, "failure": error,
               "seconds": round(time.monotonic() - started, 3), "sourcePinsBefore": before,
               "sourcePinsAfter": after, "sourceUnchanged": before == after, "binarySha256": binary_before,
               "limits": "Actual RPC process with controlled loopback V1/generic routes; no live provider, OAuth or V2 Host trial."}
    shared.dump(output / "receipt.json", receipt)
    print(json.dumps({key: receipt[key] for key in ("exitCode", "families", "failure", "seconds")}))
    return code


if __name__ == "__main__":
    raise SystemExit(main())
