"""The resume examples in docs/protocol-v1.md run as documented (requirements 13.4, 13.5).

Examples are read straight from the Markdown (`jsonl-example` code blocks); this
file holds no copy of them. Only the placeholders listed in the document's
placeholder table are replaced. Each example sequence runs against a Task
fenced the way the document describes: the same start frame without `resume`
(evidence `<ROOT>/evidence-first`, Run `run-1`), killed while its `deploy` call
`call-1` waits for the Host. Then every `from=host` block is written to a new
sidecar in order and the sidecar's frame kinds are compared with `expect`;
every `from=sidecar` block is compared by kind and field names with the actual
frame. Controlled loopback provider only; no actual model.
"""
from __future__ import annotations

import copy
import hashlib
import json
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
from test_jsonl_resume import BINARY
from test_resume_process import Sidecar, snapshot

DOC = HERE.parent / 'docs' / 'protocol-v1.md'
STEP_S = 30
BLOCK = re.compile(r'^```jsonl-example ([^\n]*)\n(.*?)^```$', re.M | re.S)
PLACEHOLDER = re.compile(r'<[A-Z0-9_]+>')
# The replacement rules this test implements; they must equal the document's table.
RULES = {'<ROOT>', '<ENDPOINT>', '<DEADLINE_UNIX_MS>', '<FENCE_SHA256>', '<CANDIDATE_BASIS>', '<SHA256>'}
BODIES = ('review', 'candidate', 'receipt', 'outcome_ref')


def examples():
    source = DOC.read_text(encoding='utf-8')
    section = source[source.index('### resume 示例'):]
    table = set(re.findall(r'^\| `"?(<[A-Z0-9_]+>)"?` \|', section, re.M))
    sequences: dict[str, list[dict]] = {}
    for info, body in BLOCK.findall(source):
        fields = dict(part.split('=', 1) for part in info.split())
        name, number = fields['id'].rsplit('.', 1)
        sequences.setdefault(name, []).append({'id': fields['id'], 'number': int(number), 'from': fields['from'],
                                               'expect': fields['expect'].split(','), 'text': body,
                                               'json': json.loads(body)})
    for steps in sequences.values():
        steps.sort(key=lambda step: step['number'])
    return table, sequences


class Context:
    def __init__(self, root: Path, endpoint: str, deadline: int):
        self.root, self.endpoint, self.deadline = root, endpoint, deadline
        self.fence_sha256 = None
        self.basis = None

    def fill(self, value):
        if isinstance(value, dict):
            return {key: self.fill(item) for key, item in value.items()}
        if isinstance(value, list):
            return [self.fill(item) for item in value]
        if not isinstance(value, str):
            return value
        if value == '<DEADLINE_UNIX_MS>':
            return self.deadline
        if value == '<CANDIDATE_BASIS>':
            assert self.basis is not None, 'no verify_request before <CANDIDATE_BASIS>'
            return self.basis
        if value == '<ENDPOINT>':
            return self.endpoint
        if value == '<FENCE_SHA256>':
            assert self.fence_sha256 is not None
            return self.fence_sha256
        if value.startswith('<ROOT>'):
            return self.root.as_posix() + value[len('<ROOT>'):]
        assert not PLACEHOLDER.search(value), f'undocumented placeholder in a Host frame: {value}'
        return value


