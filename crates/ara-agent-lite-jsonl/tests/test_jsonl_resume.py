"""JSONL `resume` start field and reconcile frames (requirements 9, 10) with the actual sidecar.

A first sidecar leaves a fenced Task (the Host declares its one tool call Unknown,
so the Task ends ReconciliationRequired with the fence kept). Each resume attempt
is a NEW sidecar process with a new evidence directory and the same provider
(the endpoint is part of the Host authority). Controlled loopback provider only;
no actual model. Binary: ARA_JSONL_TEST_BINARY, default
`$CARGO_TARGET_DIR/debug/ara-agent-lite-jsonl.exe`.
"""
from __future__ import annotations

import asyncio
import copy
import hashlib
import json
import os
from pathlib import Path
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
TOKEN = 'controlled-resume-no-provider-grant'
# Without the profile and ProgramData variables, Windows shell code in the
# child writes a literal `%SystemDrive%` folder into its working directory.
ENV_KEYS = ('PATH', 'SYSTEMROOT', 'SYSTEMDRIVE', 'TEMP', 'TMP', 'PATHEXT', 'COMSPEC', 'PROGRAMDATA',
            'ALLUSERSPROFILE', 'USERPROFILE', 'LOCALAPPDATA', 'APPDATA')
CORRELATION = ('version', 'rpc_id', 'task_id', 'run_id')


def child_env():
    env = {key: value for key, value in os.environ.items() if key.upper() in ENV_KEYS}
    env['ARA_AGENT_MODEL_TOKEN'] = TOKEN
    return env


def receipt(**changes):
    value = {'committed': True, 'settlement': 'Settled', 'tool_calls': [], 'evidence': ['host-operation-log:checked']}
    value.update(changes)
    return value


def reply_to(frame, value):
    return {'kind': 'reconcile_reply', **{key: frame[key] for key in CORRELATION}, 'receipt': value}


class Fenced:
    """A Task left fenced by its first owner, and the bytes that must not change."""

    def __init__(self, root: Path, config: dict, provider: ControlledProvider):
        self.root, self.config, self.provider = root, config, provider
        session = Path(config['run']['session_path'])
        fences = list(session.parent.glob('.ara-lite-harness-*.fence.json'))
        assert len(fences) == 1, fences
        self.fence = fences[0]
        self.fence_bytes = self.fence.read_bytes()
        self.outcome = Path(config['task']['evidence_dir']) / 'outcome.json'
        self.outcome_bytes = self.outcome.read_bytes()
        self.requests = (len(provider.requests), provider.catalog_requests)
        self.attempts = 0

    @property
    def fence_sha256(self):
        return hashlib.sha256(self.fence_bytes).hexdigest()

    def resume(self, **changes):
        self.attempts += 1
        value = copy.deepcopy(self.config)
        value['task']['evidence_dir'] = str(self.root / f'resume-evidence-{self.attempts}')
        value['run']['run_id'] = 'run-resume'
        value['run']['objective'] = 'Continue after the Host settlement.'
        value['resume'] = {'version': 1, 'fence_sha256': self.fence_sha256}
        for path, replacement in changes.items():
            target = value
            keys = path.split('.')
            for key in keys[:-1]:
                target = target[key]
            target[keys[-1]] = replacement
        return value

    def assert_unchanged(self, case):
        case.assertEqual(self.fence.read_bytes(), self.fence_bytes, 'fence bytes unchanged')
        case.assertEqual(self.outcome.read_bytes(), self.outcome_bytes, 'original outcome receipt unchanged')
        case.assertEqual((len(self.provider.requests), self.provider.catalog_requests), self.requests,
                         'no upstream HTTP request during the refused resume')


def task_config(root: Path, task_id: str, endpoint: str, deadline_s=60):
    return {'kind': 'start_task', 'version': 1,
        'task': {'task_id': task_id, 'criteria_revision': 'c1', 'authorization_revision': 'a1',
                 'kind': 'Coding', 'evidence_dir': str(root / 'first-evidence'), 'max_runs': 3, 'max_calls': 10,
                 'deadline_unix_ms': int(time.time() * 1000) + deadline_s * 1000},
        'run': {'run_id': 'run-1', 'objective': 'Deploy once.', 'cwd': str(root),
                'session_path': str(root / 'session.json'), 'environment_state': str(root / 'environment'),
                'device_fp': 'resume-fixture', 'session_key': 'resume/session', 'mode': 'execute',
                'endpoint': endpoint, 'model': 'jsonl-fixture', 'max_calls': 4, 'max_seconds': 25, 'max_output': 512,
                'model_resolution': {'kind': 'catalog'},
                'tools': [{'name': 'deploy', 'description': 'Host-owned deploy step.',
                           'parameters': {'type': 'object', 'properties': {'step': {'type': 'string'}},
                                          'required': ['step']}}]},
        'cancel_grace_ms': 1000}


