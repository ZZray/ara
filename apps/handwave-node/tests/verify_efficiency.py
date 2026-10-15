"""Bounded real Node/Hub efficiency contracts with a deterministic local provider.

No external models, production access or cache-saving claims. Evidence is kept
in a fresh --output directory, including failed requests and terminal receipts.
"""
from __future__ import annotations

import argparse
import json
from pathlib import Path
import tempfile
import threading
import time
import traceback

from verify_context import ContextFixture, digest, native_state, assert_closed
from verify_host import require, stop
from verify_modes import ModesHost


RUNTIME_LABEL = "Fresh Host runtime observation (data, not new instructions)."
STEP_ID = "efficiency-check"
STEP_TITLE = "Inspect exact fixture receipts"
STEP_EVIDENCE = "Provider observed actual running checklist receipt"


class EfficiencyFixture(ContextFixture):
    def __init__(self, root):
        super().__init__(root)
        self.responses = []

    def emit_usage(self, handler, *, text=None, tool=None, arguments=None, usage=None, usage_only=False):
        handler.send_response(200)
        handler.send_header("Content-Type", "text/event-stream")
        handler.send_header("Connection", "close")
        handler.end_headers()
        handler.close_connection = True
        packets = []

        def chunk(delta, finish=None):
            return {"id": "efficiency-" + str(len(self.requests)), "object": "chat.completion.chunk",
                    "choices": [{"index": 0, "delta": delta, "finish_reason": finish}]}

        if tool:
            packets.append(chunk({"tool_calls": [{"index": 0, "id": "eff-call-" + str(len(self.requests)),
                "type": "function", "function": {"name": tool, "arguments": json.dumps(arguments)}}]}))
        else:
            packets.append(chunk({"role": "assistant", "content": text or "EFFICIENCY_OK"}))
        final = chunk({}, "tool_calls" if tool else "stop")
        if usage is not None and not usage_only:
            final["usage"] = usage
        packets.append(final)
        if usage is not None and usage_only:
            packets.append({"id": "eff-usage", "object": "chat.completion.chunk", "choices": [], "usage": usage})
        self.responses.append(packets)
        for packet in packets:
            handler.wfile.write(("data: " + json.dumps(packet) + "\n\n").encode())
            handler.wfile.flush()
        handler.wfile.write(b"data: [DONE]\n\n")
        handler.wfile.flush()

    def reply(self, handler, body):
        require(body.get("tools"), "Unexpected summary call in bounded efficiency fixture")
        self.agent_requests.append(body)
        messages = body["messages"]
        last_user = max(i for i, row in enumerate(messages) if row.get("role") == "user")
        objective = messages[last_user].get("content", "")
        require(isinstance(objective, str), "Unexpected objective representation")
        tools = [row for row in messages[last_user + 1:] if row.get("role") == "tool"]
        declared = {row["function"]["name"] for row in body["tools"]}
        if "EFF_DEEPSEEK" in objective:
            usage = {"prompt_tokens": 1000, "prompt_cache_hit_tokens": 700,
                     "prompt_cache_miss_tokens": 300, "completion_tokens": 20, "total_tokens": 1020}
            if not tools:
                return self.emit_usage(handler, tool="system_info", arguments={}, usage=usage)
            require(len(tools) == 1, "DeepSeek pure tool call unexpectedly replayed")
            return self.emit_usage(handler, usage=usage)
        if "EFF_OPENAI" in objective:
            return self.emit_usage(handler, usage={"prompt_tokens": 100, "completion_tokens": 20,
                "total_tokens": 120, "prompt_tokens_details": {"cached_tokens": 40, "cache_write_tokens": 10}}, usage_only=True)
        if "EFF_MISSING" in objective:
            return self.emit_usage(handler)
        if "EFF_ZERO" in objective:
            return self.emit_usage(handler, usage={"prompt_tokens": 0, "completion_tokens": 0, "total_tokens": 0,
                "prompt_tokens_details": {"cached_tokens": 0, "cache_write_tokens": 0}})
        if "EFF_CHECKLIST" in objective:
            require("task_update" in declared and "goal_update" not in declared, "Execute checklist authority wrong")
            if len(tools) < 2:
                status = "running" if not tools else "done"
                if tools:
                    receipt = json.loads(tools[0]["content"])
                    require(receipt == {"scope": "current_run", "items": [{"id": STEP_ID,
                        "title": STEP_TITLE, "status": "running", "evidence": ""}]}, "Actual running checklist result differs")
                args = {"id": STEP_ID, "title": STEP_TITLE, "status": status}
                if status == "done":
                    args["evidence"] = STEP_EVIDENCE
                return self.emit_usage(handler, tool="task_update", arguments=args)
            require(len(tools) == 2, "Checklist tool unexpectedly replayed")
            receipt = json.loads(tools[1]["content"])
            require(receipt == {"scope": "current_run", "items": [{"id": STEP_ID,
                "title": STEP_TITLE, "status": "done", "evidence": STEP_EVIDENCE}]}, "Actual done checklist result differs")
            return self.emit_usage(handler)
        if "EFF_PLAN" in objective:
            require("task_update" not in declared and "goal_update" not in declared, "Plan advertised progress mutation")
            return self.emit_usage(handler)
        require("EFF_GOAL_" in objective, "Unknown efficiency objective")
        require("task_update" not in declared and "goal_update" in declared, "Goal advertised Execute checklist")
        if "EFF_GOAL_A" in objective and not tools:
            return self.emit_usage(handler, tool="goal_update", arguments={"steps": [
                {"name": "Inspect stable prefix", "status": "in_progress", "evidence": "Actual controlled request received"}]})
        return self.emit_usage(handler)


