"""Run native compaction/terminal modules, with one optional final shared gate."""
import verify_openai_daily as shared

shared.__doc__ = __doc__
shared.DEFAULT_OUTPUT = shared.Path(shared.tempfile.gettempdir()) / "ara-native-compaction-terminal"
shared.MODULES = {
    "cut": ("ara-agent", "compaction_cut"),
    "summary": ("ara-agent", "compaction_call"),
    "projection": ("ara-session", "compaction_projected"),
    "reload": ("ara-session", "compaction_projection"),
    "context": ("ara-session", "model_context"),
    "terminal": ("ara-session", "retry_recovery"),
    "host": ("ara-cli", "rpc_compaction"),
    "repl": ("ara-cli", "e2e"),
    "account": ("ara-cli", "openai_daily_cli"),
}

if __name__ == "__main__":
    raise SystemExit(shared.main())
