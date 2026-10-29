"""JSONL Unknown settlement records (requirement 23) with the actual sidecar.

Controlled loopback provider only; no actual model. Binary: ARA_JSONL_TEST_BINARY,
default `$CARGO_TARGET_DIR/debug/ara-agent-lite-jsonl.exe`.
"""
from __future__ import annotations

import asyncio
import hashlib
import importlib.util
import json
import os
from pathlib import Path
import subprocess
import sys
import tempfile
import time
import unittest

HERE = Path(__file__).resolve().parent
sys.path[:0] = [str(HERE.parent / 'clients/python'), str(HERE)]
from ara_agent_lite import encoded, parse_unknown_effects, run_task
from controlled_provider import ControlledProvider, text

TARGET = Path(os.environ.get('CARGO_TARGET_DIR', 'D:/workspace/env/cache/rust-target-c'))
BINARY = Path(os.environ.get('ARA_JSONL_TEST_BINARY', str(TARGET / 'debug' / ('ara-agent-lite-jsonl' + ('.exe' if os.name == 'nt' else '')))))
TOKEN = 'controlled-unknown-no-provider-grant'
REPO = Path(subprocess.run(['git', 'rev-parse', '--show-toplevel'], cwd=HERE, capture_output=True, text=True, check=True).stdout.strip())
CLIENTS = 'crates/ara-agent-lite-jsonl/clients'


def sha(path):
    return hashlib.sha256(Path(path).read_bytes()).hexdigest()


def calls(*ids):
    return {'delta': {'tool_calls': [{'index': index, 'id': call_id, 'type': 'function', 'function': {
        'name': 'write_answer', 'arguments': json.dumps({'content': call_id})}} for index, call_id in enumerate(ids)]},
        'finish': 'tool_calls'}


class Host:
    def __init__(self, root, settlement):
        self.root, self.settlement, self.calls = root, settlement, []

    async def tool_call(self, request, cancelled):
        self.calls.append(request)
        receipt = self.root / f"host-{request['rpc_id']}.json"
        receipt.write_text(json.dumps({'request': request}), encoding='utf-8')
        return {'value': {'written': True}, 'is_error': False, 'settlement': self.settlement,
                'operation_receipt': str(receipt)}

    async def verify(self, candidate, cancelled):
        return {'basis': candidate['basis'], 'committed': True, 'settlement': 'Settled', 'verdict': 'Verified',
                'evidence': ['controlled-review-only']}

    async def prepare(self, continuation, cancelled):
        raise AssertionError('no continuation expected')


