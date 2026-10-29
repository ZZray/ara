"""Run Broker server, management and existing consumers in one compiled batch.

Controlled HTTP/device/MCP endpoints do not prove real account authorization.
"""
import hashlib
import verify_broker_consumers as inherited

shared = inherited.shared
shared.__doc__ = __doc__
shared.DEFAULT_OUTPUT = shared.Path(shared.tempfile.gettempdir()) / "ara-broker-server-batch"
shared.MODULES = {
    "storage": ("ara-cli", "@lib"),
    "server": ("ara-cli", "auth_broker_server"),
    "management": ("ara-cli", "auth_broker_management"),
    "accounts": ("ara-cli", "openai_codex_auth"),
    "auth-command": ("ara-cli", "broker_consumers_cli"),
    "rpc-observed": ("ara-cli", "broker_consumers_rpc"),
}
original_source_pins = shared.source_pins

def source_pins():
    pins = original_source_pins()
    path = shared.ROOT / "scripts/verify_broker_server.py"
    pins[path.relative_to(shared.ROOT).as_posix()] = hashlib.sha256(path.read_bytes()).hexdigest()
    return pins

shared.source_pins = source_pins

if __name__ == "__main__":
    raise SystemExit(shared.main())
