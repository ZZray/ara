"""Host review policy checks; these do not stand for sidecar mechanism coverage."""
import asyncio
import json
from pathlib import Path
import sys
import tempfile
import unittest

from long_trial import Trial, TrialHost


class HostReviewPolicy(unittest.IsolatedAsyncioTestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory(prefix='lite-long-host-')
        self.addCleanup(self.temp.cleanup)
        self.root = Path(self.temp.name)

    def host(self, rounds):
        trial = Trial({'task_id': 'T', 'credential_env': 'OWNED_UNUSED_TOKEN', 'host_review_rounds': rounds,
                       'public_check': [sys.executable, '-c',
                                        "from pathlib import Path; assert Path('public.txt').read_text() == 'good'"]},
                      self.root, '')
        for directory in (trial.workspace, trial.state, trial.receipts, trial.private):
            directory.mkdir()
        (trial.workspace / 'public.txt').write_text('good', encoding='utf-8')
        self.addCleanup(trial.log.close)
        return TrialHost(trial)

    async def test_default_public_pass_is_verified_without_extra_rounds(self):
        host = self.host(0)
        receipt = await host.verify({'basis': {'run_id': 'r1', 'run_number': 1}}, asyncio.Event())
        self.assertEqual(receipt['verdict'], 'Verified')
        saved = json.loads(Path(receipt['evidence'][0]).read_bytes())
        self.assertTrue(saved['public_check_passed'])
        self.assertIsNone(saved['trial_review_reason'])

    async def test_explicit_two_reviews_preserve_public_result_then_accept(self):
        host = self.host(2)
        receipts = [await host.verify({'basis': {'run_id': f'r{number}', 'run_number': number}}, asyncio.Event())
                    for number in (1, 2, 3)]
        self.assertEqual([row['verdict'] == 'Verified' for row in receipts], [False, False, True])
        for receipt in receipts:
            saved = json.loads(Path(receipt['evidence'][0]).read_bytes())
            self.assertTrue(saved['public_check_passed'])
            self.assertEqual(saved['verdict'], receipt['verdict'], 'committed receipt equals the reply')
        self.assertEqual(host.t.deferred_reviews, 2)

    async def test_length_policy_continues_actual_length_but_does_not_manufacture_it(self):
        host = self.host(2)
        snapshot = {'basis': {'run_id': 'r1', 'run_number': 1}, 'evidence': {'cause': 'length'}}
        receipt = await host.verify_stop_snapshot(snapshot, asyncio.Event())
        self.assertIn('NeedsChanges', receipt['verdict'])
        saved = json.loads(Path(receipt['evidence'][0]).read_bytes())
        self.assertTrue(saved['public_check_passed'])
        self.assertEqual(saved['verdict'], receipt['verdict'])
        snapshot['evidence']['cause'] = 'cancelled'
        snapshot['basis']['run_number'] = 2
        other = await host.verify_stop_snapshot(snapshot, asyncio.Event())
        self.assertEqual(other['verdict'], 'Verified')
        self.assertEqual(host.t.counts()['length_continuation'], 0,
                         'Host reviews do not create sidecar prepare_stop receipts')

    async def test_policy_does_not_kill_before_all_requested_review_rounds(self):
        host = self.host(2)
        host.t.process = 1
        counts = {'compaction': 1, 'host_review_continuation': 1, 'length_continuation': 1}
        host.t.counts = lambda: counts
        request = {'tool_name': 'ws_write_file', 'native_call_id': 'call-1', 'rpc_id': 'rpc-1',
                   'run_id': 'r1', 'task_id': 'T', 'args_sha256': 'owned-test-arguments'}
        result = await host.side_effect(request, {'path': 'first.txt', 'content': 'first'}, asyncio.Event())
        self.assertFalse(result['is_error'])
        self.assertFalse(host.t.marker.exists())
        counts['host_review_continuation'] = 2
        request.update(native_call_id='call-2', rpc_id='rpc-2')
        cancelled = asyncio.Event()
        cancelled.set()
        with self.assertRaisesRegex(RuntimeError, 'sidecar terminated'):
            await host.side_effect(request, {'path': 'second.txt', 'content': 'second'}, cancelled)
        marker = json.loads(host.t.marker.read_bytes())
        applied = json.loads(Path(marker['receipt']).read_bytes())
        self.assertTrue(applied['committed'])
        self.assertEqual(applied['stage'], 'applied')
        self.assertEqual((host.t.workspace / 'second.txt').read_text(), 'second')


if __name__ == '__main__':
    unittest.main()
