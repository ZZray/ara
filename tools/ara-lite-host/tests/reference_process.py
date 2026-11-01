#!/usr/bin/env python3
"""Native reference-Host acceptance, using only Python's standard library.

Default: controlled workflows, crash recovery and malformed native stdin checks.
An optional live trial uses an existing credential environment variable; no key is
saved. A fresh --output directory retains requests, JSONL, SQLite/native journals
and result.json. The script never builds, installs, deploys, or deletes evidence.
"""
from __future__ import annotations

import argparse
import collections
import errno
import hashlib
import json
import os
from pathlib import Path
import queue
import sqlite3
import subprocess
import sys
import threading
import time
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from urllib.parse import urlsplit


COMMAND_SECONDS = 15
PHASE_SECONDS = 30


def check(condition, detail):
    if not condition:
        raise AssertionError(detail)


def save(path, value):
    path.write_text(json.dumps(value, ensure_ascii=False, indent=2) + "\n", encoding="utf-8")


def sha(path):
    return hashlib.sha256(path.read_bytes()).hexdigest()


def repository(path):
    path.mkdir(parents=True)
    for args in (["init", "-q"], ["-c", "user.name=Host Process Fixture", "-c", "user.email=fixture@example.invalid",
                                  "commit", "--allow-empty", "--no-gpg-sign", "-qm", "fixture baseline"]):
        result = subprocess.run(["git", "-C", str(path), *args], capture_output=True, text=True, timeout=15)
        check(result.returncode == 0, f"Git fixture command failed: {result.stderr}")


def call(call_id, name, arguments):
    return {"call_id": call_id, "name": name, "arguments": arguments}


class FakeProvider:
    def __init__(self, output):
        self.output = output
        self.requests = []
        self.counts = collections.Counter()
        self.lock = threading.Lock()
        self.errors = []
        self.scripts = {
            "CASE_A": [call("a-question", "request_input", {"question": "value?"}),
                       call("a-source", "source_read", {"id": "shared", "version": 1, "offset": 0, "limit": 16}),
                       call("a-write-1", "write_result", {"text": "42"}), "CASE_A first candidate.",
                       call("a-write-2", "write_result", {"text": "42"}), "CASE_A revised candidate."],
            "CASE_B": [call("b-write-1", "write_result", {"text": "42"}), "CASE_B candidate."],
            "CASE_CRASH": [call("crash-question", "request_input", {"question": "value?"}),
                           call("crash-write", "write_result", {"text": "42"}), "Recovered original Task candidate."],
            "CASE_REVIEW": [call("review-write", "write_result", {"text": "42"}), "Original closed candidate awaiting human review."],
        }
        provider = self

        class Handler(BaseHTTPRequestHandler):
            protocol_version = "HTTP/1.1"

            def log_message(self, *_):
                pass

            def do_GET(self):
                body = json.dumps({"data": [{"id": "host-fixture", "protocols": ["openai-completions"],
                    "context_window": 1_000_000, "max_tokens": 4096,
                    "omp": {"supportsTools": True, "input": ["text"], "reasoning": False}}]}).encode()
                self.send_response(200)
                self.send_header("Content-Type", "application/json")
                self.send_header("Content-Length", str(len(body)))
                self.send_header("Connection", "close")
                self.end_headers()
                self.wfile.write(body)

            def do_POST(self):
                try:
                    length = int(self.headers.get("Content-Length", "0"))
                    check(0 < length <= 2 * 1024 * 1024, "fake request body bound exceeded")
                    body = json.loads(self.rfile.read(length))
                    text = json.dumps(body.get("messages", []), ensure_ascii=False)
                    case = next((name for name in ("CASE_CRASH", "CASE_REVIEW", "CASE_A", "CASE_B") if name in text), None)
                    check(case is not None, "fake provider received an unrecognized actual task")
                    with provider.lock:
                        index = provider.counts[case]
                        provider.counts[case] += 1
                        check(index < len(provider.scripts[case]), f"unexpected extra model call for {case}")
                        response = provider.scripts[case][index]
                        provider.requests.append({"case": case, "ordinal": index + 1, "body": body})
                        save(provider.output / "controlled-requests.json", provider.requests)
                    if isinstance(response, dict):
                        delta = {"tool_calls": [{"index": 0, "id": response["call_id"], "type": "function",
                                  "function": {"name": response["name"], "arguments": json.dumps(response["arguments"])}}]}
                        finish = "tool_calls"
                    else:
                        delta, finish = {"content": response}, "stop"
                    self.send_response(200)
                    self.send_header("Content-Type", "text/event-stream")
                    self.send_header("Connection", "close")
                    self.end_headers()
                    for content, terminal in (({"role": "assistant"}, None), (delta, None), ({}, finish)):
                        packet = {"id": f"host-controlled-{case}-{index}", "object": "chat.completion.chunk",
                                  "choices": [{"index": 0, "delta": content, "finish_reason": terminal}]}
                        self.wfile.write(("data: " + json.dumps(packet) + "\n\n").encode())
                        self.wfile.flush()
                    self.wfile.write(b"data: [DONE]\n\n")
                    self.wfile.flush()
                except (BrokenPipeError, ConnectionResetError):
                    pass  # Original request remains captured even when its client dies.
                except Exception as error:
                    with provider.lock:
                        provider.errors.append(str(error))
                    try:
                        self.send_error(500, "controlled fixture rejected request")
                    except (BrokenPipeError, ConnectionResetError):
                        pass

        self.server = ThreadingHTTPServer(("127.0.0.1", 0), Handler)
        self.server.daemon_threads = True
        self.thread = threading.Thread(target=self.server.serve_forever, daemon=True)
        self.thread.start()
        self.endpoint = f"http://127.0.0.1:{self.server.server_port}/v1"

    def count(self, case):
        with self.lock:
            return self.counts[case]

    def close(self):
        self.server.shutdown()
        self.server.server_close()
        self.thread.join(timeout=3)


