"""Python client `reconcile_resume` callback (requirement 11) with the actual sidecar.

Each test leaves a fenced Task with a first sidecar (see test_jsonl_resume.py),
then resumes it through `run_task`. The two cases the actual sidecar cannot
produce (a reconcile_request without `resume`, and a second one) use
`fake_resume_sidecar.py`. Controlled loopback provider only; no actual model.
"""
from __future__ import annotations

import asyncio
import json
from pathlib import Path
import sys
import tempfile
import time
import unittest
from unittest import mock

HERE = Path(__file__).resolve().parent
sys.path[:0] = [str(HERE.parent / 'clients/python'), str(HERE)]
import ara_agent_lite
from ara_agent_lite import ReconciliationRequired, run_task
from controlled_provider import ControlledProvider, call, text
from test_jsonl_resume import BINARY, TOKEN, make_fenced, receipt, task_config

FAKE = HERE / 'fake_resume_sidecar.py'


class Host:
    def __init__(self, behavior=None):
        self.behavior, self.reviews, self.signal_seen = behavior, [], None

    async def tool_call(self, request, cancelled):
        raise AssertionError('a resumed Task must not replay the Unknown tool call')

    async def verify(self, candidate, cancelled):
        return {'basis': candidate['basis'], 'committed': True, 'settlement': 'Settled', 'verdict': 'Verified',
                'evidence': ['controlled-review-only']}

    async def prepare(self, continuation, cancelled):
        raise AssertionError('no continuation expected')

    async def reconcile_resume(self, review, cancelled):
        self.reviews.append(review)
        try:
            return await self.behavior(review, cancelled)
        finally:
            self.signal_seen = cancelled.is_set()


class NoReconcileHost(Host):
    reconcile_resume = None


def script():
    return [call('op-1', 'deploy', {'step': 'a'}), text('Done after settlement.')]