class JsonlUnknownTest(unittest.IsolatedAsyncioTestCase):
    def setUp(self):
        self.assertTrue(BINARY.is_file(), f'build the sidecar first: {BINARY}')
        self.root = Path(tempfile.mkdtemp(prefix='ara-jsonl-unknown-'))

    def config(self, endpoint, name='run'):
        root = self.root / name
        root.mkdir()
        return {'kind': 'start_task', 'version': 1,
            'task': {'task_id': 'unknown-' + self._testMethodName, 'criteria_revision': 'c1', 'authorization_revision': 'a1',
                     'kind': 'Coding', 'evidence_dir': str(root / 'harness'), 'max_runs': 1, 'max_calls': 4,
                     'deadline_unix_ms': int(time.time() * 1000) + 30000},
            'run': {'run_id': 'run-1', 'objective': 'Write the answer.', 'cwd': str(root),
                    'session_path': str(root / 'session.json'), 'environment_state': str(root / 'environment'),
                    'device_fp': 'unknown-fixture', 'session_key': 'unknown/session', 'mode': 'execute',
                    'endpoint': endpoint, 'model': 'jsonl-fixture', 'max_calls': 4, 'max_seconds': 25, 'max_output': 512,
                    'model_resolution': {'kind': 'catalog'},
                    'tools': [{'name': 'write_answer', 'description': 'Write one Host-owned answer.',
                               'parameters': {'type': 'object', 'properties': {'content': {'type': 'string'}},
                                              'required': ['content']}}]},
            'cancel_grace_ms': 1000}

    async def raw(self, config):
        """Drive the sidecar directly; check the receipts at the moment task_result arrives."""
        env = {key: value for key, value in os.environ.items() if key.upper() in ('PATH', 'SYSTEMROOT', 'TEMP', 'TMP')}
        env['ARA_AGENT_MODEL_TOKEN'] = TOKEN
        proc = await asyncio.create_subprocess_exec(str(BINARY), stdin=asyncio.subprocess.PIPE,
                stdout=asyncio.subprocess.PIPE, stderr=asyncio.subprocess.PIPE, env=env)
        proc.stdin.write(encoded(config))
        await proc.stdin.drain()
        replied, at_result = 0, None
        async with asyncio.timeout(40):
            while True:
                frame = json.loads(await proc.stdout.readline())
                if frame['kind'] == 'tool_call':
                    replied += 1
                    reply = {'kind': 'tool_reply', **{key: frame[key] for key in ('version', 'rpc_id', 'task_id', 'run_id',
                             'native_call_id', 'tool_name', 'args_sha256')},
                             'result': {'value': None, 'is_error': False, 'settlement': 'Unknown',
                                        'operation_receipt': 'raw-host-' + frame['rpc_id']}}
                    proc.stdin.write(encoded(reply))
                    await proc.stdin.drain()
                elif frame['kind'] == 'task_result':
                    outcome = json.loads(Path(frame['outcome_ref']['path']).read_text(encoding='utf-8'))
                    at_result = {record['native_call_id']: sha(record['receipt_ref']['host_receipt']['path'])
                                 for record in outcome.get('unknown_effects', [])}
                    break
            proc.stdin.close()
            self.assertEqual(await proc.wait(), 0)
        return frame, outcome, at_result, replied

    def assert_records_match_evidence(self, config, outcome):
        harness = Path(config['task']['evidence_dir'])
        stored = {}
        for path in harness.glob('jsonl-rpc-*-unknown.json'):
            record = json.loads(path.read_text(encoding='utf-8'))
            if 'native_call_id' in record:
                stored[record['native_call_id']] = (path, record)
        records = outcome.get('unknown_effects', [])
        self.assertEqual(sorted(r['native_call_id'] for r in records), sorted(stored), 'one record per unknown call')
        self.assertEqual(len({r['native_call_id'] for r in records}), len(records))
        for record in records:
            path, evidence = stored[record['native_call_id']]
            host = record['receipt_ref']['host_receipt']
            self.assertTrue(os.path.samefile(host['path'], path))
            self.assertEqual(host['sha256'], sha(path))
            self.assertEqual((record['run_id'], record['tool']), (evidence['run_id'], evidence['tool_name']))
            self.assertEqual(evidence['external_settlement'], 'unknown')
            self.assertTrue(evidence['no_replay'])
            self.assertIn(evidence['reason'], ('callback_unknown', 'host_declared_unknown'))
        return records

    async def test_n_unknown_calls_give_n_records_bound_to_existing_evidence(self):
        for count in (1, 2, 3):
            ids = [f'call-{count}-{index}' for index in range(count)]
            with ControlledProvider([calls(*ids), text('done')]) as provider:
                config = self.config(provider.endpoint, f'n{count}')
                frame, outcome, at_result, replied = await self.raw(config)
            self.assertIn(outcome['stop'], ('ReconciliationRequired', 'Cancelled'))
            self.assertNotIn('unknown_effects', frame, 'task_result key set is unchanged')
            # Core runs the calls in order and stops after the first Unknown; every call
            # that settled Unknown (Host-declared or refused after the bridge failed)
            # has exactly one record, the rest never ran. N = the Unknown settlements.
            records = self.assert_records_match_evidence(config, outcome)
            self.assertEqual(replied, 1)
            self.assertTrue(1 <= len(records) <= count)
            self.assertEqual([r['native_call_id'] for r in records], ids[:len(records)])
            self.assertEqual(set(at_result), {r['native_call_id'] for r in records})
            for record in records:
                self.assertEqual(at_result[record['native_call_id']], record['receipt_ref']['host_receipt']['sha256'],
                                 'evidence existed with the same bytes before task_result')
            self.assertEqual(len(parse_unknown_effects(outcome)), len(records))

    async def test_no_unknown_leaves_the_field_absent(self):
        with ControlledProvider([calls('settled-1'), text('done')]) as provider:
            config = self.config(provider.endpoint)
            host = Host(self.root, 'Settled')
            outcome = await run_task(BINARY, config, host, token=TOKEN, operation_directory=self.root / 'ops')
        self.assertEqual(outcome['stop'], 'AwaitingHostAcceptance')
        self.assertNotIn('unknown_effects', outcome)
        self.assertEqual(parse_unknown_effects(outcome), [])

    async def test_python_client_reports_host_declared_unknown(self):
        with ControlledProvider([calls('declared-1'), text('done')]) as provider:
            config = self.config(provider.endpoint)
            host = Host(self.root, 'Unknown')
            outcome = await run_task(BINARY, config, host, token=TOKEN, operation_directory=self.root / 'ops')
        records = self.assert_records_match_evidence(config, outcome)
        self.assertEqual([r['native_call_id'] for r in records], ['declared-1'])
        evidence = json.loads(Path(records[0]['receipt_ref']['host_receipt']['path']).read_text(encoding='utf-8'))
        self.assertEqual(evidence['reason'], 'host_declared_unknown')
        self.assertEqual(parse_unknown_effects(outcome)[0].native_call_id, 'declared-1')

    async def test_clients_before_this_change_still_complete_the_same_scenario(self):
        old = self.root / 'old-clients'
        old.mkdir()
        for name in ('python/ara_agent_lite.py', 'node/client.mjs'):
            source = subprocess.run(['git', 'show', f'HEAD:{CLIENTS}/{name}'], cwd=REPO, capture_output=True, check=True).stdout
            (old / Path(name).name).write_bytes(source)
        spec = importlib.util.spec_from_file_location('ara_agent_lite_head', old / 'ara_agent_lite.py')
        head = importlib.util.module_from_spec(spec)
        spec.loader.exec_module(head)
        with ControlledProvider([calls('old-py-1'), text('done')]) as provider:
            config = self.config(provider.endpoint, 'old-python')
            outcome = await head.run_task(BINARY, config, Host(self.root, 'Unknown'), token=TOKEN,
                                          operation_directory=self.root / 'old-python-ops')
        stored = json.loads((Path(config['task']['evidence_dir']) / 'outcome.json').read_text(encoding='utf-8'))
        self.assertEqual(outcome, stored)
        self.assertEqual(len(self.assert_records_match_evidence(config, outcome)), 1)
        for client, label in ((old / 'client.mjs', 'old-node'), (HERE.parent / 'clients/node/client.mjs', 'new-node')):
            with ControlledProvider([calls(label + '-1'), text('done')]) as provider:
                config = self.config(provider.endpoint, label)
                result = await self.node(client, config, label)
            self.assertEqual(result['delivery'], 'validated', result.get('fault'))
            stored = json.loads((Path(config['task']['evidence_dir']) / 'outcome.json').read_text(encoding='utf-8'))
            self.assertEqual(result['outcome'], stored)
            records = self.assert_records_match_evidence(config, stored)
            if label == 'new-node':
                self.assertEqual(result['unknownEffects'], records)
            else:
                self.assertNotIn('unknownEffects', result)

    async def node(self, client, config, label):
        driver = self.root / f'{label}-driver.mjs'
        output = self.root / f'{label}-result.json'
        driver.write_text(f"""
import {{ promises as fs }} from 'node:fs';
import {{ runTask }} from {json.dumps(Path(client).as_uri())};
const start = JSON.parse(await fs.readFile({json.dumps(str(self.root / (label + '-start.json')))}, 'utf8'));
const result = await runTask({{ binary: {json.dumps(str(BINARY))}, start, modelToken: {json.dumps(TOKEN)},
  operationDirectory: {json.dumps(str(self.root / (label + '-ops')))},
  callbacks: {{ tool: async (frame) => ({{ value: null, is_error: false, settlement: 'Unknown', operation_receipt: 'node-' + frame.rpc_id }}),
    verify: async () => {{ throw new Error('no review expected'); }}, prepare: async () => {{ throw new Error('no continuation'); }} }} }});
await fs.writeFile({json.dumps(str(output))}, JSON.stringify(result));
""", encoding='utf-8')
        (self.root / (label + '-start.json')).write_text(json.dumps(config), encoding='utf-8')
        proc = await asyncio.create_subprocess_exec('node', str(driver))
        self.assertEqual(await asyncio.wait_for(proc.wait(), timeout=60), 0)
        return json.loads(output.read_text(encoding='utf-8'))


if __name__ == '__main__':
    unittest.main()
