"""Actual Node -> actual isolated Hub -> configurable upstream model directory.

Tests declared capacity/modalities and dynamic text budgets with a controlled
provider, not provider capacity truth, image transport or a real model.
"""
import argparse
import hashlib
import json
from pathlib import Path
import threading
import time

from verify_context import Host, digest
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
    host.fixture.context_window = 1_000_000
    host.fixture.input_modalities = ["text", "image"]
    (host.work / "large-context.txt").write_text("MODEL_CAPS_BODY_153\n" + "large bounded source line\n" * 11000 + "MODEL_CAPS_END_153", encoding="utf-8")
    result = {"status": "FAIL", "scope": "actual Rust Node + isolated Go Hub + upstream directory/model fixture",
              "not_run": ["real provider capacity", "image transport", "production deployment"], "passed": []}
    started = time.monotonic()

    def reply(handler, body):
        require(body.get("tools"), "Unexpected summary request; dynamic budget should avoid it or reject before HTTP")
        host.fixture.agent_requests.append(body)
        messages = body["messages"]
        last_user = max(i for i, m in enumerate(messages) if m.get("role") == "user")
        objective = messages[last_user].get("content", "")
        receipts = [m for m in messages[last_user + 1:] if m.get("role") == "tool"]
        if "CAPS_LARGE_153" in objective and not receipts:
            host.fixture.emit(handler, tool="read_file", arguments={"path": str(host.work / "large-context.txt")})
        else:
            if "CAPS_LARGE_153" in objective:
                require(len(receipts) == 1 and "MODEL_CAPS_END_153" in receipts[0]["content"], "Large actual file receipt missing")
                require(len(json.dumps(body, ensure_ascii=False).encode()) > 192 * 1024, "Actual provider request was not above old fixed limit")
            host.fixture.emit(handler, text="MODEL_CAPS_VERIFIED_153")

    host.fixture.reply = reply
    watchdog = threading.Timer(90, lambda: (stop(host.node), stop(host.hub)))
    watchdog.daemon = True
    watchdog.start()
    try:
        host.start()
        original_frame = host.controller.frame
        selected = {"key": "model-caps/same-session"}
        def frame(value):
            original_frame(dict(value, execution_mode="plan", session_key=selected["key"]) if value.get("k") == "agent-loop" else value)
        host.controller.frame = frame
        _, events, _ = host.run("CAPS_LARGE_153 read the whole controlled UTF-8 file")
        cap = next(e for e in events if e.get("type") == "model_capabilities")
        require(cap["declared"]["context_window"] == 1_000_000 and cap["declared"]["input"] == ["text", "image"], "Actual directory capability receipt differs from upstream declaration")
        require(cap["host_input"] == ["text"] and cap["text_budget"]["projection_bytes"] > 192 * 1024, "Host misadvertised image support or retained old fixed budget")
        require(not host.fixture.summary_requests and not any(e.get("phase") == "summary" for e in events), "1M capacity still triggered 192 KiB summary")
        before = json.loads(host.raw_path().read_text(encoding="utf-8"))
        prefix_hash = hashlib.sha256(json.dumps(before["messages"], sort_keys=True, ensure_ascii=False).encode()).hexdigest()
        result["large_request_bytes"] = max(len(json.dumps(b, ensure_ascii=False).encode()) for b in host.fixture.agent_requests)
        result["passed"].append("actual_one_million_directory_modalities_receipt_text_only_host_and_large_request_without_summary")
        host.fixture.context_window = 4096
        count = len(host.fixture.requests)
        host.run("CAPS_TOO_SMALL_153 must fail without a pending-run poison", success=False)
        rejected = json.loads(host.raw_path().read_text(encoding="utf-8"))
        require(len(host.fixture.requests) == count, "4096 capacity admission failure reached provider")
        require(rejected.get("pending_run") is None, "4096 capacity admission left a durable incomplete-run fence")
        require(rejected["messages"] == before["messages"], "4096 admission failure changed native message audit")
        host.fixture.context_window = 1_000_000
        host.run("CAPS_RECOVERY_153 same Session after rejected 4096 admission")
        reopened = json.loads(host.raw_path().read_text(encoding="utf-8"))
        require(len(host.fixture.requests) == count + 1 and reopened.get("pending_run") is None, "Same Session could not resume after capacity restored")
        require(reopened["messages"][:len(before["messages"])] == before["messages"], "Capacity recovery reset original native audit")
        before = reopened
        prefix_hash = hashlib.sha256(json.dumps(before["messages"], sort_keys=True, ensure_ascii=False).encode()).hexdigest()
        result["passed"].append("4096_admission_rejected_without_pending_fence_or_provider_and_restored_same_session_resumes")
        host.fixture.context_window = 32768
        count = len(host.fixture.requests)
        _, events, _ = host.run("CAPS_SHRINK_153 same native Session", success=False)
        require(len(host.fixture.requests) == count, "Shrunk over-budget Session reached actual provider")
        cap = next(e for e in events if e.get("type") == "model_capabilities")
        require(cap["declared"]["context_window"] == 32768 and cap["text_budget"]["projection_bytes"] < 192 * 1024, "Context shrink was not observed by Node")
        after = json.loads(host.raw_path().read_text(encoding="utf-8"))
        require(after["messages"][:len(before["messages"])] == before["messages"], "Context shrink reset or rewrote native audit")
        require(hashlib.sha256(json.dumps(after["messages"][:len(before["messages"])], sort_keys=True, ensure_ascii=False).encode()).hexdigest() == prefix_hash, "Context shrink changed native prefix hash")
        require(after["route"] == before["route"], "Capability update changed native route identity")
        result["passed"].append("same_session_capacity_shrink_refused_before_provider_and_original_audit_prefix_hash_retained")
        selected["key"] = "model-caps/fresh-small-session"
        host.run("CAPS_SMALL_153 concise request with known small capacity")
        require(len(host.fixture.requests) == count + 1, "Independent small-capacity request could not flow")
        result["passed"].append("fresh_small_context_request_flows_without_raising_budget")
        host.fixture.context_window = "1M"
        count = len(host.fixture.requests)
        host.run("CAPS_MALFORMED_153", success=False)
        require(len(host.fixture.requests) == count, "Malformed declared capacity reached provider")
        require(not host.fixture.errors, "Controlled provider assertions failed")
        result["passed"].append("malformed_capacity_rejected_without_provider_call")
        result["status"] = "PASS"
    except Exception as error:
        result.update(error_type=type(error).__name__, error=str(error))
        raise
    finally:
        watchdog.cancel()
        host.close()
        result.update(binary_sha256=digest(args.binary), hub_sha256=digest(args.hub_binary), elapsed_seconds=round(time.monotonic() - started, 3))
        (root / "result.json").write_text(json.dumps(result, indent=2), encoding="utf-8")
        print(json.dumps(result))


if __name__ == "__main__":
    main()
