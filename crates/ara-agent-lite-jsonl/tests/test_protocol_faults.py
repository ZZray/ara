"""Physical JSONL/RPC fault admission against the actual shared Rust process.

Only the model provider is controlled. Host file writes, stdin/stdout, native
Journal and Harness fences are real. No Cargo, product, database or device runs.
"""
from __future__ import annotations

import asyncio
import copy
import hashlib
import json
import os
from pathlib import Path
import subprocess
import sys
import time
import unittest

HERE = Path(__file__).resolve().parent
sys.path.insert(0, str(HERE.parent / 'clients/python'))
from ara_agent_lite import encoded, read_reference
from controlled_provider import ControlledProvider, call, text

FRAME_BYTES = 262144
BINARY = Path(os.environ.get('ARA_JSONL_TEST_BINARY',
    'D:/Temp/ara-build/ara-github-target-20261005/debug/ara-agent-lite-jsonl.exe'))
EVIDENCE = Path(os.environ.get('ARA_JSONL_PROTOCOL_EVIDENCE',
    'D:/WorkSpace/tmp/ara-core-capability-20261022/product-host-adapters-v1-20261025/protocol-faults-v1'))
TOKEN = 'protocol-fixture-no-provider-grant'


def sha(path: Path) -> str:
    return hashlib.sha256(path.read_bytes()).hexdigest()


def save(path: Path, value) -> None:
    with path.open('x', encoding='utf-8') as stream:
        json.dump(value, stream, ensure_ascii=False, indent=2)
        stream.write('\n')
        stream.flush()
        os.fsync(stream.fileno())