class HostProcess:
    def __init__(self, binary, config, prefix, env, secrets=()):
        self.prefix = prefix
        self.secrets = tuple(value for value in secrets if value)
        self.queue = queue.Queue()
        self.events = []
        self.notices = {}
        self.notice_counts = collections.Counter()
        self.consumed = set()
        self.acked = set()
        self.stderr = []
        self.commands = []
        self.process = subprocess.Popen([str(binary), "run", str(config)], stdin=subprocess.PIPE,
            stdout=subprocess.PIPE, stderr=subprocess.PIPE, text=True, encoding="utf-8", bufsize=1, env=env)
        self.readers = [threading.Thread(target=self._stdout, daemon=True), threading.Thread(target=self._stderr, daemon=True)]
        for reader in self.readers:
            reader.start()
        self.wait(lambda event: event.get("event") == "host_ready", COMMAND_SECONDS)

    def clean(self, text):
        for secret in self.secrets:
            text = text.replace(secret, "[credential redacted]")
        return text

    def _stdout(self):
        with (self.prefix.with_suffix(".stdout.jsonl")).open("w", encoding="utf-8") as log:
            for line in self.process.stdout:
                safe = self.clean(line)
                log.write(safe)
                log.flush()
                try:
                    self.queue.put(json.loads(safe))
                except ValueError:
                    self.queue.put({"event": "invalid_stdout", "text": safe})
        self.queue.put(None)

    def _stderr(self):
        with (self.prefix.with_suffix(".stderr.txt")).open("w", encoding="utf-8") as log:
            for line in self.process.stderr:
                safe = self.clean(line)
                self.stderr.append(safe)
                log.write(safe)
                log.flush()

    def _record(self, event):
        self.events.append(event)
        check(event.get("event") != "invalid_stdout", "Host emitted non-JSON stdout")
        if event.get("event") == "notice":
            notice = event["notice"]
            self.notices[notice["id"]] = notice
            self.notice_counts[notice["id"]] += 1
            check(self.notice_counts[notice["id"]] == 1, f"notice emitted twice: {notice['id']}")
        return event

    def wait(self, predicate, seconds=PHASE_SECONDS):
        deadline = time.monotonic() + seconds
        while True:
            remaining = deadline - time.monotonic()
            check(remaining > 0, "Host observation timed out")
            try:
                event = self.queue.get(timeout=remaining)
            except queue.Empty:
                raise AssertionError("Host observation timed out") from None
            check(event is not None, f"Host exited before expected observation: {''.join(self.stderr[-12:])}")
            self._record(event)
            if predicate(event):
                return event

    def command(self, value, expect_error=False):
        self.commands.append(value)
        with self.prefix.with_suffix(".commands.jsonl").open("a", encoding="utf-8") as log:
            log.write(self.clean(json.dumps(value, ensure_ascii=False)) + "\n")
        check(self.process.poll() is None, "command addressed an exited Host")
        self.process.stdin.write(json.dumps(value, ensure_ascii=False) + "\n")
        self.process.stdin.flush()
        answer = self.wait(lambda item: item.get("event") in ("response", "command_error"), COMMAND_SECONDS)
        if expect_error:
            check(answer["event"] == "command_error", "expected command rejection")
            return answer
        check(answer["event"] == "response", f"Host command failed: {answer.get('error', '')}")
        return answer["value"]

    def status_until(self, predicate, seconds=PHASE_SECONDS):
        deadline = time.monotonic() + seconds
        while time.monotonic() < deadline:
            status = self.command({"op": "status"})
            if predicate(status):
                return status
            time.sleep(.03)
        raise AssertionError("Host status never reached expected state")

    def notice(self, predicate):
        deadline = time.monotonic() + PHASE_SECONDS
        while True:
            for notice_id, notice in self.notices.items():
                if notice_id not in self.consumed and predicate(notice):
                    self.consumed.add(notice_id)
                    return notice
            self.wait(lambda event: event.get("event") == "notice", deadline - time.monotonic())

    def ack_all(self):
        for notice_id in list(self.notices):
            if notice_id not in self.acked:
                self.command({"op": "ack", "notice_id": notice_id})
                self.acked.add(notice_id)

    def drain(self):
        while True:
            try:
                event = self.queue.get_nowait()
            except queue.Empty:
                return
            if event is not None:
                self._record(event)

    def stop(self):
        if self.process.poll() is None:
            try:
                self.process.stdin.write('{"op":"stop"}\n')
                self.process.stdin.flush()
            except OSError as error:
                if error.errno not in (errno.EPIPE, errno.EINVAL, errno.EBADF):
                    raise
            self.process.wait(timeout=10)
        self._close_stdin()
        for reader in self.readers:
            reader.join(timeout=2)
        self.drain()
        check(self.process.returncode == 0, f"Host graceful stop failed: {''.join(self.stderr[-12:])}")

    def kill(self):
        if self.process.poll() is None:
            try:
                self.process.kill()
            except OSError:
                if self.process.poll() is None:
                    raise
            self.process.wait(timeout=5)
        self._close_stdin()
        for reader in self.readers:
            reader.join(timeout=2)
        self.drain()

    def _close_stdin(self):
        if self.process.stdin is not None and not self.process.stdin.closed:
            try:
                self.process.stdin.close()
            except OSError as error:
                if error.errno not in (errno.EPIPE, errno.EINVAL, errno.EBADF):
                    raise


