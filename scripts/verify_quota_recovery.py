"""Run HTTP retry hints and same-Session quota recovery by module.

Use --module during development and --full once on a frozen batch. Synthetic
account/Broker endpoints verify attribution, replay and owned settlement; they
do not spend a live reset or establish complete Provider/Host parity.
"""
import hashlib
import verify_reset_controllers as inherited

shared = inherited.shared
shared.__doc__ = __doc__
shared.DEFAULT_OUTPUT = shared.Path(shared.tempfile.gettempdir()) / "ara-quota-recovery-batch"
shared.MODULES = {
    "ai": ("ara-ai", "@lib"),
    "http-hints": ("ara-ai", "retry_http"),
    "chat": ("ara-ai", "openai_http"),
    "responses": ("ara-ai", "openai_responses_http"),
    "codex": ("ara-ai", "openai_codex_http"),
    "storage": ("ara-cli", "@lib"),
    "session": ("ara-cli", "@bin:ara"),
    "requests": ("ara-cli", "model_route"),
    "controllers": ("ara-cli", "codex_reset_controller"),
    "rpc-retry": ("ara-cli", "rpc_retry"),
    "broker": ("ara-cli", "broker_consumers_rpc"),
}
original_source_pins = shared.source_pins


def source_pins():
    pins = original_source_pins()
    path = shared.ROOT / "scripts/verify_quota_recovery.py"
    pins[path.relative_to(shared.ROOT).as_posix()] = hashlib.sha256(path.read_bytes()).hexdigest()
    return pins


shared.source_pins = source_pins

if __name__ == "__main__":
    raise SystemExit(shared.main())
