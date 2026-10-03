"""Run saved-reset planning, durable operation and actual Host module families.

Use --module during development; --full adds one frozen workspace gate.
Controlled endpoints do not spend a live saved reset or establish full account,
TUI/ACP, remaining recovery consumers or provider acceptance.
"""
import hashlib
import verify_broker_consumers as inherited

shared = inherited.shared
shared.__doc__ = __doc__
shared.DEFAULT_OUTPUT = shared.Path(shared.tempfile.gettempdir()) / "ara-reset-controller-batch"
shared.MODULES = {
    "storage": ("ara-cli", "@lib"),
    "planner": ("ara-cli", "codex_auto_reset"),
    "settings": ("ara-cli", "@bin:ara"),
    "requests": ("ara-cli", "model_route"),
    "controllers": ("ara-cli", "codex_reset_controller"),
    "host": ("ara-cli", "openai_daily_cli"),
    "rpc-observed": ("ara-cli", "broker_consumers_rpc"),
}
original_source_pins = shared.source_pins


def source_pins():
    pins = original_source_pins()
    path = shared.ROOT / "scripts/verify_reset_controllers.py"
    pins[path.relative_to(shared.ROOT).as_posix()] = hashlib.sha256(path.read_bytes()).hexdigest()
    return pins


shared.source_pins = source_pins

if __name__ == "__main__":
    raise SystemExit(shared.main())
