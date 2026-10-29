"""Python and Node clients resume the same fenced Task the same way (requirement 12.3).

For each scenario both clients get the same start/resume input, the same
controlled provider script and the same Host callback behavior; each runs its own
fenced copy. `kind_proxy.mjs` relays the actual sidecar's stdio and records the
kind of every frame in both directions. Event frames are best-effort and excluded.
Controlled loopback provider only; no actual model.
"""
from __future__ import annotations

import asyncio
import json
from pathlib import Path
import shutil
import sys
import tempfile
import unittest
from unittest import mock

HERE = Path(__file__).resolve().parent
sys.path[:0] = [str(HERE.parent / 'clients/python'), str(HERE)]
import ara_agent_lite
from ara_agent_lite import ReconciliationRequired, run_task
from controlled_provider import ControlledProvider, call, text
from test_jsonl_resume import BINARY, TOKEN, make_fenced, receipt
from test_python_resume import Host, NoReconcileHost

PROXY = HERE / 'kind_proxy.mjs'
NODE = shutil.which('node')
CLIENT = HERE.parent / 'clients/node/client.mjs'
NODE_CALLBACKS = {
    'settled': f'async () => ({json.dumps(receipt())})',
    'throws': "async () => { throw new Error('Host failure'); }",
    'missing': None,
}


def kinds(log: Path):
    if not log.exists():
        return []
    rows = [json.loads(line) for line in log.read_text(encoding='utf-8').splitlines()]
    return [(row['from'], row['kind']) for row in rows if row['kind'] != 'event']


class CrossLanguageResumeTest(unittest.IsolatedAsyncioTestCase):
    def setUp(self):
        self.assertTrue(BINARY.is_file(), f'build the sidecar first: {BINARY}')
        self.root = Path(tempfile.mkdtemp(prefix='ara-resume-cross-'))
        self.node_faults = {}

    async def resumed(self, name):
        root = self.root / name
        root.mkdir()
        provider = ControlledProvider([call('op-1', 'deploy', {'step': 'a'}), text('Done after settlement.')])
        provider.__enter__()
        self.addCleanup(provider.__exit__, None, None, None)
        fenced = await make_fenced(root, 'cross-resume', provider)
        return root, fenced, fenced.resume()

    async def python(self, scenario):
        root, fenced, config = await self.resumed(f'python-{scenario}')
        log = root / 'kinds.jsonl'
        async def settled(review, cancelled):
            return receipt()
        async def throws(review, cancelled):
            raise RuntimeError('Host failure')
        host = NoReconcileHost() if scenario == 'missing' else Host({'settled': settled, 'throws': throws}[scenario])
        original = asyncio.create_subprocess_exec
        async def launch(program, *args, **kwargs):
            return await original(NODE, str(PROXY), str(BINARY), str(log), *args, **kwargs)
        stop = None
        with mock.patch.object(ara_agent_lite.asyncio, 'create_subprocess_exec', launch):
            try:
                stop = (await run_task(BINARY, config, host, token=TOKEN, operation_directory=root / 'ops'))['stop']
            except (ReconciliationRequired, ValueError):
                pass
        return kinds(log), stop

    async def node(self, scenario):
        root, fenced, config = await self.resumed(f'node-{scenario}')
        log = root / 'kinds.jsonl'
        (root / 'start.json').write_text(json.dumps(config), encoding='utf-8')
        output = root / 'result.json'
        callback = NODE_CALLBACKS[scenario]
        driver = root / 'driver.mjs'
        driver.write_text(f"""
import {{ promises as fs }} from 'node:fs';
import {{ runTask }} from {json.dumps(CLIENT.as_uri())};
const start = JSON.parse(await fs.readFile({json.dumps(str(root / 'start.json'))}, 'utf8'));
const callbacks = {{ tool: async () => {{ throw new Error('no replay'); }},
  verify: async (candidate) => ({{ basis: candidate.basis, committed: true, settlement: 'Settled', verdict: 'Verified',
    evidence: ['controlled-review-only'] }}),
  prepare: async () => {{ throw new Error('no continuation'); }} }};
{'' if callback is None else 'callbacks.reconcileResume = ' + callback + ';'}
let stop = null; let fault = null;
try {{
  const result = await runTask({{ binary: process.execPath, binaryArgs: [{json.dumps(str(PROXY))},
    {json.dumps(str(BINARY))}, {json.dumps(str(log))}], start, modelToken: {json.dumps(TOKEN)},
    operationDirectory: {json.dumps(str(root / 'ops'))}, callbacks }});
  stop = result.outcome?.stop ?? null;
  fault = result.fault;
}} catch (error) {{ stop = null; fault = error.message; }}
await fs.writeFile({json.dumps(str(output))}, JSON.stringify({{ stop, fault }}));
""", encoding='utf-8')
        proc = await asyncio.create_subprocess_exec('node', str(driver))
        self.assertEqual(await asyncio.wait_for(proc.wait(), timeout=90), 0)
        result = json.loads(output.read_text(encoding='utf-8'))
        self.node_faults[scenario] = result['fault']
        return kinds(log), result['stop']

    async def test_same_frames_and_stop_for_each_scenario(self):
        expected_stop = {'settled': 'AwaitingHostAcceptance', 'throws': None, 'missing': None}
        for scenario in ('settled', 'throws', 'missing'):
            with self.subTest(scenario=scenario):
                python_kinds, python_stop = await self.python(scenario)
                node_kinds, node_stop = await self.node(scenario)
                self.assertEqual(python_kinds, node_kinds, self.node_faults[scenario])
                self.assertEqual(python_stop, node_stop)
                self.assertEqual(python_stop, expected_stop[scenario])
                if scenario == 'settled':
                    self.assertEqual([kind for _, kind in python_kinds],
                                     ['start_task', 'reconcile_request', 'reconcile_reply', 'verify_request',
                                      'verify_reply', 'task_result'])
                elif scenario == 'throws':
                    self.assertEqual([kind for _, kind in python_kinds], ['start_task', 'reconcile_request', 'cancel'])
                else:
                    self.assertEqual(python_kinds, [], 'no process for a missing callback')


if __name__ == '__main__':
    unittest.main()
