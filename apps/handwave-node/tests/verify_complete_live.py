"""One bounded actual-model Windows acceptance run through public HandWave.

Requires explicit authorization for the cloud trial. Maximum 12 provider calls,
180 seconds including setup, one fresh device/session, no automatic retries.
Only test-directory files and a read-only five-event System query are authorized.
"""
from __future__ import annotations

import argparse
import base64
import hashlib
import json
import os
from pathlib import Path
import secrets
import subprocess
import threading
import time

from verify_cloud import read_credentials, models, wait_tunnel_health, public_node_preflight
from verify_host import WS, port, require, run_frames, status_rows, stop

BINARY_SHA256 = "c9bfae85d2641008409d1db7f587994e36733223b9317989774143b09ebcdf97"
JQ_URL = "https://github.com/jqlang/jq/releases/download/jq-1.8.1/jq-windows-amd64.exe"
JQ_SHA256 = "23cb60a1354eed6bcc8d9b9735e8c7b388cd1fdcb75726b93bc299ef22dd9334"
JQ_BYTES = 1026560
DEADLINE = 180
MAX_CALLS = 12


def digest(path):
    return hashlib.sha256(Path(path).read_bytes()).hexdigest()


def source_receipt(actual, expected):
    """Only one terminal LF is formatting-equivalent; preserve both hashes."""
    canonical = expected.encode("utf-8")
    require(actual in (canonical, canonical + b"\n"), "Rust source differs beyond the optional single terminal LF")
    return {"raw_sha256": hashlib.sha256(actual).hexdigest(),
            "canonical_sha256": hashlib.sha256(canonical).hexdigest(),
            "terminal_lf_added": actual == canonical + b"\n",
            "format_boundary": "Exact requested UTF-8 bytes, optionally followed by one LF; no other normalization"}


def verify_tool_contract(events, work, compile_command, system_command):
    starts = [e for e in events if e.get("type") == "tool_start"]
    ends = [e for e in events if e.get("type") == "tool_end"]
    require(len(starts) == 7 and len(ends) == 7, "Exactly seven requested tool operations must complete; extra tools are not accepted")
    started_ids = [e.get("step_id") for e in starts]
    ended_ids = [e.get("step_id") for e in ends]
    require(all(started_ids) and len(set(started_ids)) == 7 and set(started_ids) == set(ended_ids)
            and len(set(ended_ids)) == 7, "Tool start/end identity pairing is incomplete")
    end_map = {e["step_id"]: e for e in ends}
    expected_paths = {"write_file": {"complete.rs", "analysis.json"}, "read_file": {"program.log", "system-receipt.json"},
                      "download_file": {"jq.exe"}}
    seen_paths = {name: set() for name in expected_paths}
    commands = []
    for event in starts:
        name = event.get("title")
        end = end_map[event["step_id"]]
        require(end.get("title") == name and end.get("status") == "completed", "Requested tool failed or mismatched its completion receipt")
        args = json.loads(event["detail"])
        require(isinstance(args, dict), "Tool arguments were not a complete object")
        if name == "exec":
            require(args.get("timeout") == 45, "Exec did not retain its 45-second bound")
            cwd = Path(args.get("cwd") or ".")
            require((work / cwd).resolve() == work.resolve(), "Exec changed the approved working directory")
            commands.append(args.get("cmd"))
        else:
            require(name in expected_paths, "Unrequested tool appeared in the live task")
            path = Path(args["path"])
            resolved = (work / path).resolve()
            require(resolved.parent == work.resolve() and resolved.name in expected_paths[name], "Tool accessed an unapproved artifact path")
            require(resolved.name not in seen_paths[name], "Requested file operation was replayed")
            seen_paths[name].add(resolved.name)
            if name == "download_file":
                require(args.get("url") == JQ_URL, "Download URL differed from the pinned official asset")
    require(seen_paths == expected_paths, "Requested write/read/download operations were incomplete")
    require(commands == [compile_command, system_command], "Actual exec commands differed from the two reviewed commands or were replayed")


