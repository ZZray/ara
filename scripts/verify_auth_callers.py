"""Run native AuthStorage usage/health/check/reset groups and the reference Host;
--full adds one backend gate.

Controlled SQLite/loopback evidence does not consume a live saved reset or
establish complete native Host controllers, Broker or account acceptance.
"""
import hashlib
import os
import sys
import verify_auth_storage as inherited

if sys.platform == "win32":
    # Native parser debug links are memory-heavy. Respect an explicit Host
    # override, otherwise keep this module's one-click gate at one build job.
    os.environ.setdefault("CARGO_BUILD_JOBS", "1")

shared = inherited.shared
shared.__doc__ = __doc__
shared.DEFAULT_OUTPUT = shared.Path(shared.tempfile.gettempdir()) / "ara-auth-callers-batch"
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
    path = shared.ROOT / "scripts/verify_auth_callers.py"
    pins[path.relative_to(shared.ROOT).as_posix()] = hashlib.sha256(path.read_bytes()).hexdigest()
    return pins


shared.source_pins = source_pins

if __name__ == "__main__":
    raise SystemExit(shared.main())
