"""Grouped fixed OMP snapcompact flows through a supplied actual Rust Host.

Reuse loopback transport and bounded process fixtures. This runner does not
build, read private configuration, or call a live provider. Native module
families own deterministic publication faults; these flows inspect the actual
RPC/REPL, journal, blob files, replay, events and recovery continuation.
"""
from __future__ import annotations

import argparse
import datetime
import json
from pathlib import Path
import subprocess
import time

import verify_handoff_host as handoff
import verify_local_reduction_host as shared
import verify_remote_host as remote
import verify_snapcompact as modules

PROVIDER = "custom-snap-fixture"


def source_pins():
    pins = modules.source_pins()
    for module in (shared, remote, handoff):
        path = Path(module.__file__)
        pins[path.relative_to(shared.ROOT).as_posix()] = shared.digest(path)
    pins[Path(__file__).relative_to(shared.ROOT).as_posix()] = shared.digest(Path(__file__))
    return pins


def configure(directory, fake, *, vision=True, window=200000):
    agent = directory / "home/agent"
    agent.mkdir(parents=True, exist_ok=True)
    (agent / "config.yml").write_text(
        "compaction:\n  methodOrder: [snapcompact]\n  supersedeReads: false\n"
        "  dropUseless: false\n  autoContinue: false\n", encoding="utf-8")
    model = {"id": shared.MODEL, "input": ["text", "image"] if vision else ["text"]}
    if window is not None:
        model["contextWindow"] = window
    shared.dump(agent / "models.json", {"providers": {PROVIDER: {
        "api": "openai-completions", "baseUrl": fake.base_url, "apiKey": "ARA_API_KEY",
        "models": [model]}}})
    return agent / "models.json"


def command(trial, directory, work, session, models, *, threshold=0, repl=False):
    args = remote.command(trial, directory, work, session, models, provider=PROVIDER, threshold=threshold)
    if repl:
        index = args.index("--mode")
        args[index:index + 2] = ["--repl"]
    return args


def opened(trial, directory, work, session, models, *, threshold=0):
    process = shared.RpcProcess(command(trial, directory, work, session, models, threshold=threshold),
                                trial.environment(models.parent.parent.parent), directory, 30)
    process.until(lambda frame: frame.get("type") == "ready")
    return process


def compactions(session):
    return [entry for entry in shared.journal(session).values() if entry["type"] == "compaction"]


def archive(entry):
    return entry.get("preserveData", {}).get("snapcompact")


def no_archive_payload(frames):
    for frame in frames:
        encoded = json.dumps(frame)
        shared.require('"frames"' not in encoded and '"snapcompact": {' not in encoded,
                       "durable archive escaped into an RPC/event payload")


def normal_seed():
    entries = handoff.seed()
    entries[1]["message"]["content"] = [
        {"type": "text", "text": "ARCHIVE_OLD_SOURCE " + " old history" * 16000}, shared.IMAGE]
    entries[-2]["message"]["content"] = remote.retained("Keep current work.")
    entries[-1]["message"]["usage"] = {}
    return entries


def stale_seed(*, frames=16, kept_text="Keep this actual User source."):
    entries = [shared.row("message", "old-user", None,
                          message={"role": "user", "content": "Old task.", "timestamp": 1}),
               shared.row("message", "old-answer", "old-user",
                          message=shared.assistant([{"type": "text", "text": "Old task settled."}])),
               shared.row("message", "kept-user", "old-answer",
                          message={"role": "user", "content": kept_text, "timestamp": 1})]
    entries.append(shared.row("compaction", "stale-archive", "kept-user",
        summary="Archived history onto stale snapcompact frames.", shortSummary="stale archive",
        firstKeptEntryId="kept-user", tokensBefore=100000,
        preserveData={"extensionState": "keep-me", "snapcompact": {
            "frames": [{**shared.IMAGE, "cols": 4, "rows": 2, "chars": 8} for _ in range(frames)],
            "text": "HEAD sentinel. " + "Archived history line. " * 200 + "TAIL sentinel.",
            "totalChars": 4600, "truncatedChars": 0}}))
    return entries


