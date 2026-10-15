"""Actual Node + isolated Hub/provider: segmented history, restart, retrieval and tail fence.

No production or real-model access. Does not modify a real user Session. All
artifacts and deliberate corruption are confined to a fresh --output directory.
"""
from __future__ import annotations

import argparse
import hashlib
import json
from pathlib import Path
import tempfile
import threading
import time

import verify_context as context
from verify_host import require, stop


class JournalFixture(context.ContextFixture):
    last_receipt = None
    retrieval_checks = 0

    def __init__(self, root):
        self.http_timing = []
        super().__init__(root)

    def handler(self):
        # Diagnostic receipts describe the controlled upstream, including catalog
        # failures before any model call. They do not change HTTP behavior.
        base, fixture = super().handler(), self

        class TimedHandler(base):
            def send_response(self, code, message=None):
                self.observed_status = code
                return super().send_response(code, message)

            def timed(self, method):
                started = time.monotonic()
                self.observed_status = None
                row = {"method": method, "path": self.path, "started": started,
                       "active_threads": threading.active_count()}
                try:
                    return getattr(super(), "do_" + method)()
                except Exception as error:
                    row["error"] = type(error).__name__
                    raise
                finally:
                    row.update(seconds=round(time.monotonic()-started, 6),
                               status=getattr(self, "observed_status", None))
                    with fixture.lock:
                        fixture.http_timing.append(row)

            def do_GET(self):
                return self.timed("GET")

            def do_POST(self):
                return self.timed("POST")

        return TimedHandler

    def reply(self, handler, body):
        if "summariz" in body.get("messages", [{}])[0].get("content", "").lower():
            return super().reply(handler, body)
        messages = body.get("messages", [])
        last_user = max((i for i, m in enumerate(messages) if m.get("role") == "user"), default=-1)
        objective = json.dumps(messages[last_user].get("content", "")) if last_user >= 0 else ""
        tools = [m for m in messages[last_user + 1:] if m.get("role") == "tool"]
        if "JOURNAL_BULK_MARKER_" in objective:
            marker = objective.split("JOURNAL_BULK_MARKER_", 1)[1][:4]
            self.agent_requests.append(body)
            return self.emit(handler, text="JOURNAL_BULK_MARKER_" + marker + " " + "bounded_raw_original_" * 5200)
        if "JOURNAL_SEARCH_FIRST" in objective:
            self.agent_requests.append(body)
            if not tools:
                return self.emit(handler, tool="history_search", arguments={"query": "JOURNAL_BULK_MARKER_0000", "start": 0, "scan_limit": 32})
            result = json.loads(tools[-1]["content"])
            self.last_receipt = next(m for m in result["matches"] if m["bytes"] > 50000)
            require(not result["complete"] and result["next_cursor"] <= 32, "History search hid its bounded coverage")
            self.retrieval_checks += 1
            return self.emit(handler, text="JOURNAL_SEARCH_FOUND")
        if "JOURNAL_READ_FIRST" in objective or "JOURNAL_REVOKED_READ" in objective:
            self.agent_requests.append(body)
            require(self.last_receipt is not None, "No previous exact receipt ID/hash")
            if not tools:
                return self.emit(handler, tool="history_read", arguments={"id": self.last_receipt["id"], "hash": self.last_receipt["hash"], "offset": 0, "limit": 4096})
            if "JOURNAL_REVOKED_READ" in objective:
                require("revoked" in tools[-1]["content"].lower(), "Revoked old raw data became model context")
            else:
                result = json.loads(tools[-1]["content"])
                require("JOURNAL_BULK_MARKER_0000" in result["text"], "Original archived receipt text lost")
                require(result["hash"] == self.last_receipt["hash"] and result["truncated"], "Read did not bind hash/range")
            self.retrieval_checks += 1
            return self.emit(handler, text="JOURNAL_RECEIPT_CHECKED")
        return super().reply(handler, body)


