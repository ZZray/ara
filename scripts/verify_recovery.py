"""Run fixed OMP recovery modules and, with --full, one shared backend gate.

Reuse the existing module runner and bounded RPC harness. Credentials and live
model trials remain outside this deterministic command.
"""
import verify_openai_daily as shared

shared.__doc__ = __doc__
shared.DEFAULT_OUTPUT = shared.Path(shared.tempfile.gettempdir()) / "ara-recovery"
shared.MODULES = {
    "ai": ("ara-ai", "thinking_loop"),
    "host": ("ara-cli", "rpc_retry"),
    "config": ("ara-cli", "daily_model_config"),
    "session": ("ara-session", "loop_guard_notice"),
}

if __name__ == "__main__":
    raise SystemExit(shared.main())
