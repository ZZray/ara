"""Run native tokenizer and its actual Host module families.

Reuse fixed OMP encoders, scanners, golden inputs and differential oracles.
Use --module during development; --full adds the shared backend gate.
Private configuration and bounded live tasks have separate Root-only receipts.
"""
import hashlib
import verify_openai_daily as shared

shared.__doc__ = __doc__
shared.DEFAULT_OUTPUT = shared.Path(shared.tempfile.gettempdir()) / "ara-native-tokenizer-batch"
shared.MODULES = {
    "native": ("ara-ctok", "@lib"),
    "claude-budget": ("ara-ctok", "claude_budget"),
    "model": ("ara-ai", "@lib"),
    "agent": ("ara-agent", "tokenizer"),
    "selection": ("ara-cli", "daily_model_config"),
    "host": ("ara-cli", "openai_daily_cli"),
}


def source_pins():
    paths = [shared.ROOT / name for name in (
        "Cargo.toml", "Cargo.lock", "THIRD_PARTY_NOTICES.md",
        "scripts/verify_native_tokenizer.py", "scripts/verify_openai_daily.py",
        "scripts/verify_backend.py",
    )]
    for package in ("ara-ctok", "ara-ai", "ara-agent", "ara-cli", "ara-session", "ara-snapcompact"):
        paths.extend((shared.ROOT / "crates" / package).rglob("*"))
    return {
        path.relative_to(shared.ROOT).as_posix(): hashlib.sha256(path.read_bytes()).hexdigest()
        for path in sorted(paths) if path.is_file()
    }


shared.source_pins = source_pins

if __name__ == "__main__":
    raise SystemExit(shared.main())
