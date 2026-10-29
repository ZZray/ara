"""Real process interruption and resume of a JSONL Task (requirement 14).

One sidecar owns a Task whose first Run consumes 2 model calls and whose
second Run (admitted with a 4-call cap) emits a `deploy_step` tool call. The
Host tool writes its side-effect counter as 1 and then blocks; the test kills
the sidecar with `Popen.kill()` (TerminateProcess on Windows, no cleanup) and
reads the Session with `examples/session_probe.rs`. Every resume attempt is a
NEW sidecar with a new evidence directory, the same controlled provider and
the same deadline. Every timeout is a failure (14.6); nothing is skipped.

Refusals the Harness makes before the Host callback (authority changed, larger
budget or Run cap) follow the documented start rule: zero stdout frames,
stderr `task_admission_rejected`, no new evidence directory, so no
`jsonl-resume-rejected.json` exists to name them; those cases assert that rule.
Controlled loopback provider only; no actual model.
"""
from __future__ import annotations

import copy
import hashlib
import json
import os
from pathlib import Path
import subprocess
import sys
import tempfile
import threading
import time
import unittest

HERE = Path(__file__).resolve().parent
sys.path[:0] = [str(HERE.parent / 'clients/python'), str(HERE)]
from ara_agent_lite import encoded
from controlled_provider import ControlledProvider, call, text
from test_jsonl_resume import BINARY, CORRELATION, child_env

EXE = '.exe' if os.name == 'nt' else ''
PROBE = Path(os.environ.get('ARA_SESSION_PROBE', str(BINARY.parent / 'examples' / ('session_probe' + EXE))))
DEPLOY = {'id': 'op-deploy', 'name': 'deploy_step', 'arguments': {'step': 'one'}}
TOOL_STARTED_S, KILL_EXIT_S, RESUME_DONE_S, REFUSAL_S = 10, 5, 30, 30
TASK_CALLS, RUN_CAP = 10, 4


class Sidecar:
    """One sidecar process; `respond(frame)` returns bytes, None or 'close'."""

    def __init__(self, config, respond):
        flags = {'creationflags': subprocess.CREATE_NO_WINDOW} if os.name == 'nt' else {}
        self.proc = subprocess.Popen([str(BINARY)], stdin=subprocess.PIPE, stdout=subprocess.PIPE,
                                     stderr=subprocess.PIPE, env=child_env(), **flags)
        self.started = time.monotonic()
        self.frames, self.respond, self.stderr = [], respond, b''
        self.proc.stdin.write(encoded(config))
        self.proc.stdin.flush()
        self.reader = threading.Thread(target=self._read, daemon=True)
        self.errors = threading.Thread(target=self._errors, daemon=True)
        self.reader.start()
        self.errors.start()

    def _read(self):
        for line in self.proc.stdout:
            frame = json.loads(line)
            self.frames.append(frame)
            answer = self.respond(frame)
            try:
                if answer == 'close' or frame['kind'] == 'task_result':
                    self.proc.stdin.close()
                elif answer is not None:
                    self.proc.stdin.write(answer)
                    self.proc.stdin.flush()
            except OSError:
                pass

    def _errors(self):
        self.stderr = self.proc.stderr.read()

    def finish(self, seconds):
        try:
            code = self.proc.wait(timeout=max(0, self.started + seconds - time.monotonic()))
        except subprocess.TimeoutExpired:
            self.kill()
            raise AssertionError(f'sidecar did not finish within {seconds} s; frames {self.kinds()}') from None
        self.reader.join(5)
        self.errors.join(5)
        self.close()
        return code, self.stderr.decode('utf-8', 'replace').strip()

    def kill(self):
        if self.proc.poll() is None:
            self.proc.kill()
            self.proc.wait(timeout=KILL_EXIT_S)
        self.reader.join(5)
        self.errors.join(5)
        self.close()

    def close(self):
        for stream in (self.proc.stdin, self.proc.stdout, self.proc.stderr):
            try:
                stream.close()
            except OSError:
                pass

    def kinds(self):
        return [frame['kind'] for frame in self.frames]


def correlated(frame, **body):
    return encoded({**{key: frame[key] for key in CORRELATION}, **body})


def snapshot(*roots: Path):
    return {str(path): path.read_bytes() for root in roots for path in sorted(root.rglob('*')) if path.is_file()}


