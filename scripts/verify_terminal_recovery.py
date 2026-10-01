"""Run native terminal recovery, promotion and persistence modules, plus one optional shared gate."""
import verify_openai_daily as shared

shared.__doc__ = __doc__
shared.DEFAULT_OUTPUT = shared.Path(shared.tempfile.gettempdir()) / "ara-terminal-recovery"
shared.MODULES = {
    "provider": ("ara-ai", "openai_responses_http"),
    "summary": ("ara-agent", "compaction_call"),
    "session": ("ara-session", "retry_recovery"),
    "config": ("ara-cli", "daily_model_config"),
    "host": ("ara-cli", "rpc_compaction"),
}

if __name__ == "__main__":
    raise SystemExit(shared.main())
