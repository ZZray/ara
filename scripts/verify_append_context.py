"""One compiled batch for fixed OMP append-context Core and Host consumers.

Run only after the reviewed complete overlays are applied. --module selects a
family; --full invokes the existing final comprehensive gate after a green
module batch. Account/CAS trials are separate bounded acceptance receipts.
"""
from pathlib import Path
import hashlib
import sys

import verify_openai_daily as shared

ROOT = shared.ROOT

shared.__doc__ = __doc__
shared.DEFAULT_OUTPUT = Path(shared.tempfile.gettempdir()) / 'ara-append-context-batch'
shared.MODULES = {
    'core-append': ('ara-agent', 'append_only_context'),
    'tool-inventory': ('ara-agent', 'tool_inventory'),
    'context-prompt': ('ara-context', 'upstream'),
    'core-loop': ('ara-agent', 'agent_loop'),
    'core-live': ('ara-agent', 'live_tools'),
    'core-compaction': ('ara-agent', 'compaction'),
    'core-compaction-call': ('ara-agent', 'compaction_call'),
    'core-handoff': ('ara-agent', 'handoff'),
    'host-append': ('ara-cli', 'append_only_context_host'),
    'settings': ('ara-cli', '@bin:ara'),
    'model-controls': ('ara-cli', '@lib'),
    'selection': ('ara-cli', 'daily_model_config'),
    'daily-cli': ('ara-cli', 'openai_daily_cli'),
    'repl': ('ara-cli', 'e2e'),
    'rpc-host': ('ara-cli', 'rpc_host'),
    'retry': ('ara-cli', 'rpc_retry'),
    'promotion': ('ara-cli', 'rpc_compaction'),
    'remote-side': ('ara-cli', 'remote_compaction'),
    'journal': ('ara-session', 'journal'),
    'chat': ('ara-ai', 'openai_http'),
    'responses': ('ara-ai', 'openai_responses_http'),
    'codex': ('ara-ai', 'openai_codex_http'),
    'ai': ('ara-ai', '@lib'),
}


def source_pins():
    # All non-vendor workspace crates, including MCP and prompt helpers, rather than the
    # previous package allowlist that omitted an indirectly compiled MCP source.
    paths = [ROOT / 'Cargo.toml', ROOT / 'Cargo.lock', Path(__file__).resolve()]
    paths.extend((ROOT / 'scripts').glob('*.py'))
    paths.extend((ROOT / '.cargo').rglob('*'))
    for package in (ROOT / 'crates').iterdir():
        if package.is_dir() and package.name != 'vendor':
            paths.extend(package.rglob('*'))
    return {
        str(path.relative_to(ROOT)) if path.is_relative_to(ROOT) else str(path):
            hashlib.sha256(path.read_bytes()).hexdigest()
        for path in sorted(set(paths)) if path.is_file()
    }


shared.source_pins = source_pins
_base_windows_test_env = shared.windows_test_env


def module_environment():
    env = _base_windows_test_env() or shared.os.environ.copy()
    env['CARGO_BUILD_JOBS'] = '1'
    env['RUST_TEST_THREADS'] = '2'
    return env


shared.windows_test_env = module_environment
if __name__ == '__main__':
    if '--keep-going' not in sys.argv:
        sys.argv.append('--keep-going')
    raise SystemExit(shared.main())