def raw_messages(path):
    """Fixture audit helper: reconstruct all originals from authority, not hot checkpoint.

    This helper is intentionally outside the product and can load all fixture
    history. It also lets older acceptance assertions preserve their full-prefix
    checks when the checkpoint becomes a bounded projection.
    """
    directory = path.with_suffix(".journal")
    if not (directory / "head.json").exists():
        return json.loads(path.read_text(encoding="utf-8"))["messages"]
    head = json.loads((directory / "head.json").read_text(encoding="utf-8"))["head"]
    messages = []
    for number, segment in enumerate(head["segments"]):
        data = (directory / f"segment-{number:08}.jsonl").read_bytes()
        require(len(data) == segment["bytes"], "Fixture found unconfirmed raw tail")
        for line in data.splitlines():
            row = json.loads(line)
            require(row["index"] == len(messages), "Journal raw index gap")
            messages.append(row["message"])
    require(len(messages) == head["raw_count"], "Journal lost original messages")
    return messages


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--binary", type=Path, required=True)
    parser.add_argument("--hub-binary", type=Path, required=True)
    parser.add_argument("--output", type=Path)
    parser.add_argument("--iterations", type=int, default=180)
    parser.add_argument("--diagnostic-catalog-iterations", type=int, default=0,
                        help="1..60 actual Node runs for transport diagnostics; never functional acceptance")
    parser.add_argument("--max-seconds-total", type=int, default=900)
    args = parser.parse_args()
    require(171 <= args.iterations <= 220, "Use 171..220 iterations to cross both former history limits")
    require(0 <= args.diagnostic_catalog_iterations <= 60, "Diagnostic catalog trial must be 1..60 runs")
    if args.diagnostic_catalog_iterations:
        require(args.max_seconds_total <= 120, "Diagnostic catalog trial is bounded to 120 seconds")
    require(1 <= args.max_seconds_total <= 1200, "Watchdog must be 1..1200 seconds")
    require(args.binary.is_file() and args.hub_binary.is_file(), "Actual binaries missing")
    root = (args.output or Path(tempfile.mkdtemp(prefix="handwave-journal-"))).resolve()
    root.mkdir(parents=True, exist_ok=True)
    require(not (root / "result.json").exists(), "Use a fresh evidence directory")
    # Reuse the established real-Hub host and change only this controlled provider.
    context.ContextFixture = JournalFixture
    host = context.Host(args, root / "host")
    passed = []
    started = time.monotonic()
    result = {"scope": "actual Rust Node + isolated Go Hub + deterministic provider; no production/real-model acceptance", "passed": passed}
    iterations = args.diagnostic_catalog_iterations or args.iterations
    result.update(execution_budget_seconds=args.max_seconds_total,
                  acceptance="NOT_FUNCTIONAL_DIAGNOSTIC" if args.diagnostic_catalog_iterations else "FUNCTIONAL_WITHIN_EXECUTION_BUDGET")
    result["progress"] = {"phase": "starting", "bulk_completed": 0, "bulk_required": iterations}
    timing = {"runs": []}
    watchdog = threading.Timer(args.max_seconds_total, lambda: (stop(host.node), stop(host.hub)))
    watchdog.daemon = True
    watchdog.start()
    try:
        host.start()
        result["progress"]["phase"] = "bulk"
        peak_checkpoint_bytes = peak_hot_messages = 0
        for i in range(iterations):
            run_started = time.monotonic()
            calls_before, summaries_before = len(host.fixture.requests), len(host.fixture.summary_requests)
            terminal, _, _ = host.run(f"JOURNAL_BULK_MARKER_{i:04}", success=None)
            timing["runs"].append({"iteration": i, "elapsed_seconds": round(time.monotonic()-run_started, 6),
                "model_calls": len(host.fixture.requests)-calls_before,
                "summary_calls": len(host.fixture.summary_requests)-summaries_before,
                "code": terminal.get("code"), "error": terminal.get("err", ""),
                "active_threads": threading.active_count()})
            require(not terminal.get("err") and terminal.get("code", 0) == 0,
                    "Bulk run failed: " + str(terminal.get("err", "missing success receipt")))
            checkpoint = host.raw_path()
            data = checkpoint.read_bytes()
            hot = json.loads(data)
            peak_checkpoint_bytes = max(peak_checkpoint_bytes, len(data))
            peak_hot_messages = max(peak_hot_messages, len(hot["messages"]))
            require(len(data) < 8 * 1024 * 1024 and len(hot["messages"]) <= 512, "Hot checkpoint exceeded hard bound")
            result["progress"]["bulk_completed"] = i + 1
        if args.diagnostic_catalog_iterations:
            result.update(status="DIAGNOSTIC_COMPLETE", functional_acceptance=False)
            result["progress"]["phase"] = "diagnostic_complete"
            return
        directory = host.raw_path().with_suffix(".journal")
        head = json.loads((directory / "head.json").read_text(encoding="utf-8"))["head"]
        total_segment_bytes = sum(s["bytes"] for s in head["segments"])
        require(head["raw_count"] > 256 and total_segment_bytes > 16 * 1024 * 1024, "Actual process did not cross both old raw limits")
        require(len(head["segments"]) > 1 and head["active_base"] > 0, "History was neither segmented nor archived")
        before = raw_messages(host.raw_path())
        context.assert_closed(before)
        first_hash = hashlib.sha256(json.dumps(before[0], sort_keys=True, separators=(",", ":"), ensure_ascii=False).encode()).hexdigest()
        passed.append("actual Native Session exceeded 256 messages/16 MiB with bounded hot checkpoint and multiple segments")
        result["progress"]["phase"] = "restart_and_retrieval"
        host.restart()
        host.run("JOURNAL_SEARCH_FIRST")
        host.run("JOURNAL_READ_FIRST")
        after = raw_messages(host.raw_path())
        require(after[:len(before)] == before, "Restart changed an original archived prefix")
        passed.append("process restart retained exact raw prefix and model retrieved hash-bound bounded receipt")
        result["progress"]["phase"] = "revocation"
        reference = host.state_json()["references"][0]["id"]
        host.run("/reference revoke " + reference)
        host.run("JOURNAL_REVOKED_READ")
        require(host.fixture.retrieval_checks == 3, "Not all real model tool paths were exercised")
        passed.append("revocation watermark prevented model retrieval of old raw originals")
        result["progress"]["phase"] = "torn_tail"
        host.controller.sock.close()
        host.controller = None
        stop(host.node)
        segment = directory / f"segment-{len(json.loads((directory / 'head.json').read_text(encoding='utf-8'))['head']['segments'])-1:08}.jsonl"
        with segment.open("ab") as stream:
            stream.write(b'{"unconfirmed_tail":')
            stream.flush()
        preserved = segment.read_bytes()
        calls = len(host.fixture.requests)
        host.start_node()
        host.run("JOURNAL_MUST_NOT_REPLAY_AFTER_TORN_TAIL", success=False)
        require(len(host.fixture.requests) == calls, "Fenced history still issued a model request")
        require(segment.read_bytes() == preserved, "Unknown-effect tail was silently deleted")
        passed.append("actual restarted process fenced and retained unconfirmed tail without model call/replay")
        result.update(status="PASS", raw_count=head["raw_count"], total_segment_bytes=total_segment_bytes,
                      peak_checkpoint_bytes=peak_checkpoint_bytes, peak_hot_messages=peak_hot_messages,
                      original_first_message_canonical_sha256=first_hash)
        result["progress"]["phase"] = "complete"
    except Exception as error:
        result.update(status="FAIL", error_type=type(error).__name__, error=str(error))
        raise
    finally:
        watchdog.cancel()
        # Keep partial scale observations when the watchdog ends the trial. They
        # remain progress, never passed acceptance for the unexecuted phases.
        result["journal_observations"] = []
        for path in root.rglob("head.json"):
            try:
                observed = json.loads(path.read_text(encoding="utf-8"))["head"]
                result["journal_observations"].append({"path": str(path), "raw_count": observed["raw_count"],
                    "active_base": observed["active_base"], "segments": len(observed["segments"]),
                    "segment_bytes": sum(s["bytes"] for s in observed["segments"]),
                    "pending_fence": observed["metadata"].get("pending_run") is not None})
            except (OSError, ValueError, KeyError) as error:
                result["journal_observations"].append({"path": str(path), "error": str(error)})
        timing["http"] = host.fixture.http_timing
        timing["phase_at_exit"] = dict(result["progress"])
        (root / "timing.json").write_text(json.dumps(timing, ensure_ascii=False, indent=2), encoding="utf-8")
        host.close()
        result.update(binary_sha256=context.digest(args.binary), hub_sha256=context.digest(args.hub_binary),
                      fixture_sha256=context.digest(__file__), elapsed_seconds=round(time.monotonic()-started, 3))
        (root / "result.json").write_text(json.dumps(result, ensure_ascii=False, indent=2), encoding="utf-8")
        print(json.dumps(result, ensure_ascii=False, indent=2))


if __name__ == "__main__":
    main()
