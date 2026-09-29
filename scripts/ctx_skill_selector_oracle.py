"""Export and execute the fixed OMP Skill selector renderer without source rewrites.

Each invocation owns a new run directory beneath --output. Failed subprocess
logs and exact source bytes remain there; only a successful run gets oracle.json.
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
NATIVE_SHA256 = "fd757d36c44b8fa4cb184adc979f39b6aedabf8341d5a5316bf36f3c3949aa20"
SOURCE_ROOT = "packages/coding-agent/src/"
SOURCE_PATHS = tuple(
    SOURCE_ROOT + path
    for path in (
        "tools/read-format.ts",
        "tools/read-selector.ts",
        "tools/path-utils.ts",
        "tools/hashline-format.ts",
        "tools/tool-result.ts",
        "tools/output-meta.ts",
        "tools/tool-errors.ts",
        "utils/block-context.ts",
        "utils/file-display-mode.ts",
        "utils/edit-mode.ts",
        "session/streaming-output.ts",
        "internal-urls/filesystem-resource.ts",
    )
)


def sha256(data: bytes) -> str:
    return hashlib.sha256(data).hexdigest()


def command(args: list[str], *, cwd: Path | None = None) -> bytes:
    result = subprocess.run(args, cwd=cwd, capture_output=True, timeout=30, check=False)
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
    # subprocess bytes are intentional: PowerShell text pipelines normalize
    # newlines and therefore cannot export authoritative Git blob contents.
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
    parser.add_argument("--native", required=True, type=Path)
    parser.add_argument("--output", required=True, type=Path)
    args = parser.parse_args()
    stamp = dt.datetime.now(dt.timezone.utc).strftime("%Y%m%dT%H%M%SZ")
    run_dir = args.output.resolve() / f"run-{stamp}-{uuid.uuid4().hex[:8]}"
    run_dir.mkdir(parents=True, exist_ok=False)
    summary: dict[str, object] = {"status": "failed", "runDirectory": str(run_dir)}
    try:
        upstream = args.upstream.resolve(strict=True)
        bun = args.bun.resolve(strict=True)
        native = args.native.resolve(strict=True)
        resolved_commit = git(upstream, "rev-parse", f"{UPSTREAM_COMMIT}^{{commit}}").decode("ascii").strip()
        if resolved_commit != UPSTREAM_COMMIT:
            raise RuntimeError("The fixed upstream commit is unavailable")
        bun_version = command([str(bun), "--version"]).decode("ascii").strip()
        if bun_version != BUN_VERSION:
            raise RuntimeError(f"Expected Bun {BUN_VERSION}, got {bun_version}")
        native_sha256 = sha256(native.read_bytes())
        if native_sha256 != NATIVE_SHA256:
            raise RuntimeError(f"Native binary hash mismatch: {native_sha256}")

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
            "native": {
                "path": str(native),
                "sha256": native_sha256,
                "marker": "__piNativesV18_1_8",
                "buildProvenance": "unverified",
            },
        }
        manifest_path = run_dir / "source-manifest.json"
        manifest_path.write_text(json.dumps(manifest, indent=2) + "\n", encoding="utf-8")
        summary["sourceManifestSha256"] = sha256(manifest_path.read_bytes())
        summary["runnerSha256"] = manifest["runnerSha256"]
        invocation = [str(bun), str(copied_runner), str(manifest_path)]
        summary["command"] = invocation
        (run_dir / "command.json").write_text(json.dumps(invocation, indent=2) + "\n", encoding="utf-8")
        try:
            result = subprocess.run(invocation, cwd=run_dir, capture_output=True, timeout=120, check=False)
        except subprocess.TimeoutExpired as error:
            (run_dir / "stdout.log").write_bytes(error.stdout or b"")
            (run_dir / "stderr.log").write_bytes(error.stderr or b"")
            raise RuntimeError("Fixed OMP oracle exceeded 120 seconds") from error
        (run_dir / "stdout.log").write_bytes(result.stdout)
        (run_dir / "stderr.log").write_bytes(result.stderr)
        summary["exitCode"] = result.returncode
        if result.returncode:
            raise RuntimeError(f"Fixed OMP oracle failed; see {run_dir / 'stderr.log'}")
        oracle = json.loads(result.stdout)
        if oracle["upstreamCommit"] != UPSTREAM_COMMIT or oracle["sourceManifest"] != sources:
            raise RuntimeError("Oracle source receipts differ from the exported blobs")
        if oracle["mocks"]["totalCalls"] != 0 or any(oracle["mocks"]["exports"].values()):
            raise RuntimeError("An immutable-path mock was invoked")
        if not isinstance(oracle["nativeCalls"], int) or oracle["nativeCalls"] < 1:
            raise RuntimeError("No real native block-context call was observed")
        if any(sha256((run_dir / "upstream" / entry["path"]).read_bytes()) != entry["sha256"] for entry in sources):
            raise RuntimeError("Executed source bytes changed during the run")
        oracle_path = run_dir / "oracle.json"
        oracle_path.write_bytes(result.stdout)
        summary.update(
            status="passed",
            oracle=str(oracle_path),
            oracleSha256=sha256(result.stdout),
            caseCount=len(oracle["cases"]),
            nativeCalls=oracle["nativeCalls"],
            nativeCallReceipts=str(run_dir / "native-call-receipts.json"),
            mockCalls=oracle["mocks"]["totalCalls"],
            limitations=["Installed native build provenance is unverified", "Representative Skill selectors only"],
        )
    except Exception as error:
        summary["error"] = str(error)
        print(str(error), file=sys.stderr)
    finally:
        (run_dir / "summary.json").write_text(json.dumps(summary, indent=2) + "\n", encoding="utf-8")
        print(json.dumps(summary, indent=2))
    return 0 if summary["status"] == "passed" else 1


if __name__ == "__main__":
    raise SystemExit(main())
