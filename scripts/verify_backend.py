"""Run baseline Rust checks when a backend exists; not a product acceptance test."""

from __future__ import annotations

import json
import shutil
import subprocess
import sys
from pathlib import Path


ROOT = Path(__file__).resolve().parents[1]


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

    if shutil.which("cargo") is None:
        print("FAIL: cargo is unavailable", file=sys.stderr)
        return 1

    # Vendored upstream crates keep upstream formatting (OMP formats them with a
    # nightly rustfmt config), so the format check covers ARA-owned packages only.
    metadata = json.loads(
        subprocess.run(
            ("cargo", "metadata", "--no-deps", "--format-version", "1"),
            cwd=ROOT, check=True, capture_output=True, text=True,
        ).stdout
    )
    vendor = ROOT / "crates" / "vendor"
    owned = sorted(
        package["name"] for package in metadata["packages"]
        if vendor not in Path(package["manifest_path"]).parents
    )
    fmt_command = ("cargo", "fmt", *(arg for name in owned for arg in ("-p", name)), "--", "--check")
    commands = (
        fmt_command,
        ("cargo", "clippy", "--workspace", "--all-targets", "--all-features", "--", "-D", "warnings"),
        ("cargo", "test", "--workspace", "--all-targets", "--all-features"),
        ("cargo", "test", "--workspace", "--doc", "--all-features"),
    )
    for command in commands:
        print("RUN:", " ".join(command), flush=True)
        result = subprocess.run(command, cwd=ROOT, check=False)
        if result.returncode:
            print(f"FAIL: exit {result.returncode}", file=sys.stderr)
            return result.returncode

    print("PASS: Rust formatting, Clippy, target tests, and doc tests")
    print("NOTE: host effects, failure paths, dependency policy, and real-model tasks need separate evidence")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