def manual_reopen_repl(trial):
    directory, work, session = trial.setup("manual", normal_seed())
    before = shared.journal(session)
    with remote.RemoteUpstream(directory, generic=True) as fake:
        models = configure(directory, fake)
        process = opened(trial, directory, work, session, models)
        try:
            result = process.command("compact", "compact")
            shared.require(result.get("method") == "snapcompact", "manual compact did not select snapcompact")
            shared.require(not fake.requests, "local archive called a model")
            no_archive_payload(process.seen)
        finally:
            remote.finished(process, directory, session)
        after = shared.journal(session)
        for identity, entry in before.items():
            shared.require(after[identity] == entry, "snapcompact mutated an original raw entry")
        written = compactions(session)[-1]
        shared.require(written["firstKeptEntryId"] == "kept-user", "native kept cut changed")
        frames = archive(written)["frames"]
        shared.require(frames, "long history did not produce PNG frames")
        for frame in frames:
            reference = frame["data"]
            shared.require(reference.startswith("blob:sha256:"), "frame was not externalized")
            blob = directory / "home/agent/blobs" / reference.rsplit(":", 1)[1]
            shared.require(blob.read_bytes().startswith(b"\x89PNG\r\n\x1a\n"), "renderer did not write actual PNG")
        reopened = directory / "reopened"
        reopened.mkdir()
        process = opened(trial, reopened, work, session, models)
        try:
            remote.prompt(process, "continue", "Continue the retained work.", fake, 1)
        finally:
            remote.finished(process, reopened, session)
        wire = fake.requests[-1]["body"]["messages"]
        images = [block for message in wire if isinstance(message.get("content"), list)
                  for block in message["content"] if block.get("type") == "image_url"]
        shared.require(len(images) == len(frames), "reopened archive image count differs from journal")
        shared.require(all(block["image_url"].get("detail") == "original" for block in images),
                       "fixed archive image detail was not forwarded")
        shared.require("Keep current work." in json.dumps(wire), "same-Session retained source was lost")
        original_text = before["source-user"]["message"]["content"][0]["text"]
        shared.require(original_text not in json.dumps(wire), "full original old source resurrected")
        repl_dir, repl_work, repl_session = trial.setup("repl", normal_seed())
        repl_models = configure(repl_dir, fake)
        proc = subprocess.run(command(trial, repl_dir, repl_work, repl_session, repl_models, repl=True),
            input=b"/compact snapcompact\n/quit\n", env=trial.environment(repl_dir), cwd=repl_work,
            capture_output=True, timeout=30)
        (repl_dir / "stdout.log").write_bytes(proc.stdout)
        (repl_dir / "stderr.log").write_bytes(proc.stderr)
        shared.require(proc.returncode == 0 and compactions(repl_session), "REPL explicit mode did not publish")
        shared.require(len(fake.requests) == 1, "REPL local archive unexpectedly called upstream")
    return {"family": "manual/reopen/REPL", "frames": len(frames), "modelCalls": 1, "passed": True}


def automatic_and_stale_rescue(trial):
    results = []
    for name, entries in (("automatic", normal_seed()), ("stale", stale_seed())):
        directory, work, session = trial.setup(name, entries)
        with remote.RemoteUpstream(directory, generic=True) as fake:
            models = configure(directory, fake)
            process = opened(trial, directory, work, session, models, threshold=40000)
            try:
                remote.prompt(process, "continue", "Continue the actual task.", fake, 1)
                no_archive_payload(process.seen)
                shared.require(any(frame.get("type") == "auto_compaction_end" for frame in process.seen),
                               "threshold did not execute maintenance")
                shared.require(not any(frame.get("level") == "warning" for frame in process.seen),
                               "successful archive/recovery left a no-progress warning")
            finally:
                remote.finished(process, directory, session)
            written = compactions(session)
            shared.require(written and archive(written[-1]), "automatic maintenance did not publish archive")
            if name == "stale":
                shared.require(len(written) == 2, "stale rescue did not append exactly one replacement")
                shared.require(len(archive(written[0])["frames"]) == 16
                               and archive(written[0])["text"] == archive(entries[-1])["text"],
                               "rescue damaged old archive history")
                shared.require(len(archive(written[-1])["frames"]) < 16, "stale archive did not shrink")
                shared.require(written[-1]["firstKeptEntryId"] == "kept-user", "rescue changed kept source")
                shared.require(written[-1]["preserveData"]["extensionState"] == "keep-me", "metadata lost")
            results.append({"case": name, "archives": len(written), "frames": len(archive(written[-1])["frames"])})
    return {"family": "threshold/just-written/stale rescue", "cases": results, "passed": True}


def failed_assistant_rescue(trial):
    directory, work, session = trial.setup("failed-assistant", stale_seed())
    with shared.FakeUpstream() as fake:
        failed = shared.text_events("Incomplete output to recover.")
        failed[1]["choices"][0]["finish_reason"] = "length"
        fake.responses = [failed, shared.text_events("Continued work.")]
        models = configure(directory, fake)
        process = opened(trial, directory, work, session, models)
        try:
            remote.prompt(process, "continue", "Continue the task after the retained archive.", fake, 2)
            no_archive_payload(process.seen)
        finally:
            remote.finished(process, directory, session)
        written = compactions(session)
        shared.require(len(written) == 2 and len(archive(written[-1])["frames"]) < 16,
                       "failed-assistant recovery did not publish a smaller archive once")
        shared.require(any(entry.get("message", {}).get("content") == [{"type": "text", "text": "Continued work."}]
                           for entry in shared.journal(session).values()), "recovery did not persist continuation")
        shared.dump(directory / "requests.json", fake.requests)
        shared.dump(directory / "upstream-errors.json", fake.errors)
        shared.require(not fake.errors, "failed-assistant fixture exceeded its planned requests")
    return {"family": "failed-assistant archive ownership/retry", "modelCalls": 2, "passed": True}


