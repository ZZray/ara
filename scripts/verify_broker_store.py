"""Run the complete broker owner and its actual Rust CLI consumer groups.

Use --module during development; --full adds one stable comprehensive gate.
Loopback broker faults do not prove a real broker/account or deferred Provider.
"""
import hashlib
import verify_auth_callers as inherited

shared = inherited.shared
shared.__doc__ = __doc__
shared.DEFAULT_OUTPUT = shared.Path(shared.tempfile.gettempdir()) / "ara-broker-store-batch"
shared.MODULES = {
    "storage": ("ara-cli", "@lib"),
    "accounts": ("ara-cli", "openai_codex_auth"),
    "requests": ("ara-cli", "model_route"),
    "registry": ("ara-cli", "model_registry"),
    "host": ("ara-cli", "openai_daily_cli"),
}
original_source_pins = shared.source_pins


def source_pins():
    pins = original_source_pins()
    path = shared.ROOT / "scripts/verify_broker_store.py"
    pins[path.relative_to(shared.ROOT).as_posix()] = hashlib.sha256(path.read_bytes()).hexdigest()
    return pins


shared.source_pins = source_pins

if __name__ == "__main__":
    raise SystemExit(shared.main())
