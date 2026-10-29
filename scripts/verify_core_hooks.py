"""One compiled module batch for Core hooks and their actual Host consumers.

Run after the frozen source overlays are reviewed and applied. --module narrows
a family; --full invokes the existing final comprehensive gate after a green
batch. Live CAS/account trials use separate bounded receipts.
"""
import sys

import verify_append_context as inherited

shared = inherited.shared
shared.__doc__ = __doc__
shared.DEFAULT_OUTPUT = shared.Path(shared.tempfile.gettempdir()) / "ara-core-hooks-batch"
shared.MODULES = {
    "core-hooks": ("ara-agent", "core_hooks"),
    "core-owner": ("ara-agent", "@lib"),
    "core-loop": ("ara-agent", "agent_loop"),
    "core-live": ("ara-agent", "live_tools"),
    "core-compaction": ("ara-agent", "compaction"),
    "core-compaction-call": ("ara-agent", "compaction_call"),
    "core-handoff": ("ara-agent", "handoff"),
    "host-hooks": ("ara-cli", "@bin:ara"),
    "rpc-host": ("ara-cli", "rpc_host"),
    "repl": ("ara-cli", "e2e"),
    "journal": ("ara-session", "journal"),
    "ai": ("ara-ai", "@lib"),
    "chat": ("ara-ai", "openai_http"),
    "responses": ("ara-ai", "openai_responses_http"),
    "codex": ("ara-ai", "openai_codex_http"),
}

if __name__ == "__main__":
    if "--keep-going" not in sys.argv:
        sys.argv.append("--keep-going")
    raise SystemExit(shared.main())
