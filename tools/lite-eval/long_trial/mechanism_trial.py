"""Run the actual long-trial Host/sidecar over an owned loopback provider.

This is controlled mechanism coverage, not real-model task acceptance. The
provider sends an actual SSE length finish and valid summaries of the closed
transcript. The driver owns the public checks, two extra review rounds, OS
termination and receipt-based resume. Original real-model trials are untouched.
"""
from __future__ import annotations

import argparse
import asyncio
import hashlib
import json
from pathlib import Path
import sys
import time

import long_trial

REPO = Path(__file__).resolve().parents[3]
sys.path.insert(0, str(REPO / 'crates/ara-agent-lite-jsonl/tests'))
from controlled_provider import ControlledProvider, call, text

TOKEN = 'owned-long-mechanism-no-remote-grant'


def save(path, value):
    with path.open('x', encoding='utf-8') as stream:
        json.dump(value, stream, ensure_ascii=False, indent=2)
        stream.write('\n')


def content(message):
    value = message.get('content', '')
    return value if isinstance(value, str) else '\n'.join(row.get('text', '') for row in value)


class Script:
    def __init__(self):
        self.agent_calls = 0
        self.summaries = []

    def reply(self, body):
        if content(body['messages'][0]).startswith('Summarize the supplied closed conversation'):
            supplied = json.loads(content(body['messages'][-1]))
            closed = json.dumps(supplied['closed_old_messages'], ensure_ascii=False)
            output = {'summary': 'Controlled closed transcript SHA256=' + hashlib.sha256(closed.encode()).hexdigest(),
                      'candidates': []}
            self.summaries.append({'input': supplied, 'output': output})
            return text(json.dumps(output))
        self.agent_calls += 1
        n = self.agent_calls
        if n in (1, 3):
            return call(f'mechanism-read-{n}', 'ws_read_file', {'path': f'payload-{n}.txt'})
        if n in (2, 4):
            return text(f'Controlled review checkpoint {n // 2}; public fixture remains intact.')
        if n == 5:
            return {'delta': {'content': 'Controlled partial answer.'}, 'finish': 'length'}
        if n == 6:
            return call('mechanism-committed-write', 'ws_write_file',
                        {'path': 'committed-once.txt', 'content': 'owned effect applied once\n'})
        if n == 7:
            return text('The interrupted write was settled by its committed Host receipt; no replay is needed.')
        raise AssertionError(f'unexpected agent call {n}')


async def run(binary: Path, root: Path, context_window: int):
    fixture = root / 'fixture'
    fixture.mkdir()
    (fixture / 'README.md').write_text('Controlled mechanism fixture; keep public.txt unchanged.\n', encoding='utf-8')
    (fixture / 'public.txt').write_text('owned public fixture\n', encoding='utf-8')
    (fixture / 'payload-1.txt').write_text('a' * (28 * 1024), encoding='utf-8')
    (fixture / 'payload-3.txt').write_text('b' * (12 * 1024), encoding='utf-8')
    trial_dir = root / 'trial'
    trial_dir.mkdir()
    script = Script()
    with ControlledProvider([script.reply] * 24) as provider:
        spec = {'version': 1, 'task_id': 'MECHANISM', 'kind': 'Coding', 'fixture': str(fixture),
                'sidecar': str(binary), 'credential_env': 'MECHANISM_OWNED_TOKEN',
                'public_check': [sys.executable, '-c',
                                 "from pathlib import Path; assert Path('public.txt').read_text() == 'owned public fixture\\n'"],
                'model': {'endpoint': provider.endpoint, 'id': provider.model,
                          'generation_policy': 'provider_default', 'length_recovery': 'stop'},
                'budget': {'max_runs': 6, 'max_calls': 160, 'deadline_seconds': 120,
                           'run_max_calls': 40, 'run_max_seconds': 45},
                'max_output': 2048, 'context_window_cap': context_window, 'host_review_rounds': 2}
        save(trial_dir / 'trial-input.json', spec)
        started = time.monotonic()
        record = await asyncio.wait_for(long_trial.run_trial(spec, trial_dir, TOKEN), timeout=150)
        save(root / 'provider-requests.json', provider.requests)
        save(root / 'provider-summaries.json', script.summaries)
    count = {'compaction': len(script.summaries), 'agent_requests': script.agent_calls}
    summary = {'verification': 'controlled_test', 'scope': 'mechanism_coverage_only',
               'binary': str(binary), 'binary_sha256': hashlib.sha256(binary.read_bytes()).hexdigest(),
               'driver_sha256': hashlib.sha256(Path(long_trial.__file__).read_bytes()).hexdigest(),
               'script_sha256': hashlib.sha256(Path(__file__).read_bytes()).hexdigest(),
               'wall_seconds': time.monotonic() - started, 'provider': count,
               'trial': str(trial_dir / 'trial.json'), 'stop': record['stop'],
               'kill': record['kill'], 'resume': record['resume']}
    save(root / 'mechanism-summary.json', summary)
    print(json.dumps(summary, ensure_ascii=False))
    return 0 if record['stop'] == 'AwaitingHostAcceptance' and record['resume'] \
        and not record['resume']['refused'] and script.summaries else 1


def main():
    parser = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    parser.add_argument('--sidecar', required=True, type=Path)
    parser.add_argument('--out', required=True, type=Path, help='new evidence directory outside the repository')
    parser.add_argument('--context-window', type=int, default=98304)
    args = parser.parse_args()
    binary = args.sidecar.resolve(strict=True)
    root = args.out.resolve()
    root.mkdir(parents=True, exist_ok=False)
    return asyncio.run(run(binary, root, args.context_window))


if __name__ == '__main__':
    sys.exit(main())
