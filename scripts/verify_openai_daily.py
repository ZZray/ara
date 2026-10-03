"""Run OpenAI daily-use modules, with one optional final backend gate.

Source-backed modules exercise configuration, private account lifecycle, Codex
SSE and the actual CLI. This runner never reads local credentials or performs
account login. Live model trials are separate bounded acceptance receipts.
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
import tempfile
import time

from verify_backend import windows_test_env

ROOT = Path(__file__).resolve().parents[1]
DEFAULT_OUTPUT = Path(tempfile.gettempdir()) / "ara-openai-daily"
MODULES = {
    "config": ("ara-cli", "daily_model_config"),
    "auth": ("ara-cli", "openai_codex_auth"),
    "sse": ("ara-ai", "openai_codex_http"),
    "cli": ("ara-cli", "openai_daily_cli"),
}


def source_pins() -> dict[str, str]:
    paths = {"Cargo.toml", "Cargo.lock", "scripts/verify_openai_daily.py"}
    for arguments in (
        ("git", "ls-files", "crates/ara-cli", "crates/ara-ai"),
        ("git", "diff", "--name-only"),
        ("git", "ls-files", "--others", "--exclude-standard"),
    ):
        paths.update(subprocess.check_output(arguments, cwd=ROOT).decode("utf-8").splitlines())
    return {
        path: hashlib.sha256((ROOT / path).read_bytes()).hexdigest()
        for path in sorted(paths)
        if not path.startswith("docs/") and (ROOT / path).is_file()
    }


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--module", choices=(*MODULES, "all"), default="all")
    parser.add_argument("--output", type=Path, default=DEFAULT_OUTPUT)
    parser.add_argument("--full", action="store_true", help="Also run fmt, Clippy, all target/doc tests, inventory, deny and build")
    args = parser.parse_args()
    selected = MODULES if args.module == "all" else {args.module: MODULES[args.module]}
    env = windows_test_env() or os.environ.copy()
    stamp = datetime.datetime.now(datetime.timezone.utc).strftime("%Y%m%dT%H%M%SZ")
    output = args.output / ("module-" + stamp)
    output.mkdir(parents=True, exist_ok=False)
    before = source_pins()
    started = time.monotonic()
    steps = []

    def run(name: str, command: list[str]) -> tuple[int, bytes]:
        start = time.monotonic()
        try:
            # The Windows one-job all-target gate includes native debug links;
            # a measured final suite took 12m22s. Keep its outer wait bounded.
            result = subprocess.run(command, cwd=ROOT, env=env, capture_output=True,
                                    timeout=900 if name == "backend" else 600)
            code, stdout, stderr = result.returncode, result.stdout, result.stderr
        except subprocess.TimeoutExpired as error:
            code, stdout, stderr = 124, error.stdout or b"", error.stderr or b""
        log = output / (name + ".log")
        log.write_bytes(stdout + stderr)
        counts = [tuple(map(int, match)) for match in re.findall(
            rb"test result: \w+\. (\d+) passed; (\d+) failed; (\d+) ignored;", stdout + stderr)]
        step = {
            "name": name, "command": command, "exitCode": code,
            "seconds": round(time.monotonic() - start, 3), "log": str(log),
            "testTotals": {"passed": sum(c[0] for c in counts),
                           "failed": sum(c[1] for c in counts), "ignored": sum(c[2] for c in counts)},
        }
        steps.append(step)
        print(json.dumps({key: value for key, value in step.items() if key != "command"}), flush=True)
        if code:
            print((stdout + stderr)[-3500:].decode("utf-8", errors="replace"), flush=True)
        return code, stdout

    packages = sorted({package for package, _ in selected.values()})
    compile_command = ["cargo", "test"]
    for package in packages:
        compile_command.extend(("-p", package))
    # Only compile the selected module executables here. The optional backend
    # gate owns the complete all-target build/test once the batch is stable.
    for target in sorted({target for _, target in selected.values()}):
        if target == "@lib":
            compile_command.append("--lib")
        else:
            compile_command.extend(("--test", target))
    compile_command.extend(("--all-features", "--locked", "--no-run", "--message-format=json"))
    code, compiler = run("compile", compile_command)
    binaries = {}
    if code == 0:
        metadata = json.loads(subprocess.check_output(
            ["cargo", "metadata", "--no-deps", "--format-version=1", "--locked"], cwd=ROOT, env=env))
        package_names = {package["id"]: package["name"] for package in metadata["packages"]}
        for line in compiler.splitlines():
            message = json.loads(line)
            if message.get("reason") == "compiler-artifact" and message.get("executable") and message.get("profile", {}).get("test"):
                kind = message["target"]["kind"]
                target = "@lib" if "lib" in kind else message["target"]["name"]
                binaries[(package_names[message["package_id"]], target)] = message["executable"]
        for module, (package, target) in selected.items():
            if (package, target) not in binaries:
                code = 126
                print("Missing compiled module target: " + target, flush=True)
                break
            code, _ = run(module, [binaries[(package, target)], "--nocapture"])
            if code:
                break
    if code == 0 and args.full:
        for name, command in (
            ("backend", ["python", "-X", "utf8", "scripts/verify_backend.py"]),
            ("inventory", ["python", "-X", "utf8", "scripts/omp_inventory.py", "check"]),
            ("deny", ["cargo", "deny", "check"]),
            ("build", ["cargo", "build", "-p", "ara-cli", "--all-features", "--locked", "--bin", "ara"]),
        ):
            code, _ = run(name, command)
            if code:
                break
    after = source_pins()
    if before != after:
        code = 125
    receipt = {
        "startedAt": stamp, "module": args.module, "seconds": round(time.monotonic() - started, 3),
        "exitCode": code, "sourceUnchanged": before == after, "before": before, "after": after,
        "steps": steps, "fullGateRequested": args.full,
        "liveAccountTrial": "not run by this deterministic module runner",
    }
    path = output / "receipt.json"
    path.write_text(json.dumps(receipt, indent=2), encoding="utf-8")
    print(json.dumps({"receipt": str(path), "exitCode": code, "sourceUnchanged": before == after}), flush=True)
    return code


if __name__ == "__main__":
    raise SystemExit(main())
