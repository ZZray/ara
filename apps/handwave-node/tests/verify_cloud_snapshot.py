"""Real Rust node + isolated Hub/provider acceptance of current cloud snapshots.

No production device, production credentials or real model calls are used.
"""
from __future__ import annotations

import argparse
import base64
import hashlib
import json
from pathlib import Path
import threading
import time

from verify_context import Host, digest, native_state, working_view
from verify_host import require, stop


def carrier(device, epoch, text=None, *, status="ready", legacy=""):
    records = [] if text is None else [{
        "id": "cloud-record-87", "revision": 1, "kind": "fact", "text": text,
        "hash": hashlib.sha256(text.encode()).hexdigest(), "scope": "device", "scope_id": device,
    }]
    value = {"version": 1, "status": status, "device_id": device, "epoch": epoch, "records": records}
    return "HANDWAVE_CLOUD_SNAPSHOT_V1\n" + json.dumps(value, ensure_ascii=False, separators=(",", ":")) + "\nHANDWAVE_BRIEF\n" + legacy


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--binary", type=Path, required=True)
    parser.add_argument("--hub-binary", type=Path, required=True)
    parser.add_argument("--output", type=Path, required=True)
    args = parser.parse_args()
    root = args.output.resolve()
    root.mkdir(parents=True, exist_ok=False)
    result = {"status": "FAIL", "scope": "actual Rust node + isolated Hub + controlled provider; no production or real model", "passed": []}
    started = time.monotonic()
    host = Host(args, root / "host")
    original_reply = host.fixture.reply
    follow_started = threading.Event()
    follow_release = threading.Event()

    def reply(handler, body):
        if not body.get("tools"):
            return original_reply(handler, body)
        host.fixture.agent_requests.append(body)
        messages = body.get("messages", [])
        last_user = max((i for i, m in enumerate(messages) if m.get("role") == "user"), default=-1)
        objective = json.dumps(messages[last_user].get("content", ""), ensure_ascii=False)
        tools = [m for m in messages[last_user + 1:] if m.get("role") == "tool"]
        if "HOLD_FOLLOW_87" in objective:
            follow_started.set()
            require(follow_release.wait(10), "Follow-up fixture was not released")
            return host.fixture.emit(handler, text="STALE_FOLLOW_ASSISTANT_87")
        if "PROPOSE_87" in objective and not tools:
            reference = next(r for r in host.state_json()["references"] if r["text"] == "LOCAL_SOURCE_87")
            return host.fixture.emit(handler, tool="memory_propose", arguments={"kind": "fact", "text": "CLOUD_DERIVED_CANDIDATE_87", "source_ids": [reference["id"]]})
        if "OLD_ASSISTANT_87" in objective:
            return host.fixture.emit(handler, text="STALE_CLOUD_ASSISTANT_87")
        return host.fixture.emit(handler, text="CURRENT_SNAPSHOT_OK_87")

    host.fixture.reply = reply
    watchdog = threading.Timer(150, lambda: (stop(host.node), stop(host.hub)))
    watchdog.daemon = True
    watchdog.start()
    try:
        host.start()
        device = host.device["fp"]
        old = carrier(device, "epoch-1", "OLD_CLOUD_SOURCE_87")
        new = carrier(device, "epoch-2", "NEW_CLOUD_SOURCE_87")
        host.run("LOCAL_SOURCE_87", brief=old)
        view = working_view(host.fixture.agent_requests[-1])
        require(view["cloud_snapshot"]["records"][0]["text"] == "OLD_CLOUD_SOURCE_87", "Snapshot absent from actual provider")
        require("OLD_CLOUD_SOURCE_87" not in host.sidecar().read_text(), "Snapshot body persisted in sidecar")
        require("OLD_CLOUD_SOURCE_87" not in json.dumps(native_state(host.raw_path())), "Snapshot body persisted in native raw")
        require(all("OLD_CLOUD_SOURCE_87" not in r["text"] for r in host.state_json()["references"]), "Snapshot was promoted to local reference")
        result["passed"].append("current_snapshot_real_wire_ephemeral_body_no_reference_promotion")

        host.run("PROPOSE_87", brief=old)
        memory = next(m for m in host.state_json()["memories"] if m["text"] == "CLOUD_DERIVED_CANDIDATE_87")
        require(memory["status"] == "candidate" and memory["cloud_stamp"]["epoch"] == "epoch-1", "Derived candidate lacks host epoch")
        count = len(host.fixture.requests)
        host.run("/memory accept " + memory["id"], brief=old)
        require(len(host.fixture.requests) == count, "Management unexpectedly called model")
        host.run("OLD_ASSISTANT_87", brief=old)
        # The checkpoint is only a replaceable hot tail. Compare the original
        # journal, validating its head, record hashes and append chain as well.
        raw_before = native_state(host.raw_path())
        require("STALE_CLOUD_ASSISTANT_87" in json.dumps(raw_before), "Fixture did not create stale assistant audit")
        host.run("CURRENT_AFTER_UPDATE_87", brief=new)
        current = json.dumps(host.fixture.agent_requests[-1])
        require("NEW_CLOUD_SOURCE_87" in current, "Correction absent from actual provider")
        require(all(marker not in current for marker in ("OLD_CLOUD_SOURCE_87", "STALE_CLOUD_ASSISTANT_87", "CLOUD_DERIVED_CANDIDATE_87")), "Stale cloud derivative revived")
        raw_after = native_state(host.raw_path())
        require(raw_after["messages"][:len(raw_before["messages"])] == raw_before["messages"], "Correction rewrote native raw history")
        require(host.state_json()["memories"][0]["status"] == "accepted", "Correction destroyed adoption audit")
        result["passed"].append("epoch_change_filters_accepted_cloud_derivative_and_raw_assistant_preserves_audit")

        host.restart()
        host.run("AFTER_RESTART_87", brief=new)
        require("NEW_CLOUD_SOURCE_87" in json.dumps(host.fixture.agent_requests[-1]), "Restart lost fresh snapshot")
        host.run("AFTER_UNAVAILABLE_87", brief=carrier(device, "", status="unavailable"))
        request = json.dumps(host.fixture.agent_requests[-1])
        require("NEW_CLOUD_SOURCE_87" not in request and working_view(host.fixture.agent_requests[-1])["cloud_snapshot"]["status"] == "unavailable", "Fetch failure reused cached cloud")
        host.run("AFTER_READY_EMPTY_87", brief=carrier(device, "epoch-3"))
        require(working_view(host.fixture.agent_requests[-1])["cloud_snapshot"]["records"] == [], "All-revoked snapshot not empty")
        host.run("REINTRODUCE_87", brief=new)
        host.run("MISSING_CARRIER_87")
        require(working_view(host.fixture.agent_requests[-1])["cloud_snapshot"] is None, "Missing carrier reused snapshot")
        require("NEW_CLOUD_SOURCE_87" not in json.dumps(host.fixture.agent_requests[-1]), "Missing carrier resurrected raw cloud")
        result["passed"].append("restart_unavailable_empty_and_missing_carrier_do_not_reuse_old_content")

        count = len(host.fixture.requests)
        host.run("WRONG_DEVICE_87", brief=carrier("device-not-authorized", "epoch-x", "FOREIGN_DEVICE_87"), success=False)
        require(len(host.fixture.requests) == count, "Wrong-device carrier reached provider")
        malformed = carrier(device, "epoch-4", "INTEGRITY_87").replace(hashlib.sha256(b"INTEGRITY_87").hexdigest(), "bad-hash")
        host.run("BAD_HASH_87", brief=malformed, success=False)
        require(len(host.fixture.requests) == count, "Bad-hash carrier reached provider")
        host.run("PLAIN_LEGACY_87", brief="legacy compatibility data")
        require("legacy compatibility data" in json.dumps(host.fixture.agent_requests[-1]), "Legacy brief compatibility lost")
        require(not host.fixture.errors, "Controlled provider assertion failed")
        result["passed"].append("wrong_device_and_hash_refused_before_provider_legacy_brief_compatible")

        # Queue while the original run is active, then revoke its cloud data.
        # agent-follow has no freshly fetched snapshot: it must not inherit one.
        errors = []
        def active_run():
            try:
                host.run("HOLD_FOLLOW_87", brief=carrier(device, "before-revoke", "REVOKED_FOLLOW_CLOUD_87"))
            except Exception as error:
                errors.append(error)
        active = threading.Thread(target=active_run, daemon=True)
        active.start()
        require(follow_started.wait(10), "Active run did not reach actual provider")
        # The control user has revoked the old record at this point. A queued
        # follow without a fresh carrier must use no cloud context at all.
        count = len(host.fixture.agent_requests)
        host.controller.frame({"k": "agent-follow", "msg": "AFTER_REVOKE_FOLLOW_87"})
        follow_release.set()
        active.join(15)
        require(not active.is_alive() and not errors, "Original active run did not settle")
        end = time.monotonic() + 20
        follow_done = False
        while time.monotonic() < end:
            packet = host.controller.receive(end - time.monotonic())
            if packet.get("k") != "dc":
                continue
            frame = json.loads(base64.b64decode(packet["d"]))
            host.transcript.append(frame)
            if frame.get("k") == "out" and frame.get("done"):
                require(not frame.get("err") and frame.get("code", 0) == 0, "Queued follow failed")
                follow_done = True
                break
        require(follow_done and len(host.fixture.agent_requests) > count, "Queued follow never called provider")
        follow_request = host.fixture.agent_requests[-1]
        require(working_view(follow_request)["cloud_snapshot"] is None, "Queued follow inherited stale snapshot")
        wire = json.dumps(follow_request)
        require("REVOKED_FOLLOW_CLOUD_87" not in wire and "STALE_FOLLOW_ASSISTANT_87" not in wire, "Queued follow resurrected revoked cloud via raw transcript")
        require(not host.fixture.errors, "Follow provider assertion failed")
        result["passed"].append("active_agent_follow_drops_inherited_snapshot_and_old_assistant")
        result["status"] = "PASS"
    except Exception as error:
        result.update(error_type=type(error).__name__, error=str(error))
        raise
    finally:
        follow_release.set()
        watchdog.cancel()
        host.close()
        result.update(binary_sha256=digest(args.binary), hub_sha256=digest(args.hub_binary), fixture_sha256=digest(__file__), elapsed_seconds=round(time.monotonic()-started, 3))
        (root / "result.json").write_text(json.dumps(result, indent=2, ensure_ascii=False), encoding="utf-8")
        print(json.dumps(result, indent=2, ensure_ascii=False))


if __name__ == "__main__":
    main()
