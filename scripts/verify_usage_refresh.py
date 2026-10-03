"""Run fixed OMP usage refresh/cache families; --full adds one backend gate.

Real SQLite and loopback HTTP receipts exercise the shared Host auth owner.
Native source inputs remain separate from actual OpenAI account acceptance.
"""
import hashlib
import verify_auth_storage as inherited

shared = inherited.shared
shared.__doc__ = __doc__
shared.DEFAULT_OUTPUT = shared.Path(shared.tempfile.gettempdir()) / "ara-usage-refresh-batch"
shared.MODULES = {
    "storage": ("ara-cli", "@lib"),
    "accounts": ("ara-cli", "openai_codex_auth"),
    "host": ("ara-cli", "openai_daily_cli"),
}
original_source_pins = shared.source_pins


def source_pins():
    pins = original_source_pins()
    path = shared.ROOT / "scripts/verify_usage_refresh.py"
    pins[path.relative_to(shared.ROOT).as_posix()] = hashlib.sha256(path.read_bytes()).hexdigest()
    return pins


shared.source_pins = source_pins

if __name__ == "__main__":
    raise SystemExit(shared.main())
