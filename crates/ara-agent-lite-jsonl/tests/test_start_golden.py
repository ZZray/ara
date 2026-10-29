"""Start-frame golden comparison (requirement 9.2) with the actual sidecar.

`tests/golden/start-baseline/` was captured with the binary BEFORE the `resume`
start field existed. For every start frame without `resume`, the stdout frame
kinds, order and content, the stderr category and the exit code must stay equal.

Controlled loopback provider only; no actual model. Binary: ARA_JSONL_TEST_BINARY,
default `$CARGO_TARGET_DIR/debug/ara-agent-lite-jsonl.exe`. Set
ARA_JSONL_GOLDEN_CAPTURE=1 only to (re)capture the baseline from a known binary.
"""
from __future__ import annotations

import asyncio
import json
import os
from pathlib import Path
import re
import sys
import tempfile
import time
import unittest

HERE = Path(__file__).resolve().parent
sys.path[:0] = [str(HERE.parent / 'clients/python'), str(HERE)]
from ara_agent_lite import encoded
from controlled_provider import ControlledProvider, call, text

TARGET = Path(os.environ.get('CARGO_TARGET_DIR', 'D:/workspace/env/cache/rust-target-c'))
BINARY = Path(os.environ.get('ARA_JSONL_TEST_BINARY', str(TARGET / 'debug' / ('ara-agent-lite-jsonl' + ('.exe' if os.name == 'nt' else '')))))
GOLDEN = HERE / 'golden' / 'start-baseline'
CAPTURE = os.environ.get('ARA_JSONL_GOLDEN_CAPTURE') == '1'
TOKEN = 'controlled-golden-no-provider-grant'
# Without the profile and ProgramData variables, Windows shell code in the
# child writes a literal `%SystemDrive%` folder into its working directory.
ENV_KEYS = ('PATH', 'SYSTEMROOT', 'SYSTEMDRIVE', 'TEMP', 'TMP', 'PATHEXT', 'COMSPEC', 'PROGRAMDATA',
            'ALLUSERSPROFILE', 'USERPROFILE', 'LOCALAPPDATA', 'APPDATA')
SHA = re.compile(r'[0-9a-f]{64}')


def child_env(token=True):
    env = {key: value for key, value in os.environ.items() if key.upper() in ENV_KEYS}
    if token:
        env['ARA_AGENT_MODEL_TOKEN'] = TOKEN
    return env


def frame(root: Path, endpoint: str, **changes):
    value = {'kind': 'start_task', 'version': 1,
        'task': {'task_id': 'golden-task', 'criteria_revision': 'c1', 'authorization_revision': 'a1', 'kind': 'Coding',
                 'evidence_dir': str(root / 'harness'), 'max_runs': 1, 'max_calls': 4,
                 'deadline_unix_ms': int(time.time() * 1000) + 30000},
        'run': {'run_id': 'run-1', 'objective': 'Answer once.', 'cwd': str(root),
                'session_path': str(root / 'session.json'), 'environment_state': str(root / 'environment'),
                'device_fp': 'golden-fixture', 'session_key': 'golden/session', 'mode': 'execute',
                'endpoint': endpoint, 'model': 'jsonl-fixture', 'max_calls': 4, 'max_seconds': 25, 'max_output': 512,
                'model_resolution': {'kind': 'catalog'},
                'tools': [{'name': 'write_answer', 'description': 'Write one Host-owned answer.',
                           'parameters': {'type': 'object', 'properties': {'content': {'type': 'string'}},
                                          'required': ['content']}}]},
        'cancel_grace_ms': 1000}
    for path, replacement in changes.items():
        target = value
        keys = path.split('.')
        for key in keys[:-1]:
            target = target[key]
        if replacement is DELETE:
            del target[keys[-1]]
        else:
            target[keys[-1]] = replacement
    return value


DELETE = object()

