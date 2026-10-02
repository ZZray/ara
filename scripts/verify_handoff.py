"""Run handoff module families; --full shares one final backend gate."""
import verify_openai_daily as shared

shared.__doc__ = __doc__
shared.DEFAULT_OUTPUT = shared.Path(shared.tempfile.gettempdir()) / "ara-handoff-batch"
shared.MODULES = {
    "core": ("ara-agent", "handoff"),
    "session": ("ara-session", "handoff"),
    "projection": ("ara-session", "compaction_projected"),
    "route": ("ara-cli", "model_route"),
    "host": ("ara-cli", "rpc_compaction"),
    "repl": ("ara-cli", "e2e"),
}

if __name__ == "__main__":
    raise SystemExit(shared.main())
