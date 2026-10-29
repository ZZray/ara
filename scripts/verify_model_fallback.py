"""Run model fallback module families; --full adds one final shared gate.

Uses the existing deterministic runner and process/HTTP families. Auto,
reserve and restore acceptance must be checked against their actual consumers;
a passing helper or subset does not accept the complete fallback surface.
"""
import hashlib
import argparse
import sys

import verify_config_values as inherited

shared = inherited.shared
shared.__doc__ = __doc__
shared.DEFAULT_OUTPUT = shared.Path(shared.tempfile.gettempdir()) / "ara-model-fallback-batch"
shared.MODULES = {
    "chat": ("ara-ai", "openai_http"),
    "responses": ("ara-ai", "openai_responses_http"),
    "codex": ("ara-ai", "openai_codex_http"),
    "ai": ("ara-ai", "@lib"),
    "journal": ("ara-session", "journal"),
    "session": ("ara-session", "@lib"),
    "settings": ("ara-cli", "@bin:ara"),
    "model-controls": ("ara-cli", "@lib"),
    "registry": ("ara-cli", "model_registry"),
    "selection": ("ara-cli", "daily_model_config"),
    "retry": ("ara-cli", "rpc_retry"),
    "promotion": ("ara-cli", "rpc_compaction"),
    "core-loop": ("ara-agent", "agent_loop"),
    "core-live": ("ara-agent", "live_tools"),
    "account-host": ("ara-cli", "codex_reset_controller"),
    "rpc-host": ("ara-cli", "rpc_host"),
}

original_source_pins = inherited.source_pins


def source_pins():
    pins = original_source_pins()
    path = shared.ROOT / "scripts/verify_model_fallback.py"
    pins[path.relative_to(shared.ROOT).as_posix()] = hashlib.sha256(path.read_bytes()).hexdigest()
    return pins


shared.source_pins = source_pins

if __name__ == "__main__":
    parser = argparse.ArgumentParser(add_help=False)
    parser.add_argument("--consumers-only", action="store_true", help="Reuse unchanged AI/Session evidence")
    options, remaining = parser.parse_known_args()
    if options.consumers_only:
        shared.MODULES = {name: target for name, target in shared.MODULES.items() if target[0] in ("ara-agent", "ara-cli")}
    sys.argv[1:] = remaining
    raise SystemExit(shared.main())