def cleanup_host(host):
    """Retain an assertion's original cause if pipe cleanup fails afterwards."""
    preserving_failure = sys.exc_info()[0] is not None
    try:
        host.stop()
    except Exception as cleanup_error:
        try:
            host.kill()
        except Exception as kill_error:
            save(host.prefix.with_suffix(".cleanup.json"), {
                "stop_error": host.clean(str(cleanup_error)), "kill_error": host.clean(str(kill_error)),
                "original_failure_preserved": preserving_failure})
        else:
            save(host.prefix.with_suffix(".cleanup.json"), {
                "stop_error": host.clean(str(cleanup_error)), "original_failure_preserved": preserving_failure})
        if not preserving_failure:
            raise


def settings(root, endpoint, model="host-fixture", key_env="HOST_TEST_KEY", seconds=120):
    return {"runtime": {"state_dir": str(root / "state"), "workspace_root": str(root / "work"),
        "authorization_revision": "process-grant-v1", "allowed_actors": ["operator"], "max_parallel": 2,
        "max_calls": 12, "max_runs": 3, "max_seconds": seconds, "accept_mode": "observe", "review_mode": "human"},
        "model": {"endpoint": endpoint, "model": model, "key_env": key_env, "expected_result": "42",
                  "max_run_calls": 8, "max_output": 1024}}


def event(event_id, body):
    return {"op": "event", "event": {"source": "native-process-fixture", "event_id": event_id,
        "actor": "operator", "authorization_revision": "process-grant-v1", "explicit": True, "body": body}}


def work(event_id, workspace, objective):
    return event(event_id, {"kind": "work", "title": event_id, "workspace": str(workspace), "objective": objective})


def binding(status, task_id):
    return next(item for item in status["execution"] if item["spec"]["task_id"] == task_id)


def review(host, notice, decision="pass", reason="Actual result.txt and source receipts checked by native process fixture."):
    body = json.loads(notice["body"])
    host.command({"op": "review", "task_id": notice["task_id"], "review_id": notice["id"],
        "snapshot": body["snapshot"], "decision": decision,
        "reason": reason})


def sql_record(state, table, record_id):
    check(table in ("events", "attentions", "notices"), "invalid fixture table")
    connection = sqlite3.connect((state / "execution.sqlite").as_uri() + "?mode=ro", uri=True, timeout=1)
    try:
        row = connection.execute(f"SELECT data FROM {table} WHERE id=?", (record_id,)).fetchone()
        return json.loads(row[0]) if row else None
    finally:
        connection.close()


def wait_sql(state, table, record_id, predicate):
    deadline = time.monotonic() + COMMAND_SECONDS
    while time.monotonic() < deadline:
        record = sql_record(state, table, record_id)
        if record is not None and predicate(record):
            return record
        time.sleep(.015)
    raise AssertionError("durable Host record never reached expected state")


def native_artifacts(task_binding):
    session = Path(task_binding["session_path"])
    journal = session.with_suffix(".journal")
    records = []
    for segment in sorted(journal.glob("segment-*.jsonl")):
        records.extend(json.loads(line) for line in segment.read_text(encoding="utf-8").splitlines() if line)
    calls = []

    def visit(value):
        if isinstance(value, dict):
            if "name" in value and "arguments" in value and "id" in value:
                calls.append({"id": value["id"], "name": value["name"], "arguments": value["arguments"]})
            for child in value.values():
                visit(child)
        elif isinstance(value, list):
            for child in value:
                visit(child)

    for record in records:
        visit(record.get("message", {}))
    return {"session": str(session), "journal": str(journal), "records": len(records), "tool_calls": calls,
            "head_sha256": sha(journal / "head.json")}