# name -> (provider script, how to build stdin bytes, child has token)
CASES = {
    'ok_text': ([text('Done.')], lambda r, e: encoded(frame(r, e)), True),
    'ok_tool_then_text': ([call('call-1', 'write_answer', {'content': 'x'}), text('Done.')],
                          lambda r, e: encoded(frame(r, e)), True),
    'empty_stdin': ([], lambda r, e: b'', True),
    'unterminated_frame': ([], lambda r, e: encoded(frame(r, e))[:-1], True),
    'malformed_json': ([], lambda r, e: b'{not json}\n', True),
    'wrong_version': ([], lambda r, e: encoded(frame(r, e, version=2)), True),
    'wrong_kind': ([], lambda r, e: encoded(frame(r, e, kind='start')), True),
    'unknown_top_field': ([], lambda r, e: encoded(frame(r, e, unexpected=True)), True),
    'unknown_run_field': ([], lambda r, e: encoded(frame(r, e, **{'run.unexpected': 1})), True),
    'missing_run_field': ([], lambda r, e: encoded(frame(r, e, **{'run.model': DELETE})), True),
    'max_output_zero': ([], lambda r, e: encoded(frame(r, e, **{'run.max_output': 0})), True),
    'max_output_over_bound': ([], lambda r, e: encoded(frame(r, e, **{'run.max_output': 8193})), True),
    'host_output_ceiling_zero': ([], lambda r, e: encoded(frame(r, e, **{'run.host_output_ceiling': 0})), True),
    'cancel_grace_out_of_range': ([], lambda r, e: encoded(frame(r, e, cancel_grace_ms=999)), True),
    'length_review_in_plan': ([], lambda r, e: encoded(frame(r, e, length_stop_review='review_closed_length',
                                                              **{'run.mode': 'plan'})), True),
    'budget_review_in_plan': ([], lambda r, e: encoded(frame(r, e, budget_stop_review='review_closed_tool_round',
                                                              **{'run.mode': 'plan'})), True),
    'mcp_options_without_servers': ([], lambda r, e: encoded(frame(r, e, mcp_options={'required': []})), True),
    'unknown_generation_policy': ([], lambda r, e: encoded(frame(r, e, **{'run.generation_policy': 'bogus'})), True),
    'relative_session_path': ([], lambda r, e: encoded(frame(r, e, **{'run.session_path': 'session.json'})), True),
    'tool_named_read': ([], lambda r, e: encoded(frame(r, e, **{'run.tools': [{'name': 'read', 'description': 'x',
                                                                    'parameters': {'type': 'object'}}]})), True),
    'model_token_missing': ([], lambda r, e: encoded(frame(r, e)), False),
    'evidence_dir_exists': ([], lambda r, e: (r / 'harness').mkdir() or encoded(frame(r, e)), True),
}


def normalize(value, roots):
    if isinstance(value, dict):
        return {key: ('<deadline>' if key == 'deadline_unix_ms' else normalize(item, roots))
                for key, item in value.items()}
    if isinstance(value, list):
        return [normalize(item, roots) for item in value]
    if isinstance(value, str):
        for root in roots:
            value = value.replace(root, '<ROOT>')
        return SHA.sub('<sha256>', value.replace('\\', '/'))
    return value


async def drive(name, root: Path):
    script, stdin, token = CASES[name]
    with ControlledProvider(script) as provider:
        payload = stdin(root, provider.endpoint)
        proc = await asyncio.create_subprocess_exec(str(BINARY), stdin=asyncio.subprocess.PIPE,
                stdout=asyncio.subprocess.PIPE, stderr=asyncio.subprocess.PIPE, env=child_env(token))
        proc.stdin.write(payload)
        await proc.stdin.drain()
        if not payload.endswith(b'\n'):
            proc.stdin.close()
        frames = []
        async with asyncio.timeout(60):
            while line := await proc.stdout.readline():
                current = json.loads(line)
                frames.append(current)
                if current['kind'] == 'tool_call':
                    reply = {'kind': 'tool_reply', **{key: current[key] for key in ('version', 'rpc_id', 'task_id', 'run_id',
                             'native_call_id', 'tool_name', 'args_sha256')},
                             'result': {'value': {'written': True}, 'is_error': False, 'settlement': 'Settled',
                                        'operation_receipt': 'golden-host-receipt'}}
                    proc.stdin.write(encoded(reply))
                    await proc.stdin.drain()
                elif current['kind'] == 'verify_request':
                    reply = {'kind': 'verify_reply', **{key: current[key] for key in ('version', 'rpc_id', 'task_id', 'run_id')},
                             'receipt': {'basis': current['candidate']['basis'], 'committed': True, 'settlement': 'Settled',
                                         'verdict': 'Verified', 'evidence': ['golden-review-only']}}
                    proc.stdin.write(encoded(reply))
                    await proc.stdin.drain()
                elif current['kind'] == 'task_result':
                    proc.stdin.close()
            stderr = await proc.stderr.read()
            code = await proc.wait()
        requests = len(provider.requests)
    real = root.resolve()
    roots = sorted({'\\\\?\\' + str(real), str(real), str(root), real.as_posix(), root.as_posix()}, key=len, reverse=True)
    return {'stdout': normalize(frames, roots), 'stderr': stderr.decode('utf-8', 'replace').strip(),
            'exit_code': code, 'provider_requests': requests}


class StartGoldenTest(unittest.IsolatedAsyncioTestCase):
    def setUp(self):
        self.assertTrue(BINARY.is_file(), f'build the sidecar first: {BINARY}')
        self.root = Path(tempfile.mkdtemp(prefix='ara-jsonl-golden-'))

    async def test_every_start_case_matches_the_baseline(self):
        if CAPTURE:
            GOLDEN.mkdir(parents=True, exist_ok=True)
        stored = sorted(path.stem for path in GOLDEN.glob('*.json'))
        if not CAPTURE:
            self.assertEqual(stored, sorted(CASES), 'every case has exactly one baseline file')
        for name in CASES:
            with self.subTest(case=name):
                root = self.root / name
                root.mkdir()
                actual = await drive(name, root)
                path = GOLDEN / f'{name}.json'
                if CAPTURE:
                    path.write_text(json.dumps(actual, indent=2, sort_keys=True, ensure_ascii=False) + '\n',
                                    encoding='utf-8')
                    continue
                self.assertEqual(actual, json.loads(path.read_text(encoding='utf-8')))


if __name__ == '__main__':
    unittest.main()