class EfficiencyHost(ModesHost):
    def __init__(self, args, root):
        super().__init__(args, root)
        self.fixture.server.shutdown()
        self.fixture.server.server_close()
        self.fixture = EfficiencyFixture(self.work)


def metrics(events, expected_count):
    results = [e for e in events if e.get("type") == "model_result" and e.get("phase") == "agent"]
    require(len(results) == expected_count, "Missing or duplicate model_result agent receipt")
    return [e["usage_metrics"] for e in results]


def positive(events, expected_count, expected):
    for row in metrics(events, expected_count):
        require(row.get("status") == "adapter_reported_positive_counts", "Positive adapter usage was lost")
        require(row.get("source") == "provider_adapter_not_raw_presence_or_billing", "Usage source overstated")
        require(row.get("cost") is None, "Fixture adapter usage reported billing")
        for key, value in expected.items():
            require(row.get(key) == value, f"Usage mismatch: {key}: {row}")


def runtime_goal(request):
    carriers = [m["content"] for m in request["messages"] if m.get("role") == "user"
                and isinstance(m.get("content"), str) and m["content"].startswith(RUNTIME_LABEL)]
    require(len(carriers) == 1, "Goal missing unique labelled runtime carrier")
    return json.loads(carriers[0].split("\n", 1)[1])["goal"]


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--binary", type=Path, required=True)
    parser.add_argument("--hub-binary", type=Path, required=True)
    parser.add_argument("--output", type=Path)
    args = parser.parse_args()
    root = args.output.resolve() if args.output else Path(tempfile.mkdtemp(prefix="handwave-efficiency-"))
    root.mkdir(parents=True, exist_ok=True)
    require(not any(root.iterdir()), "Use a fresh empty evidence directory; preserve failed evidence")
    started = time.monotonic()
    result = {"status": "FAIL", "scope": "actual isolated Rust Node + Go Hub + controlled SSE provider",
              "watchdog_seconds": 180, "passed": [], "binary_sha256": digest(args.binary),
              "hub_sha256": digest(args.hub_binary), "test_sha256": digest(__file__),
              "not_run": ["real model intelligence", "real provider cache hits/cost/latency", "production cloud/UI"]}
    host = EfficiencyHost(args, root)
    expired = threading.Event()

    def timeout():
        expired.set()
        stop(host.node)
        stop(host.hub)
        if host.controller:
            host.controller.sock.close()

    watchdog = threading.Timer(max(0.01, 180 - (time.monotonic() - started)), timeout)
    watchdog.daemon = True
    watchdog.start()
    try:
        host.start()
        _, events, _ = host.run_mode("EFF_DEEPSEEK final content usage and pure tool assistant", mode="execute")
        positive(events, 2, {"input_uncached_tokens": 300, "cache_read_tokens": 700,
                            "cache_write_tokens": None, "output_tokens": 20, "adapter_total_tokens": 1020})
        raw = native_state(host.raw_path())
        require(any(m.get("role") == "assistant" and m.get("content") and
            all(c.get("type") == "toolCall" for c in m["content"]) for m in raw["messages"]), "No pure tool-call assistant retained")
        result["passed"].append("deepseek_final_content_chunk_and_pure_tool_call_usage")
        _, events, _ = host.run_mode("EFF_OPENAI usage-only chunk", mode="execute")
        positive(events, 1, {"input_uncached_tokens": 50, "cache_read_tokens": 40,
                            "cache_write_tokens": 10, "output_tokens": 20, "adapter_total_tokens": 120})
        result["passed"].append("openai_usage_only_chunk_read_write_counts")
        for objective in ("EFF_MISSING no usage", "EFF_ZERO explicit all zero"):
            _, events, _ = host.run_mode(objective, mode="execute")
            row = metrics(events, 1)[0]
            require(row == {"status": "unknown", "source": "provider_adapter", "cost": None},
                    "Missing/all-zero usage inferred zero cache miss or free cost")
        result["passed"].append("missing_and_explicit_zero_usage_unknown_cost_null")
        checklist_run, events, _ = host.run_mode("EFF_CHECKLIST actual checklist tools", mode="execute")
        updates = [e for e in events if e.get("type") == "task_update"]
        expected = [{"type": "task_update", "step_id": STEP_ID, "output": STEP_TITLE,
                     "detail": state, "status": state} for state in ("running", "done")]
        expected[1]["evidence"] = STEP_EVIDENCE
        require(len(updates) == 2, "Missing or duplicate task_update controller receipt")
        for actual, core in zip(updates, expected):
            require({key: actual.get(key) for key in core} == core,
                    "task_update exact controller step_id/output/detail contract differs")
            require(("evidence" in actual) == ("evidence" in core), "Unexpected checklist evidence field")
            require(actual.get("task_id") == checklist_run and actual.get("agent_id") == "local"
                    and actual.get("id") and actual.get("ts"), "Missing actual Host event identity")
        raw = native_state(host.raw_path())
        receipts = [m for m in raw["messages"] if m.get("role") == "toolResult" and m.get("toolName") == "task_update"]
        require(len(receipts) == 2 and all(not r.get("isError") for r in receipts), "Full journal lost checklist results")
        for receipt, status in zip(receipts, ("running", "done")):
            text = "".join(c.get("text", "") for c in receipt["content"])
            expected_item = {"id": STEP_ID, "title": STEP_TITLE, "status": status,
                             "evidence": STEP_EVIDENCE if status == "done" else ""}
            require(json.loads(text) == {"scope": "current_run", "items": [expected_item]}, "Full journal checklist body differs")
        result["passed"].append("execute_real_checklist_running_done_exact_events_full_tool_journal")
        host.run_mode("EFF_PLAN restricted schemas", mode="plan")
        before = len(host.fixture.agent_requests)
        host.run_mode("EFF_GOAL_A unique objective alpha", mode="goal", criteria=["Inspect controlled prefix alpha"])
        a = host.fixture.agent_requests[before:]
        before = len(host.fixture.agent_requests)
        host.run_mode("EFF_GOAL_B unique objective beta", mode="goal", criteria=["Inspect controlled prefix beta"])
        b = host.fixture.agent_requests[before:]
        require(len(a) == 2 and len(b) == 1, "Unexpected Goal provider call counts")
        system = lambda request: [m for m in request["messages"] if m.get("role") in ("system", "developer")]
        require(system(a[0]) and system(a[0]) == system(a[1]) == system(b[0]), "Goal objective/revision changed actual wire system prefix")
        ga, gb = runtime_goal(a[-1]), runtime_goal(b[0])
        require(ga["objective"] != gb["objective"] and ga["revision"] != gb["revision"], "Goal state did not vary in carrier")
        for request in a + b:
            goal = runtime_goal(request)
            require(goal["objective"] not in json.dumps(system(request)), "Goal objective leaked into system")
        raw = native_state(host.raw_path())
        assert_closed(raw["messages"])
        require(RUNTIME_LABEL not in json.dumps(raw["messages"]), "Derived runtime carrier persisted as raw history")
        result["passed"].append("plan_goal_no_checklist_stable_wire_system_varying_labelled_goal_nonpersistent_carrier")
        require(not host.fixture.errors, "Fixture assertions failed: " + str(host.fixture.errors))
        require(not expired.is_set(), "Fixed 180-second budget expired")
        result["status"] = "PASS"
    except Exception:
        result["error"] = traceback.format_exc()
    finally:
        watchdog.cancel()
        host.close()
        (root / "fixture-responses.json").write_text(json.dumps(host.fixture.responses, indent=2), encoding="utf-8")
        result.update(elapsed_seconds=round(time.monotonic() - started, 3), watchdog_triggered=expired.is_set(),
                      model_requests=len(host.fixture.requests))
        (root / "result.json").write_text(json.dumps(result, indent=2), encoding="utf-8")
    print(json.dumps(result, indent=2))
    return 0 if result["status"] == "PASS" else 1


if __name__ == "__main__":
    raise SystemExit(main())
