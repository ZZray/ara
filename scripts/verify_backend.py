"""Run baseline Rust checks when a backend exists; not a product acceptance test."""

from __future__ import annotations

import json
import os
import shutil
import subprocess
import sys
from pathlib import Path


ROOT = Path(__file__).resolve().parents[1]


def windows_test_env() -> dict[str, str] | None:
    """Provide Git Bash only to Windows all-target tests, without changing PATH globally."""
    if sys.platform != "win32":
        return None
    existing = shutil.which("bash")
    if existing:
        print(f"NOTE: Windows backend tests use Bash at {existing}", flush=True)
        return None

    candidates: list[Path] = []
    git = shutil.which("git")
    if git:
        git_root = Path(git).resolve().parent.parent
        candidates.extend((git_root / "usr" / "bin" / "bash.exe", git_root / "bin" / "bash.exe"))
    for variable in ("ProgramFiles", "ProgramFiles(x86)"):
        if root := os.environ.get(variable):
            candidates.append(Path(root) / "Git" / "usr" / "bin" / "bash.exe")
    if local_app_data := os.environ.get("LOCALAPPDATA"):
        candidates.append(Path(local_app_data) / "Programs" / "Git" / "usr" / "bin" / "bash.exe")

    bash = next((path for path in candidates if path.is_file()), None)
    if bash is None:
        raise FileNotFoundError("Git Bash is required for Windows backend tests; install it or put bash on PATH")
    print(f"NOTE: Windows backend tests use Git Bash at {bash}", flush=True)
    return {**os.environ, "PATH": f"{bash.parent}{os.pathsep}{os.environ.get('PATH', '')}"}


def main() -> int:
    manifest = ROOT / "Cargo.toml"
    if not manifest.is_file():
        rust_files = [
            path for path in ROOT.rglob("*.rs")
            if not any(part in {".git", "target", "tmp"} for part in path.relative_to(ROOT).parts)
        ]
        if rust_files:
            print("FAIL: Rust source exists without a root Cargo.toml", file=sys.stderr)
            return 1
        print("NOT RUN: no Rust backend exists in this repository yet")
        return 0

    cargo = shutil.which("cargo")
    if cargo is None:
        print("FAIL: cargo is unavailable", file=sys.stderr)
        return 1

    # Vendored upstream crates keep upstream formatting (OMP formats them with a
    # nightly rustfmt config), so the format check covers ARA-owned packages only.
    metadata = json.loads(
        subprocess.run(
            (cargo, "metadata", "--no-deps", "--format-version", "1"),
            cwd=ROOT, check=True, capture_output=True, text=True, encoding="utf-8",
        ).stdout
    )
    vendor = ROOT / "crates" / "vendor"
    owned = sorted(
        package["name"] for package in metadata["packages"]
        if vendor not in Path(package["manifest_path"]).parents
    )
    fmt_command = (cargo, "fmt", *(arg for name in owned for arg in ("-p", name)), "--", "--check")
    commands = (
        fmt_command,
        (cargo, "clippy", "--workspace", "--all-targets", "--all-features", "--", "-D", "warnings"),
        (cargo, "test", "--workspace", "--all-targets", "--all-features"),
        (cargo, "test", "--workspace", "--doc", "--all-features"),
    )
    for command in commands:
        print("RUN:", " ".join(command), flush=True)
        all_target_tests = command[1] == "test" and "--all-targets" in command
        if all_target_tests:
            try:
                test_env = windows_test_env()
            except FileNotFoundError as exc:
                print(f"FAIL: {exc}", file=sys.stderr)
                return 1
        else:
            test_env = None
        result = subprocess.run(command, cwd=ROOT, check=False, env=test_env)
        if result.returncode:
            print(f"FAIL: exit {result.returncode}", file=sys.stderr)
            return result.returncode

    print("PASS: Rust formatting, Clippy, target tests, and doc tests")
    print("NOTE: host effects, failure paths, dependency policy, and real-model tasks need separate evidence")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