async def drive(config, respond, stdin_bytes=None, timeout=60):
    """Drive one sidecar. `respond(frame)` returns bytes to write, `None`, or 'close'."""
    proc = await asyncio.create_subprocess_exec(str(BINARY), stdin=asyncio.subprocess.PIPE,
            stdout=asyncio.subprocess.PIPE, stderr=asyncio.subprocess.PIPE, env=child_env())
    proc.stdin.write(stdin_bytes if stdin_bytes is not None else encoded(config))
    await proc.stdin.drain()
    frames = []
    async with asyncio.timeout(timeout):
        while line := await proc.stdout.readline():
            frame = json.loads(line)
            frames.append(frame)
            answer = respond(frame)
            if answer == 'close' or frame['kind'] == 'task_result':
                proc.stdin.close()
            elif answer is not None:
                proc.stdin.write(answer)
                await proc.stdin.drain()
        stderr = await proc.stderr.read()
        code = await proc.wait()
    return frames, stderr.decode('utf-8', 'replace').strip(), code


def host_reply(frame, reconcile=None):
    """The scripted Host: every tool call is declared Unknown, every candidate Verified."""
    if frame['kind'] == 'tool_call':
        return encoded({'kind': 'tool_reply', **{key: frame[key] for key in CORRELATION + (
            'native_call_id', 'tool_name', 'args_sha256')},
            'result': {'value': None, 'is_error': False, 'settlement': 'Unknown',
                       'operation_receipt': 'host-deploy-' + frame['rpc_id']}})
    if frame['kind'] == 'verify_request':
        return encoded({'kind': 'verify_reply', **{key: frame[key] for key in CORRELATION},
                        'receipt': {'basis': frame['candidate']['basis'], 'committed': True, 'settlement': 'Settled',
                                    'verdict': 'Verified', 'evidence': ['controlled-review-only']}})
    if frame['kind'] == 'reconcile_request' and reconcile is not None:
        return reconcile(frame)
    return None


async def make_fenced(root: Path, task_id: str, provider, deadline_s=60) -> 'Fenced':
    """Run the first owner to a kept fence: its one tool call is declared Unknown."""
    config = task_config(root, task_id, provider.endpoint, deadline_s)
    frames, stderr, code = await drive(config, host_reply)
    assert code == 0, stderr
    # An Unknown tool settlement fails the Bridge, which cancels the Task.
    assert frames[-1]['stop'] in ('Cancelled', 'ReconciliationRequired'), frames[-1]
    assert frames[-1]['fence_presence'] == 'Present', frames[-1]
    return Fenced(root, config, provider)


