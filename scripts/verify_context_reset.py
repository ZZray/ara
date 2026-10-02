"""Run Session/context-reset modules and one optional final shared backend gate."""
import verify_openai_daily as shared

shared.__doc__ = __doc__
shared.DEFAULT_OUTPUT = shared.Path(shared.tempfile.gettempdir()) / "ara-context-reset"
shared.MODULES = {
    "source": ("ara-session", "compaction_source"),
    "projection": ("ara-session", "compaction_projected"),
    "reload": ("ara-session", "compaction_projection"),
    "context": ("ara-session", "model_context"),
    "recovery": ("ara-session", "retry_recovery"),
    "repl": ("ara-cli", "e2e"),
    "account": ("ara-cli", "openai_daily_cli"),
}

if __name__ == "__main__":
    raise SystemExit(shared.main())
