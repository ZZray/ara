"""Run the fixed OMP RPC transport tests and produce byte-level Bun oracles.

The exported TypeScript and upstream tests are exact Git blobs. Every run gets
its own directory, including failure logs; this script does not edit the repo.
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


COMMIT = "596f2da7101178214aa27a753529d15e6b7ad91d"
BUN_VERSION = "1.4.0"
SOURCES = (
    "packages/coding-agent/src/modes/rpc/rpc-frame.ts",
    "packages/coding-agent/src/modes/rpc/rpc-input.ts",
    "packages/coding-agent/src/modes/rpc/rpc-types.ts",
    "packages/coding-agent/src/modes/rpc/rpc-mode.ts",
    "packages/coding-agent/src/modes/rpc/rpc-client.ts",
    "packages/coding-agent/test/rpc-frame.test.ts",
    "packages/coding-agent/test/rpc-malformed-input.test.ts",
    "packages/utils/src/type-guards.ts",
    "packages/utils/src/stream.ts",
    "packages/utils/src/abortable.ts",
    "packages/utils/src/json-parse.ts",
    "packages/utils/src/json-lexer.ts",
    "LICENSE",
)
TESTS = (
    "packages/coding-agent/test/rpc-frame.test.ts",
    "packages/coding-agent/test/rpc-malformed-input.test.ts",
)


def sha256(data: bytes) -> str:
    return hashlib.sha256(data).hexdigest()


def file_sha256(path: Path) -> str:
    digest = hashlib.sha256()
    with path.open("rb") as stream:
        for block in iter(lambda: stream.read(1024 * 1024), b""):
            digest.update(block)
    return digest.hexdigest()


def run(args: list[str], *, cwd: Path | None = None, timeout: int = 30) -> subprocess.CompletedProcess[bytes]:
    try:
        return subprocess.run(args, cwd=cwd, capture_output=True, timeout=timeout, check=False)
    except subprocess.TimeoutExpired as exc:
        # Keep partial test/oracle output and mark the check failed.
        return subprocess.CompletedProcess(args, -124, exc.stdout or b"", exc.stderr or b"")


def git(upstream: Path, *args: str) -> bytes:
    result = run(["git", "-C", str(upstream), *args])
    if result.returncode:
        raise RuntimeError(f"git {args!r} failed: {result.stderr.decode('utf-8', 'replace')}")
    return result.stdout


def export(upstream: Path, destination: Path, path: str) -> dict[str, str]:
    spec = f"{COMMIT}:{path}"
    blob = git(upstream, "rev-parse", spec).decode("ascii").strip()
    data = git(upstream, "show", spec)
    actual = hashlib.sha1(b"blob " + str(len(data)).encode("ascii") + b"\0" + data).hexdigest()
    if actual != blob:
        raise RuntimeError(f"Git blob hash mismatch: {path}")
    target = destination / path
    target.parent.mkdir(parents=True, exist_ok=True)
    target.write_bytes(data)
    if target.read_bytes() != data:
        raise RuntimeError(f"Git blob write mismatch: {path}")
    return {"path": path, "gitBlob": blob, "sha256": sha256(data)}


def save_result(run_dir: Path, label: str, result: subprocess.CompletedProcess[bytes]) -> dict[str, object]:
    (run_dir / f"{label}.stdout.txt").write_bytes(result.stdout)
    (run_dir / f"{label}.stderr.txt").write_bytes(result.stderr)
    return {"exitCode": result.returncode, "timedOut": result.returncode == -124,
            "stdoutSha256": sha256(result.stdout), "stderrSha256": sha256(result.stderr)}


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--upstream", required=True, type=Path, help="Git checkout containing the fixed OMP commit")
    parser.add_argument("--bun", required=True, type=Path, help="Exact Bun 1.4.0 executable")
    parser.add_argument("--output", required=True, type=Path, help="Parent directory for a fresh retained run")
    args = parser.parse_args()
    stamp = dt.datetime.now(dt.timezone.utc).strftime("%Y%m%dT%H%M%SZ")
    run_dir = args.output.resolve() / f"run-{stamp}-{uuid.uuid4().hex[:8]}"
    run_dir.mkdir(parents=True, exist_ok=False)
    summary: dict[str, object] = {"status": "failed", "runDirectory": str(run_dir), "checks": {}}
    try:
        upstream = args.upstream.resolve(strict=True)
        bun = args.bun.resolve(strict=True)
        resolved = git(upstream, "rev-parse", f"{COMMIT}^{{commit}}").decode("ascii").strip()
        if resolved != COMMIT:
            raise RuntimeError("Fixed OMP commit unavailable")
        version = run([str(bun), "--version"])
        if version.returncode or version.stdout.decode("ascii").strip() != BUN_VERSION:
            raise RuntimeError(f"Expected Bun {BUN_VERSION}; got {version.stdout!r}, {version.stderr!r}")

        root = run_dir / "upstream"
        sources = [export(upstream, root, path) for path in SOURCES]
        runner = Path(__file__).with_suffix(".mjs")
        runner_bytes = runner.read_bytes()
        (run_dir / "runner.mjs").write_bytes(runner_bytes)

        # Package aliases expose only the unchanged source exports needed by
        # the original tests. They contain no substitute behavior.
        alias = root / "oracle-pi-utils.ts"
        alias.write_text(
            'export { isRecord } from "./packages/utils/src/type-guards.ts";\n'
            'export { readLines } from "./packages/utils/src/stream.ts";\n',
            encoding="utf-8",
        )
        tsconfig = root / "tsconfig.json"
        tsconfig.write_text(
            json.dumps({"compilerOptions": {"baseUrl": ".", "paths": {
                "@oh-my-pi/pi-utils": ["./oracle-pi-utils.ts"],
                "@oh-my-pi/pi-coding-agent/modes/rpc/rpc-input": [
                    "./packages/coding-agent/src/modes/rpc/rpc-input.ts"
                ],
            }}}, indent=2) + "\n", encoding="utf-8",
        )
        helpers = [
            {"path": "oracle-pi-utils.ts", "sha256": sha256(alias.read_bytes())},
            {"path": "tsconfig.json", "sha256": sha256(tsconfig.read_bytes())},
        ]
        python_executable = Path(sys.executable)
        try:
            python_executable_sha256 = file_sha256(python_executable)
        except OSError:
            # Windows Store execution aliases may be runnable but unreadable.
            python_executable_sha256 = None
        manifest = {"upstreamCommit": COMMIT, "bunVersion": version.stdout.decode("ascii").strip(),
                    "sources": sources, "runnerSha256": sha256(runner_bytes), "helpers": helpers,
                    "launcherSha256": file_sha256(Path(__file__)),
                    "pythonVersion": sys.version,
                    "pythonExecutableSha256": python_executable_sha256,
                    "bunExecutableSha256": file_sha256(bun)}
        manifest_path = run_dir / "source-manifest.json"
        manifest_path.write_text(json.dumps(manifest, indent=2) + "\n", encoding="utf-8")
        summary.update(sourceManifestSha256=file_sha256(manifest_path), runnerSha256=manifest["runnerSha256"],
                       launcherSha256=manifest["launcherSha256"], bunExecutableSha256=manifest["bunExecutableSha256"],
                       pythonExecutableSha256=manifest["pythonExecutableSha256"])

        checks = summary["checks"]
        assert isinstance(checks, dict)
        original = run([str(bun), "test", *TESTS], cwd=root, timeout=180)
        checks["originalTests"] = save_result(run_dir, "original-tests", original)
        oracle = run([str(bun), str(run_dir / "runner.mjs"), str(manifest_path)], cwd=run_dir, timeout=240)
        checks["oracle"] = save_result(run_dir, "oracle", oracle)
        if original.returncode or oracle.returncode:
            raise RuntimeError("Original tests or added oracle failed; see retained output files")
        oracle_path = run_dir / "oracle.json"
        output = json.loads(oracle_path.read_text(encoding="utf-8"))
        if output.get("sourceManifestSha256") != sha256(manifest_path.read_bytes()):
            raise RuntimeError("Oracle did not bind itself to the exported source manifest")
        summary.update(status="passed", oracle=str(oracle_path), caseCount=len(output["cases"]))
        (run_dir / "summary.json").write_text(json.dumps(summary, indent=2, ensure_ascii=False) + "\n", encoding="utf-8")
        print(json.dumps(summary, ensure_ascii=False))
        return 0
    except Exception as exc:
        summary["error"] = str(exc)
        (run_dir / "failure.json").write_text(json.dumps(summary, indent=2, ensure_ascii=False) + "\n", encoding="utf-8")
        print(json.dumps(summary, ensure_ascii=False), file=sys.stderr)
        return 1


if __name__ == "__main__":
    raise SystemExit(main())
