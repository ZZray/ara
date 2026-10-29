"""Map OMP inventory behaviors of vendored crates to executed Rust tests.

Runs the vendored crates' own test suites (`cargo test -p <crate>`), parses
each test result, and matches it to the inventory behavior with the same
upstream file and test function name. Writes a TSV record and exits non-zero
when any behavior is missing or failed. Tests upstream marks `#[ignore]` are
reported as `ignored-upstream` (upstream runs them on demand). Platform-only
upstream tests are reported as not applicable on Windows, never as passed.
The full Cargo output is saved next to the TSV; Cargo failure is propagated.

    python scripts/vendored_behaviors.py [--out docs/evidence/vendored-behaviors.tsv] [--corpus OMP_CHECKOUT]

`--corpus` points ara-ast's (upstream pi-ast) repository-corpus sweeps at an OMP checkout
(`ARA_AST_CORPUS_ROOT`); without it the sample sweep skips itself in this tree.
"""

from __future__ import annotations

import argparse
import csv
import os
import re
import subprocess
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
BEHAVIORS = ROOT / "docs/upstream/inventory/behaviors.tsv"
# Upstream crate directory -> vendored package name.
CRATES = {"crates/pi-diff": "ara-diff", "crates/pi-ast": "ara-ast", "crates/pi-edit": "ara-edit"}

# Tests that return early (and pass) unless an OMP corpus is supplied.
CORPUS_SWEEPS = {("crates/pi-ast", "pruned_walk_matches_unpruned_on_repo_corpus_sample")}

# Fixed OMP replace_parity.rs marks this test #[cfg(unix)].
UNIX_TESTS = {("crates/pi-edit/tests/replace_parity.rs", "canonical_snapshot_keys_fuse_symlink_equivalent_paths")}

# Windows prints `src\lib.rs` and `...\deps\name-hash.exe`.
RUNNING = re.compile(r"^\s*Running (?:unittests )?(\S+) \((?:\S*[\\/])?deps[\\/]([A-Za-z0-9_]+)-[0-9a-f]+(?:\.exe)?\)")
RESULT = re.compile(r"^test (\S+) \.\.\. (ok|FAILED|ignored)")


def module_file(crate_dir: str, module_path: list[str]) -> list[str]:
    """Candidate source files for a unit test's module path."""
    if not module_path:
        return [f"{crate_dir}/src/lib.rs"]
    base = "/".join(module_path)
    return [f"{crate_dir}/src/{base}.rs", f"{crate_dir}/src/{base}/mod.rs"]


def run_tests(corpus: str | None, log: Path) -> tuple[dict[tuple[str, str], str], int]:
    """Map the same Cargo run and retain its actual exit status and output."""
    packages = [arg for name in CRATES.values() for arg in ("-p", name)]
    env = dict(os.environ)
    if corpus:
        env["ARA_AST_CORPUS_ROOT"] = corpus
    # stderr carries the "Running <target>" headers; merge to keep the order.
    proc = subprocess.run(
        ["cargo", "test", "--locked", *packages, "--no-fail-fast"],
        cwd=ROOT, stdout=subprocess.PIPE, stderr=subprocess.STDOUT, text=True,
        encoding="utf-8", errors="replace", check=False, env=env,
    )
    log.write_text(proc.stdout, encoding="utf-8")
    print(f"Cargo exit {proc.returncode}; full output: {log}", flush=True)
    results: dict[tuple[str, str], str] = {}
    current: tuple[str, str] | None = None
    by_binary = {name.replace("-", "_"): d for d, name in CRATES.items()}
    for line in proc.stdout.splitlines():
        running = RUNNING.match(line)
        if running:
            target, binary = running.groups()
            target = target.replace("\\", "/")
            # Integration test binaries are named after the test file.
            crate_dir = by_binary.get(binary)
            if crate_dir is None:
                crate_dir = next(
                    (d for d, name in CRATES.items() if (ROOT / "crates/vendor" / name / target).exists()), None
                )
            current = (crate_dir, target) if crate_dir else None
            continue
        result = RESULT.match(line)
        if not result or current is None:
            continue
        crate_dir, target = current
        name, status = result.groups()
        parts = name.split("::")
        test = parts[-1]
        if status == "ignored":
            status = "ignored-upstream"
        if status == "ok" and not corpus and (crate_dir, test) in CORPUS_SWEEPS:
            status = "ok-no-corpus"
        if target.startswith("src/"):
            module = [p for p in parts[:-1] if p != "tests"]
            for candidate in module_file(crate_dir, module):
                results[(candidate, test)] = status
        else:
            results[(f"{crate_dir}/{target}", test)] = status
    return results, proc.returncode


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("--out", default="docs/evidence/vendored-behaviors.tsv")
    parser.add_argument("--corpus", help="OMP checkout for ara-ast corpus sweeps")
    args = parser.parse_args()
    out = ROOT / args.out
    results, cargo_exit = run_tests(args.corpus, out.with_suffix(".cargo.log"))
    rows = []
    missing = failed = 0
    with BEHAVIORS.open(encoding="utf-8") as handle:
        for line in handle:
            if line.startswith("#") or line.startswith("id\t"):
                continue
            bid, surface, path, _line, kind, title = line.rstrip("\n").split("\t", 5)
            if kind != "rs-test" or not any(path.startswith(d + "/") for d in CRATES):
                continue
            status = results.get((path, title), "missing")
            if status == "missing" and os.name == "nt" and (path, title) in UNIX_TESTS:
                status = "not-applicable-windows"
            missing += status == "missing"
            failed += status == "FAILED"
            rows.append((bid, surface, path, title, status))
    with out.open("w", encoding="utf-8", newline="") as handle:
        writer = csv.writer(handle, delimiter="\t", lineterminator="\n")
        writer.writerow(("id", "surface", "upstream_path", "test", "status"))
        writer.writerows(rows)
    counts: dict[str, int] = {}
    for row in rows:
        counts[row[4]] = counts.get(row[4], 0) + 1
    print(f"{len(rows)} vendored behaviors: " + ", ".join(f"{k} {v}" for k, v in sorted(counts.items())))
    if cargo_exit:
        return cargo_exit if cargo_exit > 0 else 1
    return 1 if missing or failed else 0


if __name__ == "__main__":
    raise SystemExit(main())