def blocked_dead_ends(trial):
    cases = []
    for name, count, vision, threshold, kept in (
        ("minimum", 1, True, 3000, "Keep actual source."),
        ("text-only", 16, False, 40000, "Keep actual source."),
        ("kept-before", 16, True, 40000, "Retained actual source. " * 16000),
    ):
        directory, work, session = trial.setup(name, stale_seed(frames=count, kept_text=kept))
        before = shared.journal(session)
        with shared.FakeUpstream() as fake:
            models = configure(directory, fake, vision=vision)
            process = opened(trial, directory, work, session, models, threshold=threshold)
            try:
                process.command("continue", "prompt", message="Continue the task.")
                end = process.until(lambda frame: frame.get("type") == "auto_compaction_end")
                notice = process.until(lambda frame: frame.get("type") == "notice" and frame.get("level") == "warning"
                    and "Compaction freed too little context to make progress" in frame.get("message", ""))
                shared.require("errorMessage" in end and not end.get("willRetry"), "dead end silently continued")
                notices = [frame for frame in process.seen if "Compaction freed too little context to make progress"
                           in frame.get("message", "")]
                shared.require(len(notices) == 1 and notice["source"] == "compaction", "dead end warning was duplicated")
            finally:
                remote.finished(process, directory, session)
            written = compactions(session)
            shared.require(len(written) == 1 and "warning" not in written[0],
                           "unpublished rescue added a barrier or mutated old warning state")
            shared.require(len(archive(written[0])["frames"]) == count and not fake.requests,
                           "blocked maintenance rendered/called upstream/continued")
            shared.preserved(before, shared.journal(session))
            cases.append({"case": name, "notices": 1, "modelCalls": 0})
    return {"family": "minimum/text-only/no-frame-budget warning and stop", "cases": cases, "passed": True}


def insufficient_rebuild(trial):
    directory, work, session = trial.setup("insufficient", stale_seed())
    with shared.FakeUpstream() as fake:
        models = configure(directory, fake, window=None)
        process = opened(trial, directory, work, session, models, threshold=1000)
        try:
            process.command("continue", "prompt", message="Continue the task.")
            end = process.until(lambda frame: frame.get("type") == "auto_compaction_end")
            warning = process.until(lambda frame: frame.get("type") == "notice" and frame.get("level") == "warning"
                and "Compaction freed too little context to make progress" in frame.get("message", ""))
            shared.require("errorMessage" in end and end.get("result", {}).get("method") == "snapcompact",
                           "published insufficient rescue missing from lifecycle result")
            shared.require(end["result"]["preserveData"] == {"extensionState": "keep-me"}, "opaque metadata lost from result")
            shared.require("reduce archived image frames (" in warning["message"], "image remedy missing")
            no_archive_payload(process.seen)
        finally:
            remote.finished(process, directory, session)
        written = compactions(session)
        shared.require(len(written) == 2 and written[-1]["warning"] == warning["message"],
                       "insufficient rescue did not append once/stamp the actual active archive")
        shared.require(not fake.requests, "insufficient rescue resumed the model")
    return {"family": "insufficient rebuild/lower tiers/result/badge/warning", "modelCalls": 0, "passed": True}


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--binary", type=Path, required=True)
    parser.add_argument("--output", type=Path, default=Path("C:/Temp/ara-snap-batch/host"))
    parser.add_argument("--family", choices=("all", "manual", "automatic", "failed", "deadends", "insufficient"), default="all")
    args = parser.parse_args()
    output = args.output / ("run-" + datetime.datetime.now(datetime.timezone.utc).strftime("%Y%m%dT%H%M%SZ"))
    output.mkdir(parents=True)
    before, binary_pin, started = source_pins(), shared.digest(args.binary), time.monotonic()
    trial = shared.Trial(args.binary, output, 30)
    families, error = [], None
    try:
        selected = {"manual": manual_reopen_repl, "automatic": automatic_and_stale_rescue,
                    "failed": failed_assistant_rescue, "deadends": blocked_dead_ends, "insufficient": insufficient_rebuild}
        for family in selected.values() if args.family == "all" else (selected[args.family],):
            families.append(family(trial))
            print(json.dumps(families[-1]), flush=True)
    except Exception as failure:
        error = str(failure)
    after = source_pins()
    code = 0 if error is None and before == after and binary_pin == shared.digest(args.binary) else 1
    shared.dump(output / "receipt.json", {"exitCode": code, "selectedFamily": args.family, "families": families, "failure": error,
        "seconds": round(time.monotonic() - started, 3), "sourcePinsBefore": before, "sourcePinsAfter": after,
        "sourceUnchanged": before == after, "binarySha256": binary_pin,
        "limits": "Controlled actual Rust Host; live vision and publication faults have separate receipts."})
    print(json.dumps({"exitCode": code, "failure": error, "receipt": str(output / "receipt.json")}))
    return code


if __name__ == "__main__":
    raise SystemExit(main())
