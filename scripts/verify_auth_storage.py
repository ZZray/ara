"""Run fixed OMP AuthStorage families and the real controlled reference Host.

Use --module during development; --full adds one stable workspace gate.
The inherited runner records source hashes and commands without private data.
"""
import verify_config_values as inherited

shared = inherited.shared
shared.__doc__ = __doc__
shared.DEFAULT_OUTPUT = shared.Path(shared.tempfile.gettempdir()) / "ara-authstorage-batch"
shared.MODULES = {
    "storage": ("ara-cli", "@lib"),
    "accounts": ("ara-cli", "openai_codex_auth"),
    "requests": ("ara-cli", "model_route"),
    "selection": ("ara-cli", "daily_model_config"),
    "host": ("ara-cli", "openai_daily_cli"),
    "registry": ("ara-cli", "model_registry"),
    "helpers": ("ara-cli", "config_command_cli"),
}

original_source_pins = inherited.source_pins


def source_pins():
    pins = original_source_pins()
    path = shared.ROOT / "scripts/verify_auth_storage.py"
    pins[path.relative_to(shared.ROOT).as_posix()] = inherited.hashlib.sha256(path.read_bytes()).hexdigest()
    return pins


shared.source_pins = source_pins

if __name__ == "__main__":
    raise SystemExit(shared.main())
