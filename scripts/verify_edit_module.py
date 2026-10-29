"""Build EDIT once and execute the existing module/process groups.

No credentials or live model are read. --groups narrows repair verification;
--build-receipt reuses a prior build only while all source pins still match.
--full adds the existing workspace gate after a green deterministic batch.
--include-write shares the original WRITE groups with the EDIT build.
--print-build-plan performs preflight without compiling or running tests.
--build-groups retains a module's Cargo feature/target combination while
--groups selects only the executions needed for a repair.
This receipt alone does not accept complete EDIT or any external service.
"""
from __future__ import annotations

import argparse
import hashlib
import json
import os
from pathlib import Path
import re
import subprocess
import sys
import time

ROOT = Path(__file__).resolve().parents[1]
GROUPS = {
    "engine": ("edit", ""),
    "snapshot": ("model_edit_snapshot", ""),
    "post-apply": ("edit_post_apply_family", ""),
    "policy": ("edit_mode_policy", ""),
    "actual-tools": ("ara_cli", "actual_tools::"),
    "terminal": ("ara_cli", "terminal::"),
    "vcs": ("ara_cli", "vcs_diff::"),
    "value-heap": ("value_heap", ""),
    "rpc-ast": ("rpc_host", "ast_edit_rpc::"),
    "retained-repl": ("e2e", "repl_clear_and_new_keep_retained_edit_snapshots"),
    "new-session": ("e2e", "repl_new_starts_a_fresh_session_file"),
    "clear-compact": ("e2e", "repl_clear_compact_and_reopen_keep_only_the_active_suffix"),
    "journal-failure": ("ara", "tests::journal_failure_cancels_the_run"),
    "host-idle-settlement": ("ara", "rpc_host::tests::terminal_is_published_only_after_owned_run_settles"),
    "host-uris": ("host_uris", ""),
    "read-tools": ("tools", "read_"),
    "ast-family": ("ara_tools", "ast_edit::tests::"),
    "streaming-guard": ("ara_cli", "streaming_edit_guard::tests::fixed_streaming_edit_abort_and_generated_guard_group"),
    "self-error": ("agent_loop", "validation_unknown_tool_blocked_and_empty_error"),
}
WRITE_GROUPS = {
    "write-tools": ("tools", "write_creates_dirs_counts_utf16_and_marks_shebang"),
    "sqlite-write": ("write_special_targets", ""),
    "conflict-core": ("conflict_detect", ""),
    "conflict-host": ("conflict_integration", ""),
    "journal-failure": GROUPS["journal-failure"],
}
ACP_GROUPS = {
    "host-bridges": ("host_bridges", ""),
}
REPORT_GROUPS = {
    "report-consent": ("ara_cli", "report_tool_issue::tests::fixed_report_issue_consent8_grouped"),
    "report-main-native": ("ara_cli", "report_tool_issue::tests::fixed_report_issue_main21_native_grouped"),
    "report-renderer": ("ara_cli", "terminal::xdev_renderer::report_issue_call_preview_grouped"),
    "report-host-settings": ("ara", "rpc_host_settings::report_issue_adapter_grouped"),
    "report-xdev-regression": ("ara_cli", "actual_tools::xdev_native::tests::exercise_xdev_native_protocol_contracts"),
}
TARGET_PACKAGES = {
    "edit": "ara-tools", "model_edit_snapshot": "ara-tools",
    "conflict_detect": "ara-tools", "conflict_integration": "ara-tools",
    "edit_post_apply_family": "ara-tools", "tools": "ara-tools", "write_special_targets": "ara-tools",
    "value_heap": "ara-rpc",
    "host_uris": "ara-tools",
    "ara_tools": "ara-tools", "agent_loop": "ara-agent",
}


def digest(path):
    return hashlib.sha256(Path(path).read_bytes()).hexdigest()


def source_pins():
    paths = [ROOT / "Cargo.toml", ROOT / "Cargo.lock", Path(__file__).resolve()]
    paths.extend((ROOT / "scripts").glob("*.py"))
    paths.extend((ROOT / ".cargo").rglob("*"))
    # Local vendored Rust packages (especially ara-edit) are delivered code.
    # Their manifests, source, fixtures and templates are part of build identity.
    paths.extend((ROOT / "crates").rglob("*"))
    return {str(p.relative_to(ROOT)): digest(p) for p in sorted(set(paths)) if p.is_file()}


