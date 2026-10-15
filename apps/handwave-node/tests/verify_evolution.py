"""Actual Node + Go Hub acceptance of reviewed, evidence-bound experience.

The provider is deterministic: this verifies host effects, provenance and recall
boundaries, not model intelligence, causal learning or production installation.
"""
from __future__ import annotations

import argparse
import hashlib
import json
import os
from pathlib import Path
import sys
import tempfile
import threading
import time

from verify_context import ContextFixture, Host, working_view
from verify_host import require, stop


LESSON = "EVOLUTION_REVIEWED_LESSON_73"
CONDITIONS = "When the isolated Python fixture exits seven before its correction"
RECEIPT_DIRECTORY = "Host current-Run receipt directory;"


def receipt_directories(body):
    directories = []
    for message in body.get("messages", []):
        content = message.get("content", "")
        if isinstance(content, list):
            content = "".join(part.get("text", "") for part in content if isinstance(part, dict))
        if isinstance(content, str) and content.startswith(RECEIPT_DIRECTORY):
            require(message.get("role") == "user", "Receipt directory has unexpected provider role")
            directories.append(json.loads(content.split("\n", 1)[1]))
    return directories


class EvolutionFixture(ContextFixture):
    def __init__(self, root):
        self.actual_receipts = {}
        self.searches = []
        self.proposal_sources = {}
        self.validated = []
        super().__init__(root)

    def emit_call(self, handler, name, arguments, call_id):
        handler.send_response(200)
        handler.send_header("Content-Type", "text/event-stream")
        handler.send_header("Connection", "close")
        handler.end_headers()
        handler.close_connection = True
        for delta, finish in [({"tool_calls": [{"index": 0, "id": call_id, "type": "function", "function": {"name": name, "arguments": json.dumps(arguments)}}]}, None), ({}, "tool_calls")]:
            packet = {"id": "evolution-fixture", "object": "chat.completion.chunk", "choices": [{"index": 0, "delta": delta, "finish_reason": finish}]}
            handler.wfile.write(("data: " + json.dumps(packet) + "\n\n").encode())
        handler.wfile.write(b"data: [DONE]\n\n")
        handler.wfile.flush()

    def reply(self, handler, body):
        require(body.get("tools"), "Unexpected compaction/model call in bounded evolution fixture")
        self.agent_requests.append(body)
        messages = body["messages"]
        last_user = max(i for i, row in enumerate(messages) if row.get("role") == "user")
        objective = messages[last_user].get("content", "")
        require(isinstance(objective, str), "Unexpected current user representation")
        scenario = next((s for s in ("BUILD_A", "BUILD_B", "FORGED", "CROSS_RUN", "SEARCH_CANDIDATE", "SEARCH_RESTART", "SEARCH_ACCEPTED_A", "SEARCH_REVOKED", "SEARCH_ACCEPTED_B", "SEARCH_STALE") if "EVOLUTION_" + s in objective), None)
        require(scenario, "Unknown evolution scenario")
        schemas = {t["function"]["name"]: t["function"]["parameters"] for t in body["tools"]}
        require({"run_script", "experience_propose", "experience_search"} <= schemas.keys(), "Evolution tools absent from actual provider request")
        require(set(schemas["experience_propose"]["properties"]) == {"lesson", "conditions", "source_ids", "tool_call_ids"}, "Model could supply unsupported provenance fields")
        current = messages[last_user + 1:]
        receipts = [m for m in current if m.get("role") == "tool"]
        calls = {c["id"]: c["function"]["name"] for m in current for c in m.get("tool_calls", [])}
        for receipt in receipts:
            cid = receipt["tool_call_id"]
            require(cid in calls, "Actual tool receipt lacks matching call")
            self.actual_receipts[cid] = {"tool_name": calls[cid], "content": receipt["content"]}
        stage = len(receipts)
        directories = receipt_directories(body)
        if scenario.startswith("BUILD_") and stage >= 2:
            require(len(directories) == 1, "Actual failure/recovery did not provide one current-Run receipt directory")
            directory = directories[0]
            require(directory["source"] == "native_current_run", "Receipt directory does not identify native provenance")
            require(directory["receipts"] == [
                {"tool_call_id": scenario + "-failure", "tool_name": "run_script", "is_error": True},
                {"tool_call_id": scenario + "-recovery", "tool_name": "run_script", "is_error": False},
            ], "Receipt directory IDs/statuses differ from actual current-Run native calls")
            require(all(set(row) == {"tool_call_id", "tool_name", "is_error"} for row in directory["receipts"]), "Receipt directory carries output bodies rather than bounded metadata")
        else:
            require(not directories, "Receipt directory exists before recovery or leaked from an earlier Run/restart")
        if scenario.startswith("BUILD_"):
            if stage == 0:
                self.emit_call(handler, "run_script", {"language": "python", "script": "import sys\nprint('ACTUAL_FAILURE_73')\nsys.exit(7)\n", "timeout": 10, "output_limit": 2048}, scenario + "-failure")
            elif stage == 1:
                require("7" in receipts[0]["content"] and "ACTUAL_FAILURE_73" in receipts[0]["content"], "Actual failure output missing")
                script = "from pathlib import Path\nPath('" + scenario + ".txt').write_text('RECOVERED_ARTIFACT_73', encoding='utf-8')\nprint('ACTUAL_RECOVERY_73')\n"
                self.emit_call(handler, "run_script", {"language": "python", "script": script, "timeout": 10, "output_limit": 2048}, scenario + "-recovery")
            elif stage == 2:
                require("ACTUAL_RECOVERY_73" in receipts[1]["content"], "Actual recovery receipt absent")
                require((self.root / (scenario + ".txt")).read_text() == "RECOVERED_ARTIFACT_73", "Script artifact was not recovered")
                refs = working_view(body)["source_directory"]["references"]
                source = next(r["id"] for r in refs if "EVOLUTION_" + scenario in r["excerpt"])
                self.proposal_sources[scenario] = source
                self.emit_call(handler, "experience_propose", {"lesson": LESSON + "_" + scenario, "conditions": CONDITIONS, "source_ids": [source], "tool_call_ids": [scenario + "-failure", scenario + "-recovery"]}, scenario + "-proposal")
            else:
                require(stage == 3, "Unexpected extra evolution calls")
                require("candidate" in receipts[2]["content"], "Proposal did not return candidate receipt")
                self.validated.append(scenario)
                self.emit(handler, text="EVOLUTION_BUILD_VALIDATED")
        elif scenario in ("FORGED", "CROSS_RUN"):
            if stage == 0:
                refs = working_view(body)["source_directory"]["references"]
                ids = ["invented-failure", "invented-recovery"] if scenario == "FORGED" else ["BUILD_A-failure", "BUILD_A-recovery"]
                self.emit_call(handler, "experience_propose", {"lesson": "REJECTED_PROPOSAL_73", "conditions": CONDITIONS, "source_ids": [refs[-1]["id"]], "tool_call_ids": ids}, scenario + "-proposal")
            else:
                require(stage == 1 and "candidate" not in receipts[0]["content"], "Invalid evidence unexpectedly created candidate")
                self.validated.append(scenario)
                self.emit(handler, text="EVOLUTION_INVALID_EVIDENCE_VALIDATED")
        elif stage == 0:
            wire = json.dumps(body, ensure_ascii=False)
            require(all(LESSON + "_" + label not in wire for label in ("BUILD_A", "BUILD_B")), "Experience body leaked into ordinary provider context before explicit retrieval")
            self.emit_call(handler, "experience_search", {"query": LESSON}, scenario + "-search")
        else:
            require(stage == 1, "Unexpected repeated retrieval")
            found = json.loads(receipts[0]["content"])
            require(found["limit"] == 3 and len(found["matches"]) <= 3, "Experience retrieval exceeded three results")
            expected = scenario in ("SEARCH_ACCEPTED_A", "SEARCH_ACCEPTED_B")
            require(bool(found["matches"]) == expected, "Candidate/revoked/stale recall boundary failed: " + scenario)
            if expected:
                require(len(found["matches"]) == 1 and LESSON in found["matches"][0]["lesson"], "Accepted experience retrieval lost lesson")
            self.searches.append({"scenario": scenario, "result": found})
            self.validated.append(scenario)
            self.emit(handler, text="EVOLUTION_SEARCH_VALIDATED")


