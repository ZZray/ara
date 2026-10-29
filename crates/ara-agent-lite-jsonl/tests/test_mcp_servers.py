"""start_task `mcp_servers` (requirement 42.9) with the actual sidecar.

Uses the controlled stdio MCP server of ara-agent-lite-mcp and the controlled
loopback provider; no network and no actual model. Binary: ARA_JSONL_TEST_BINARY,
default `$CARGO_TARGET_DIR/debug/ara-agent-lite-jsonl.exe`.
"""
from __future__ import annotations

import asyncio
import json
import os
from pathlib import Path
import sys
import tempfile
import time
import unittest

HERE = Path(__file__).resolve().parent
sys.path[:0] = [str(HERE.parent / 'clients/python'), str(HERE)]
from ara_agent_lite import encoded, run_task
from controlled_provider import ControlledProvider, call, text

TARGET = Path(os.environ.get('CARGO_TARGET_DIR', 'D:/workspace/env/cache/rust-target-c'))
BINARY = Path(os.environ.get('ARA_JSONL_TEST_BINARY', str(TARGET / 'debug' / ('ara-agent-lite-jsonl' + ('.exe' if os.name == 'nt' else '')))))
FAKE = HERE.parents[1] / 'ara-agent-lite-mcp' / 'tests' / 'fixtures' / 'fake_mcp_server.py'
TOKEN = 'controlled-mcp-no-provider-grant'
RESULT_KEYS = {'kind', 'version', 'task_id', 'stop', 'task_accepted', 'runs', 'observed_calls', 'fence_presence',
               'reconciliation_fence', 'outcome_ref', 'local_callbacks_finished'}


def server(*args, log=None):
    entry = {'command': sys.executable, 'args': [str(FAKE), *args]}
    if log is not None:
        entry['env'] = {'FAKE_MCP_LOG': str(log)}
    return entry


class Host:
    def __init__(self):
        self.tool_calls = []

    async def tool_call(self, request, cancelled):
        self.tool_calls.append(request)
        return {'value': 'host', 'is_error': False, 'settlement': 'Settled', 'operation_receipt': 'host-receipt'}

    async def verify(self, candidate, cancelled):
        return {'basis': candidate['basis'], 'committed': True, 'settlement': 'Settled', 'verdict': 'Verified',
                'evidence': ['controlled-review-only']}

    async def prepare(self, continuation, cancelled):
        raise AssertionError('no continuation expected')