def outcome(task_binding):
    path = Path(task_binding["spec"]["evidence_dir"]) / "host-outcome.json"
    return json.loads(path.read_text(encoding="utf-8"))


def same_terms(before, after):
    for key in ("session_path", "environment_path", "workspace", "generation", "adapter_revision", "initial_run_id"):
        check(before[key] == after[key], f"original execution binding changed: {key}")
    for key in ("task_id", "criteria_revision", "authorization_revision", "max_runs", "max_calls", "deadline_unix_ms"):
        check(before["spec"][key] == after["spec"][key], f"original Task terms changed: {key}")


def controlled_workflow(binary, root, provider, env):
    root.mkdir()
    a, b = root / "work" / "a", root / "work" / "b"
    repository(a)
    repository(b)
    config = root / "config.json"
    save(config, settings(root, provider.endpoint))
    host = HostProcess(binary, config, root / "host", env)
    try:
        first = host.command(work("workflow-a", a, "CASE_A: ask value?, then read shared source, write result.txt, submit."))
        check(host.command(work("workflow-a", a, "CASE_A: ask value?, then read shared source, write result.txt, submit."))["key"] == first["key"], "duplicate Work identity changed")
        question = host.notice(lambda notice: notice["kind"] == "question")
        a_id = question["task_id"]
        before = binding(host.command({"op": "status"}), a_id)
        check(before["phase"] == "waiting_human", "A did not expose structured waiting_human")
        check(provider.count("CASE_A") == 1, "question wait polled the model")
        host.command(work("workflow-b", b, "CASE_B: write the exact text 42 to result.txt and submit."))
        b_review = host.notice(lambda notice: notice["kind"] == "review" and notice["task_id"] != a_id)
        b_id = b_review["task_id"]
        status = host.command({"op": "status"})
        check(binding(status, a_id)["phase"] == "waiting_human", "unrelated B completion disturbed A")
        check(provider.count("CASE_A") == 1, "A made model calls while waiting for operator")
        check((b / "result.txt").read_bytes() == b"42", "B actual result is incorrect")
        review(host, b_review)
        host.status_until(lambda state: state["tasks"][b_id]["state"] == "passed" and binding(state, b_id)["phase"] == "finished")
        host.command({"op": "register_source", "source": {"id": "shared", "task_id": b_id, "generation": 1,
            "version": 1, "path": str(b / "result.txt"), "sha256": sha(b / "result.txt"), "allowed_tasks": [a_id], "active": True}})
        host.command(event("workflow-answer-a", {"kind": "answer", "task_id": a_id, "generation": 1,
            "question_id": question["id"], "text": "42; read the shared source before writing the result."}))
        a_first = host.notice(lambda notice: notice["kind"] == "review" and notice["task_id"] == a_id)
        review(host, a_first, "changes_requested")
        a_second = host.notice(lambda notice: notice["kind"] == "review" and notice["task_id"] == a_id)
        review(host, a_second)
        final = host.status_until(lambda state: state["tasks"][a_id]["state"] == "passed" and binding(state, a_id)["phase"] == "finished")
        after = binding(final, a_id)
        same_terms(before, after)
        actual = outcome(after)
        check(actual["runs"] == 2 and actual["observed_calls"] == 6, "human rework did not retain actual Task call totals")
        check(actual["task_accepted"] is False, "Harness silently accepted a business Task")
        check((a / "result.txt").read_bytes() == b"42", "A actual result is incorrect")
        native = native_artifacts(after)
        check(sum(call["name"] == "request_input" for call in native["tool_calls"]) == 1, "human question native call was replayed")
        check(sum(call["name"] == "source_read" for call in native["tool_calls"]) == 1, "cross-Task source was not read once")
        host.ack_all()
        host.command({"op": "status"})
        host.ack_all()
        result = {"name": "concurrency_sources_human_rework", "passed": True, "task_a": a_id, "task_b": b_id,
            "before": before, "after": after, "outcome": actual, "native": native,
            "actual_model_calls": {"A": provider.count("CASE_A"), "B": provider.count("CASE_B")},
            "notice_counts": dict(host.notice_counts), "status": final}
        save(root / "receipt.json", result)
        return result
    finally:
        cleanup_host(host)


