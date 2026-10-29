"""Actual Rust Host getter/page consumption of fixed provider summary DTOs.

Original compactedEntries and an optional-metadata variant use zero model calls.
Full/collapsed Rust contracts have separate receipts; this does not accept TUI.
"""
from __future__ import annotations
import argparse
import datetime
import json
from pathlib import Path
import time
import verify_snapcompact_host as host


def source_pins():
    pins = host.source_pins()
    path = Path(__file__).resolve()
    pins[path.relative_to(host.shared.ROOT).as_posix()] = host.shared.digest(path)
    return pins


def run(trial):
    observations = []
    for name in ("original-three-mode-provider", "optional-metadata"):
        stamp = "2026-07-09T00:00:00.000Z"
        message = lambda identity, parent, text: {
            "type": "message", "id": identity, "parentId": parent, "timestamp": stamp,
            "message": {"role": "user", "content": [{"type": "text", "text": text}], "timestamp": 2 if identity == "m2" else 1}}
        summary = {"type": "compaction", "id": "c1", "parentId": "m1", "timestamp": stamp,
                   "summary": "summary", "firstKeptEntryId": "m1", "tokensBefore": 123,
                   "preserveData": {"snapcompact": {"frames": [{"data": "base64-frame", "mimeType": "image/png",
                   "cols": 10, "rows": 10, "chars": 100}], "totalChars": 100, "truncatedChars": 0,
                   "textHead": "head", "textTail": "tail"}}}
        if name == "optional-metadata":
            summary.update(shortSummary="brief", tokensAfter=45, warning="ACTIVE warning", method="snapcompact")
        entries = [message("m1", None, "before compaction"), summary, message("m2", "c1", "after compaction")]
        directory, work, session = trial.setup(name, entries)
        original = session.read_bytes()
        with host.remote.RemoteUpstream(directory, generic=True) as fake:
            models = host.configure(directory, fake)
            process = host.opened(trial, directory, work, session, models)
            try:
                all_messages = process.command("all", "get_messages")["messages"]
                host.shared.require([item["role"] for item in all_messages] == ["compactionSummary", "user", "user"],
                                    "provider getter ordering changed to display chronology")
                expected_users = [entries[0]["message"], entries[2]["message"]]
                host.shared.require(all_messages[1:] == expected_users, "whole original user body/timestamp differs")
                dto = all_messages[0]
                host.shared.require([item["data"] for item in dto["images"]] == ["base64-frame"], "original image observation differs")
                host.shared.require([item["type"] for item in dto["blocks"]] == ["text", "image", "text"], "original ordered blocks differ")
                host.shared.require("compactionFrame" not in dto["images"][0], "model-private marker leaked into DTO")
                host.shared.require(dto["tokensBefore"] == 123 and dto["timestamp"] == 1783555200000,
                                    "fixed summary required metadata differs")
                if name == "optional-metadata":
                    host.shared.require(all(dto.get(key) == summary[key] for key in ("shortSummary", "tokensAfter", "warning", "method")),
                                        "fixed optional metadata omitted")
                else:
                    host.shared.require(all(key not in dto for key in ("shortSummary", "tokensAfter", "warning", "method")),
                                        "fixed absent optional metadata fabricated")
                first = process.command("first", "get_messages_page", limit=1)
                host.shared.require(first["messages"] == [dto] and first["totalMessages"] == 3, "first page differs from getter snapshot")
                tail = process.command("tail", "get_messages_page", limit=2, cursor=first["nextCursor"])
                host.shared.require(tail["messages"] == all_messages[1:] and "nextCursor" not in tail, "page tail differs")
                host.no_archive_payload(process.seen)
                host.shared.require(not fake.requests, "read-only getter made a model request")
                host.shared.require(not fake.errors, "unexpected loopback provider failure")
            finally:
                host.remote.finished(process, directory, session)
        host.shared.dump(directory / "fake-requests.json", fake.requests)
        host.shared.dump(directory / "fake-errors.json", fake.errors)
        host.shared.require(session.read_bytes() == original, "public projection or paging wrote Session")
        observations.append({"case": name, "providerSummaryFirst": True, "images": 1,
                             "orderedBlocks": ["text", "image", "text"], "pageMessages": 3, "modelCalls": 0,
                             "requestsReceipt": str(directory / "fake-requests.json"),
                             "errorsReceipt": str(directory / "fake-errors.json")})
    return observations


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--binary", type=Path, required=True)
    parser.add_argument("--output", type=Path, required=True)
    args = parser.parse_args()
    output = args.output / ("run-" + datetime.datetime.now(datetime.timezone.utc).strftime("%Y%m%dT%H%M%SZ"))
    output.mkdir(parents=True)
    before, binary_pin, started = source_pins(), host.shared.digest(args.binary), time.monotonic()
    observations, error = [], None
    try:
        observations = run(host.shared.Trial(args.binary, output, 30))
    except Exception as failure:
        error = str(failure)
    after = source_pins()
    code = 0 if error is None and before == after and binary_pin == host.shared.digest(args.binary) else 1
    receipt = output / "receipt.json"
    host.shared.dump(receipt, {"exitCode": code, "failure": error, "observations": observations,
        "sourcePinsBefore": before, "sourcePinsAfter": after, "sourceUnchanged": before == after,
        "binarySha256": binary_pin, "seconds": round(time.monotonic() - started, 3),
        "limits": "Actual Rust Host getter/page; controlled loopback provider made zero calls. No TUI or real-image decoding acceptance."})
    print(json.dumps({"exitCode": code, "receipt": str(receipt), "failure": error}))
    return code


if __name__ == "__main__":
    raise SystemExit(main())
