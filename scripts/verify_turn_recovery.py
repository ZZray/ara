"""Run native turn-recovery modules with one optional shared final gate."""
import verify_openai_daily as shared

shared.__doc__ = __doc__
shared.DEFAULT_OUTPUT = shared.Path(shared.tempfile.gettempdir()) / "ara-turn-recovery"
shared.MODULES = {
    "responses": ("ara-ai", "openai_responses_http"),
    "host": ("ara-cli", "rpc_retry"),
}

if __name__ == "__main__":
    raise SystemExit(shared.main())
