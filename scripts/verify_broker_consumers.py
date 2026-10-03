"""Run shared credential consumers and their actual CLI/RPC process families.

Use --module during development; --full adds one stable workspace gate.
Synthetic device/Broker fixtures do not establish real account authorization.
"""
import hashlib
import verify_broker_store as inherited

shared = inherited.shared
shared.__doc__ = __doc__
shared.DEFAULT_OUTPUT = shared.Path(shared.tempfile.gettempdir()) / "ara-broker-consumers-batch"
shared.MODULES = {
    "storage": ("ara-cli", "@lib"),
    "accounts": ("ara-cli", "openai_codex_auth"),
    "requests": ("ara-cli", "model_route"),
    "host": ("ara-cli", "openai_daily_cli"),
    "auth-command": ("ara-cli", "broker_consumers_cli"),
    "rpc-observed": ("ara-cli", "broker_consumers_rpc"),
}
original_source_pins = shared.source_pins


def source_pins():
    pins = original_source_pins()
    path = shared.ROOT / "scripts/verify_broker_consumers.py"
    pins[path.relative_to(shared.ROOT).as_posix()] = hashlib.sha256(path.read_bytes()).hexdigest()
    return pins


shared.source_pins = source_pins

if __name__ == "__main__":
    raise SystemExit(shared.main())
