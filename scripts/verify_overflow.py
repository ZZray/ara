"""Run native overflow recovery modules, with one optional final shared gate."""
import verify_openai_daily as shared

shared.__doc__ = __doc__
shared.DEFAULT_OUTPUT = shared.Path(shared.tempfile.gettempdir()) / "ara-overflow"
shared.MODULES = {
    "config": ("ara-cli", "daily_model_config"),
    "session": ("ara-session", "retry_recovery"),
    "host": ("ara-cli", "rpc_compaction"),
}

if __name__ == "__main__":
    raise SystemExit(shared.main())
