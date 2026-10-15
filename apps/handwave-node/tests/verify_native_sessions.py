"""Real node + isolated Hub/provider test of explicit web/native session mapping."""
import argparse
import base64
import json
from pathlib import Path
import threading
import time

from verify_context import Host, digest, working_view
from verify_host import require, stop


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--binary", type=Path, required=True)
    parser.add_argument("--hub-binary", type=Path, required=True)
    parser.add_argument("--output", type=Path, required=True)
    args = parser.parse_args()
    root = args.output.resolve()
    root.mkdir(parents=True, exist_ok=False)
    host = Host(args, root / "host")
    result = {"status": "FAIL", "scope": "actual Rust node + isolated real Hub + controlled provider", "passed": []}
    started = time.monotonic()
    selected = {"key": "", "model": "default"}
    held = threading.Event()
    released = threading.Event()
    original_reply = host.fixture.reply

    def reply(handler, body):
        if "HOLD_SESSION_A_87" in json.dumps(body.get("messages", [])[-1]):
            host.fixture.agent_requests.append(body)
            held.set()
            require(released.wait(10), "Held session did not release")
            return host.fixture.emit(handler, text="HELD_A_FINISHED_87")
        return original_reply(handler, body)

    host.fixture.reply = reply
    watchdog = threading.Timer(120, lambda: (stop(host.node), stop(host.hub)))
    watchdog.daemon = True
    watchdog.start()
    try:
        host.start()
        original_frame = host.controller.frame
        def frame(value):
            if value.get("k") in ("agent-loop", "agent-follow"):
                value = dict(value, session_key=selected["key"], model=selected["model"])
            original_frame(value)
        host.controller.frame = frame
        def run(key, objective, *, model="default", success=True):
            selected.update(key=key, model=model)
            host.fixture.expected_model = "fixture-model" if model == "default" else model
            return host.run(objective, success=success)
        def paths():
            return set((host.state / "sessions" / host.owner).glob("*.context.json"))
        def raw(path):
            return path.with_name(path.name.replace(".context.json", ".json"))

        run("workspace/session-a", "REFERENCE_ORIGINAL_73 ONLY_SESSION_A_87")
        a = next(iter(paths()))
        host.fixture.context_path = a
        run("workspace/session-a", "PROPOSE_CONTEXT_MEMORY_73")
        memory = next(m for m in host.state_json()["memories"] if m["text"] == "CANDIDATE_ONLY_73")
        run("workspace/session-a", "/memory accept " + memory["id"])
        a_before = json.loads(raw(a).read_text())
        run("workspace/session-b", "ONLY_SESSION_B_87")
        b = next(iter(paths() - {a}))
        require(a != b and a.name.startswith("ui-") and b.name.startswith("ui-"), "Explicit sessions share one path")
        request = json.dumps(host.fixture.agent_requests[-1])
        require("ONLY_SESSION_A_87" not in request and not working_view(host.fixture.agent_requests[-1])["accepted_memories"], "Session B inherited A raw or accepted memory")
        require(json.loads(b.read_text())["memories"] == [], "B sidecar inherited A memory")
        run("workspace/session-a", "RESUME_SESSION_A_87")
        request = json.dumps(host.fixture.agent_requests[-1])
        require("ONLY_SESSION_A_87" in request and "ONLY_SESSION_B_87" not in request and "CANDIDATE_ONLY_73" in request, "Same key did not resume A exactly")
        require(json.loads(raw(a).read_text())["messages"][:len(a_before["messages"])] == a_before["messages"], "Resume replaced A native audit")
        result["passed"].append("two_explicit_keys_isolate_native_raw_and_accepted_memory_same_key_resumes")

        before = paths()
        run("", "LEGACY_SESSION_87")
        legacy = next(iter(paths() - before))
        require(not legacy.name.startswith("ui-"), "Missing session key changed legacy filename")
        legacy_before = raw(legacy).read_bytes()
        run("workspace/session-a", "AFTER_LEGACY_A_87")
        require(raw(legacy).read_bytes() == legacy_before, "Explicit key rewrote legacy history")
        count = len(host.fixture.requests)
        run("../escape", "INVALID_KEY_87", success=False)
        require(len(host.fixture.requests) == count, "Invalid session key reached provider")
        result["passed"].append("legacy_missing_key_path_retained_invalid_key_rejected_without_provider")

        a_before_mismatch = raw(a).read_bytes()
        count = len(host.fixture.requests)
        failed, _, _ = run("workspace/session-a", "MISMATCH_SAME_SESSION_87", model="fixture-legacy-null", success=False)
        require("differs" in failed.get("err", ""), "Saved route fence was relaxed")
        require(raw(a).read_bytes() == a_before_mismatch and len(host.fixture.requests) == count, "Mismatch reset or replayed native session")
        run("workspace/session-c", "NEW_EXPLICIT_SESSION_OTHER_MODEL_87", model="fixture-legacy-null")
        require(host.fixture.agent_requests[-1]["model"] == "fixture-legacy-null", "New explicit session failed with different route")
        result["passed"].append("same_session_route_mismatch_preserves_fence_new_explicit_session_allows_new_route")

        errors = []
        def active():
            try:
                run("workspace/session-a", "HOLD_SESSION_A_87")
            except Exception as error:
                errors.append(repr(error))
        thread = threading.Thread(target=active, daemon=True)
        thread.start()
        require(held.wait(10), "Active A did not reach provider")
        selected.update(key="workspace/session-b", model="default")
        rejected_id = "WRONG_SESSION_FOLLOW_87"
        host.controller.frame({"k": "agent-follow", "id": rejected_id, "msg": "MUST_NOT_QUEUE_WRONG_SESSION_87"})
        deadline = time.monotonic() + 5
        while time.monotonic() < deadline and not any(frame.get("k") == "out" and frame.get("id") == rejected_id and frame.get("done") for frame in host.transcript):
            time.sleep(.02)
        rejection = next((frame for frame in host.transcript if frame.get("k") == "out" and frame.get("id") == rejected_id and frame.get("done")), None)
        require(rejection and "session differs" in rejection.get("err", ""), "Explicit wrong-session follow was silently reassigned")
        selected.update(key="workspace/session-a", model="default")
        supplied_id = "SUPPLIED_FOLLOW_ID_87"
        follow = {"k": "agent-follow", "id": supplied_id, "msg": "QUEUED_FOLLOW_A_87"}
        host.controller.frame(follow)
        host.controller.frame(follow)
        released.set()
        thread.join(15)
        require(not thread.is_alive() and not errors, "Active A did not settle")
        end = time.monotonic() + 15
        follow_done = False
        while time.monotonic() < end:
            packet = host.controller.receive(end-time.monotonic())
            if packet.get("k") != "dc":
                continue
            value = json.loads(base64.b64decode(packet["d"]))
            host.transcript.append(value)
            if value.get("k") == "out" and value.get("done"):
                require(not value.get("err"), "Queued follow failed")
                require(value.get("id") == supplied_id, "Follow did not retain the supplied run ID")
                follow_done = True
                break
        require(follow_done, "Queued follow did not finish")
        a_raw = raw(a).read_text()
        require("QUEUED_FOLLOW_A_87" in a_raw and "QUEUED_FOLLOW_A_87" not in raw(b).read_text(), "Follow-up followed changed UI selection instead of active A")
        require("MUST_NOT_QUEUE_WRONG_SESSION_87" not in a_raw and "MUST_NOT_QUEUE_WRONG_SESSION_87" not in raw(b).read_text(), "Rejected wrong-session follow reached native history")
        require(json.loads(a_raw)["completed_runs"].count(supplied_id) == 1, "Duplicate follow ID executed twice")
        require(not host.fixture.errors, "Provider assertions failed: " + str(host.fixture.errors))
        result["passed"].append("explicit_follow_retains_supplied_id_deduplicates_and_rejects_wrong_session")
        result["status"] = "PASS"
    except Exception as error:
        result.update(error_type=type(error).__name__, error=str(error))
        raise
    finally:
        released.set()
        watchdog.cancel()
        host.close()
        result.update(binary_sha256=digest(args.binary), hub_sha256=digest(args.hub_binary), fixture_sha256=digest(__file__), elapsed_seconds=round(time.monotonic()-started,3))
        (root / "result.json").write_text(json.dumps(result,indent=2,ensure_ascii=False),encoding="utf-8")
        print(json.dumps(result,indent=2,ensure_ascii=False))


if __name__ == "__main__":
    main()