class EvolutionHost(Host):
    def __init__(self, args, root):
        super().__init__(args, root, owner="evolution-owner", max_calls=12)
        self.fixture.server.shutdown()
        self.fixture.server.server_close()
        self.fixture = EvolutionFixture(self.work)
        self.env["PATH"] = str(Path(sys.executable).parent) + os.pathsep + self.env.get("PATH", "")


def native_receipts(host):
    raw = json.loads(host.raw_path().read_text(encoding="utf-8"))
    require(RECEIPT_DIRECTORY not in json.dumps(raw, ensure_ascii=False), "Provider-only receipt directory polluted persisted native audit")
    result = {}
    for row in raw["messages"]:
        if row.get("role") == "toolResult":
            content = "\n".join(c["text"] for c in row["content"] if c["type"] == "text")
            result[row["toolCallId"]] = {"tool_name": row["toolName"], "is_error": row["isError"], "output_hash": hashlib.sha256(content.encode()).hexdigest(), "content": content}
    return result


def candidate(host, label):
    return next(m for m in host.state_json()["memories"] if m["text"] == LESSON + "_" + label)


def command(host, text):
    count = len(host.fixture.requests)
    receipt = host.run(text)
    require(len(host.fixture.requests) == count, "Trusted management command reached provider")
    return receipt


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--binary", type=Path, required=True)
    parser.add_argument("--hub-binary", type=Path, required=True)
    parser.add_argument("--output", type=Path)
    args = parser.parse_args()
    root = args.output.resolve() if args.output else Path(tempfile.mkdtemp(prefix="handwave-evolution-"))
    root.mkdir(parents=True, exist_ok=True)
    require(not any(root.iterdir()), "Use fresh empty evidence directory")
    host = EvolutionHost(args, root)
    result = {"scope": "actual Rust Node + Go Hub; deterministic model; isolated script artifacts", "not_run": ["real-model intelligence", "causal correctness of proposed lesson", "production installation"], "passed": []}
    started = time.monotonic()
    watchdog = threading.Timer(60, lambda: (stop(host.node), stop(host.hub)))
    watchdog.daemon = True
    watchdog.start()
    try:
        host.start()
        _, events, _ = host.run("EVOLUTION_BUILD_A: demonstrate controlled failure and recovery")
        require(any(e.get("type") == "tool_end" and e.get("status") == "failed" for e in events), "Failed Python tool not exposed to controller")
        a = candidate(host, "BUILD_A")
        require(a["status"] == "candidate" and a.get("adoption") is None, "Run silently adopted experience")
        require(LESSON + "_BUILD_A" in host.raw_path().read_text(encoding="utf-8"), "Raw audit discarded proposal arguments")
        evidence = a["experience"]
        native = native_receipts(host)
        require(evidence["run"] == a["run"] and evidence["conditions"] == CONDITIONS, "Experience ownership changed")
        require([r["is_error"] for r in evidence["receipts"]] == [True, False], "Native failure/recovery statuses not preserved")
        for row in evidence["receipts"]:
            actual = native[row["tool_call_id"]]
            require(row["source"] == "native_tool_result", "Receipt does not identify native provenance")
            require(all(row[key] == actual[key] for key in ("tool_name", "is_error", "output_hash")), "Receipt differs from actual persisted native result")
            require(actual["content"].startswith(row["excerpt"]), "Receipt excerpt differs from native text")
        result["passed"].append("failure_recovery_artifact_and_native_hash_status_provenance")
        host.run("EVOLUTION_SEARCH_CANDIDATE")
        before = len(host.state_json()["memories"])
        for marker in ("FORGED", "CROSS_RUN"):
            _, events, _ = host.run("EVOLUTION_" + marker)
            require(any(e.get("type") == "tool_end" and e.get("status") == "failed" for e in events), "Invalid evidence was not failed tool")
        require(len(host.state_json()["memories"]) == before, "Rejected proposal wrote memory")
        result["passed"].append("forged_and_cross_run_ids_rejected_without_memory_write")
        host.restart()
        require(candidate(host, "BUILD_A")["status"] == "candidate", "Restart promoted candidate")
        host.run("EVOLUTION_SEARCH_RESTART")
        _, _, listed = command(host, "/memory list")
        require(a["id"] in listed and LESSON + "_BUILD_A" in listed, "Controller cannot inspect the candidate before adoption")
        command(host, "/memory accept " + a["id"])
        require(candidate(host, "BUILD_A")["status"] == "accepted", "Trusted adoption not durable")
        host.run("EVOLUTION_SEARCH_ACCEPTED_A")
        command(host, "/memory revoke " + a["id"])
        host.run("EVOLUTION_SEARCH_REVOKED")
        result["passed"].append("candidate_restart_explicit_accept_recall_revoke_zero_model_commands")
        host.run("EVOLUTION_BUILD_B: independent source for invalidation")
        b = candidate(host, "BUILD_B")
        command(host, "/memory accept " + b["id"])
        host.run("EVOLUTION_SEARCH_ACCEPTED_B")
        command(host, "/reference revoke " + host.fixture.proposal_sources["BUILD_B"])
        host.run("EVOLUTION_SEARCH_STALE")
        native_receipts(host)
        result["passed"].append("accepted_experience_excluded_after_source_revocation")
        result["passed"].append("current_run_recovery_ids_provider_only_no_cross_run_or_restart_directory")
        require(not host.fixture.errors, "Controlled provider assertions failed")
        result["status"] = "PASS"
    except Exception as error:
        result.update(status="FAIL", error=f"{type(error).__name__}: {error}")
        raise
    finally:
        watchdog.cancel()
        result.update(elapsed_seconds=round(time.monotonic() - started, 3), binary_sha256=hashlib.sha256(args.binary.read_bytes()).hexdigest(), hub_sha256=hashlib.sha256(args.hub_binary.read_bytes()).hexdigest(), model_requests=len(host.fixture.requests), fixture_errors=host.fixture.errors)
        for name, value in (("result", result), ("actual-model-receipts", host.fixture.actual_receipts), ("searches", host.fixture.searches)):
            (root / (name + ".json")).write_text(json.dumps(value, ensure_ascii=False, indent=2), encoding="utf-8")
        host.close()
        print(json.dumps(result, ensure_ascii=False, indent=2))


if __name__ == "__main__":
    main()
