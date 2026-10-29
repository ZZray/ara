"""Run model edit/rebinding and freeform protocol modules in one compiled batch.

--module selects a family; --full adds the existing final comprehensive gate.
This runner collects independent failures and never reads local credentials.
Real CAS/account trials and complete OMP surface acceptance remain separate.
"""
import hashlib
import sys

import verify_model_fallback as inherited

shared = inherited.shared
shared.__doc__ = __doc__
shared.DEFAULT_OUTPUT = shared.Path(shared.tempfile.gettempdir()) / "ara-model-tools-batch"
shared.MODULES = {
    "edit-policy": ("ara-cli", "edit_mode_policy"),
    "tool-snapshot": ("ara-tools", "model_edit_snapshot"),
    "edit-engine": ("ara-tools", "edit"),
    "host-uris": ("ara-tools", "host_uris"),
    "settings": ("ara-cli", "@bin:ara"),
    "model-controls": ("ara-cli", "@lib"),
    "registry": ("ara-cli", "model_registry"),
    "loader": ("ara-cli", "model_registry_loader"),
    "static-registry": ("ara-cli", "static_model_registry"),
    "scope": ("ara-cli", "model_scope"),
    "selection": ("ara-cli", "daily_model_config"),
    "daily-cli": ("ara-cli", "openai_daily_cli"),
    "repl": ("ara-cli", "e2e"),
    "rpc-host": ("ara-cli", "rpc_host"),
    "retry": ("ara-cli", "rpc_retry"),
    "promotion": ("ara-cli", "rpc_compaction"),
    "chat": ("ara-ai", "openai_http"),
    "responses": ("ara-ai", "openai_responses_http"),
    "codex": ("ara-ai", "openai_codex_http"),
    "ai": ("ara-ai", "@lib"),
    "core-loop": ("ara-agent", "agent_loop"),
    "core-live": ("ara-agent", "live_tools"),
}

_base_source_pins = inherited.source_pins


def source_pins():
    pins = _base_source_pins()
    paths = [shared.ROOT / "scripts/verify_model_tools.py"]
    paths.extend((shared.ROOT / "crates/ara-tools").rglob("*"))
    for path in paths:
        if path.is_file():
            pins[path.relative_to(shared.ROOT).as_posix()] = hashlib.sha256(path.read_bytes()).hexdigest()
    return pins


shared.source_pins = source_pins

if __name__ == "__main__":
    if "--keep-going" not in sys.argv:
        sys.argv.append("--keep-going")
    raise SystemExit(shared.main())
