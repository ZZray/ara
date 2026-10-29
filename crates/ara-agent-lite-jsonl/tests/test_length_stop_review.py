"""JSONL length-stop review (16.1, 16.2) with the actual sidecar and both clients.

Controlled loopback provider only; no actual model. Binary: ARA_JSONL_TEST_BINARY,
default `$CARGO_TARGET_DIR/debug/ara-agent-lite-jsonl.exe`.
"""
from __future__ import annotations

import asyncio
import copy
import json
import os
from pathlib import Path
import sys
import tempfile
import time
import unittest

HERE = Path(__file__).resolve().parent
sys.path[:0] = [str(HERE.parent / 'clients/python'), str(HERE)]
from ara_agent_lite import run_task
from controlled_provider import ControlledProvider, text

TARGET = Path(os.environ.get('CARGO_TARGET_DIR', 'D:/workspace/env/cache/rust-target-c'))
BINARY = Path(os.environ.get('ARA_JSONL_TEST_BINARY', str(TARGET / 'debug' / ('ara-agent-lite-jsonl' + ('.exe' if os.name == 'nt' else '')))))
TOKEN = 'controlled-length-no-provider-grant'
TRUNCATED = {'delta': {'content': 'Partial text cut at the output limit.'}, 'finish': 'length'}


class StopHost:
    def __init__(self, verdict):
        self.verdict, self.snapshots, self.continuations, self.reviews = verdict, [], [], []

    async def tool_call(self, request, cancelled):
        raise AssertionError('no tool expected')

    async def verify(self, candidate, cancelled):
        self.reviews.append(candidate)
        return {'basis': candidate['basis'], 'committed': True, 'settlement': 'Settled', 'verdict': 'Verified',
                'evidence': ['controlled-review-only']}

    async def prepare(self, continuation, cancelled):
        raise AssertionError('ordinary continuation not expected')

    async def verify_stop_snapshot(self, snapshot, cancelled):
        self.snapshots.append(copy.deepcopy(snapshot))
        return {'basis': snapshot['basis'], 'committed': True, 'settlement': 'Settled', 'verdict': self.verdict,
                'evidence': ['controlled-stop-review']}

    async def prepare_stop_continuation(self, continuation, cancelled):
        self.continuations.append(copy.deepcopy(continuation))
        return {'run_id_next': f"run-{continuation['next_run_number']}", 'objective': 'Finish the cut answer briefly.'}


