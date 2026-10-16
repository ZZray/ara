"""Bounded real Rust binary -> real Go Hub identity/reconnect acceptance.

Uses isolated fake credentials and effects; never connects to production or calls
a model. Negative cryptographic variants are exercised by Go real-WS tests.
"""
import argparse
import base64
import hashlib
import json
from pathlib import Path
import tempfile
import threading

from verify_context import Host
from verify_host import WS, require, status_rows, stop


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--binary", type=Path, required=True)
    parser.add_argument("--hub-binary", type=Path, required=True)
    parser.add_argument("--output", type=Path)
    args = parser.parse_args()
    root = (args.output or Path(tempfile.mkdtemp(prefix="hw-identity-"))).resolve()
    root.mkdir(parents=True, exist_ok=True)
    require(not any(root.iterdir()), "Use a fresh isolated evidence directory")
    host = Host(args, root, owner="identity-authority")
    result = {"passed": [], "scope": "actual Rust/Go binaries, isolated loopback only; no production or model calls"}
    watchdog = threading.Timer(70, lambda: (stop(host.node), stop(host.hub)))
    watchdog.daemon = True
    watchdog.start()
    try:
        host.start()
        fp = host.device["fp"]
        identity = (host.state / "identity.key").read_bytes()
        approvals = json.loads((root / "hub-conf" / "approvals.json").read_text())
        pin = approvals["approvals"][fp]["node_public_key"]
        public_key = base64.b64decode(pin, validate=True)
        require(len(public_key) == 32 and base64.b32encode(public_key[:8]).decode().rstrip("=") == fp,
                "Hub approval pin is not the full key corresponding to the preserved display FP")
        require(approvals["approvals"][fp]["ctlfp"] == host.owner,
                "Approval owner differs from authenticated controller identity")
        result["passed"].append("real_rust_ed25519_socket_proof_explicit_approval_full_key_pin")

        host.controller.send({"k": "status"})
        status = host.controller.until(lambda m: m.get("k") == "status" and any(r.get("fp") == fp for r in status_rows(m)))
        original_nid = next(r["nid"] for r in status_rows(status) if r["fp"] == fp)
        attacker = WS(host.hub_port, "node", "controlled-context-enrollment")
        try:
            challenge = attacker.receive()
            require(challenge.get("k") == "node-challenge", "Hub did not require socket-specific proof")
            attacker.send({"k": "join", "fp": fp, "info": {"hostname": "unsigned-impostor"}})
            rejected = False
            try:
                attacker.receive(3)
            except (ConnectionError, OSError):
                rejected = True
            require(rejected, "Unsigned admitted node was not rejected")
        finally:
            attacker.sock.close()
        host.controller.send({"k": "status"})
        packet = host.controller.until(lambda m: m.get("k") == "status" and any(r.get("fp") == fp for r in status_rows(m)))
        row = next(r for r in status_rows(packet) if r["fp"] == fp)
        require(row["nid"] == original_nid and row["state"] == "paired",
                "Unsigned impersonation displaced the actual Rust node or its pairing")
        result["passed"].append("unsigned_victim_fp_join_cannot_replace_actual_rust_node")

        host.restart()
        require((host.state / "identity.key").read_bytes() == identity, "Rust reconnect regenerated identity")
        after = json.loads((root / "hub-conf" / "approvals.json").read_text())
        require(after["approvals"][fp]["node_public_key"] == pin, "Reconnect changed approved full key")
        result["passed"].append("real_rust_restart_fresh_socket_proof_same_seed_fp_and_pin")

        host.controller.sock.close()
        host.controller = None
        stop(host.node)
        stop(host.hub)
        host.start()
        require(host.device["fp"] == fp and (host.state / "identity.key").read_bytes() == identity,
                "Hub restart changed Node identity")
        final = json.loads((root / "hub-conf" / "approvals.json").read_text())
        require(final["approvals"][fp]["node_public_key"] == pin, "Hub restart failed to retain full key approval")
        require(not host.fixture.requests, "Identity acceptance unexpectedly contacted a model")
        result["passed"].append("real_hub_restart_retains_pin_and_reauthenticates_rust_without_reapproval_or_model_calls")
        result.update(ok=True, binary_sha256=hashlib.sha256(args.binary.read_bytes()).hexdigest(),
                      hub_sha256=hashlib.sha256(args.hub_binary.read_bytes()).hexdigest())
    except BaseException as error:
        result.update(ok=False, error=str(error))
        raise
    finally:
        host.close()
        watchdog.cancel()
        (root / "result.json").write_text(json.dumps(result, ensure_ascii=False, indent=2), encoding="utf-8")
        print(json.dumps(result, ensure_ascii=False))


if __name__ == "__main__":
    main()