class ResumeProcessTest(unittest.TestCase):
    def setUp(self):
        self.assertTrue(BINARY.is_file(), f'build the sidecar first: {BINARY}')
        self.assertTrue(PROBE.is_file(), f'build the probe first (cargo build --example session_probe): {PROBE}')
        self.root = Path(tempfile.mkdtemp(prefix='ara-resume-process-'))
        self.receipts = self.root / 'host-receipts'
        self.receipts.mkdir()
        self.counter = self.root / 'deploy-counter.txt'
        self.counter.write_text('0', encoding='utf-8')
        self.sidecars: list[Sidecar] = []
        self.addCleanup(lambda: [sidecar.kill() for sidecar in self.sidecars])

    def count(self):
        return int(self.counter.read_text(encoding='utf-8'))

    def sidecar(self, config, respond):
        sidecar = Sidecar(config, respond)
        self.sidecars.append(sidecar)
        return sidecar

    def deploy(self, frame):
        """The Host tool: count the effect, keep its own receipt, then block (no reply)."""
        self.counter.write_text(str(self.count() + 1), encoding='utf-8')
        (self.receipts / f"deploy-{frame['native_call_id']}.json").write_text(
            json.dumps({'native_call_id': frame['native_call_id'], 'arguments': frame['arguments'],
                        'status': 'applied'}), encoding='utf-8')
        return None

    def config(self):
        deadline = int(time.time() * 1000) + 240_000
        return {'kind': 'start_task', 'version': 1,
            'task': {'task_id': 'resume-process', 'criteria_revision': 'c1', 'authorization_revision': 'a1',
                     'kind': 'Coding', 'evidence_dir': str(self.root / 'first-evidence'), 'max_runs': 3,
                     'max_calls': TASK_CALLS, 'deadline_unix_ms': deadline},
            'run': {'run_id': 'run-1', 'objective': 'Look up the target, then deploy step one.', 'cwd': str(self.root),
                    'session_path': str(self.root / 'session.json'), 'environment_state': str(self.root / 'environment'),
                    'device_fp': 'resume-process', 'session_key': 'resume/process', 'mode': 'execute',
                    'endpoint': self.provider.endpoint, 'model': 'jsonl-fixture', 'max_calls': RUN_CAP,
                    'max_seconds': 60, 'max_output': 512, 'model_resolution': {'kind': 'catalog'},
                    'tools': [{'name': 'lookup', 'description': 'Host-owned lookup.',
                               'parameters': {'type': 'object', 'properties': {'q': {'type': 'string'}}}},
                              {'name': 'deploy_step', 'description': 'Host-owned deploy step with an external effect.',
                               'parameters': {'type': 'object', 'properties': {'step': {'type': 'string'}},
                                              'required': ['step']}}]},
            'cancel_grace_ms': 1000}

    def first_owner(self, frame):
        kind = frame['kind']
        if kind == 'tool_call' and frame['tool_name'] == 'lookup':
            return correlated(frame, kind='tool_reply', **{key: frame[key] for key in ('native_call_id', 'tool_name', 'args_sha256')},
                              result={'value': {'found': True}, 'is_error': False, 'settlement': 'Settled',
                                      'operation_receipt': 'host-lookup-' + frame['rpc_id']})
        if kind == 'tool_call':
            return self.deploy(frame)
        if kind == 'verify_request':
            return correlated(frame, kind='verify_reply', receipt={
                'basis': frame['candidate']['basis'], 'committed': True, 'settlement': 'Settled',
                'verdict': {'NeedsChanges': {'feedback': 'Not deployed yet; deploy step one.'}},
                'evidence': ['host-check:first-attempt']})
        if kind == 'prepare_request':
            return correlated(frame, kind='prepare_reply', run_id_next='run-2', objective='Deploy step one now.')
        return None

    def receipt(self, **changes):
        value = {'committed': True, 'settlement': 'Settled',
                 'tool_calls': [{'call_id': DEPLOY['id'], 'settlement': {'kind': 'completed',
                                 'output': 'deploy step one applied once (Host receipt deploy-op-deploy.json)'}}],
                 'evidence': [str(self.receipts / 'deploy-op-deploy.json')]}
        value.update(changes)
        return value

    def resumed_host(self, reply):
        """A resume Host: `reply(frame)` answers reconcile_request; a tool call is never expected."""
        def respond(frame):
            kind = frame['kind']
            if kind == 'reconcile_request':
                return reply(frame)
            if kind == 'tool_call':
                # Counted so a replay fails the counter assertion instead of hanging.
                self.counter.write_text(str(self.count() + 1), encoding='utf-8')
                return correlated(frame, kind='tool_reply', **{key: frame[key] for key in ('native_call_id', 'tool_name', 'args_sha256')},
                                  result={'value': 'replayed', 'is_error': True, 'settlement': 'Settled',
                                          'operation_receipt': 'unexpected-replay'})
            if kind == 'verify_request':
                return correlated(frame, kind='verify_reply', receipt={
                    'basis': frame['candidate']['basis'], 'committed': True, 'settlement': 'Settled',
                    'verdict': 'Verified', 'evidence': ['host-check:after-resume']})
            return None
        return respond

    def resume_config(self, original, attempt, **changes):
        value = copy.deepcopy(original)
        value['task']['evidence_dir'] = str(self.root / f'resume-evidence-{attempt}')
        value['run']['run_id'] = f'run-resume-{attempt}'
        value['run']['objective'] = 'Continue after the Host settled the interrupted deploy.'
        value['resume'] = {'version': 1, 'fence_sha256': hashlib.sha256(self.fence.read_bytes()).hexdigest()}
        for path, replacement in changes.items():
            target = value
            keys = path.split('.')
            for key in keys[:-1]:
                target = target[key]
            target[keys[-1]] = replacement
        return value

    def probe(self):
        result = subprocess.run([str(PROBE), str(self.root / 'session.json'), str(self.root)], capture_output=True,
                                env=child_env(), timeout=30)
        self.assertEqual(result.returncode, 0, result.stderr.decode('utf-8', 'replace'))
        return json.loads(result.stdout)

    def test_killed_during_a_tool_call_resumes_once_with_the_host_receipt(self):
        with ControlledProvider([call('op-lookup', 'lookup', {'q': 'target'}), text('First attempt.'),
                                 call(DEPLOY['id'], DEPLOY['name'], DEPLOY['arguments']),
                                 text('Done after settlement.')]) as provider:
            self.provider = provider
            original = self.config()
            first_evidence = Path(original['task']['evidence_dir'])

            # 14.1, 14.6: the tool counted its effect and blocks; kill the process.
            owner = self.sidecar(original, self.first_owner)
            limit = time.monotonic() + TOOL_STARTED_S
            while self.count() != 1 and time.monotonic() < limit and owner.proc.poll() is None:
                time.sleep(0.05)
            self.assertEqual(self.count(), 1, f'tool effect not counted within {TOOL_STARTED_S} s; frames {owner.kinds()}')
            self.assertIsNone(owner.proc.poll(), 'the sidecar is still waiting for the blocked tool')
            owner.proc.kill()
            try:
                owner.proc.wait(timeout=KILL_EXIT_S)
            except subprocess.TimeoutExpired:
                self.fail(f'sidecar did not exit within {KILL_EXIT_S} s of TerminateProcess')
            owner.reader.join(5)
            self.assertEqual(owner.kinds(), ['tool_call', 'verify_request', 'prepare_request', 'tool_call'])
            self.assertEqual(len(provider.requests), 3)

            fences = list(self.root.glob('.ara-lite-harness-*.fence.json'))
            self.assertEqual(len(fences), 1, 'the fence exists after the kill')
            self.fence = fences[0]
            probe = self.probe()
            self.assertEqual(probe['pending_run'], 'run-2', 'exactly one pending Run, the interrupted one')
            self.assertEqual(probe['interrupted_run']['run_id'], 'run-2')
            self.assertEqual(probe['interrupted_run']['open_tool_calls'], [DEPLOY])
            self.assertEqual(probe['interrupted_run']['raw_count'], probe['raw_count'])
            self.assertNotIn(DEPLOY['id'], [result['toolCallId'] for result in probe['tool_results']],
                             'the interrupted call has no ToolResult')
            self.assertFalse((first_evidence / 'outcome.json').exists(), 'the killed owner published nothing')

            kept = snapshot(first_evidence, self.receipts)
            fence_bytes = self.fence.read_bytes()
            requests = (len(provider.requests), provider.catalog_requests)

            def unchanged(case):
                self.assertEqual(self.fence.read_bytes(), fence_bytes, f'{case}: fence bytes')
                self.assertEqual(snapshot(first_evidence, self.receipts), kept, f'{case}: original receipts')
                self.assertEqual((len(provider.requests), provider.catalog_requests), requests, f'{case}: upstream requests')
                self.assertEqual(self.count(), 1, f'{case}: tool effect count')
                after = self.probe()
                self.assertEqual((after['pending_run'], after['interrupted_run']), (probe['pending_run'], probe['interrupted_run']),
                                 f'{case}: pending_run kept')

            # 14.4, 14.5: each refusal is its own new sidecar.
            reply = lambda value: lambda frame: correlated(frame, kind='reconcile_reply', receipt=value)
            settled = self.receipt()['tool_calls'][0]
            callback_refusals = {
                'no_receipt': (lambda frame: 'close', 'stdin_closed'),
                'uncommitted': (reply(self.receipt(committed=False)), 'receipt_uncommitted'),
                'settlement_unknown': (reply(self.receipt(settlement='Unknown')), 'settlement_unknown'),
                'evidence_empty': (reply(self.receipt(evidence=[])), 'evidence_empty'),
                'calls_missing': (reply(self.receipt(tool_calls=[])), 'call_mismatch'),
                'calls_extra': (reply(self.receipt(tool_calls=[settled, {**settled, 'call_id': 'op-extra'}])), 'call_mismatch'),
                'call_id_mismatch': (reply(self.receipt(tool_calls=[{**settled, 'call_id': 'op-other'}])), 'call_mismatch'),
            }
            attempt = 0
            for case, (answer, category) in callback_refusals.items():
                with self.subTest(case=case):
                    attempt += 1
                    config = self.resume_config(original, attempt)
                    sidecar = self.sidecar(config, self.resumed_host(answer))
                    code, stderr = sidecar.finish(REFUSAL_S)
                    self.assertEqual((code, stderr, sidecar.kinds()), (2, 'task_admission_rejected', ['reconcile_request']))
                    evidence = Path(config['task']['evidence_dir'])
                    self.assertEqual(json.loads((evidence / 'jsonl-resume-rejected.json').read_text(encoding='utf-8')),
                                     {'category': category, 'no_replay': True})
                    unchanged(case)
            for case, changes in {'authority_changed': {'run.device_fp': 'other-device'},
                                  'task_budget_larger': {'task.max_calls': TASK_CALLS + 1},
                                  'run_cap_larger': {'run.max_calls': RUN_CAP + 1}}.items():
                with self.subTest(case=case):
                    attempt += 1
                    config = self.resume_config(original, attempt, **changes)
                    sidecar = self.sidecar(config, self.resumed_host(lambda frame: None))
                    code, stderr = sidecar.finish(REFUSAL_S)
                    self.assertEqual((code, stderr, sidecar.kinds()), (2, 'task_admission_rejected', []))
                    self.assertFalse(Path(config['task']['evidence_dir']).exists())
                    unchanged(case)

            # 14.2, 14.3, 14.6: the valid receipt continues the same Session within 30 s.
            attempt += 1
            config = self.resume_config(original, attempt)
            sidecar = self.sidecar(config, self.resumed_host(reply(self.receipt())))
            code, stderr = sidecar.finish(RESUME_DONE_S)
            self.assertEqual(code, 0, stderr)
            self.assertEqual(sidecar.kinds(), ['reconcile_request', 'verify_request', 'task_result'])
            review = sidecar.frames[0]['review']
            self.assertEqual(review['interrupted_run']['open_tool_calls'], [DEPLOY])
            self.assertEqual((sidecar.frames[-1]['stop'], sidecar.frames[-1]['fence_presence']),
                             ('AwaitingHostAcceptance', 'Released'))
            self.assertEqual(self.count(), 1, 'the interrupted tool was not called again')
            evidence = Path(config['task']['evidence_dir'])
            resume = json.loads((evidence / 'resume.json').read_text(encoding='utf-8'))
            # Budget 10; Run 1 observed 2; Run 2 was admitted with 4 and completed 1 before the kill.
            self.assertEqual((resume['consumed_runs'], resume['consumed_calls']), (2, 2 + RUN_CAP))
            admitted = json.loads((evidence / 'run-3-admission.json').read_text(encoding='utf-8'))
            self.assertEqual(admitted['remaining_calls'], TASK_CALLS - (2 + RUN_CAP))
            self.assertEqual(admitted['remaining_calls'], 4)
            first = json.loads((first_evidence / 'run-1-admission.json').read_text(encoding='utf-8'))
            self.assertEqual(admitted['session_path'], first['session_path'], 'the same Session continued')
            after = self.probe()
            self.assertIsNone(after['pending_run'])
            self.assertIsNone(after['interrupted_run'])
            self.assertGreater(after['raw_count'], probe['raw_count'])
            settled_results = [result for result in after['tool_results'] if result['toolCallId'] == DEPLOY['id']]
            self.assertEqual(settled_results, [{'toolCallId': DEPLOY['id'], 'toolName': DEPLOY['name'], 'isError': False,
                                                'source': 'host_reconciled_interrupted_run'}])
            self.assertEqual(len(provider.requests), 4, 'one upstream request after the settlement')
            sent = [message for message in provider.requests[-1]['messages']
                    if message.get('role') == 'tool' and message.get('tool_call_id') == DEPLOY['id']]
            self.assertEqual(len(sent), 1, 'the Host result reached the upstream exactly once')
            self.assertIn('Host reconciliation of an interrupted call (completed)', json.dumps(sent[0], ensure_ascii=False))
            self.assertFalse(self.fence.exists())


if __name__ == '__main__':
    unittest.main()