class ProtocolFaultTest(unittest.IsolatedAsyncioTestCase):
    @classmethod
    def setUpClass(cls):
        if not BINARY.is_file():
            raise RuntimeError('actual Rust sidecar required; missing binary is not a skip/pass')
        EVIDENCE.mkdir()  # Preserve any previous run; callers choose another fresh directory.
        cls.binary_sha256 = sha(BINARY)
        save(EVIDENCE / 'identity.json', {
            'binary': str(BINARY), 'binary_sha256': cls.binary_sha256,
            'python': sys.executable, 'python_version': sys.version,
            'test_source': str(Path(__file__).resolve()), 'test_sha256': sha(Path(__file__)),
            'provider_source': str(HERE / 'controlled_provider.py'),
            'provider_sha256': sha(HERE / 'controlled_provider.py'),
            'frame_bytes_including_lf': FRAME_BYTES,
            'actual_native_process': True, 'controlled_provider_only': True,
            'cargo_product_database_device_real_model': 'NOT_RUN',
        })

    async def asyncSetUp(self):
        self.root = EVIDENCE / self._testMethodName
        self.root.mkdir()
        self.procs = []
        self.frames = []
        self.host_effects = 0
        self.stdout = self.root / 'sidecar-stdout.jsonl'
        self.stderr = self.root / 'sidecar-stderr.bin'
        self.stderr_task = None

    async def asyncTearDown(self):
        forced = []
        for proc in self.procs:
            if proc.returncode is None:
                proc.kill()
                await asyncio.wait_for(proc.wait(), timeout=5)
                forced.append(proc.pid)
        if self.stderr_task:
            await asyncio.wait_for(self.stderr_task, timeout=5)
        save(self.root / 'cleanup.json', {'forced_kill_pids': forced,
             'forced_cleanup_does_not_prove_external_rollback': True})
        self.assertEqual(sha(BINARY), self.binary_sha256, 'binary changed during this evidence run')

    def config(self, endpoint):
        config = {'kind': 'start_task', 'version': 1,
            'task': {'task_id': 'protocol-' + self._testMethodName,
                     'criteria_revision': 'actual-owned-file-v1',
                     'authorization_revision': 'single-owned-write-v1', 'kind': 'Coding',
                     'evidence_dir': str(self.root / 'harness'), 'max_runs': 2, 'max_calls': 4,
                     'deadline_unix_ms': int(time.time() * 1000) + 15000},
            'run': {'run_id': 'original-run', 'objective': 'Use write_answer once and submit the actual file.',
                    'cwd': str(self.root), 'session_path': str(self.root / 'session.json'),
                    'environment_state': str(self.root / 'environment'),
                    'device_fp': 'protocol-fixture', 'session_key': 'protocol/original-session',
                    'mode': 'execute', 'endpoint': endpoint, 'model': 'jsonl-fixture',
                    'max_calls': 4, 'max_seconds': 12, 'max_output': 512,
                    'model_resolution': {'kind': 'catalog'},
                    'tools': [{'name': 'write_answer', 'description': 'Write the one Host-owned fixture file.',
                        'parameters': {'type': 'object', 'properties': {'content': {'type': 'string'}},
                                       'required': ['content'], 'additionalProperties': False},
                        'allow_in_plan': False}]}, 'cancel_grace_ms': 1000, 'emit_events': False}
        save(self.root / 'start.json', config)
        return config

    async def start_raw(self, config):
        self.assertEqual(sha(BINARY), self.binary_sha256)
        env = {key: value for key, value in os.environ.items()
               if key in ('PATH', 'SYSTEMROOT', 'SystemRoot', 'TEMP', 'TMP', 'HOME', 'USERPROFILE', 'LANG')}
        env['ARA_AGENT_MODEL_TOKEN'] = TOKEN
        options = {'creationflags': subprocess.CREATE_NO_WINDOW} if os.name == 'nt' else {}
        proc = await asyncio.create_subprocess_exec(str(BINARY), stdin=asyncio.subprocess.PIPE,
            stdout=asyncio.subprocess.PIPE, stderr=asyncio.subprocess.PIPE,
            env=env, limit=FRAME_BYTES + 1, **options)
        self.procs.append(proc)

        async def errors():
            with self.stderr.open('xb') as stream:
                while chunk := await proc.stderr.read(8192):
                    stream.write(chunk)
                stream.flush()
                os.fsync(stream.fileno())
        self.stderr_task = asyncio.create_task(errors())
        await self.write_raw(proc, encoded(config), 'sent-start.bin')
        return proc

    async def write_raw(self, proc, payload, filename):
        (self.root / filename).write_bytes(payload)
        result = 'drained'
        try:
            proc.stdin.write(payload)
            await asyncio.wait_for(proc.stdin.drain(), timeout=4)
        except (BrokenPipeError, ConnectionResetError):
            # A peer that has already refused the physical frame may close its
            # input. Actual terminal/fence evidence is still mandatory below.
            result = 'peer_closed_during_drain'
        save(self.root / (filename + '.receipt.json'), {
            'attempted_bytes': len(payload), 'sha256': hashlib.sha256(payload).hexdigest(),
            'write_status': result, 'all_bytes_claimed_received': result == 'drained'})

    async def read_until(self, proc, kind):
        while True:
            raw = await asyncio.wait_for(proc.stdout.readline(), timeout=8)
            self.assertTrue(raw, 'native sidecar exited without required original terminal')
            with self.stdout.open('ab') as stream:
                stream.write(raw)
                stream.flush()
                os.fsync(stream.fileno())
            frame = json.loads(raw)
            self.frames.append(frame)
            if frame['kind'] == kind:
                return frame

    def effect_once(self, request):
        self.assertEqual(request['kind'], 'tool_call')
        self.assertEqual(request['tool_name'], 'write_answer')
        self.assertEqual(self.host_effects, 0)
        self.host_effects += 1
        save(self.root / 'original-operation-started.json', {'request': request, 'state': 'started'})
        with (self.root / 'answer.txt').open('x', encoding='utf-8', newline='') as stream:
            stream.write(request['arguments']['content'])
            stream.flush()
            os.fsync(stream.fileno())
        completed = self.root / 'original-operation-completed.json'
        save(completed, {'state': 'completed', 'host_effects': self.host_effects,
                         'answer_sha256': sha(self.root / 'answer.txt')})
        reply = {key: value for key, value in request.items() if key != 'arguments'}
        reply['kind'] = 'tool_reply'
        reply['result'] = {'value': {'written': True}, 'is_error': False,
                           'settlement': 'Settled', 'operation_receipt': str(completed)}
        return reply

    async def finish(self, proc):
        terminal = await self.read_until(proc, 'task_result')
        outcome = read_reference(terminal['outcome_ref'], self.root / 'harness')
        await asyncio.wait_for(proc.wait(), timeout=5)
        await asyncio.wait_for(self.stderr_task, timeout=5)
        remainder = await proc.stdout.read()
        self.assertFalse(remainder, 'unexpected output after the one terminal')
        save(self.root / 'terminal.json', terminal)
        save(self.root / 'outcome.json', outcome)
        save(self.root / 'process.json', {'pid': proc.pid, 'exit_code': proc.returncode,
             'stdout_sha256': sha(self.stdout), 'stderr_sha256': sha(self.stderr),
             'host_effects': self.host_effects, 'tool_callbacks': sum(f['kind'] == 'tool_call' for f in self.frames),
             'remote_rollback_claimed': False})
        self.assertEqual(proc.returncode, 0, self.stderr.read_text(errors='replace'))
        return outcome

    def preserved_effect(self):
        self.assertEqual(self.host_effects, 1)
        self.assertEqual((self.root / 'answer.txt').read_text(), 'written-once')
        self.assertTrue((self.root / 'original-operation-started.json').is_file())
        self.assertTrue((self.root / 'original-operation-completed.json').is_file())
        self.assertTrue((self.root / 'session.json').is_file())
        # Session checkpoint and Journal are retained separately; record all
        # actual files instead of guessing one filename as a complete ledger.
        native = []
        for path in sorted(self.root.rglob('*')):
            if path.is_file() and ('session' in path.name or 'journal' in str(path.relative_to(self.root)).lower()):
                native.append({'path': str(path), 'sha256': sha(path), 'bytes': path.stat().st_size})
        self.assertTrue(native)
        save(self.root / 'retained-native-files.json', native)
        rows = [json.loads(line) for line in (self.root / 'harness/run-1-events.jsonl').read_bytes().splitlines()]
        self.assertEqual(sum(row['event']['type'] == 'tool_start' and row['event'].get('title') == 'write_answer' for row in rows), 1)
        self.assertEqual(sum(frame['kind'] == 'tool_call' for frame in self.frames), 1)

    async def assert_fault(self, proc, config, provider, require_unknown=True):
        outcome = await self.finish(proc)
        self.assertIn(outcome['stop'], ('Cancelled', 'ReconciliationRequired'))
        self.assertFalse(outcome['task_accepted'])
        self.assertEqual(outcome['fence_presence'], 'Present')
        self.assertTrue(Path(outcome['reconciliation_fence']).is_file())
        self.assertFalse(any(frame['kind'] in ('verify_request', 'prepare_request') for frame in self.frames))
        self.preserved_effect()
        if require_unknown:
            unknown = json.loads((self.root / 'harness/jsonl-rpc-1-unknown.json').read_bytes())
            self.assertEqual(unknown['external_settlement'], 'unknown')
            self.assertTrue(unknown['no_replay'])
            self.assertFalse((self.root / 'harness/jsonl-rpc-1-reply.json').exists())
        requests_before = len(provider.requests)
        self.assertIn(requests_before, (1, 2))
        save(self.root / 'provider-requests.json', provider.requests)
        save(self.root / 'fault-boundary.json', {
            'original_host_file_effect': 'known completed once',
            'transport_rpc_settlement': 'unknown' if require_unknown else 'first reply may have settled before duplicate fault',
            'original_task_acceptance': False, 'fence': 'Present',
            'no_remote_join_or_rollback_inferred': True})
        # A separate native process attempts the SAME Session. This is a denial
        # probe, not a new conversation or replay of the original operation.
        reentry = copy.deepcopy(config)
        reentry['task']['task_id'] += '-contender'
        reentry['task']['evidence_dir'] = str(self.root / 'contender-evidence')
        reentry['run']['run_id'] = 'contender-run-never-admitted'
        save(self.root / 'contender-start.json', reentry)
        env = {**os.environ, 'ARA_AGENT_MODEL_TOKEN': TOKEN}
        options = {'creationflags': subprocess.CREATE_NO_WINDOW} if os.name == 'nt' else {}
        contender = await asyncio.create_subprocess_exec(str(BINARY), stdin=asyncio.subprocess.PIPE,
            stdout=asyncio.subprocess.PIPE, stderr=asyncio.subprocess.PIPE, env=env, **options)
        self.procs.append(contender)
        out, err = await asyncio.wait_for(contender.communicate(encoded(reentry)), timeout=6)
        (self.root / 'contender-stdout.bin').write_bytes(out)
        (self.root / 'contender-stderr.bin').write_bytes(err)
        save(self.root / 'contender-process.json', {'exit_code': contender.returncode,
             'stdout_sha256': hashlib.sha256(out).hexdigest(), 'stderr_sha256': hashlib.sha256(err).hexdigest()})
        self.assertNotEqual(contender.returncode, 0)
        self.assertIn(b'task_admission_rejected', err)
        self.assertEqual(out, b'')
        self.assertFalse((self.root / 'contender-evidence').exists())
        self.assertEqual(len(provider.requests), requests_before)
        self.assertEqual(self.host_effects, 1)
        self.assertEqual((self.root / 'answer.txt').read_text(), 'written-once')

    async def test_duplicate_rpc_reply_does_not_replay_a_known_host_effect(self):
        with ControlledProvider([call('original', 'write_answer', {'content': 'written-once'}),
                                 text('Not independent acceptance.', delay_ms=400)]) as provider:
            config = self.config(provider.endpoint)
            proc = await self.start_raw(config)
            request = await self.read_until(proc, 'tool_call')
            reply = self.effect_once(request)
            payload = encoded(reply)
            await self.write_raw(proc, payload + payload, 'duplicate-reply.bin')
            await self.assert_fault(proc, config, provider, require_unknown=False)
            stored = json.loads((self.root / 'harness/jsonl-rpc-1-reply.json').read_bytes())
            self.assertEqual(stored, reply)

    async def test_physical_frame_one_byte_over_256k_is_refused_after_one_effect(self):
        with ControlledProvider([call('original', 'write_answer', {'content': 'written-once'})]) as provider:
            config = self.config(provider.endpoint)
            proc = await self.start_raw(config)
            reply = self.effect_once(await self.read_until(proc, 'tool_call'))
            normal = encoded(reply)
            payload = normal[:-1] + b' ' * (FRAME_BYTES + 1 - len(normal)) + b'\n'
            self.assertEqual(len(payload), FRAME_BYTES + 1)
            self.assertEqual(json.loads(payload), reply)
            self.assertLess(len(encoded(reply['result'])), 131072)
            await self.write_raw(proc, payload, 'physical-overlimit.bin')
            await self.assert_fault(proc, config, provider)

    async def test_unterminated_json_reply_then_eof_preserves_original_operation(self):
        with ControlledProvider([call('original', 'write_answer', {'content': 'written-once'})]) as provider:
            config = self.config(provider.endpoint)
            proc = await self.start_raw(config)
            reply = self.effect_once(await self.read_until(proc, 'tool_call'))
            payload = encoded(reply)[:-1]
            self.assertEqual(json.loads(payload), reply)  # JSON is valid; physical LF is missing.
            await self.write_raw(proc, payload, 'unterminated-reply.bin')
            proc.stdin.close()
            await self.assert_fault(proc, config, provider)

    async def test_bare_eof_during_original_callback_preserves_unknown_receipt(self):
        with ControlledProvider([call('original', 'write_answer', {'content': 'written-once'})]) as provider:
            config = self.config(provider.endpoint)
            proc = await self.start_raw(config)
            self.effect_once(await self.read_until(proc, 'tool_call'))
            save(self.root / 'injected-eof.json', {'reply_bytes': 0, 'after_actual_host_effect': True})
            proc.stdin.close()
            await self.assert_fault(proc, config, provider)

    async def test_exact_256k_physical_reply_is_admitted_when_semantic_result_is_small(self):
        with ControlledProvider([call('original', 'write_answer', {'content': 'written-once'}),
                                 text('Actual file submitted.')]) as provider:
            config = self.config(provider.endpoint)
            proc = await self.start_raw(config)
            reply = self.effect_once(await self.read_until(proc, 'tool_call'))
            normal = encoded(reply)
            payload = normal[:-1] + b' ' * (FRAME_BYTES - len(normal)) + b'\n'
            self.assertEqual(len(payload), FRAME_BYTES)
            self.assertEqual(json.loads(payload), reply)
            await self.write_raw(proc, payload, 'physical-at-limit.bin')
            verify = await self.read_until(proc, 'verify_request')
            candidate = verify.get('candidate') or read_reference(verify['candidate_ref'], self.root / 'harness')
            evidence = self.root / 'actual-file-review.json'
            save(evidence, {'basis': candidate['basis'], 'actual_answer_sha256': sha(self.root / 'answer.txt'),
                           'matches': (self.root / 'answer.txt').read_text() == 'written-once'})
            review = {key: verify[key] for key in ('version', 'rpc_id', 'task_id', 'run_id')}
            review.update(kind='verify_reply', receipt={'basis': candidate['basis'], 'committed': True,
                'settlement': 'Settled', 'verdict': 'Verified', 'evidence': [str(evidence) + '#sha256=' + sha(evidence)]})
            await self.write_raw(proc, encoded(review), 'sent-review.bin')
            outcome = await self.finish(proc)
            self.assertEqual(outcome['stop'], 'AwaitingHostAcceptance')
            self.assertEqual(outcome['fence_presence'], 'Released')
            self.assertFalse(outcome['task_accepted'])
            self.assertEqual(outcome['runs'], 1)
            self.assertEqual(outcome['observed_calls'], 2)
            self.preserved_effect()
            self.assertEqual(len(provider.requests), 2)
            self.assertEqual(sum(frame['kind'] == 'verify_request' for frame in self.frames), 1)
            self.assertFalse(any(frame['kind'] == 'prepare_request' for frame in self.frames))
            save(self.root / 'provider-requests.json', provider.requests)


if __name__ == '__main__':
    suite = unittest.defaultTestLoader.loadTestsFromTestCase(ProtocolFaultTest)
    started = time.monotonic()
    result = unittest.TextTestRunner(verbosity=2).run(suite)
    if EVIDENCE.is_dir():
        save(EVIDENCE / 'test-result.json', {
            'tests_run': result.testsRun, 'failures': len(result.failures), 'errors': len(result.errors),
            'skipped': len(result.skipped), 'success': result.wasSuccessful(),
            'elapsed_seconds': time.monotonic() - started,
            'failures_detail': result.failures, 'errors_detail': result.errors,
            'binary_sha256_final': sha(BINARY), 'test_source_sha256_final': sha(Path(__file__)),
        })
    raise SystemExit(0 if result.wasSuccessful() else 1)
