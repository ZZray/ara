"""Run remote compaction module families; --full shares one final backend gate.

This deterministic runner never reads private configuration or calls a real
model. Actual CAS/native account trials retain separate bounded receipts.
"""
import hashlib
import verify_openai_daily as shared

shared.__doc__ = __doc__
shared.DEFAULT_OUTPUT = shared.Path(shared.tempfile.gettempdir()) / "ara-remote-batch"
shared.MODULES = {
    "wire": ("ara-ai", "remote_compaction"),
    "session": ("ara-session", "remote_compaction"),
    "prepare": ("ara-cli", "remote_compaction"),
    "config": ("ara-cli", "daily_model_config"),
    "route": ("ara-cli", "model_route"),
    "host": ("ara-cli", "rpc_compaction"),
    "repl": ("ara-cli", "e2e"),
    "account-wire": ("ara-ai", "openai_codex_http"),
    "account-cli": ("ara-cli", "openai_daily_cli"),
}


def source_pins():
    """Pin all four participating crates, including new untracked sources."""
    paths = [shared.ROOT / name for name in (
        "Cargo.toml", "Cargo.lock", "scripts/verify_remote.py",
        "scripts/verify_openai_daily.py", "scripts/verify_backend.py",
    )]
    for package in ("ara-agent", "ara-ai", "ara-session", "ara-cli"):
        paths.extend((shared.ROOT / "crates" / package).rglob("*"))
    return {
        path.relative_to(shared.ROOT).as_posix(): hashlib.sha256(path.read_bytes()).hexdigest()
        for path in sorted(paths) if path.is_file()
    }


shared.source_pins = source_pins

if __name__ == "__main__":
    raise SystemExit(shared.main())
