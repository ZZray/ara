"""Run fixed OMP snapcompact module families; --full adds one backend gate.

Examples: python scripts/verify_snapcompact.py --module core
          python scripts/verify_snapcompact.py --full
Private configuration and real-model trials are separate bounded acceptance.
"""
import hashlib
import verify_openai_daily as shared

shared.__doc__ = __doc__
shared.DEFAULT_OUTPUT = shared.Path(shared.tempfile.gettempdir()) / "ara-snap-batch"
shared.MODULES = {
    "core": ("ara-snapcompact", "@lib"),
    "core-families": ("ara-snapcompact", "upstream_families"),
    "session": ("ara-session", "snapcompact"),
    "session-migration": ("ara-session", "compaction_projected"),
    "frame-count": ("ara-agent", "tokenizer"),
    "image-wire": ("ara-ai", "openai_http"),
    "budget-rescue": ("ara-cli", "snapcompact"),
    "host": ("ara-cli", "rpc_compaction"),
    "repl": ("ara-cli", "e2e"),
}


def source_pins():
    paths = [shared.ROOT / name for name in (
        "Cargo.toml", "Cargo.lock", "scripts/verify_snapcompact.py",
        "scripts/verify_openai_daily.py", "scripts/verify_backend.py",
    )]
    for package in ("ara-snapcompact", "ara-agent", "ara-ai", "ara-session", "ara-cli"):
        paths.extend((shared.ROOT / "crates" / package).rglob("*"))
    return {
        path.relative_to(shared.ROOT).as_posix(): hashlib.sha256(path.read_bytes()).hexdigest()
        for path in sorted(paths) if path.is_file()
    }


shared.source_pins = source_pins

if __name__ == "__main__":
    raise SystemExit(shared.main())
