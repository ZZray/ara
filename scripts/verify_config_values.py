"""Run fixed OMP config-command module families; --full adds one backend gate.

Uses original command/header input families and actual controlled Rust CLI
processes. Private local configuration and bounded live tasks are separate.
"""
import hashlib
import verify_openai_daily as shared

shared.__doc__ = __doc__
shared.DEFAULT_OUTPUT = shared.Path(shared.tempfile.gettempdir()) / "ara-regauth-batch"
shared.MODULES = {
    "values": ("ara-cli", "model_config_values"),
    "selection": ("ara-cli", "daily_model_config"),
    "route": ("ara-cli", "model_route"),
    "host": ("ara-cli", "config_command_cli"),
    "daily-cli": ("ara-cli", "openai_daily_cli"),
    "promotion": ("ara-cli", "rpc_compaction"),
}


def source_pins():
    paths = [shared.ROOT / name for name in (
        "Cargo.toml", "Cargo.lock", "scripts/verify_config_values.py",
        "scripts/verify_openai_daily.py", "scripts/verify_backend.py",
    )]
    for package in ("ara-cli", "ara-ai", "ara-agent", "ara-session", "ara-snapcompact"):
        paths.extend((shared.ROOT / "crates" / package).rglob("*"))
    return {
        path.relative_to(shared.ROOT).as_posix(): hashlib.sha256(path.read_bytes()).hexdigest()
        for path in sorted(paths) if path.is_file()
    }


shared.source_pins = source_pins

if __name__ == "__main__":
    raise SystemExit(shared.main())