class ProtocolDocExamplesTest(unittest.TestCase):
    def setUp(self):
        self.assertTrue(BINARY.is_file(), f'build the sidecar first: {BINARY}')
        self.table, self.sequences = examples()
        self.temp = Path(tempfile.mkdtemp(prefix='ara-doc-examples-'))

    def test_examples_use_only_documented_placeholders_and_cover_13_5(self):
        self.assertEqual(self.table, RULES, 'the placeholder table and the replacement rules differ')
        for steps in self.sequences.values():
            for step in steps:
                used = set(PLACEHOLDER.findall(step['text']))
                self.assertLessEqual(used, self.table, step['id'])
                if step['from'] == 'host':
                    self.assertNotIn('<SHA256>', used, step['id'])
                self.assertNotIn('ARA_AGENT_MODEL_TOKEN', step['text'])
                self.assertNotRegex(step['text'], r'(?i)\bbearer\b|api[_-]?key|\bsk-[a-z0-9]{8,}', step['id'])
        ok = self.sequences['resume-ok']
        self.assertEqual([(step['from'], step['json']['kind']) for step in ok],
                         [('host', 'start_task'), ('sidecar', 'reconcile_request'), ('host', 'reconcile_reply'),
                          ('sidecar', 'verify_request'), ('host', 'verify_reply'), ('sidecar', 'task_result')])
        self.assertIn('resume', ok[0]['json'])
        invalid = self.sequences['resume-invalid-field']
        self.assertEqual((invalid[0]['json']['kind'], invalid[0]['expect']), ('start_task', ['none']))
        mismatch = self.sequences['resume-rpc-mismatch']
        self.assertEqual([step['json']['kind'] for step in mismatch], ['start_task', 'reconcile_reply'])
        self.assertNotEqual(mismatch[1]['json']['rpc_id'], ok[1]['json']['rpc_id'])

    def fenced(self, name):
        """A Task fenced as the document describes: killed during `deploy` call `call-1`."""
        root = self.temp / name
        (root / 'workspace').mkdir(parents=True)
        (root / 'state').mkdir()
        provider = ControlledProvider([call('call-1', 'deploy', {'step': 'a'}), text('Deployed after the Host settlement.')])
        provider.__enter__()
        self.addCleanup(provider.__exit__, None, None, None)
        context = Context(root, provider.endpoint, int(time.time() * 1000) + 180_000)
        start = copy.deepcopy(self.sequences['resume-ok'][0]['json'])
        del start['resume']
        first = context.fill(start)
        first['task']['evidence_dir'] = (root / 'evidence-first').as_posix()
        first['run']['run_id'] = 'run-1'
        first['run']['objective'] = 'Deploy step a.'
        owner = Sidecar(first, lambda frame: None)
        self.addCleanup(owner.kill)
        limit = time.monotonic() + 10
        while 'tool_call' not in owner.kinds() and time.monotonic() < limit and owner.proc.poll() is None:
            time.sleep(0.05)
        self.assertEqual(owner.kinds(), ['tool_call'], 'the deploy call did not start within 10 s')
        owner.proc.kill()
        owner.proc.wait(timeout=5)
        fences = list((root / 'state').glob('.ara-lite-harness-*.fence.json'))
        self.assertEqual(len(fences), 1)
        context.fence_sha256 = hashlib.sha256(fences[0].read_bytes()).hexdigest()
        kept = (fences[0].read_bytes(), snapshot(root / 'evidence-first'), len(provider.requests))
        return context, provider, fences[0], kept

    def run_sequence(self, name):
        context, provider, fence, kept = self.fenced(name)
        sidecar, emitted, matched = None, 0, 0
        steps = self.sequences[name]
        for step in steps:
            with self.subTest(example=step['id']):
                if step['from'] == 'sidecar':
                    self.assertLess(matched, len(sidecar.frames), step['id'])
                    actual, documented = sidecar.frames[matched], step['json']
                    matched += 1
                    self.assertEqual((actual['kind'], documented['kind']), (step['expect'][0], step['expect'][0]))
                    self.assertEqual(set(actual), set(documented), f"{step['id']}: field names")
                    for body in BODIES:
                        if isinstance(documented.get(body), dict):
                            self.assertEqual(set(actual[body]), set(documented[body]), f"{step['id']}: {body} field names")
                    continue
                frame = context.fill(step['json'])
                if sidecar is None:
                    sidecar = Sidecar(frame, lambda frame: None)
                    self.addCleanup(sidecar.kill)
                else:
                    sidecar.proc.stdin.write(encoded(frame))
                    sidecar.proc.stdin.flush()
                if step['expect'] == ['none']:
                    code, _ = sidecar.finish(STEP_S)
                    self.assertEqual(code, 2, step['id'])
                    self.assertEqual(len(sidecar.frames), emitted, f"{step['id']}: no further frame")
                    continue
                emitted += len(step['expect'])
                limit = time.monotonic() + STEP_S
                while len(sidecar.frames) < emitted and time.monotonic() < limit and sidecar.proc.poll() is None:
                    time.sleep(0.05)
                if sidecar.proc.poll() is not None:
                    sidecar.reader.join(5)  # an exited sidecar: read every frame it wrote
                self.assertEqual(sidecar.kinds()[emitted - len(step['expect']):emitted], step['expect'],
                                 f"{step['id']}: frames {sidecar.kinds()}")
                verify = [item for item in sidecar.frames[:emitted] if item['kind'] == 'verify_request']
                context.basis = verify[-1]['candidate']['basis'] if verify else None
        if steps[-1]['expect'] != ['none']:
            code, stderr = sidecar.finish(STEP_S)
            self.assertEqual(code, 0, stderr)
            self.assertEqual(len(sidecar.frames), emitted, f'no undocumented frame: {sidecar.kinds()}')
        return context, provider, fence, kept, sidecar

    def assert_refused(self, context, provider, fence, kept):
        self.assertEqual(fence.read_bytes(), kept[0], 'fence bytes unchanged')
        self.assertEqual(snapshot(context.root / 'evidence-first'), kept[1], 'original receipts unchanged')
        self.assertEqual(len(provider.requests), kept[2], 'no upstream request')

    def test_successful_resume_sequence(self):
        context, provider, fence, kept, sidecar = self.run_sequence('resume-ok')
        documented = self.sequences['resume-ok'][-1]['json']
        result = sidecar.frames[-1]
        self.assertEqual({key: result[key] for key in ('stop', 'task_accepted', 'fence_presence')},
                         {key: documented[key] for key in ('stop', 'task_accepted', 'fence_presence')})
        review = sidecar.frames[0]['review']
        self.assertEqual(review['interrupted_run']['open_tool_calls'],
                         self.sequences['resume-ok'][1]['json']['review']['interrupted_run']['open_tool_calls'])
        self.assertEqual(review['consumed_calls'], self.sequences['resume-ok'][1]['json']['review']['consumed_calls'])
        self.assertFalse(fence.exists())

    def test_invalid_resume_field_start_frame(self):
        context, provider, fence, kept, sidecar = self.run_sequence('resume-invalid-field')
        self.assertEqual(sidecar.stderr.decode('utf-8').strip(), 'start_config_rejected')
        self.assert_refused(context, provider, fence, kept)

    def test_reconcile_reply_with_another_request_id(self):
        context, provider, fence, kept, sidecar = self.run_sequence('resume-rpc-mismatch')
        self.assertEqual(sidecar.stderr.decode('utf-8').strip(), 'task_admission_rejected')
        rejected = context.root / 'evidence-resume' / 'jsonl-resume-rejected.json'
        self.assertEqual(json.loads(rejected.read_text(encoding='utf-8')), {'category': 'rpc_mismatch', 'no_replay': True})
        self.assert_refused(context, provider, fence, kept)


if __name__ == '__main__':
    unittest.main()
