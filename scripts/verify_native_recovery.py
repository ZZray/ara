"""Run native Responses recovery and summary folding modules, with one optional shared gate."""
import verify_openai_daily as shared

shared.__doc__ = __doc__
shared.DEFAULT_OUTPUT = shared.Path(shared.tempfile.gettempdir()) / "ara-native-recovery"
shared.MODULES = {
    "provider": ("ara-ai", "openai_responses_http"),
    "summary": ("ara-agent", "compaction_call"),
    "host": ("ara-cli", "rpc_compaction"),
}

if __name__ == "__main__":
    raise SystemExit(shared.main())