class PythonResumeTest(unittest.IsolatedAsyncioTestCase):
    def setUp(self):
        self.assertTrue(BINARY.is_file(), f'build the sidecar first: {BINARY}')
        self.root = Path(tempfile.mkdtemp(prefix='ara-python-resume-'))

    async def fenced(self, provider, name='task', deadline_s=60):
        root = self.root / name
        root.mkdir()
        return await make_fenced(root, f'python-resume-{name}', provider, deadline_s)

    async def refused(self, fenced, host, category):
        config = fenced.resume()
        with self.assertRaises(ReconciliationRequired) as raised:
            await run_task(BINARY, config, host, token=TOKEN, operation_directory=fenced.root / f'ops-{fenced.attempts}')
        message = str(raised.exception)
        self.assertTrue(message.startswith(category + ':'), message)
        self.assertNotIn(TOKEN, message)
        evidence = Path(config['task']['evidence_dir'])
        self.assertTrue((evidence / 'jsonl-rpc-1-request.json').is_file())
        self.assertFalse((evidence / 'jsonl-rpc-1-reply.json').exists(), 'no reconcile_reply was written')
        self.assertFalse((evidence / 'resume.json').exists())
        fenced.assert_unchanged(self)
        self.assertEqual(len(host.reviews), 1, 'the callback is called exactly once')
        return json.loads((evidence / 'jsonl-resume-rejected.json').read_text(encoding='utf-8'))

    # 11.1: one callback, one reply with the request's ID, and the Task continues.
    async def test_callback_settles_once_and_its_reply_keeps_the_request_id(self):
        with ControlledProvider(script()) as provider:
            fenced = await self.fenced(provider)
            config = fenced.resume()
            async def settle(review, cancelled):
                return receipt()
            host = Host(settle)
            outcome = await run_task(BINARY, config, host, token=TOKEN, operation_directory=self.root / 'ops')
        self.assertEqual(outcome['stop'], 'AwaitingHostAcceptance')
        self.assertEqual(len(host.reviews), 1)
        self.assertEqual(host.reviews[0]['fence'], json.loads(fenced.fence_bytes))
        evidence = Path(config['task']['evidence_dir'])
        request = json.loads((evidence / 'jsonl-rpc-1-request.json').read_text(encoding='utf-8'))
        replies = [json.loads(path.read_text(encoding='utf-8')) for path in evidence.glob('jsonl-rpc-*-reply.json')]
        reconcile = [reply for reply in replies if reply['kind'] == 'reconcile_reply']
        self.assertEqual(len(reconcile), 1)
        self.assertEqual({key: reconcile[0][key] for key in ('rpc_id', 'task_id', 'run_id')},
                         {key: request[key] for key in ('rpc_id', 'task_id', 'run_id')})
        self.assertEqual(reconcile[0]['receipt'], receipt())

    # 11.4: no callback, no sidecar.
    async def test_missing_callback_is_refused_before_the_sidecar_starts(self):
        with ControlledProvider(script()) as provider:
            fenced = await self.fenced(provider)
            config = fenced.resume()
            operations = self.root / 'ops-missing'
            with mock.patch.object(ara_agent_lite.asyncio, 'create_subprocess_exec',
                                   side_effect=AssertionError('sidecar started')) as spawn:
                with self.assertRaises(ValueError) as raised:
                    await run_task(BINARY, config, NoReconcileHost(), token=TOKEN, operation_directory=operations)
            self.assertEqual(str(raised.exception), 'resume requires a reconcile_resume callback')
            spawn.assert_not_called()
            self.assertFalse(operations.exists())
            self.assertFalse(Path(config['task']['evidence_dir']).exists())
            fenced.assert_unchanged(self)

    # 11.2: exception (its text and the model token never reach the error), invalid and oversized receipts.
    async def test_failed_callback_writes_no_reply_and_names_the_category(self):
        async def throws(review, cancelled):
            raise RuntimeError(f'Host failure leaking {TOKEN}')
        async def invalid(review, cancelled):
            return {'committed': True, 'settlement': 'Settled'}
        async def wrong_type(review, cancelled):
            return receipt(evidence='host-log')
        async def oversized(review, cancelled):
            return receipt(evidence=['x' * 270_000])
        with ControlledProvider(script()) as provider:
            fenced = await self.fenced(provider)
            for behavior, category in ((throws, 'reconcile_callback_exception'), (invalid, 'reconcile_receipt_invalid'),
                                       (wrong_type, 'reconcile_receipt_invalid'), (oversized, 'reconcile_frame_too_large')):
                with self.subTest(case=behavior.__name__):
                    rejected = await self.refused(fenced, Host(behavior), category)
                    self.assertEqual(rejected['category'], 'resume_cancelled')

    # 11.2: a callback past the Task deadline gets the signal, then the grace; it is never replied to.
    async def test_timeout_sets_the_signal_and_ends_within_the_bound(self):
        async def ignores(review, cancelled):
            await asyncio.sleep(600)
        async def honors(review, cancelled):
            await cancelled.wait()
            return receipt()
        for name, behavior in (('ignores', ignores), ('honors', honors)):
            with self.subTest(case=name), ControlledProvider(script()) as provider:
                    fenced = await self.fenced(provider, name, deadline_s=8)
                    deadline = fenced.config['task']['deadline_unix_ms'] / 1000
                    host = Host(behavior)
                    rejected = await self.refused(fenced, host, 'reconcile_timeout')
                    self.assertTrue(host.signal_seen, 'the cancellation signal was set')
                    self.assertLessEqual(time.time(), deadline + 1 + 20)
                    self.assertIn(rejected['category'], ('reconcile_timeout', 'resume_cancelled'))

    async def fake(self, mode, config, host):
        log = self.root / f'{mode}-stdin.log'
        original = asyncio.create_subprocess_exec
        async def launch(program, *args, **kwargs):
            return await original(sys.executable, str(FAKE), mode, str(log), *args, **kwargs)
        with mock.patch.object(ara_agent_lite.asyncio, 'create_subprocess_exec', launch):
            with self.assertRaises(ReconciliationRequired) as raised:
                await run_task(BINARY, config, host, token=TOKEN, operation_directory=self.root / f'ops-{mode}')
        lines = [json.loads(line) for line in log.read_text(encoding='utf-8').splitlines()] if log.exists() else []
        self.assertNotIn('reconcile_reply', [line['kind'] for line in lines])
        return str(raised.exception)

    # 11.5: a reconcile_request without `resume`, or a second one, never reaches the callback.
    async def test_unexpected_or_second_request_is_refused(self):
        async def settle(review, cancelled):
            await asyncio.sleep(0.5)
            return receipt()
        plain = Host(settle)
        config = task_config(self.root, 'python-resume-fake', 'http://127.0.0.1:9/v1')
        message = await self.fake('unexpected', config, plain)
        self.assertTrue(message.startswith('reconcile_request_unexpected:'), message)
        self.assertEqual(plain.reviews, [])
        twice = Host(settle)
        config = {**config, 'resume': {'version': 1, 'fence_sha256': '0' * 64}}
        message = await self.fake('twice', config, twice)
        self.assertTrue(message.startswith('reconcile_request_unexpected:'), message)
        self.assertLessEqual(len(twice.reviews), 1)


if __name__ == '__main__':
    unittest.main()