class JsonlResumeTest(unittest.IsolatedAsyncioTestCase):
    def setUp(self):
        self.assertTrue(BINARY.is_file(), f'build the sidecar first: {BINARY}')
        self.root = Path(tempfile.mkdtemp(prefix='ara-jsonl-resume-'))

    async def raw(self, config, respond, stdin_bytes=None, timeout=60):
        return await drive(config, respond, stdin_bytes, timeout)

    async def fenced(self, provider, deadline_s=60):
        return await make_fenced(self.root, 'resume-' + self._testMethodName, provider, deadline_s)

    async def refused(self, fenced, config, respond=lambda frame: None, stdin_bytes=None):
        frames, stderr, code = await self.raw(config, respond, stdin_bytes)
        self.assertEqual(code, 2)
        fenced.assert_unchanged(self)
        return frames, stderr

    def rejection(self, config):
        return json.loads((Path(config['task']['evidence_dir']) / 'jsonl-resume-rejected.json').read_text(encoding='utf-8'))

    # 9.3: invalid `resume` fields are start rejections before anything else.
    async def test_invalid_resume_field_is_a_start_rejection(self):
        with ControlledProvider([call('op-1', 'deploy', {'step': 'a'}), text('Done.')]) as provider:
            fenced = await self.fenced(provider)
            good = fenced.fence_sha256
            for name, value in (('wrong_type', good), ('null', None), ('empty', {}), ('missing_fence', {'version': 1}),
                                ('missing_version', {'fence_sha256': good}),
                                ('unknown_subfield', {'version': 1, 'fence_sha256': good, 'task_id': 'x'}),
                                ('version_2', {'version': 2, 'fence_sha256': good}),
                                ('uppercase_hex', {'version': 1, 'fence_sha256': good.upper()}),
                                ('short_hex', {'version': 1, 'fence_sha256': good[1:]})):
                with self.subTest(case=name):
                    config = fenced.resume(resume=value)
                    frames, stderr = await self.refused(fenced, config)
                    self.assertEqual((frames, stderr), ([], 'start_config_rejected'))
                    self.assertFalse(Path(config['task']['evidence_dir']).exists())

    # 9.4: a valid field naming no fenced Task, or another fence, is refused before any request.
    async def test_missing_or_other_fence_is_refused_without_frames(self):
        with ControlledProvider([call('op-1', 'deploy', {'step': 'a'}), text('Done.')]) as provider:
            fenced = await self.fenced(provider)
            other = self.root / 'unfenced'
            other.mkdir()
            cases = {
                'no_fence': fenced.resume(**{'run.session_path': str(other / 'session.json')}),
                'other_fence_sha256': fenced.resume(**{'resume.fence_sha256': '0' * 64}),
                'task_terms_changed': fenced.resume(**{'task.criteria_revision': 'c2'}),
                'larger_run_cap': fenced.resume(**{'run.max_calls': 5}),
            }
            for name, config in cases.items():
                with self.subTest(case=name):
                    frames, stderr = await self.refused(fenced, config)
                    self.assertEqual((frames, stderr), ([], 'task_admission_rejected'))
                    self.assertFalse(Path(config['task']['evidence_dir']).exists())

    # 9.1, 10.1, 10.2, 10.3: one resume, one request, the reply written during Admission is read.
    async def test_valid_resume_settles_once_and_continues_in_the_original_task(self):
        with ControlledProvider([call('op-1', 'deploy', {'step': 'a'}), text('Done after settlement.')]) as provider:
            fenced = await self.fenced(provider)
            config = fenced.resume()
            settled = receipt()
            frames, stderr, code = await self.raw(config, lambda frame: host_reply(frame, lambda f: encoded(reply_to(f, settled))))
            self.assertEqual(code, 0, stderr)
            self.assertEqual([frame['kind'] for frame in frames], ['reconcile_request', 'verify_request', 'task_result'])
            request = frames[0]
            self.assertEqual(set(request), {'kind', 'version', 'rpc_id', 'task_id', 'run_id', 'review'})
            self.assertEqual((request['version'], request['rpc_id'], request['task_id'], request['run_id']),
                             (1, 'rpc-1', config['task']['task_id'], 'run-resume'))
            review = request['review']
            self.assertEqual(review['fence'], json.loads(fenced.fence_bytes))
            self.assertEqual(review['task_id'], config['task']['task_id'])
            self.assertEqual(review['fenced_outcome'], json.loads(fenced.outcome_bytes))
            self.assertTrue(os.path.samefile(review['resume_evidence_dir'], config['task']['evidence_dir']))
            evidence = Path(config['task']['evidence_dir'])
            self.assertEqual(json.loads((evidence / 'jsonl-rpc-1-request.json').read_text(encoding='utf-8')), request)
            self.assertEqual(json.loads((evidence / 'jsonl-rpc-1-reply.json').read_text(encoding='utf-8')),
                             reply_to(request, settled))
            self.assertFalse((evidence / 'jsonl-resume-rejected.json').exists())
            # The same Task continued: the old fence was taken over, no new Task was admitted.
            resume = json.loads((evidence / 'resume.json').read_text(encoding='utf-8'))
            self.assertEqual(resume['kind'], 'fenced_task_resume')
            self.assertEqual(resume['receipt'], settled)
            result = frames[-1]
            self.assertEqual((result['task_id'], result['stop'], result['fence_presence']),
                             (config['task']['task_id'], 'AwaitingHostAcceptance', 'Released'))
            self.assertFalse(fenced.fence.exists())
            self.assertEqual(len(provider.requests), fenced.requests[0] + 1)
            self.assertEqual(fenced.outcome.read_bytes(), fenced.outcome_bytes, 'the fenced outcome is history')

    # 10.5, 10.6: a mismatched or invalid reply is never handed to Harness.
    async def test_mismatched_or_invalid_reply_refuses_without_side_effects(self):
        with ControlledProvider([call('op-1', 'deploy', {'step': 'a'}), text('Done.')]) as provider:
            fenced = await self.fenced(provider)
            def missing(frame):
                value = reply_to(frame, receipt())
                del value['receipt']['evidence']
                return encoded(value)
            cases = {
                'other_rpc_id': (lambda f: encoded({**reply_to(f, receipt()), 'rpc_id': 'rpc-9'}), 'rpc_mismatch'),
                'other_run_id': (lambda f: encoded({**reply_to(f, receipt()), 'run_id': 'run-1'}), 'rpc_mismatch'),
                'other_task_id': (lambda f: encoded({**reply_to(f, receipt()), 'task_id': 'other'}), 'rpc_mismatch'),
                'version_2': (lambda f: encoded({**reply_to(f, receipt()), 'version': 2}), 'rpc_mismatch'),
                'missing_receipt_field': (missing, 'invalid_reply_fields'),
                'extra_receipt_field': (lambda f: encoded(reply_to(f, receipt(replayed=False))), 'invalid_reply_fields'),
                'wrong_type': (lambda f: encoded(reply_to(f, receipt(committed='true'))), 'invalid_reply_fields'),
                'not_json': (lambda f: b'{"kind":"reconcile_reply",\n', 'invalid_reply_fields'),
            }
            for name, (respond, category) in cases.items():
                with self.subTest(case=name):
                    config = fenced.resume()
                    frames, stderr = await self.refused(fenced, config, lambda f, r=respond: r(f) if f['kind'] == 'reconcile_request' else None)
                    self.assertEqual(([frame['kind'] for frame in frames], stderr), (['reconcile_request'], 'task_admission_rejected'))
                    self.assertEqual(self.rejection(config), {'category': category, 'no_replay': True})
                    evidence = Path(config['task']['evidence_dir'])
                    self.assertTrue((evidence / 'jsonl-rpc-1-request.json').is_file())
                    self.assertFalse((evidence / 'jsonl-rpc-1-reply.json').exists())
                    self.assertFalse((evidence / 'resume.json').exists())

    # 10.3, 10.6: a correlated reply is checked by Harness; its typed refusal names the category.
    async def test_harness_refusal_of_a_correlated_receipt_keeps_the_fence(self):
        with ControlledProvider([call('op-1', 'deploy', {'step': 'a'}), text('Done.')]) as provider:
            fenced = await self.fenced(provider)
            cases = {
                'receipt_uncommitted': receipt(committed=False),
                'settlement_unknown': receipt(settlement='Unknown'),
                'evidence_empty': receipt(evidence=[]),
                'call_mismatch': receipt(tool_calls=[{'call_id': 'op-1', 'settlement': {'kind': 'not_executed'}}]),
            }
            for category, value in cases.items():
                with self.subTest(case=category):
                    config = fenced.resume()
                    frames, stderr = await self.refused(fenced, config, lambda f, v=value: encoded(reply_to(f, v))
                                                        if f['kind'] == 'reconcile_request' else None)
                    self.assertEqual(([frame['kind'] for frame in frames], stderr), (['reconcile_request'], 'task_admission_rejected'))
                    self.assertEqual(self.rejection(config), {'category': category, 'no_replay': True})
                    evidence = Path(config['task']['evidence_dir'])
                    self.assertTrue((evidence / 'jsonl-rpc-1-reply.json').is_file(), 'reply persisted before Harness checked it')
                    self.assertFalse((evidence / 'resume.json').exists())

    # 10.4: stdin closed before a reply.
    async def test_stdin_closed_before_the_reply_refuses(self):
        with ControlledProvider([call('op-1', 'deploy', {'step': 'a'}), text('Done.')]) as provider:
            fenced = await self.fenced(provider)
            config = fenced.resume()
            frames, stderr = await self.refused(fenced, config, lambda f: 'close' if f['kind'] == 'reconcile_request' else None)
            self.assertEqual(([frame['kind'] for frame in frames], stderr), (['reconcile_request'], 'task_admission_rejected'))
            self.assertEqual(self.rejection(config), {'category': 'stdin_closed', 'no_replay': True})

    # 10.4: deadline without a reply: callback_cancel, the grace, then reconcile_timeout; never resent.
    async def test_deadline_without_a_reply_cancels_and_never_resends(self):
        with ControlledProvider([call('op-1', 'deploy', {'step': 'a'}), text('Done.')]) as provider:
            fenced = await self.fenced(provider, deadline_s=8)
            config = fenced.resume()
            deadline = config['task']['deadline_unix_ms'] / 1000
            frames, stderr = await self.refused(fenced, config)
            ended = time.time()
            self.assertEqual(([frame['kind'] for frame in frames], stderr),
                             (['reconcile_request', 'callback_cancel'], 'task_admission_rejected'))
            self.assertEqual({key: frames[1][key] for key in CORRELATION}, {key: frames[0][key] for key in CORRELATION})
            self.assertEqual(self.rejection(config), {'category': 'reconcile_timeout', 'no_replay': True})
            self.assertGreaterEqual(ended, deadline + 0.9, 'the grace is waited after the deadline')
            self.assertLess(ended, deadline + 10)
            self.assertFalse((Path(config['task']['evidence_dir']) / 'jsonl-rpc-2-request.json').exists())


if __name__ == '__main__':
    unittest.main()
