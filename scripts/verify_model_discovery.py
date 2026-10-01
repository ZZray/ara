"""Run the catalog module once; optionally run the complete backend gate.

Inputs is a local JSON object mapping ARA_CATALOG_* environment names to retained
source artifacts and tools. It contains paths only, never credentials. Original
source is captured separately with the catalog_*_oracle.py/probe.py scripts.
"""
from __future__ import annotations

import argparse
import datetime
import hashlib
import json
import os
from pathlib import Path
import re
import subprocess
import time

ROOT = Path(__file__).resolve().parents[1]
TARGETS = (
    "catalog_discovery_oracle", "catalog_protobuf_oracle",
    "catalog_discovery_transport", "catalog_extra_ca_filesystem",
    "catalog_extra_ca_oracle", "catalog_extra_ca_tls",
    "catalog_fetch_tls", "catalog_cursor_ca_tls",
)


def source_pins() -> dict[str, str]:
    paths = {"Cargo.toml", "Cargo.lock", "crates/ara-cli/Cargo.toml"}
    for command in (("git", "diff", "--name-only"), ("git", "ls-files", "--others", "--exclude-standard")):
        paths.update(subprocess.check_output(command, cwd=ROOT).decode().splitlines())
    return {p: hashlib.sha256((ROOT / p).read_bytes()).hexdigest() for p in sorted(paths)
            if not p.startswith("docs/") and (ROOT / p).is_file()}


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--inputs", required=True, type=Path)
    parser.add_argument("--output", required=True, type=Path)
    parser.add_argument("--full", action="store_true", help="Also run fmt/Clippy/all tests/docs, inventory, deny and binary build")
    args = parser.parse_args()
    inputs = json.loads(args.inputs.read_text(encoding="utf-8-sig"))
    if not isinstance(inputs, dict) or not all(isinstance(k, str) and k.startswith("ARA_CATALOG_") and isinstance(v, str) for k, v in inputs.items()):
        parser.error("inputs must map ARA_CATALOG_* names to paths")
    env = {**os.environ, **inputs}
    stamp = datetime.datetime.now(datetime.timezone.utc).strftime("%Y%m%dT%H%M%SZ")
    output = args.output / ("module-" + stamp)
    output.mkdir(parents=True, exist_ok=False)
    before = source_pins()
    started = time.monotonic()
    steps = []
    artifacts = {k: {"path": v, "sha256": hashlib.sha256(Path(v).read_bytes()).hexdigest()}
                 for k, v in inputs.items() if "SOURCE_RECEIPT" in k or k.endswith("_ORACLE")}

    def run(name: str, command: list[str]) -> tuple[int, bytes]:
        start = time.monotonic()
        try:
            result = subprocess.run(command, cwd=ROOT, env=env, capture_output=True, timeout=600)
            code, stdout, stderr = result.returncode, result.stdout, result.stderr
        except subprocess.TimeoutExpired as error:
            code, stdout, stderr = 124, error.stdout or b"", error.stderr or b""
        log = output / (name + ".log")
        log.write_bytes(stdout + stderr)
        totals = [tuple(map(int, m)) for m in re.findall(rb"test result: \w+\. (\d+) passed; (\d+) failed; (\d+) ignored;", stdout + stderr)]
        step = {"name": name, "command": command, "exitCode": code, "seconds": round(time.monotonic() - start, 3),
                "log": str(log), "testTotals": {"passed": sum(t[0] for t in totals), "failed": sum(t[1] for t in totals), "ignored": sum(t[2] for t in totals)}}
        steps.append(step)
        print(json.dumps({k: v for k, v in step.items() if k != "command"}), flush=True)
        if code:
            print((stdout + stderr)[-2500:].decode("utf-8", errors="replace"), flush=True)
        return code, stdout

    code, compiler = run("compile", ["cargo", "test", "-p", "ara-cli", "--all-features", "--locked", "--no-run", "--message-format=json"])
    binaries = {}
    if code == 0:
        for line in compiler.splitlines():
            message = json.loads(line)
            if message.get("reason") == "compiler-artifact" and message.get("executable") and "test" in message["target"]["kind"]:
                binaries[message["target"]["name"]] = message["executable"]
        for target in TARGETS:
            if target not in binaries:
                raise RuntimeError("missing compiled module target: " + target)
            code, _ = run(target, [binaries[target], "--include-ignored", "--nocapture"])
            if code:
                break
    if code == 0 and args.full:
        for name, command in (
            ("backend", ["python", "scripts/verify_backend.py"]),
            ("inventory", ["python", "scripts/omp_inventory.py", "check"]),
            ("deny", ["cargo", "deny", "check"]),
            ("build", ["cargo", "build", "-p", "ara-cli", "--all-features", "--locked", "--bin", "ara"]),
        ):
            code, _ = run(name, command)
            if code:
                break
    after = source_pins()
    if before != after:
        code = 125
    receipt = {"startedAt": stamp, "seconds": round(time.monotonic() - started, 3), "exitCode": code,
               "sourceUnchanged": before == after, "before": before, "after": after,
               "inputArtifacts": artifacts, "steps": steps, "fullGateRequested": args.full}
    path = output / "receipt.json"
    path.write_text(json.dumps(receipt, indent=2), encoding="utf-8")
    print(json.dumps({"receipt": str(path), "exitCode": code, "sourceUnchanged": before == after}), flush=True)
    return code


if __name__ == "__main__":
    raise SystemExit(main())