def system_script():
    # This reviewed host fixture is immutable during the run. Hash verification
    # precedes execution of the downloaded official executable.
    return r'''$ErrorActionPreference = 'Stop'
function Get-ReviewedSha256([string]$path) {
    $stream = [IO.File]::OpenRead([IO.Path]::GetFullPath($path))
    try {
        $hasher = [Security.Cryptography.SHA256]::Create()
        try { return [BitConverter]::ToString($hasher.ComputeHash($stream)).Replace('-','').ToLowerInvariant() }
        finally { $hasher.Dispose() }
    } finally { $stream.Dispose() }
}
$actualHash = Get-ReviewedSha256 '.\jq.exe'
if ($actualHash -ne 'EXPECTED_JQ_HASH') { throw 'Official jq checksum mismatch; not executed' }
$version = (& .\jq.exe --version | Out-String).Trim()
if ($LASTEXITCODE -ne 0 -or $version -ne 'jq-1.8.1') { throw 'Official jq version verification failed' }
$receipt = [ordered]@{ jq_sha256=$actualHash; jq_version=$version; system=[ordered]@{ log_name='System'; max_events=5; message_char_limit=240; status='unknown'; queried_at_utc=[DateTime]::UtcNow.ToString('o'); events=@() } }
try {
    $events = @(Get-WinEvent -LogName System -MaxEvents 5 -ErrorAction Stop)
    $rows = @($events | ForEach-Object {
        $msg = [string]$_.Message
        if ($msg.Length -gt 240) { $msg = $msg.Substring(0,240) }
        [ordered]@{ id=[int]$_.Id; time_created=$_.TimeCreated.ToUniversalTime().ToString('o'); level=[string]$_.LevelDisplayName; provider=[string]$_.ProviderName; message=$msg }
    })
    $receipt.system.status = if ($rows.Count -eq 0) { 'empty' } else { 'ok' }
    $receipt.system.events = $rows
} catch {
    $receipt.system.status = if ($_.FullyQualifiedErrorId -like 'NoMatchingEventsFound*') { 'empty' } else { 'error' }
    $receipt.system.error_type = $_.Exception.GetType().FullName
    $receipt.system.error_id = ([string]$_.FullyQualifiedErrorId).Substring(0,[Math]::Min(120,([string]$_.FullyQualifiedErrorId).Length))
}
$receipt.system.event_count = @($receipt.system.events).Count
$json = $receipt | ConvertTo-Json -Depth 8
[IO.File]::WriteAllText((Join-Path (Get-Location).Path 'system-receipt.json'),$json,(New-Object Text.UTF8Encoding($false)))
$receiptHash = Get-ReviewedSha256 '.\system-receipt.json'
Write-Output ('SYSTEM_RECEIPT_SHA256=' + $receiptHash)
Write-Output ('JQ_VERIFIED=' + $version)
'''.replace("EXPECTED_JQ_HASH", JQ_SHA256)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--binary", type=Path, required=True)
    parser.add_argument("--output", type=Path, required=True)
    parser.add_argument("--ssh-alias", default="ray-dev")
    parser.add_argument("--remote-config", default="/opt/handwave/hub.conf")
    parser.add_argument("--model", default="deepseek-v4.1-flash")
    parser.add_argument("--public-hub", default="https://118.25.103.226/handwave")
    args = parser.parse_args()
    require(os.name == "nt", "This acceptance requires Windows System event log")
    require(args.binary.is_file() and digest(args.binary) == BINARY_SHA256, "Use the exact accepted final c9bf executable")
    root = args.output.resolve()
    require(root.is_relative_to(Path("D:/WorkSpace/tmp").resolve()) and root != Path("D:/WorkSpace/tmp").resolve(),
            "Acceptance output must be a fresh child of D:/WorkSpace/tmp")
    root.mkdir(parents=True, exist_ok=False)
    work, state = root / "work", root / "state"
    work.mkdir(); state.mkdir()
    started = time.monotonic()
    tunnel = node = controller = watchdog = None
    device = None
    logs, transcript, passed = [], [], []
    result = {"status": "FAIL", "scope": "actual public HandWave + actual cloud model + fresh Windows Rust node; isolated artifacts and read-only System query",
              "max_runs": 1, "max_provider_calls": MAX_CALLS, "overall_deadline_seconds": DEADLINE,
              "binary_sha256": BINARY_SHA256, "verifier_sha256": digest(__file__), "model": args.model,
              "protocol": "openai-completions", "passed": passed,
              "jq_provenance": {"release": "https://github.com/jqlang/jq/releases/tag/jq-1.8.1", "asset": JQ_URL,
                  "checksums": "https://github.com/jqlang/jq/releases/download/jq-1.8.1/sha256sum.txt",
                  "sha256": JQ_SHA256, "bytes": JQ_BYTES,
                  "verification": "Official release API digest and release sha256sum.txt independently matched before this trial"}}
    marker = "COMPLETE_PROGRAM_" + secrets.token_hex(10)
    stdout_marker = "COMPILED_EXECUTION_" + secrets.token_hex(10)
    rust_source = f'fn main() {{ std::fs::write("program.log", "{marker}\\n").unwrap(); println!("{stdout_marker}"); }}'
    helper = work / "verified-system-query.ps1"
    helper.write_text(system_script(), encoding="utf-8")
    helper_hash = digest(helper)
    compile_command = "$ErrorActionPreference='Stop'; rustc --crate-name complete_trial complete.rs -o complete.exe; if ($LASTEXITCODE -ne 0) { throw 'rustc failed' }; & .\\complete.exe; if ($LASTEXITCODE -ne 0) { throw 'compiled program failed' }"
    system_command = "& ([scriptblock]::Create((Get-Content -LiteralPath '.\\verified-system-query.ps1' -Raw)))"
    prompt = f"""Complete one bounded acceptance task only in current directory {work}.
Use the native tools directly. Do not use reference_read/search, memory tools, list_dir or search_files; all inputs are supplied here. No installation, services, settings, credentials, background processes or files outside this directory. The only permitted outside read is the supplied helper's bounded read-only Windows System log query.
1. Use write_file to create complete.rs with this exact source (one optional terminal LF is allowed; no other changes):
{rust_source}
2. Use exec with timeout 45 and exactly this PowerShell command to compile and execute it:
{compile_command}
3. Use read_file(path='program.log') to read the actual generated log.
4. Use download_file(url='{JQ_URL}', path='jq.exe'). Do not use exec to download it.
5. Use exec with timeout 45 and exactly this command, without editing the existing helper:
{system_command}
The immutable helper checks jq's official SHA256 before executing jq --version, then reads at most five System events with messages bounded to 240 characters. It writes system-receipt.json with explicit ok/empty/error status; do not fabricate or modify that receipt.
6. Use read_file(path='system-receipt.json') to inspect the actual receipt.
7. Use write_file to create analysis.json as a JSON object with these required fields: program_marker (the exact log marker), jq_version (actual version), system_status (actual receipt status), system_event_count (actual integer count), system_event_ids (actual IDs in receipt order), analysis (a short explanation of the observed recent events, or the explicit empty/error limitation; do not claim a root cause from this tiny sample).
8. Give a short final answer naming the artifacts. Do not perform extra verification tools or rerun any commands. Preserve errors explicitly; do not turn unknown status into success.
You have at most 12 model calls and 180 seconds for this complete run. A single response may invoke independent tools together when appropriate."""
    (root / "task.json").write_text(json.dumps({"objective": prompt, "helper_sha256": helper_hash,
        "expected_source_sha256": hashlib.sha256(rust_source.encode()).hexdigest()}, indent=2), encoding="utf-8")
    try:
        root_token, node_token = read_credentials(args.ssh_alias, args.remote_config)
        endpoint_port = port()
        logs = [(root / "tunnel.log").open("wb"), (root / "node.log").open("wb")]
        tunnel = subprocess.Popen(["ssh", "-N", "-o", "BatchMode=yes", "-o", "StrictHostKeyChecking=yes",
                    "-o", "ExitOnForwardFailure=yes", "-L", f"127.0.0.1:{endpoint_port}:127.0.0.1:18080", args.ssh_alias],
                    stdout=subprocess.DEVNULL, stderr=logs[0])
        base = f"http://127.0.0.1:{endpoint_port}"
        wait_tunnel_health(base)
        available, receipt = models(base, root_token)
        require(receipt["http_status"] == 200 and any(r["id"] == args.model for r in available), "Exact model absent from live inventory")
        result["inventory"] = receipt
        result["public_ingress_preflight"] = public_node_preflight(args.public_hub, node_token)
        env = os.environ.copy()
        for key in list(env):
            if key.lower() in ("http_proxy", "https_proxy", "all_proxy", "no_proxy", "hw_proxy", "hw_node_state", "hw_hub", "hw_token", "hw_gateway_token"):
                del env[key]
        env.update(HW_TOKEN=node_token, NO_PROXY="127.0.0.1,localhost")
        remaining = DEADLINE - (time.monotonic() - started)
        require(remaining > 30, "Setup exhausted most of the bounded trial deadline")
        node_args = [str(args.binary.resolve()), "--hub", args.public_hub, "--state-dir", str(state), "--cwd", str(work),
                     "--max-calls", str(MAX_CALLS), "--max-seconds", str(max(1, int(remaining)))]
        identity = subprocess.run(node_args + ["--identity"], env=env, capture_output=True, timeout=min(15, remaining))
        require(identity.returncode == 0, "Temporary public identity query failed")
        fp = identity.stdout.decode().split("device ", 1)[-1].strip().splitlines()[0]
        require(fp and " " not in fp, "Invalid temporary public identity")
        node = subprocess.Popen(node_args, env=env, cwd=work, stdout=logs[1], stderr=logs[1])
        watchdog = threading.Timer(max(1, DEADLINE-(time.monotonic()-started)), lambda: stop(node))
        watchdog.daemon = True; watchdog.start()
        controller = WS(endpoint_port, "ctl", root_token)
        controller.send({"k": "hello", "fp": "complete-live-" + secrets.token_hex(8), "relay": True})
        status = controller.until(lambda m: m.get("k") == "status" and any(r.get("fp") == fp and r.get("nid", "").startswith("n") for r in status_rows(m)))
        device = next(r for r in status_rows(status) if r["fp"] == fp)
        require(device.get("state") == "pending", "Fresh temporary identity unexpectedly already approved")
        controller.send({"k": "approve", "nid": device["nid"], "relay": True, "ttl": DEADLINE + 30})
        controller.until(lambda m: m.get("k") == "paired")
        controller.until(lambda m: m.get("k") == "dc" and json.loads(base64.b64decode(m["d"])).get("k") == "node-hello")
        remaining = DEADLINE - (time.monotonic()-started)
        require(remaining > 1, "Trial deadline exhausted before dispatch")
        out, events, live = run_frames(controller, "complete-live-once", prompt, transcript, timeout=remaining,
                                       model=args.model, protocol="openai-completions")
        calls = [e for e in events if e.get("type") == "model_call"]
        result["model_call_receipts"] = [{k: e.get(k) for k in ("phase", "call", "status", "usage")} for e in calls]
        require(calls and len(calls) <= MAX_CALLS and max(e["call"] for e in calls) <= MAX_CALLS, "Call receipts exceeded bounded trial")
        result["terminal"] = {"code": out.get("code", 0), "has_error": bool(out.get("err"))}
        verify_tool_contract(events, work, compile_command, system_command)
        passed.append("exact_seven_tool_operations_reviewed_commands_paths_and_completed_pairs")
        # Inspect known effects before judging the model terminal outcome.
        require(digest(helper) == helper_hash, "Reviewed helper was modified")
        result["rust_source"] = source_receipt((work / "complete.rs").read_bytes(), rust_source)
        require((work / "complete.exe").read_bytes().startswith(b"MZ"), "Compiled Windows executable missing")
        require((work / "program.log").read_text().strip() == marker, "Actual generated program log marker mismatch")
        ends = [e for e in events if e.get("type") == "tool_end"]
        starts = [e for e in events if e.get("type") == "tool_start"]
        require(any(e.get("title") == "exec" and stdout_marker in e.get("output", "") for e in ends), "Compiled program stdout receipt missing")
        require(any(e.get("title") == "read_file" and marker in e.get("output", "") for e in ends), "Actual program log was not read back")
        passed.append("actual_cloud_rust_write_compile_execute_and_log_readback")
        require((work / "jq.exe").stat().st_size == JQ_BYTES and digest(work / "jq.exe") == JQ_SHA256, "Official jq artifact hash/size mismatch")
        require(any(e.get("title") == "download_file" and JQ_URL in e.get("detail", "") for e in starts), "Official jq download_file receipt missing")
        require(any(e.get("title") == "exec" and "JQ_VERIFIED=jq-1.8.1" in e.get("output", "") for e in ends), "Checksum-gated jq execution receipt missing")
        passed.append("official_pinned_jq_download_checksum_before_execution_and_version")
        system = json.loads((work / "system-receipt.json").read_text(encoding="utf-8"))
        observed = system["system"]
        receipt_hash = digest(work / "system-receipt.json")
        require(any(e.get("title") == "exec" and "SYSTEM_RECEIPT_SHA256="+receipt_hash in e.get("output", "") for e in ends), "System receipt was not produced by unchanged verified helper")
        require(any(e.get("title") == "read_file" and "jq_sha256" in e.get("output", "") for e in ends), "Actual System receipt was not read by the model")
        require(system["jq_sha256"] == JQ_SHA256 and system["jq_version"] == "jq-1.8.1", "Tool execution receipt mismatched")
        require(observed["log_name"] == "System" and observed["max_events"] == 5 and observed["message_char_limit"] == 240,
                "System query bounds changed")
        rows = observed["events"]
        require(isinstance(rows, list) and 0 <= len(rows) <= 5 and observed["event_count"] == len(rows), "System event count is invalid")
        require(all(isinstance(r["id"], int) and isinstance(r["message"], str) and len(r["message"]) <= 240 for r in rows), "System event metadata exceeded bounds")
        result["system_query"] = {"status": observed["status"], "event_count": len(rows), "receipt_sha256": receipt_hash}
        require(observed["status"] in ("ok", "empty"), "System query failed or remained unknown; earlier artifacts retained")
        require((observed["status"] == "empty") == (not rows), "System empty status contradicts actual rows")
        passed.append("actual_bounded_read_only_system_query_and_model_readback")
        report = json.loads((work / "analysis.json").read_text(encoding="utf-8"))
        require(report["program_marker"] == marker and report["jq_version"] == "jq-1.8.1", "Final analysis lost actual program/tool outcomes")
        require(report["system_status"] == observed["status"] and report["system_event_count"] == len(rows)
                and report["system_event_ids"] == [r["id"] for r in rows], "Final analysis disagrees with actual System receipt")
        require(isinstance(report["analysis"], str) and len(report["analysis"].strip()) >= 10, "Final analysis is absent")
        passed.append("model_written_analysis_matches_real_artifacts_and_system_receipt")
        require(not out.get("err") and out.get("code", 0) == 0, "Model run failed after observed effects; no automatic replay")
        require(live, "No live event observed before the terminal result")
        require(all(e.get("status") == "completed" for e in ends), "A model tool failed during the run")
        require(time.monotonic()-started <= DEADLINE, "Work exceeded the 180-second overall deadline")
        result.update(status="PASS", usage_boundary="Provider-call reservations are observed; missing supplier usage is unknown, not zero.")
    except Exception as error:
        # Avoid propagating remote bodies, command output or private credentials.
        result["error"] = {"type": type(error).__name__, "safe_reason": str(error)[:240] if isinstance(error, AssertionError) else "Inspect retained private fixture artifacts; no automatic replay"}
    finally:
        result["work_elapsed_seconds"] = round(time.monotonic()-started,3)
        cleanup_started = time.monotonic()
        cleanup_errors = []
        if watchdog: watchdog.cancel()
        try:
            stop(node)
            result["temporary_node_stopped"] = node is None or node.poll() is not None
            if not result["temporary_node_stopped"]: cleanup_errors.append("temporary_node_stop_unconfirmed")
        except Exception as error:
            result["temporary_node_stopped"] = False
            cleanup_errors.append("temporary_node_stop:"+type(error).__name__)
        if controller and device:
            try:
                controller.send({"k": "revoke", "fp": device["fp"]})
                controller.until(lambda m: m.get("k") == "status" and not any(r.get("fp") == device["fp"]
                    and r.get("state") in ("approved", "paired", "approved-offline") for r in status_rows(m)), timeout=5)
                result["temporary_approval_revoked"] = True
            except Exception as error:
                result["temporary_approval_revoked"] = False
                cleanup_errors.append("temporary_revoke:"+type(error).__name__)
        if controller:
            try: controller.close()
            except Exception as error: cleanup_errors.append("controller_close:"+type(error).__name__)
        try:
            stop(tunnel)
            result["private_tunnel_stopped"] = tunnel is None or tunnel.poll() is not None
            if not result["private_tunnel_stopped"]: cleanup_errors.append("private_tunnel_stop_unconfirmed")
        except Exception as error:
            result["private_tunnel_stopped"] = False
            cleanup_errors.append("private_tunnel_stop:"+type(error).__name__)
        for log in logs:
            try: log.close()
            except Exception as error: cleanup_errors.append("log_close:"+type(error).__name__)
        if device and not result.get("temporary_approval_revoked"):
            result["status"] = "FAIL"
            result["cleanup_failure"] = "Temporary approval revocation unconfirmed; bounded TTL remains"
        if cleanup_errors:
            result["status"] = "FAIL"
        result["cleanup_errors"] = cleanup_errors
        result["cleanup_seconds"] = round(time.monotonic()-cleanup_started,3)
        result["deadline_boundary"] = "Work and model execution are bounded to 180 seconds; bounded cleanup is recorded separately"
        result["elapsed_seconds"] = round(time.monotonic()-started,3)
        result["artifacts"] = {p.name: {"bytes": p.stat().st_size, "sha256": digest(p)} for p in work.iterdir() if p.is_file()}
        (root / "result.json").write_text(json.dumps(result,indent=2),encoding="utf-8")
        (root / "controller.json").write_text(json.dumps(transcript,indent=2),encoding="utf-8")
        print(json.dumps({"status":result["status"],"passed":passed,"elapsed_seconds":result["elapsed_seconds"],"evidence":str(root)},indent=2))
    require(result["status"] == "PASS", "Complete live acceptance failed; inspect retained effects before any separately authorized new trial")


if __name__ == "__main__":
    main()
