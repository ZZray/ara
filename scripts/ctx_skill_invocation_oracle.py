"""Run the fixed OMP Skill invocation parser and builder without editing source.

Each invocation retains its exact Git exports and logs in a fresh directory.
Only a successful run writes oracle.json. The JSON cases can be replayed by
the Rust comparison test through ARA_CTX_INVOCATION_ORACLE.
"""

from __future__ import annotations

import argparse
import datetime as dt
import hashlib
import json
import subprocess
import sys
import uuid
from pathlib import Path


UPSTREAM_COMMIT = "596f2da7101178214aa27a753529d15e6b7ad91d"
BUN_VERSION = "1.4.0"
SOURCE_PATHS = (
    "packages/coding-agent/src/extensibility/skills.ts",
    "packages/coding-agent/src/prompts/skills/user-invocation.md",
    "packages/coding-agent/src/prompts/skills/autoload.md",
    "packages/utils/src/prompt.ts",
    "packages/utils/src/template.ts",
)


def sha256(data: bytes) -> str:
    return hashlib.sha256(data).hexdigest()


def command(args: list[str], *, timeout: int = 30) -> bytes:
    result = subprocess.run(args, capture_output=True, timeout=timeout, check=False)
    if result.returncode:
        raise RuntimeError(
            f"Command failed ({result.returncode}): {args!r}\n"
            + result.stderr.decode("utf-8", errors="replace")
        )
    return result.stdout


def git(upstream: Path, *args: str) -> bytes:
    return command(["git", "-C", str(upstream), *args])


def export_blob(upstream: Path, run_dir: Path, path: str) -> dict[str, str]:
    object_spec = f"{UPSTREAM_COMMIT}:{path}"
    blob_id = git(upstream, "rev-parse", object_spec).decode("ascii").strip()
    # Git output must remain bytes: PowerShell's text pipeline changes newlines.
    source = git(upstream, "show", object_spec)
    object_bytes = b"blob " + str(len(source)).encode("ascii") + b"\0" + source
    if hashlib.sha1(object_bytes).hexdigest() != blob_id:
        raise RuntimeError(f"Exported Git blob bytes differ: {path}")
    target = run_dir / "upstream" / path
    target.parent.mkdir(parents=True, exist_ok=True)
    target.write_bytes(source)
    if target.read_bytes() != source:
        raise RuntimeError(f"Written source bytes differ: {path}")
    return {"path": path, "gitBlob": blob_id, "sha256": sha256(source)}


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--upstream", required=True, type=Path)
    parser.add_argument("--bun", required=True, type=Path)
    parser.add_argument("--output", required=True, type=Path)
    args = parser.parse_args()
    stamp = dt.datetime.now(dt.timezone.utc).strftime("%Y%m%dT%H%M%SZ")
    run_dir = args.output.resolve() / f"run-{stamp}-{uuid.uuid4().hex[:8]}"
    run_dir.mkdir(parents=True, exist_ok=False)
    summary: dict[str, object] = {"status": "failed", "runDirectory": str(run_dir)}
    try:
        upstream = args.upstream.resolve(strict=True)
        bun = args.bun.resolve(strict=True)
        resolved_commit = git(upstream, "rev-parse", f"{UPSTREAM_COMMIT}^{{commit}}").decode("ascii").strip()
        if resolved_commit != UPSTREAM_COMMIT:
            raise RuntimeError("The fixed upstream commit is unavailable")
        bun_version = command([str(bun), "--version"]).decode("ascii").strip()
        if bun_version != BUN_VERSION:
            raise RuntimeError(f"Expected Bun {BUN_VERSION}, got {bun_version}")

        sources = [export_blob(upstream, run_dir, path) for path in SOURCE_PATHS]
        license_entry = export_blob(upstream, run_dir, "LICENSE")
        runner = Path(__file__).with_suffix(".mjs")
        runner_bytes = runner.read_bytes()
        copied_runner = run_dir / "runner.mjs"
        copied_runner.write_bytes(runner_bytes)
        manifest = {
            "upstreamCommit": UPSTREAM_COMMIT,
            "bunVersion": bun_version,
            "sourceManifest": sources,
            "license": license_entry,
            "runnerSha256": sha256(runner_bytes),
        }
        manifest_path = run_dir / "source-manifest.json"
        manifest_path.write_text(json.dumps(manifest, indent=2) + "\n", encoding="utf-8")

        result = subprocess.run(
            [str(bun), str(copied_runner), str(manifest_path)],
            cwd=run_dir,
            capture_output=True,
            timeout=120,
            check=False,
        )
        (run_dir / "stdout.txt").write_bytes(result.stdout)
        (run_dir / "stderr.txt").write_bytes(result.stderr)
        if result.returncode:
            raise RuntimeError(f"Oracle runner exited {result.returncode}; see stderr.txt")
        oracle_path = run_dir / "oracle.json"
        oracle = json.loads(oracle_path.read_text(encoding="utf-8"))
        if oracle["upstreamCommit"] != UPSTREAM_COMMIT or oracle["sourceManifest"] != sources:
            raise RuntimeError("Oracle source receipts differ from the exported Git blobs")
        if oracle["mockCalls"] != 0:
            raise RuntimeError("An immutable Skill invocation dependency mock was invoked")
        summary = {
            "status": "passed",
            "runDirectory": str(run_dir),
            "oracle": str(oracle_path),
            "parserCases": sum(case["kind"] == "parse" for case in oracle["cases"]),
            "builderCases": sum(case["kind"] == "build" for case in oracle["cases"]),
            "mockCalls": oracle["mockCalls"],
        }
        print(json.dumps(summary, ensure_ascii=False))
        return 0
    except Exception as exc:
        summary["error"] = str(exc)
        (run_dir / "failure.json").write_text(json.dumps(summary, indent=2) + "\n", encoding="utf-8")
        print(json.dumps(summary, ensure_ascii=False), file=sys.stderr)
        return 1


if __name__ == "__main__":
    raise SystemExit(main())
