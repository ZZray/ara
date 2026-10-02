"""Run local reducer modules; --full adds the one stable shared backend gate."""
import verify_openai_daily as shared

shared.__doc__ = __doc__
shared.DEFAULT_OUTPUT = shared.Path(shared.tempfile.gettempdir()) / "ara-local-reducers-batch"
shared.MODULES = {
    "core": ("ara-agent", "local_reduction"),
    "session": ("ara-session", "local_reduction"),
    "adapter": ("ara-cli", "local_reduction"),
    "cut": ("ara-agent", "compaction_cut"),
    "projection": ("ara-session", "compaction_projected"),
    "notice": ("ara-session", "loop_guard_notice"),
    "terminal": ("ara-session", "retry_recovery"),
    "host": ("ara-cli", "rpc_compaction"),
    "repl": ("ara-cli", "e2e"),
}

if __name__ == "__main__":
    raise SystemExit(shared.main())