class McpServersTest(unittest.IsolatedAsyncioTestCase):
    def setUp(self):
        self.assertTrue(BINARY.is_file(), f'build the sidecar first: {BINARY}')
        self.assertTrue(FAKE.is_file(), str(FAKE))
        self.root = Path(tempfile.mkdtemp(prefix='ara-jsonl-mcp-'))

    def config(self, endpoint, name, **extra):
        root = self.root / name
        root.mkdir()
        result = {'kind': 'start_task', 'version': 1,
            'task': {'task_id': 'mcp-' + name, 'criteria_revision': 'c1', 'authorization_revision': 'a1',
                     'kind': 'Coding', 'evidence_dir': str(root / 'harness'), 'max_runs': 1, 'max_calls': 4,
                     'deadline_unix_ms': int(time.time() * 1000) + 40000},
            'run': {'run_id': 'run-1', 'objective': 'Echo through MCP.', 'cwd': str(root),
                    'session_path': str(root / 'session.json'), 'environment_state': str(root / 'environment'),
                    'device_fp': 'mcp-fixture', 'session_key': 'mcp/session', 'mode': 'execute',
                    'endpoint': endpoint, 'model': 'jsonl-fixture', 'max_calls': 4, 'max_seconds': 30, 'max_output': 512,
                    'model_resolution': {'kind': 'catalog'},
                    'tools': [{'name': 'host_note', 'description': 'Host-owned tool.',
                               'parameters': {'type': 'object', 'properties': {}}}]},
            'cancel_grace_ms': 1000}
        result.update(extra)
        return result

    async def raw(self, config):
        """All stdout frames in order, the stderr bytes and the exit code."""
        env = {key: value for key, value in os.environ.items()
               # Without the profile and ProgramData variables, Windows shell
               # code in the child writes a literal `%SystemDrive%` cache folder
               # into its working directory.
               if key.upper() in ('PATH', 'SYSTEMROOT', 'SYSTEMDRIVE', 'TEMP', 'TMP', 'PATHEXT', 'COMSPEC',
                                  'PROGRAMDATA', 'ALLUSERSPROFILE', 'USERPROFILE', 'LOCALAPPDATA', 'APPDATA')}
        env['ARA_AGENT_MODEL_TOKEN'] = TOKEN
        proc = await asyncio.create_subprocess_exec(str(BINARY), stdin=asyncio.subprocess.PIPE,
                stdout=asyncio.subprocess.PIPE, stderr=asyncio.subprocess.PIPE, env=env)
        proc.stdin.write(encoded(config))
        await proc.stdin.drain()
        frames = []
        async with asyncio.timeout(60):
            while line := await proc.stdout.readline():
                frame = json.loads(line)
                frames.append(frame)
                if frame['kind'] == 'verify_request':
                    reply = {'kind': 'verify_reply', **{key: frame[key] for key in ('version', 'rpc_id', 'task_id', 'run_id')},
                             'receipt': {'basis': frame['candidate']['basis'], 'committed': True, 'settlement': 'Settled',
                                         'verdict': 'Verified', 'evidence': ['controlled-review-only']}}
                    proc.stdin.write(encoded(reply))
                    await proc.stdin.drain()
                elif frame['kind'] == 'task_result':
                    proc.stdin.close()
            stderr = await proc.stderr.read()
            code = await proc.wait()
        return frames, stderr, code

    async def test_without_the_field_frames_and_tools_are_unchanged(self):
        with ControlledProvider([text('Nothing to do.')]) as provider:
            frames, stderr, code = await self.raw(self.config(provider.endpoint, 'plain'))
            requests = provider.requests
        self.assertEqual(code, 0, stderr)
        self.assertEqual([frame['kind'] for frame in frames], ['verify_request', 'task_result'])
        self.assertEqual(set(frames[-1]), RESULT_KEYS)
        names = [tool['function']['name'] for tool in requests[0].get('tools', [])]
        self.assertIn('host_note', names)
        self.assertFalse([name for name in names if name.startswith('mcp__') or 'mcp_resource' in name])

    async def test_connected_tools_join_the_task_and_status_is_reported(self):
        log = self.root / 'fake.log'
        servers = {'fake': server('--naming', '--instructions', 'Use echo.', log=log), 'broken': server('--exit-at-start')}
        replies = [call('mcp-1', 'mcp__fake__echo', {'text': 'hello from mcp'}), text('Echo done.')]
        with ControlledProvider(replies) as provider:
            config = self.config(provider.endpoint, 'connected', mcp_servers=servers,
                                 mcp_options={'required': ['fake'], 'startup_timeout_ms': 20000})
            frames, stderr, code = await self.raw(config)
            requests = provider.requests
        self.assertEqual(code, 0, stderr)
        kinds = [frame['kind'] for frame in frames]
        self.assertEqual(kinds[0], 'mcp_status', 'status precedes every Task frame')
        self.assertEqual(kinds[1:], ['verify_request', 'task_result'], 'MCP calls never reach the Host')
        status = frames[0]
        self.assertEqual(set(status), {'kind', 'version', 'task_id', 'servers', 'excluded'})
        self.assertEqual(status['servers']['fake']['status'], 'connected')
        self.assertEqual(status['servers']['fake']['instructions'], 'Use echo.')
        self.assertEqual(status['servers']['broken']['status'], 'failed')
        self.assertTrue(status['servers']['broken']['reason'])
        excluded = {(item['server'], item['tool']) for item in status['excluded']}
        self.assertIn(('fake', 'not_object'), excluded)
        self.assertEqual(set(frames[-1]), RESULT_KEYS)
        names = [tool['function']['name'] for tool in requests[0]['tools']]
        self.assertIn('mcp__fake__echo', names)
        self.assertIn('host_note', names)
        self.assertEqual(status['servers']['fake']['tools'], len([name for name in names if name.startswith('mcp__fake__')]))
        self.assertIn('hello from mcp', json.dumps(requests[1]))
        outcome = json.loads(Path(frames[-1]['outcome_ref']['path']).read_text(encoding='utf-8'))
        self.assertEqual(outcome['stop'], 'AwaitingHostAcceptance')
        self.assertNotIn('unknown_effects', outcome)
        lines = log.read_text(encoding='utf-8').splitlines()
        self.assertTrue(any(line.startswith('call echo') for line in lines))
        self.assertEqual(lines[-1], 'stdin closed', 'session shut down when the Task ended')

    async def test_required_failure_rejects_the_start_without_upstream_requests(self):
        with ControlledProvider([text('must not be requested')]) as provider:
            config = self.config(provider.endpoint, 'required', mcp_servers={'broken': server('--exit-at-start')},
                                 mcp_options={'required': ['broken']})
            frames, stderr, code = await self.raw(config)
            counts = (len(provider.requests), provider.catalog_requests)
        self.assertEqual((code, frames, counts), (2, [], (0, 0)))
        self.assertEqual(stderr.decode().strip(), 'start_config_rejected')
        self.assertFalse(Path(config['task']['evidence_dir']).exists(), 'no Task admission')

    async def test_invalid_configuration_is_a_start_rejection(self):
        cases = [{'mcp_servers': {'bad': {'type': 'sse', 'url': 'http://127.0.0.1:1/sse'}}},
                 {'mcp_servers': {'fake': server()}, 'mcp_options': {'required': ['missing']}},
                 {'mcp_options': {'required': []}},
                 {'mcp_servers': {'fake': server()}, 'mcp_options': {'unexpected': True}}]
        for index, extra in enumerate(cases):
            with ControlledProvider([]) as provider:
                frames, stderr, code = await self.raw(self.config(provider.endpoint, f'invalid-{index}', **extra))
                counts = (len(provider.requests), provider.catalog_requests)
            self.assertEqual((code, frames, counts), (2, [], (0, 0)), extra)
            self.assertEqual(stderr.decode().strip(), 'start_config_rejected')

    async def test_python_client_passes_the_status_to_on_event(self):
        events = []
        with ControlledProvider([call('mcp-py', 'mcp__fake__echo', {'text': 'py'}), text('done')]) as provider:
            config = self.config(provider.endpoint, 'python', mcp_servers={'fake': server()})
            host = Host()
            outcome = await run_task(BINARY, config, host, token=TOKEN, operation_directory=self.root / 'py-ops',
                                     on_event=events.append)
        self.assertEqual(outcome['stop'], 'AwaitingHostAcceptance')
        self.assertEqual(host.tool_calls, [])
        self.assertEqual([event['kind'] for event in events], ['mcp_status'])
        self.assertEqual(events[0]['servers']['fake']['status'], 'connected')


if __name__ == '__main__':
    unittest.main()
