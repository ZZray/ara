"""Run unchanged fixed OMP RPC dispatch/shutdown tests against exact source.

This is an upstream behavior baseline, not Rust parity evidence. The harness
extracts only the declarations needed by the original tests; full runRpcMode,
AgentSession, native dependencies and production EOF delivery are not executed.
Every invocation retains a fresh export, provenance manifest and process logs.
"""

from __future__ import annotations

import argparse
import datetime as dt
import json
import re
import sys
import time
import uuid
from pathlib import Path

# Importing the shared runner must not create __pycache__ in the repository.
sys.dont_write_bytecode = True
from rpc_transport_oracle import (  # noqa: E402
    BUN_VERSION,
    COMMIT,
    export,
    file_sha256,
    git,
    run,
    save_result,
    sha256,
)


SOURCES = (
    "packages/coding-agent/src/modes/rpc/rpc-mode.ts",
    "packages/coding-agent/src/modes/rpc/host-tools.ts",
    "packages/coding-agent/src/modes/rpc/host-uris.ts",
    "packages/coding-agent/src/modes/rpc/rpc-types.ts",
    "packages/coding-agent/src/extensibility/tool-proxy.ts",
    "packages/coding-agent/src/tools/essential-tools.ts",
    "packages/coding-agent/test/rpc-input-frame.test.ts",
    "packages/utils/src/type-guards.ts",
    "packages/utils/src/snowflake.ts",
    "packages/ai/src/utils/schema/wire.ts",
    "LICENSE",
)
TEST = "packages/coding-agent/test/rpc-input-frame.test.ts"
BOUNDARY = (
    "Unchanged fixed RPC dispatch/shutdown tests against exact declarations. "
    "This is upstream source evidence, not Rust parity evidence. "
    "Full runRpcMode, AgentSession, native product dependencies and production "
    "EOF delivery are not executed."
)


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--upstream", required=True, type=Path, help="Git checkout containing the fixed OMP commit")
    parser.add_argument("--bun", required=True, type=Path, help=f"Exact Bun {BUN_VERSION} executable")
    parser.add_argument("--output", required=True, type=Path, help="Parent directory for a fresh retained run")
    args = parser.parse_args()
    started_at = dt.datetime.now(dt.timezone.utc)
    started = time.monotonic()
    stamp = started_at.strftime("%Y%m%dT%H%M%SZ")
    run_dir = args.output.resolve() / f"run-{stamp}-{uuid.uuid4().hex[:8]}"
    run_dir.mkdir(parents=True, exist_ok=False)
    root = run_dir / "upstream"
    checks: dict[str, object] = {}
    summary: dict[str, object] = {
        "status": "failed", "runDirectory": str(run_dir), "upstreamCommit": COMMIT,
        "boundary": BOUNDARY, "startedAt": started_at.isoformat(), "checks": checks,
    }
    manifest: dict[str, object] = {
        "upstreamCommit": COMMIT, "boundary": BOUNDARY,
        "sources": [], "extractions": [], "helpers": [], "harnessArtifacts": [],
    }
    manifest_path = run_dir / "source-manifest.json"

    def save_manifest() -> None:
        manifest_path.write_bytes((json.dumps(manifest, indent=2) + "\n").encode("utf-8"))
        summary["sourceManifestSha256"] = file_sha256(manifest_path)

    def check(label: str, command: list[str], *, cwd: Path | None = None, timeout: int = 30):
        check_started = time.monotonic()
        result = run(command, cwd=cwd, timeout=timeout)
        receipt = save_result(run_dir, label, result)
        receipt.update(command=command, cwd=str(cwd) if cwd else None,
                       elapsedSeconds=time.monotonic() - check_started)
        checks[label] = receipt
        return result

    try:
        # Archive the exact scripts used, including shared export/runner helpers.
        launcher = Path(__file__).resolve()
        infrastructure = launcher.with_name("rpc_transport_oracle.py")
        artifacts = []
        for source in (launcher, infrastructure):
            body = source.read_bytes()
            (run_dir / source.name).write_bytes(body)
            artifacts.append({"path": source.name, "sha256": sha256(body)})
        manifest["harnessArtifacts"] = artifacts
        manifest["launcherSha256"] = artifacts[0]["sha256"]
        manifest["sharedInfrastructureSha256"] = artifacts[1]["sha256"]
        manifest["pythonVersion"] = sys.version
        try:
            manifest["pythonExecutableSha256"] = file_sha256(Path(sys.executable))
        except OSError:
            # Windows Store execution aliases can be runnable but unreadable.
            manifest["pythonExecutableSha256"] = None
        save_manifest()

        upstream = args.upstream.resolve(strict=True)
        bun = args.bun.resolve(strict=True)
        resolved = git(upstream, "rev-parse", f"{COMMIT}^{{commit}}").decode("ascii").strip()
        if resolved != COMMIT:
            raise RuntimeError("Fixed OMP commit unavailable")
        version = check("bun-version", [str(bun), "--version"])
        actual_version = version.stdout.decode("ascii", "replace").strip()
        manifest["bunVersion"] = actual_version
        manifest["bunExecutableSha256"] = file_sha256(bun)
        save_manifest()
        if version.returncode or actual_version != BUN_VERSION:
            raise RuntimeError(f"Expected Bun {BUN_VERSION}; see retained bun-version logs")

        export_started = time.monotonic()
        sources = [export(upstream, root, path) for path in SOURCES]
        manifest["sources"] = sources
        summary["exportElapsedSeconds"] = time.monotonic() - export_started
        source_by_path = {source["path"]: source for source in sources}
        extractions: list[dict[str, object]] = []
        helpers: list[dict[str, object]] = []
        manifest["extractions"] = extractions
        manifest["helpers"] = helpers

        def extract(path: str, start: str, end: str) -> bytes:
            body = (root / path).read_bytes()
            start_marker, end_marker = start.encode("utf-8"), end.encode("utf-8")
            if body.count(start_marker) != 1 or body.count(end_marker) != 1:
                raise RuntimeError(f"Extraction markers are not unique: {path}")
            first = body.index(start_marker)
            last = body.index(end_marker, first)
            piece = body[first:last]
            extractions.append({
                "path": path, "gitBlob": source_by_path[path]["gitBlob"],
                "byteStart": first, "byteEndExclusive": last,
                "startLine": body[:first].count(b"\n") + 1,
                "endLine": body[:last].count(b"\n"), "sha256": sha256(piece),
                "startMarker": start, "endMarker": end,
            })
            return piece

        def write_helper(path: str, body: bytes) -> None:
            destination = root / path
            destination.write_bytes(body)
            if destination.read_bytes() != body:
                raise RuntimeError(f"Helper write mismatch: {path}")
            helpers.append({"path": path, "sha256": sha256(body)})

        # All runtime bodies below are unchanged source slices or full blobs.
        # Package aliases contain only imports/re-exports, never fake functions.
        mode_path = "packages/coding-agent/src/modes/rpc/rpc-mode.ts"
        imports = (
            b'import { isRecord } from "./packages/utils/src/type-guards.ts";\n'
            b'import { isRpcHostToolResult, isRpcHostToolUpdate } from '
            b'"./packages/coding-agent/src/modes/rpc/host-tools.ts";\n'
            b'import { isRpcHostUriResult } from "./oracle-host-uri-guard.ts";\n'
        )
        write_helper("oracle-rpc-mode.ts", imports + extract(
            mode_path, "export type PendingExtensionRequest =", "export type RpcSessionChangeCommand ="
        ) + extract(
            mode_path, "export interface RpcInputFrameDeps", "export type RpcSubagentResetRegistry ="
        ))
        write_helper("oracle-host-uri-guard.ts", extract(
            "packages/coding-agent/src/modes/rpc/host-uris.ts",
            "export function isRpcHostUriResult", "/**\n * One handler instance",
        ))
        write_helper("oracle-schema-guard.ts", extract(
            "packages/ai/src/utils/schema/wire.ts", "export function isArkSchema", "function isArkJsonAst",
        ))
        write_helper("oracle-pi-utils.ts", (
            b'export { isRecord } from "./packages/utils/src/type-guards.ts";\n'
            b'export { Snowflake } from "./packages/utils/src/snowflake.ts";\n'
        ))
        config = {"compilerOptions": {"baseUrl": ".", "paths": {
            "@oh-my-pi/pi-utils": ["./oracle-pi-utils.ts"],
            "@oh-my-pi/pi-ai/utils/schema": ["./oracle-schema-guard.ts"],
            "@oh-my-pi/pi-coding-agent/modes/rpc/rpc-mode": ["./oracle-rpc-mode.ts"],
            "@oh-my-pi/pi-coding-agent/modes/rpc/host-tools": [
                "./packages/coding-agent/src/modes/rpc/host-tools.ts"
            ],
            "@oh-my-pi/pi-coding-agent/modes/rpc/rpc-types": [
                "./packages/coding-agent/src/modes/rpc/rpc-types.ts"
            ],
        }}}
        write_helper("tsconfig.json", (json.dumps(config, indent=2) + "\n").encode("utf-8"))
        save_manifest()
        original = check("original-tests", [str(bun), "test", TEST], cwd=root, timeout=180)
        if original.returncode:
            raise RuntimeError("Unchanged upstream tests failed; see retained original-tests logs")
        counts = (original.stdout + original.stderr).decode("utf-8", "replace")
        if not re.search(r"\b15 pass\b", counts) or not re.search(r"\b0 fail\b", counts):
            raise RuntimeError("Expected all 15 upstream tests to execute; see retained original-tests logs")
        # Bind the receipt to the actual inputs remaining after execution.
        for source in sources:
            if file_sha256(root / source["path"]) != source["sha256"]:
                raise RuntimeError(f"Export changed during execution: {source['path']}")
        for helper in helpers:
            if file_sha256(root / str(helper["path"])) != helper["sha256"]:
                raise RuntimeError(f"Helper changed during execution: {helper['path']}")
        summary.update(status="passed", testCount=15, failedTestCount=0)
    except Exception as exc:
        summary["error"] = str(exc)
    finally:
        save_manifest()
        summary.update(finishedAt=dt.datetime.now(dt.timezone.utc).isoformat(),
                       elapsedSeconds=time.monotonic() - started,
                       launcherSha256=manifest.get("launcherSha256"),
                       sharedInfrastructureSha256=manifest.get("sharedInfrastructureSha256"),
                       bunExecutableSha256=manifest.get("bunExecutableSha256"),
                       pythonExecutableSha256=manifest.get("pythonExecutableSha256"))
        result_path = run_dir / ("summary.json" if summary["status"] == "passed" else "failure.json")
        result_path.write_bytes((json.dumps(summary, indent=2, ensure_ascii=False) + "\n").encode("utf-8"))
        print(json.dumps(summary, ensure_ascii=False),
              file=sys.stdout if summary["status"] == "passed" else sys.stderr)
    return 0 if summary["status"] == "passed" else 1


if __name__ == "__main__":
    raise SystemExit(main())
