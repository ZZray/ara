"""Actual Rust Node -> maintained Hub -> controlled provider queue acceptance.

The provider is deliberately held at an observable interleaving. No production
access, paid models, browser-state injection, or queue replay after disconnect.
"""
from __future__ import annotations

import argparse
import base64
import json
import os
from pathlib import Path
import sys
import threading
import time

from verify_context import Host, digest
from verify_host import require, stop, WS


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--binary", required=True, type=Path)
    parser.add_argument("--hub-binary", required=True, type=Path)
    parser.add_argument("--output", required=True, type=Path)
    args = parser.parse_args()
    root = args.output.resolve()
    root.mkdir(parents=True, exist_ok=False)
    host = Host(args, root / "host", owner="queue-owner-93", max_calls=6)
    host.env["PATH"] = str(Path(sys.executable).parent) + os.pathsep + host.env.get("PATH", "")
    held, release = threading.Event(), threading.Event()
    cancel_phase = threading.Event()
    expected_disconnects = []
    key, run = "workspace/queue", "queue-active-93"
    result = {"ok": False, "passed": [], "limits": ["controlled provider; not real-model intelligence",
        "connection-local queue; no durable replay or browser acceptance"]}
    started = time.monotonic()

    def reply(handler, body):
        host.fixture.agent_requests.append(body)
        text = json.dumps(body.get("messages", []), ensure_ascii=False)
        if "HOLD_QUEUE_93" in text and not (host.work / "queue-effect.txt").exists():
            held.set()
            require(release.wait(25), "Queue provider hold was not released")
            try:
                host.fixture.emit(handler, tool="run_script", arguments={"language": "python", "script":
                    "from pathlib import Path\nwith Path('queue-effect.txt').open('x') as f:f.write('EXACTLY_ONCE_93')\nprint('EXACTLY_ONCE_93')"})
            except OSError as error:
                if not cancel_phase.is_set() or not (getattr(error, "winerror", None) in (10053, 10054) or error.errno in (32, 104)):
                    raise
                expected_disconnects.append({"type": type(error).__name__, "errno": error.errno, "winerror": getattr(error, "winerror", None)})
        else:
            host.fixture.emit(handler, text="QUEUE_PROCESSED_93")

    host.fixture.reply = reply
    watchdog = threading.Timer(110, lambda: (stop(host.node), stop(host.hub)))
    watchdog.daemon = True
    watchdog.start()

    def wait(predicate, timeout=12):
        end = time.monotonic() + timeout
        while time.monotonic() < end:
            packet = host.controller.receive(end - time.monotonic())
            if packet.get("k") != "dc":
                continue
            frame = json.loads(base64.b64decode(packet["d"]))
            host.transcript.append(frame)
            if predicate(frame):
                return frame
        raise TimeoutError("Missing native queue receipt")

    state = None

    def command(op, item="", msg="", *, changes=None, fail=False):
        nonlocal state
        correlation = "queue-command-" + str(time.time_ns())
        frame = {"k": "agent-queue", "id": correlation, "queue_op": op, "session_key": key}
        if op != "list":
            require(state is not None, "No authoritative queue snapshot")
            frame.update(queue_id=item, msg=msg, queue_epoch=state["queue_epoch"],
                expected_revision=state["revision"], target_run_id=state["active_run_id"])
        frame.update(changes or {})
        host.controller.frame(frame)
        reply = wait(lambda value: value.get("k") == "agent-queue-state" and value.get("id") == correlation)
        require(reply.get("ok") is not fail, "Unexpected queue operation result: " + json.dumps(reply))
        if not fail:
            state = reply
        return reply

    def enqueue(item, msg):
        nonlocal state
        host.controller.frame({"k": "agent-loop", "id": item, "session_key": key, "msg": msg, "model": "default"})
        state = wait(lambda value: value.get("k") == "agent-queue-state"
            and any(row["id"] == item for row in value.get("items", [])))
        return state

    try:
        host.start()
        host.controller.frame({"k": "agent-loop", "id": run, "session_key": key, "msg": "HOLD_QUEUE_93", "model": "default"})
        require(held.wait(15), "Run did not reach held actual provider")
        command("list")
        require(state["active_run_id"] == run and state["items"] == [], "Wrong active queue owner")
        enqueue("q-edit-93", "ORIGINAL_EDIT_93")
        enqueue("q-remove-93", "REMOVED_MUST_NOT_RUN_93")
        enqueue("q-steer-93", "APPLIED_STEERING_93")
        enqueue("q-front-93", "FRONT_FOLLOW_93")
        require([row["id"] for row in state["items"]] == ["q-edit-93", "q-remove-93", "q-steer-93", "q-front-93"], "FIFO differs")
        old_revision = state["revision"]
        command("edit", "q-edit-93", "EDITED_FOLLOW_93")
        require(state["items"][0]["msg"] == "EDITED_FOLLOW_93", "Text edit did not reach native queue")
        command("remove", "q-remove-93", changes={"expected_revision": old_revision}, fail=True)
        command("remove", "q-remove-93", changes={"queue_epoch": "old-connection"}, fail=True)
        command("remove", "q-remove-93", changes={"target_run_id": "old-run"}, fail=True)
        wrong = command("list", changes={"session_key": "workspace/other"}, fail=True)
        require(not wrong.get("items"), "Wrong Session list leaked pending text")
        command("remove", "q-remove-93")
        command("front", "q-front-93")
        require(state["items"][0]["id"] == "q-front-93", "Move to front did not reorder native queue")
        result["passed"].append("live_native_fifo_edit_remove_front_revision_epoch_run_and_session_fences")

        # Duplicate sends retain one stable native identity and do not add work.
        before = list(state["items"])
        host.controller.frame({"k": "agent-loop", "id": "q-edit-93", "session_key": key, "msg": "DUPLICATE_MUST_NOT_REPLACE_93"})
        command("list")
        require(state["items"] == before, "Duplicate pending ID changed queue")
        # Fill to the existing sixteen-item bound, reject one, then remove extras.
        for number in range(13):
            enqueue("q-extra-" + str(number), "EXTRA_PENDING_93_" + str(number))
        require(len(state["items"]) == 16, "Queue capacity changed")
        host.controller.frame({"k": "agent-loop", "id": "q-overflow-93", "session_key": key, "msg": "OVERFLOW_MUST_NOT_RUN_93"})
        rejected = wait(lambda value: value.get("k") == "out" and value.get("id") == "q-overflow-93" and value.get("done"))
        require(rejected.get("err"), "Overflow was accepted")
        command("list")
        require(len(state["items"]) == 16, "Overflow changed pending queue")
        for number in range(13):
            command("remove", "q-extra-" + str(number))
        command("steer", "q-steer-93")
        require(all(row["id"] != "q-steer-93" for row in state["items"]), "Steering remained in follow-up queue")
        result["passed"].append("stable_pending_identity_capacity16_and_atomic_queue_to_steering")

        release.set()
        wait(lambda value: value.get("k") == "out" and value.get("id") == "q-edit-93" and value.get("done"), timeout=25)
        command("list")
        require(state["items"] == [] and state["active_run_id"] == "", "Settled queue still shows work")
        require((host.work / "queue-effect.txt").read_text() == "EXACTLY_ONCE_93", "Actual tool effect incorrect")
        wire = json.dumps(host.fixture.agent_requests, ensure_ascii=False)
        require("APPLIED_STEERING_93" in wire and "EDITED_FOLLOW_93" in wire and "FRONT_FOLLOW_93" in wire, "Expected messages did not reach provider")
        steering = [value for value in host.transcript if value.get("k") == "agent-queue-steer-state" and value.get("queue_id") == "q-steer-93"]
        require(any(value.get("status") == "submitted" for value in steering), "No native steering submission receipt")
        require(any(value.get("status") == "applied" and value.get("target_run_id") == run for value in steering), "No actual context-admission receipt")
        require(not any(value.get("status") == "unapplied" for value in steering), "Applied steering also reported unapplied")
        require(all(marker not in wire for marker in ["REMOVED_MUST_NOT_RUN_93", "ORIGINAL_EDIT_93", "EXTRA_PENDING_93_", "OVERFLOW_MUST_NOT_RUN_93", "DUPLICATE_MUST_NOT_REPLACE_93"]), "Removed/replaced/rejected work reached provider")
        queue_handover = next(i for i, value in enumerate(host.transcript) if value.get("k") == "agent-queue-state" and value.get("active_run_id") == "q-front-93")
        first_event = next(i for i, value in enumerate(host.transcript) if value.get("k") == "agent-event" and value.get("id") == "q-front-93")
        require(queue_handover < first_event, "Follow event preceded authoritative handover")
        applied_index = next(i for i, value in enumerate(host.transcript) if value.get("k") == "agent-queue-steer-state" and value.get("queue_id") == "q-steer-93" and value.get("status") == "applied")
        require(applied_index < queue_handover, "Old Run's steering admission arrived after next Run handover")
        terminals = [value["id"] for value in host.transcript if value.get("k") == "out" and value.get("done") and not value.get("err")]
        require(terminals[-3:] == [run, "q-front-93", "q-edit-93"], "Follow-up execution order changed")
        command("remove", "q-front-93", fail=True)
        result["passed"].append("actual_tool_effect_steering_context_fifo_handover_before_events_and_no_deleted_replay")

        # Cancellation before a tool boundary must not claim guidance applied.
        held.clear(); release.clear()
        (host.work / "queue-effect.txt").unlink()
        host.controller.frame({"k": "agent-loop", "id": "queue-abort-93", "session_key": key, "msg": "HOLD_QUEUE_93", "model": "default"})
        require(held.wait(10), "Abort scenario did not reach held request")
        command("list")
        enqueue("q-unapplied-93", "UNAPPLIED_GUIDANCE_93")
        command("steer", "q-unapplied-93")
        cancel_phase.set()
        host.controller.frame({"k": "agent-abort", "id": "queue-abort-93", "session_key": key})
        wait(lambda value: value.get("k") == "agent-queue-steer-state" and value.get("queue_id") == "q-unapplied-93" and value.get("status") == "unapplied")
        command("list")
        require(state["items"] == [], "Unapplied guidance was automatically requeued")
        require(not any(value.get("k") == "agent-queue-steer-state" and value.get("queue_id") == "q-unapplied-93" and value.get("status") == "applied" for value in host.transcript), "Canceled guidance also claimed applied")
        release.set()
        time.sleep(.2)
        result["passed"].append("cancel_before_context_admission_reports_unapplied_without_requeue")

        # Disconnect clears this temporary queue. Reconnect never replays it.
        held.clear(); release.clear()
        host.controller.frame({"k": "agent-loop", "id": "queue-detach-93", "session_key": key, "msg": "HOLD_QUEUE_93", "model": "default"})
        require(held.wait(10), "Detach scenario did not reach held request")
        enqueue("q-disconnected-93", "DETACHED_MUST_NOT_REPLAY_93")
        previous_epoch = state["queue_epoch"]
        host.controller.close(); host.controller = None
        release.set()
        time.sleep(.6)
        host.controller = WS(host.hub_port, "ctl", "controlled-context-root")
        host.controller.send({"k": "hello", "fp": host.owner, "relay": True})
        host.controller.until(lambda value: value.get("k") == "paired")
        host.controller.until(lambda value: value.get("k") == "dc" and json.loads(base64.b64decode(value["d"])).get("k") == "node-hello")
        command("list")
        require(state["queue_epoch"] != previous_epoch and state["items"] == [], "Disconnect retained pending queue")
        require("DETACHED_MUST_NOT_REPLAY_93" not in json.dumps(host.fixture.agent_requests), "Disconnected queue was replayed")
        result["passed"].append("disconnect_clears_temporary_queue_new_epoch_no_implicit_replay")
        require(not host.fixture.errors, "Controlled provider failed: " + str(host.fixture.errors))
        result.update(ok=True, model_calls=len(host.fixture.agent_requests), expected_cancel_disconnects=expected_disconnects)
    except Exception as error:
        result.update(error_type=type(error).__name__, error=str(error))
        raise
    finally:
        release.set(); watchdog.cancel(); host.close()
        result.update(node_sha256=digest(args.binary), hub_sha256=digest(args.hub_binary), script_sha256=digest(__file__), elapsed_seconds=round(time.monotonic() - started, 3))
        (root / "result.json").write_text(json.dumps(result, indent=2, ensure_ascii=False), encoding="utf-8")
        print(json.dumps(result, ensure_ascii=False))


if __name__ == "__main__":
    main()