def build_command(selected, groups):
    targets = list(dict.fromkeys(groups[group][0] for group in selected))
    packages = list(dict.fromkeys(TARGET_PACKAGES.get(target, "ara-cli") for target in targets))
    command = ["cargo", "test", "--locked"]
    for package in packages:
        command.extend(["-p", package])
    command.append("--all-features")
    if "ara_cli" in targets or "ara_tools" in targets:
        command.append("--lib")
    if "ara" in targets:
        command.extend(["--bin", "ara"])
    for target in targets:
        if target not in ("ara_cli", "ara_tools", "ara"):
            command.extend(["--test", target])
    if "ara" in targets or "e2e" in targets:
        # The existing integration target requests Cargo's real executable
        # artifact; a bin test harness cannot serve the Content-Length child.
        if "e2e" not in targets:
            command.extend(["--test", "e2e"])
    return command + ["--no-run", "--message-format=json"]


def delivery_preflight(path, expected_sha256):
    if not path:
        return None
    if digest(path) != expected_sha256:
        raise ValueError("Delivered manifest hash changed")
    manifest = json.loads(path.read_text(encoding="utf-8-sig"))
    if manifest["state"] != "DELIVERED_SOURCE_ONLY":
        raise ValueError("Source delivery has not completed")
    if manifest["fixed_omp_commit"] != "596f2da7101178214aa27a753529d15e6b7ad91d":
        raise ValueError("Fixed OMP source changed")
    if manifest["index_sha256"] != digest(ROOT / ".git/index"):
        raise ValueError("Git index changed")
    files = manifest["files"]
    if not files:
        raise ValueError("Delivered manifest has no source files")
    for row in files:
        delivered = (ROOT / row["path"]).resolve()
        delivered.relative_to(ROOT.resolve())
        if digest(delivered) != row["delivered_sha256"]:
            raise ValueError(f"Delivered source changed: {row['path']}")
    return dict(path=str(path.resolve()), sha256=expected_sha256, checked_files=len(files))


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--output", type=Path, required=True)
    groups = {**GROUPS, **WRITE_GROUPS, **ACP_GROUPS, **REPORT_GROUPS}
    parser.add_argument("--groups", nargs="+", choices=groups, default=list(GROUPS))
    parser.add_argument("--build-groups", nargs="+", choices=groups)
    parser.add_argument("--build-receipt", type=Path)
    parser.add_argument("--full", action="store_true")
    parser.add_argument("--include-write", action="store_true")
    parser.add_argument("--delivery-manifest", type=Path)
    parser.add_argument("--manifest-sha256")
    parser.add_argument("--print-build-plan", action="store_true")
    args = parser.parse_args()
    if bool(args.delivery_manifest) != bool(args.manifest_sha256):
        parser.error("--delivery-manifest and --manifest-sha256 must be supplied together")
    selected = list(dict.fromkeys(args.groups + (list(WRITE_GROUPS) if args.include_write else [])))
    build_selected = list(dict.fromkeys(args.build_groups or selected))
    missing_targets = {groups[group][0] for group in selected} - {groups[group][0] for group in build_selected}
    if missing_targets:
        parser.error("--build-groups must include every selected execution target")
    delivery = delivery_preflight(args.delivery_manifest, args.manifest_sha256)
    command = build_command(build_selected, groups)
    if args.print_build_plan:
        print(json.dumps(dict(groups={group: groups[group] for group in selected}, command=command,
                              build_groups=build_selected,
                              delivery=delivery, boundary="Plan only; no build, tests or acceptance")))
        return 0
    out = args.output.resolve()
    out.mkdir(parents=True, exist_ok=True)
    env = os.environ.copy()
    # Preserve inherited runtime/library paths; fix Cargo identity for the batch.
    env.update(CARGO_BUILD_JOBS="2", CARGO_HOME=r"D:\workspace\env\cache\cargo",
               RUSTUP_HOME=r"D:\workspace\env\tools\rustup",
               CARGO_TARGET_DIR=os.environ.get("CARGO_TARGET_DIR", r"D:\workspace\env\cache\rust-target"),
               CARGO_INCREMENTAL="0")
    pins = source_pins()
    started = time.monotonic()
    records = []

    def run(name, command, timeout=180):
        stdout_path, stderr_path = out / (name + "-stdout.txt"), out / (name + "-stderr.txt")
        if stdout_path.exists() or stderr_path.exists():
            raise FileExistsError(f"Retain old receipts; choose a fresh output: {name}")
        tick = time.monotonic()
        print(json.dumps({"started": name}), flush=True)
        with stdout_path.open("wb") as stdout, stderr_path.open("wb") as stderr:
            try:
                result = subprocess.run(command, cwd=ROOT, env=env, stdout=stdout, stderr=stderr, timeout=timeout)
                code = result.returncode
            except subprocess.TimeoutExpired:
                code = 124
        record = dict(group=name, command=command, exit_code=code,
                      elapsed_seconds=round(time.monotonic() - tick, 3),
                      stdout=str(stdout_path), stderr=str(stderr_path))
        print(json.dumps(record), flush=True)
        return record

    build_path = out / "build-result.json"
    if args.build_receipt:
        build_path = args.build_receipt.resolve()
        build = json.loads(build_path.read_text(encoding="utf-8"))
        assert build["source_pins"] == pins and build["exit_code"] == 0, "Build source changed"
        for artifact in build["artifacts"]:
            assert digest(artifact["executable"]) == artifact["sha256"], artifact["executable"]
    else:
        if build_path.exists():
            raise FileExistsError("Keep historical build results; use a fresh output")
        step = run("build", command, timeout=1200)
        artifacts, counts = [], dict(reused=0, compiled=0, warnings=0, errors=0)
        for line in Path(step["stdout"]).read_text(encoding="utf-8").splitlines():
            try:
                item = json.loads(line)
            except ValueError:
                continue
            if item.get("reason") == "compiler-artifact":
                counts["reused" if item["fresh"] else "compiled"] += 1
                if item.get("executable"):
                    artifacts.append(dict(target=item["target"]["name"], kind=item["target"]["kind"],
                                          test=item["profile"]["test"], executable=item["executable"],
                                          sha256=digest(item["executable"])))
            elif item.get("reason") == "compiler-message" and item["message"]["level"] in ("warning", "error"):
                counts[item["message"]["level"] + "s"] += 1
        build = dict(**step, counts=counts, artifacts=artifacts, source_pins=pins,
                     source_unchanged=pins == source_pins(), delivery=delivery)
        build_path.write_text(json.dumps(build, indent=2), encoding="utf-8")
        if step["exit_code"] or not build["source_unchanged"]:
            return step["exit_code"] or 125
    executables = {a["target"]: a for a in build["artifacts"] if a["test"]}
    fixture_required = any(groups[group][0] in ("ara", "e2e") for group in selected)
    if fixture_required:
        fixtures = [a for a in build["artifacts"] if a["target"] == "ara-lsp-fixture" and not a["test"]]
        assert len(fixtures) == 1, "Real LSP child fixture artifact is required"
        fixture = fixtures[0]
        assert digest(fixture["executable"]) == fixture["sha256"]
        env["ARA_LSP_FIXTURE_PATH"] = fixture["executable"]
    listings = {}
    for group in selected:
        target, test_filter = groups[group]
        artifact = executables[target]
        binary = artifact["executable"]
        if target not in listings:
            listed = subprocess.run([binary, "--list"], cwd=ROOT, env=env, capture_output=True,
                                    text=True, encoding="utf-8", errors="strict", timeout=30)
            assert listed.returncode == 0
            listings[target] = [line[:-6] for line in listed.stdout.splitlines() if line.endswith(": test")]
        names = listings[target]
        expected = [name for name in names if test_filter in name]
        assert expected, (group, test_filter)
        step = run(group, [binary, test_filter, "--test-threads=1", "--nocapture"])
        raw = Path(step["stdout"]).read_bytes()
        match = re.search(rb"test result: (\w+)\. (\d+) passed; (\d+) failed; (\d+) ignored;", raw)
        counts = dict(result=match[1].decode(), passed=int(match[2]), failed=int(match[3]), ignored=int(match[4])) if match else None
        step.update(expected_names=expected, counts=counts, executable_sha256=artifact["sha256"],
                    passed=bool(step["exit_code"] == 0 and counts and counts["result"] == "ok" and
                                counts["passed"] == len(expected) and counts["failed"] == counts["ignored"] == 0 and
                                digest(binary) == artifact["sha256"]))
        records.append(step)
    passed = all(step["passed"] for step in records)
    if passed and args.full:
        step = run("backend", [sys.executable, "-X", "utf8", str(ROOT / "scripts/verify_backend.py")], timeout=1200)
        step["passed"] = step["exit_code"] == 0
        records.append(step)
        passed = step["passed"]
    unchanged = source_pins() == pins
    receipt = dict(state="PASS" if passed and unchanged else "FAIL", source_unchanged=unchanged,
                   build_groups=build_selected,
                   elapsed_seconds=round(time.monotonic() - started, 3), build=str(build_path),
                   records=records, full_gate=args.full, delivery=delivery,
                   boundary="Selected original module groups only. ConPTY, real model, supplied service and independent acceptance use separate receipts.")
    (out / "module-result.json").write_text(json.dumps(receipt, indent=2), encoding="utf-8")
    print(json.dumps({k: v for k, v in receipt.items() if k != "records"}), flush=True)
    return 0 if receipt["state"] == "PASS" else 1


if __name__ == "__main__":
    raise SystemExit(main())