class LengthStopReviewTest(unittest.IsolatedAsyncioTestCase):
    def setUp(self):
        self.assertTrue(BINARY.is_file(), f'build the sidecar first: {BINARY}')
        self.root = Path(tempfile.mkdtemp(prefix='ara-jsonl-length-'))

    def config(self, endpoint, name, policy='review_closed_length'):
        root = self.root / name
        root.mkdir()
        result = {'kind': 'start_task', 'version': 1,
            'task': {'task_id': 'length-' + name, 'criteria_revision': 'c1', 'authorization_revision': 'a1',
                     'kind': 'Reasoning', 'evidence_dir': str(root / 'harness'), 'max_runs': 2, 'max_calls': 4,
                     'deadline_unix_ms': int(time.time() * 1000) + 30000},
            'run': {'run_id': 'run-1', 'objective': 'Answer.', 'cwd': str(root),
                    'session_path': str(root / 'session.json'), 'environment_state': str(root / 'environment'),
                    'device_fp': 'length-fixture', 'session_key': 'length/session', 'mode': 'execute',
                    'endpoint': endpoint, 'model': 'jsonl-fixture', 'max_calls': 2, 'max_seconds': 25, 'max_output': 64,
                    'model_resolution': {'kind': 'catalog'}, 'tools': []},
            'cancel_grace_ms': 1000}
        if policy is not None:
            result['length_stop_review'] = policy
        return result

    async def test_needs_changes_continues_in_the_same_session_then_verifies(self):
        host = StopHost({'NeedsChanges': {'feedback': 'The answer was cut; finish it.'}})
        with ControlledProvider([TRUNCATED, text('Complete answer.')]) as provider:
            config = self.config(provider.endpoint, 'continue')
            outcome = await run_task(BINARY, config, host, token=TOKEN, operation_directory=self.root / 'continue-ops')
            self.assertEqual(len(provider.requests), 2)
        self.assertEqual(outcome['stop'], 'AwaitingHostAcceptance')
        self.assertEqual(outcome['runs'], 2)
        self.assertEqual(len(host.snapshots), 1)
        snapshot = host.snapshots[0]
        self.assertEqual(set(snapshot), {'basis', 'diagnostic_output', 'goal', 'evidence', 'truncated'})
        self.assertEqual(snapshot['evidence']['cause'], 'length')
        self.assertEqual(snapshot['basis']['run_id'], 'run-1')
        self.assertEqual([c['next_run_number'] for c in host.continuations], [2])
        harness = Path(config['task']['evidence_dir'])
        self.assertTrue((harness / 'run-1-stop-snapshot.json').is_file())
        self.assertTrue((harness / 'jsonl-stop-continuation-2.json').is_file())
        self.assertEqual(len(host.reviews), 1, 'the continued Run ends with an ordinary candidate review')

    async def test_verified_stop_ends_awaiting_host_acceptance(self):
        host = StopHost('Verified')
        with ControlledProvider([TRUNCATED]) as provider:
            outcome = await run_task(BINARY, self.config(provider.endpoint, 'verified'), host, token=TOKEN,
                                     operation_directory=self.root / 'verified-ops')
        self.assertEqual((outcome['stop'], outcome['runs'], outcome['task_accepted']), ('AwaitingHostAcceptance', 1, False))
        self.assertEqual((len(host.snapshots), host.continuations, host.reviews), (1, [], []))

    async def test_verified_stop_with_budget_review_also_enabled_is_not_a_budget_result(self):
        host = StopHost('Verified')
        host.verify_budget_snapshot = host.verify_stop_snapshot
        host.prepare_budget_stop_continuation = host.prepare_stop_continuation
        with ControlledProvider([TRUNCATED]) as provider:
            config = self.config(provider.endpoint, 'both')
            config['budget_stop_review'] = 'review_closed_tool_round'
            outcome = await run_task(BINARY, config, host, token=TOKEN, operation_directory=self.root / 'both-ops')
        self.assertEqual((outcome['stop'], len(host.snapshots)), ('AwaitingHostAcceptance', 1))
        self.assertFalse((self.root / 'both-ops' / 'budget-review.json').exists())

    async def test_default_policy_sends_no_stop_frames(self):
        host = StopHost('Verified')
        with ControlledProvider([TRUNCATED]) as provider:
            outcome = await run_task(BINARY, self.config(provider.endpoint, 'default', policy=None), host, token=TOKEN,
                                     operation_directory=self.root / 'default-ops')
        self.assertEqual(outcome['stop'], 'ReconciliationRequired')
        self.assertEqual(host.snapshots, [])

    async def test_python_opt_in_without_callbacks_is_refused_before_spawn(self):
        class Plain:
            async def tool_call(self, *_): ...
            async def verify(self, *_): ...
            async def prepare(self, *_): ...
        config = self.config('http://127.0.0.1:9/v1', 'refused')
        with self.assertRaises(ValueError):
            await run_task(BINARY, config, Plain(), token=TOKEN, operation_directory=self.root / 'refused-ops')
        self.assertFalse((self.root / 'refused-ops').exists())

    async def test_node_client_stop_callbacks(self):
        with ControlledProvider([TRUNCATED, text('Complete answer.')]) as provider:
            config = self.config(provider.endpoint, 'node')
            result = await self.node(config, 'node', with_callbacks=True)
        self.assertEqual(result['delivery'], 'validated', result.get('fault'))
        self.assertEqual((result['outcome']['stop'], result['outcome']['runs']), ('AwaitingHostAcceptance', 2))
        self.assertEqual(result['calls'], ['verifyStopSnapshot', 'prepareStopContinuation', 'verify'])
        refused = await self.node(self.config('http://127.0.0.1:9/v1', 'node-refused'), 'node-refused', with_callbacks=False)
        self.assertEqual(refused['error'], 'Execute length-stop opt-in requires both dedicated callbacks')

    async def node(self, config, label, with_callbacks):
        start = self.root / f'{label}-start.json'
        start.write_text(json.dumps(config), encoding='utf-8')
        output = self.root / f'{label}-result.json'
        client = (HERE.parent / 'clients/node/client.mjs').as_uri()
        stop = """
    verifyStopSnapshot: async (snapshot) => { calls.push('verifyStopSnapshot');
      return { basis: snapshot.basis, committed: true, settlement: 'Settled', verdict: { NeedsChanges: { feedback: 'Finish it.' } }, evidence: ['node-stop-review'] }; },
    prepareStopContinuation: async (c) => { calls.push('prepareStopContinuation');
      return { run_id_next: 'run-' + c.next_run_number, objective: 'Finish the cut answer briefly.' }; },""" if with_callbacks else ''
        driver = self.root / f'{label}-driver.mjs'
        driver.write_text(f"""
import {{ promises as fs }} from 'node:fs';
import {{ runTask }} from {json.dumps(client)};
const calls = [];
const start = JSON.parse(await fs.readFile({json.dumps(str(start))}, 'utf8'));
let result;
try {{
  result = await runTask({{ binary: {json.dumps(str(BINARY))}, start, modelToken: {json.dumps(TOKEN)},
    operationDirectory: {json.dumps(str(self.root / (label + '-ops')))},
    callbacks: {{ tool: async () => {{ throw new Error('no tool'); }},
      verify: async (candidate) => {{ calls.push('verify'); return {{ basis: candidate.basis, committed: true, settlement: 'Settled', verdict: 'Verified', evidence: ['node-review'] }}; }},
      prepare: async () => {{ throw new Error('no ordinary continuation'); }},{stop} }} }});
  result.calls = calls;
}} catch (error) {{ result = {{ error: error.message }}; }}
await fs.writeFile({json.dumps(str(output))}, JSON.stringify(result));
""", encoding='utf-8')
        proc = await asyncio.create_subprocess_exec('node', str(driver))
        self.assertEqual(await asyncio.wait_for(proc.wait(), timeout=60), 0)
        return json.loads(output.read_text(encoding='utf-8'))


if __name__ == '__main__':
    unittest.main()