def controlled_crash(binary, root, provider, env):
    root.mkdir()
    workspace = root / "work" / "crash"
    repository(workspace)
    config = root / "config.json"
    save(config, settings(root, provider.endpoint))
    first = HostProcess(binary, config, root / "before-kill", env)
    restarted = None
    try:
        first.command(work("crash-work", workspace, "CASE_CRASH: ask value? before writing a checked result.txt."))
        question = first.notice(lambda notice: notice["kind"] == "question")
        task_id = question["task_id"]
        before = binding(first.command({"op": "status"}), task_id)
        followup = event("crash-followup", {"kind": "followup", "task_id": task_id, "generation": 1,
            "text": "FOLLOWUP_CRASH_MARKER: inspect this original event; do not replay uncertain work."})
        accepted = first.command(followup)
        wait_sql(root / "state", "events", accepted["key"], lambda row: row["state"] == "dispatching")
        check(provider.count("CASE_CRASH") == 1, "crash window was not inside the original waiting tool")
        first.kill()
        restarted = HostProcess(binary, config, root / "after-restart", env)
        status = restarted.command({"op": "status"})
        restored = binding(status, task_id)
        same_terms(before, restored)
        check(restored["phase"] == "needs_reconciliation", "restart fabricated an active/queued task")
        check(any(row["key"] == accepted["key"] and row["state"] == "unknown" for row in status["unknown_events"]), "uncertain dispatch did not remain unknown")
        check(any(row["id"] == question["id"] for row in status["unknown_notices"]), "unacknowledged emitted question was not retained as unknown")
        duplicate = restarted.command(followup)
        check(duplicate["key"] == accepted["key"] and duplicate["state"] == "unknown", "duplicate event reset uncertain delivery")
        for _ in range(3):
            restarted.command({"op": "status"})
            time.sleep(.05)
        check(provider.count("CASE_CRASH") == 1, "restart automatically called the model or replayed a tool")
        restarted.drain()
        check(question["id"] not in restarted.notices, "restart blindly re-emitted the question")
        inspected = restarted.command({"op": "inspect_recovery", "task_id": task_id})
        save(root / "inspection-before-resume.json", inspected)
        open_calls = inspected["interrupted"]["open_tool_calls"]
        check(len(open_calls) == 1 and open_calls[0]["name"] == "request_input", "unexpected native open calls at recovery frontier")
        check("FOLLOWUP_CRASH_MARKER" not in Path(before["session_path"]).with_suffix(".context.json").read_text(encoding="utf-8"), "test cannot prove the followup remained unapplied")
        restarted.command({"op": "settle_event", "key": accepted["key"], "state": "unapplied",
            "evidence": "Original native request_input never settled; inspect saved context and original dispatch event. Marker is absent from context; no replay requested."})
        answer = restarted.command(event("crash-answer", {"kind": "answer", "task_id": task_id, "generation": 1,
            "question_id": question["id"], "text": "42"}))
        wait_sql(root / "state", "events", answer["key"], lambda row: row["state"] == "applied")
        saved_attention = sql_record(root / "state", "attentions", question["id"])
        check(saved_attention["answer"] == "42", "recovery answer was not durably saved")
        proof = {"task_id": task_id, "generation": 1, "session_path": inspected["binding"]["session_path"],
            "fence_sha256": inspected["fence_sha256"], "tool_calls": [{"call_id": open_calls[0]["id"],
                "settlement": {"kind": "completed", "output": json.dumps({"question_id": question["id"], "answer": saved_attention["answer"]})}}],
            "evidence": [str(root / "inspection-before-resume.json"), "Read-only SQLite inspection confirms the exact saved answer for the original question and Task."]}
        save(root / "operator-reconciliation.json", proof)
        restarted.command({"op": "recover", "task_id": task_id, "not_started": False,
            "evidence": "Explicitly settle only the original durable human-answer tool; original Session and Task budget remain fixed.", "proof": proof})
        candidate = restarted.notice(lambda notice: notice["kind"] == "review" and notice["task_id"] == task_id)
        review(restarted, candidate)
        final = restarted.status_until(lambda state: state["tasks"][task_id]["state"] == "passed" and binding(state, task_id)["phase"] == "finished")
        after = binding(final, task_id)
        same_terms(before, after)
        actual = outcome(after)
        check(actual["runs"] == 2 and 3 <= actual["observed_calls"] <= before["spec"]["max_calls"], "resume reset or exceeded original conservative Task budget")
        check(actual["task_accepted"] is False, "resumed Harness silently accepted the business Task")
        check(provider.count("CASE_CRASH") == 3, "original question or followup was replayed")
        check((workspace / "result.txt").read_bytes() == b"42", "resumed original Task artifact failed")
        native = native_artifacts(after)
        check(sum(call["name"] == "request_input" for call in native["tool_calls"]) == 1, "native question was executed again after crash")
        restarted.command({"op": "settle_notice", "notice_id": question["id"], "evidence": "before-kill.stdout.jsonl contains the original emitted question; transport receipt reviewed."})
        restarted.ack_all()
        result = {"name": "native_kill_restart_original_resume", "passed": True, "task_id": task_id,
            "followup_key": accepted["key"], "before": before, "restored": restored, "after": after,
            "outcome": actual, "native": native, "actual_model_calls": provider.count("CASE_CRASH"),
            "note": "A killed Run may conservatively consume its admitted call cap; actual HTTP requests and logical Task reservations are recorded separately."}
        save(root / "receipt.json", result)
        return result
    finally:
        first.kill()
        if restarted is not None:
            cleanup_host(restarted)


