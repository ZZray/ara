"""Run fixed tool-loop guard modules, with one optional shared final gate."""
import verify_openai_daily as shared

shared.__doc__ = __doc__
shared.DEFAULT_OUTPUT = shared.Path(shared.tempfile.gettempdir()) / "ara-tool-loop"
shared.MODULES = {
    "ai": ("ara-ai", "tool_call_loop_guard"),
    "host": ("ara-cli", "rpc_retry"),
    "session": ("ara-session", "loop_guard_notice"),
}

if __name__ == "__main__":
    raise SystemExit(shared.main())