def controlled_review_recovery(binary, root, provider, env):
    root.mkdir()
    workspace = root / "work" / "review"
    repository(workspace)
    config = root / "config.json"
    save(config, settings(root, provider.endpoint))
    first = HostProcess(binary, config, root / "before-kill", env)
    restarted = None
    acknowledged_restart = None
    try:
        first.command(work("closed-review-work", workspace,
            "CASE_REVIEW: write exactly 42 to result.txt and submit the original candidate for human review."))
        original_offer = first.notice(lambda notice: notice["kind"] == "review")
        task_id = original_offer["task_id"]
        original_status = first.command({"op": "status"})
        before = binding(original_status, task_id)
        check(before["phase"] == "ready_for_review", "original closed candidate is not awaiting human review")
        check(original_status["tasks"][task_id]["state"] == "awaiting_review", "business Task was accepted without human review")
        check((workspace / "result.txt").read_bytes() == b"42", "original review artifact is incorrect")
        inspected_before = first.command({"op": "inspect_recovery", "task_id": task_id})
        check(inspected_before["interrupted"] is None, "closed-review scenario still contains an open native Run")
        check(inspected_before["fence"] is not None, "waiting human review unexpectedly released the native fence")
        check(not (Path(before["spec"]["evidence_dir"]) / "released.json").exists(), "review was already released before kill")
        native_before = native_artifacts(before)
        check(provider.count("CASE_REVIEW") == 2, "closed-review scenario made unexpected model calls")
        save(root / "original-offer.json", original_offer)
        save(root / "inspection-before-kill.json", inspected_before)
        first.kill()

        restarted = HostProcess(binary, config, root / "after-restart", env)
        restored_status = restarted.command({"op": "status"})
        restored = binding(restored_status, task_id)
        same_terms(before, restored)
        check(restored["phase"] == "needs_reconciliation", "restart silently accepted or relaunched the original candidate")
        check(restored_status["tasks"][task_id]["state"] == "awaiting_review", "restart changed business review ownership")
        restarted.command({"op": "recover", "task_id": task_id, "not_started": False,
            "evidence": "Original ordinary Stop, saved candidate, artifact receipt and unchanged workspace snapshot reviewed; restore only the original human review offer.",
            "proof": None})
        ready_status = restarted.command({"op": "status"})
        check(binding(ready_status, task_id)["phase"] == "ready_for_review", "explicit recovery did not restore original review state")
        check(provider.count("CASE_REVIEW") == 2, "restoring a closed review launched a model Run")
        check(original_offer["id"] not in restarted.notices, "unknown prior review offer was blindly emitted again")
        review(restarted, original_offer, decision="blocked",
            reason="Native recovery decision: block this original offer pending a second explicit review.")
        blocked_status = restarted.command({"op": "status"})
        check(blocked_status["tasks"][task_id]["state"] == "blocked", "explicit blocked decision was not applied")
        check(binding(blocked_status, task_id)["phase"] == "needs_reconciliation", "blocked recovered offer retained a runnable phase")
        check(provider.count("CASE_REVIEW") == 2, "blocking the recovered offer launched a model Run")
        restarted.command({"op": "recover", "task_id": task_id, "not_started": False,
            "evidence": "Reinspect the same original artifact receipt and retained fence after explicit block; reopen only its human review offer.",
            "proof": None})
        second_ready = restarted.command({"op": "status"})
        check(binding(second_ready, task_id)["phase"] == "ready_for_review", "same blocked offer could not be explicitly reopened")
        review(restarted, original_offer, decision="pass",
            reason="Native recovery decision: original source and actual result checked; accept this same offer.")
        result_notice = restarted.notice(lambda notice: notice["kind"] == "result" and notice["task_id"] == task_id)
        handoff = json.loads(result_notice["body"])
        check(handoff["kind"] == "recovered_human_review", "recovered review was mislabeled as an ordinary Harness result")
        check(handoff["native_fence_retained"] is True and handoff["not_a_harness_task_outcome"] is True,
              "recovered review claimed native release or fabricated a TaskOutcome")
        final_status = restarted.status_until(lambda state: state["tasks"][task_id]["state"] == "passed"
                                               and binding(state, task_id)["phase"] == "finished")
        after = binding(final_status, task_id)
        same_terms(before, after)
        inspected_after = restarted.command({"op": "inspect_recovery", "task_id": task_id})
        check(inspected_after["fence_sha256"] == inspected_before["fence_sha256"], "recovered review changed or cleared the original native fence")
        native_after = native_artifacts(after)
        check(native_after["head_sha256"] == native_before["head_sha256"], "human review recovery changed native execution history")
        check(not (Path(after["spec"]["evidence_dir"]) / "released.json").exists(), "Host-only review recovery fabricated native release")
        check(provider.count("CASE_REVIEW") == 2, "human review pass replayed model or tools")
        restarted.command({"op": "settle_notice", "notice_id": original_offer["id"],
            "evidence": "original-offer.json and before-kill.stdout.jsonl confirm this exact review offer was delivered before the crash."})
        restarted.ack_all()
        check(sql_record(root / "state", "notices", result_notice["id"])["state"] == "sent", "result ACK was not durable")
        restarted.kill()  # A second actual restart checks the persisted ACK.

        acknowledged_restart = HostProcess(binary, config, root / "after-ack-restart", env)
        final_reopen = acknowledged_restart.command({"op": "status"})
        check(binding(final_reopen, task_id)["phase"] == "finished" and final_reopen["tasks"][task_id]["state"] == "passed",
              "terminal review state did not survive the ACK restart")
        for _ in range(3):
            acknowledged_restart.command({"op": "status"})
            time.sleep(.05)
        acknowledged_restart.drain()
        check(result_notice["id"] not in acknowledged_restart.notices, "ACKed recovered result was emitted after restart")
        check(provider.count("CASE_REVIEW") == 2, "terminal ACK restart relaunched the model")
        result = {"name": "closed_native_candidate_human_review_recovery", "passed": True, "task_id": task_id,
            "before": before, "restored": restored, "after": after, "handoff": handoff,
            "native_before": native_before, "native_after": native_after,
            "original_fence_sha256": inspected_before["fence_sha256"], "actual_model_calls": provider.count("CASE_REVIEW"),
            "result_notice_id": result_notice["id"], "ack_survived_restart": True,
            "blocked_then_pass_same_offer": True,
            "note": "Host business handoff accepted after explicit review; original native fence remains. This is not a completed Harness TaskOutcome."}
        save(root / "receipt.json", result)
        return result
    finally:
        first.kill()
        if acknowledged_restart is not None:
            cleanup_host(acknowledged_restart)
        elif restarted is not None and restarted.process.poll() is None:
            cleanup_host(restarted)


def malformed_native_stdin(binary, root, provider, env):
    root.mkdir()
    (root / "work").mkdir()
    config = root / "config.json"
    save(config, settings(root, provider.endpoint))
    host = HostProcess(binary, config, root / "host", env)
    before_calls = dict(provider.counts)
    try:
        # The stop-looking suffix belongs to the same rejected oversized line.
        # It must be discarded to newline rather than becoming a second command.
        malformed = " " * 65538 + '{"op":"stop"}\n'
        raw_path = root / "oversized-input.txt"
        raw_path.write_text(malformed, encoding="utf-8")
        host.process.stdin.write(malformed)
        host.process.stdin.flush()
        error = host.wait(lambda item: item.get("event") == "command_error", COMMAND_SECONDS)
        state = host.command({"op": "status"})
        check(state["tasks"] == {} and state["execution"] == [], "malformed stdin created or ran a Task")
        check(not (root / "state" / "intentional-stop").exists(), "oversized line suffix was executed as a stop")
        command_errors = [item for item in host.events if item.get("event") == "command_error"]
        check(len(command_errors) == 1, "one malformed line did not produce exactly one rejection")
        check(dict(provider.counts) == before_calls, "malformed stdin caused a model request")
        invalid_utf8 = b" " * 65537 + b"\xff" + b'{"op":"stop"}\n'
        (root / "invalid-utf8-input.bin").write_bytes(invalid_utf8)
        host.process.stdin.buffer.write(invalid_utf8)
        host.process.stdin.buffer.flush()
        utf8_error = host.wait(lambda item: item.get("event") == "command_error", COMMAND_SECONDS)
        host.process.wait(timeout=COMMAND_SECONDS)
        host.drain()
        check(host.process.returncode == 0, "invalid UTF-8 did not close the reader with joined cleanup")
        check(not (root / "state" / "intentional-stop").exists(), "invalid UTF-8 suffix was executed as Stop")
        check(len([item for item in host.events if item.get("event") == "command_error"]) == 2,
              "each rejected physical input must report one error")
        result = {"name": "oversized_native_stdin_discards_remainder", "passed": True,
            "input_bytes": len(malformed.encode("utf-8")), "input_sha256": sha(raw_path),
            "command_errors": len(command_errors), "error": error, "legal_status_after_rejection": state,
            "stop_suffix_not_executed": True, "invalid_utf8_closed_reader": True,
            "invalid_utf8_error": utf8_error, "additional_model_calls": 0}
        save(root / "receipt.json", result)
        return result
    finally:
        cleanup_host(host)


def live_trial(binary, root, endpoint, model, key_env, env):
    root.mkdir()
    workspace = root / "work" / "live"
    repository(workspace)
    source = workspace / "input-source.txt"
    source.write_text("42", encoding="utf-8")
    config = root / "config.json"
    save(config, settings(root, endpoint, model, key_env, seconds=90))
    host = HostProcess(binary, config, root / "host", env, secrets=[env.get(key_env, "")])
    try:
        host.command(work("live-task", workspace,
            "Complete one bounded task. First call request_input with one concrete readiness question, then wait. "
            "After the operator replies, source_list and source_read the registered live-source. Write exactly that source's complete text to result.txt using write_result. "
            "Stop after writing. Use no other file or shell operation. The Host verifies actual bytes and a person reviews the candidate."))
        question = host.notice(lambda notice: notice["kind"] == "question")
        task_id = question["task_id"]
        before = binding(host.command({"op": "status"}), task_id)
        host.command({"op": "register_source", "source": {"id": "live-source", "task_id": task_id, "generation": 1,
            "version": 1, "path": str(source), "sha256": sha(source), "allowed_tasks": [], "active": True}})
        host.command(event("live-answer", {"kind": "answer", "task_id": task_id, "generation": 1,
            "question_id": question["id"], "text": "Ready. Read the live-source and write its exact bytes using the available tools."}))
        candidate = host.notice(lambda notice: notice["kind"] == "review")
        check((workspace / "result.txt").read_bytes() == source.read_bytes(), "actual live artifact is wrong")
        review(host, candidate)
        final = host.status_until(lambda state: state["tasks"][task_id]["state"] == "passed" and binding(state, task_id)["phase"] == "finished")
        after = binding(final, task_id)
        same_terms(before, after)
        actual = outcome(after)
        check(actual["observed_calls"] <= 12 and actual["task_accepted"] is False, "live Task exceeded bounds or changed acceptance owner")
        native = native_artifacts(after)
        check(any(call["name"] == "source_read" for call in native["tool_calls"]), "live model did not actually read the source")
        check(any(call["name"] == "request_input" for call in native["tool_calls"]), "live model did not execute a human question")
        host.ack_all()
        result = {"name": "bounded_live_source_question_artifact", "passed": True, "model": model,
            "endpoint_host": urlsplit(endpoint).hostname, "task_id": task_id, "outcome": actual,
            "native": native, "result_sha256": sha(workspace / "result.txt")}
        save(root / "receipt.json", result)
        return result
    finally:
        cleanup_host(host)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--binary", type=Path, required=True)
    parser.add_argument("--output", type=Path, required=True)
    parser.add_argument("--live-endpoint")
    parser.add_argument("--live-model", default="deepseek-v4.1-flash")
    parser.add_argument("--live-key-env", default="RAY_GATEWAY_KEY")
    args = parser.parse_args()
    binary, output = args.binary.resolve(), args.output.resolve()
    check(binary.is_file(), "--binary must name the actual built Host executable")
    check(not output.exists(), "--output must be a fresh evidence directory")
    output.mkdir(parents=True)
    env = os.environ.copy()
    env["HOST_TEST_KEY"] = "controlled-fixture-key"
    result = {"passed": False, "binary": str(binary), "binary_sha256": sha(binary), "scenarios": [],
              "started_at_unix_ms": int(time.time() * 1000), "coverage": "controlled actual Host processes; optional selected live task"}
    provider = FakeProvider(output)
    try:
        result["scenarios"].append(controlled_workflow(binary, output / "workflow", provider, env))
        result["scenarios"].append(controlled_crash(binary, output / "crash", provider, env))
        result["scenarios"].append(controlled_review_recovery(binary, output / "review-recovery", provider, env))
        result["scenarios"].append(malformed_native_stdin(binary, output / "malformed-stdin", provider, env))
        check(not provider.errors, f"controlled provider faults: {provider.errors}")
        if args.live_endpoint:
            parsed = urlsplit(args.live_endpoint)
            check(parsed.scheme == "https" and parsed.hostname and not parsed.username and not parsed.password and not parsed.query,
                  "live endpoint must be HTTPS without embedded credentials or query")
            check(bool(env.get(args.live_key_env)), "selected live credential environment variable is missing")
            result["scenarios"].append(live_trial(binary, output / "live", args.live_endpoint, args.live_model, args.live_key_env, env))
        result["passed"] = True
    except Exception as error:
        detail = str(error)
        secret = env.get(args.live_key_env, "") if args.live_endpoint else ""
        result["error"] = detail.replace(secret, "[credential redacted]") if secret else detail
    finally:
        provider.close()
        result["finished_at_unix_ms"] = int(time.time() * 1000)
        result["elapsed_seconds"] = (result["finished_at_unix_ms"] - result["started_at_unix_ms"]) / 1000
        result["controlled_provider_counts"] = dict(provider.counts)
        save(output / "result.json", result)
    print(json.dumps({"passed": result["passed"], "evidence": str(output / "result.json"),
                      "elapsed_seconds": result["elapsed_seconds"], "error": result.get("error")}, ensure_ascii=False))
    return 0 if result["passed"] else 1


if __name__ == "__main__":
    sys.exit(main())
