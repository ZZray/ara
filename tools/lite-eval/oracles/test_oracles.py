"""Replay the five Oracle adapters on recorded candidates and compare with the
recorded judgements. Needs the private trial data under
D:/WorkSpace/tmp/ara-core-capability-20261022 (skipped when absent), g++ for
S1/S2, and the S3 build tools pinned in its stage manifest.

    python -m unittest discover -s tools/lite-eval/oracles -p "test_*.py"
"""
import json
import shutil
import subprocess
import sys
import tempfile
import unittest
from pathlib import Path

HERE = Path(__file__).resolve().parent
BASE = Path('D:/WorkSpace/tmp/ara-core-capability-20261022')
PRIVATE = BASE / 'analysis-private'
sys.path.insert(0, str(HERE))
import _common  # noqa: E402  (also puts the live-trial scripts on sys.path)


@unittest.skipUnless(BASE.is_dir(), 'private trial data is not on this machine')
class OracleAdapters(unittest.TestCase):
    def setUp(self):
        self.tmp = Path(tempfile.mkdtemp(prefix='lite-eval-oracle-'))
        self.addCleanup(shutil.rmtree, self.tmp, ignore_errors=True)

    def judge(self, task, tree, text=''):
        artifacts = self.tmp / 'artifacts'
        if tree is not None:
            shutil.copytree(tree, artifacts)
        else:
            artifacts.mkdir()
        final = self.tmp / 'final.txt'
        final.write_text(text, encoding='utf-8', newline='\n')
        out = self.tmp / 'oracle.json'
        process = subprocess.run([sys.executable, str(HERE / f'{task.lower()}.py'), '--artifacts', str(artifacts),
                                  '--final-text', str(final), '--out', str(out)], capture_output=True, timeout=900)
        result = json.loads(out.read_text(encoding='utf-8'))
        self.assertEqual(result['schema'], 'lite-eval-oracle-v1')
        self.assertEqual(result['task_id'], task)
        return process.returncode, result

    def test_cpp_reference_passes_and_seed_fails(self):
        for task, total in (('S1', 13), ('S2', 35)):
            with self.subTest(task=task):
                code, result = self.judge(task, PRIVATE / f'{task}-reference')
                self.assertEqual((code, result['status'], result['hidden']), (0, 'PASS', {'passed': total, 'total': total}))
                shutil.rmtree(self.tmp); self.tmp.mkdir()
                code, result = self.judge(task, PRIVATE / f'{task}-seed')
                self.assertEqual((code, result['status'], result['hidden']['total']), (0, 'FAIL', total))
                self.assertLess(result['hidden']['passed'], total)
                shutil.rmtree(self.tmp); self.tmp.mkdir()

    def test_changed_contract_is_rejected_without_counts(self):
        tree = self.tmp / 'changed'
        shutil.copytree(PRIVATE / 'S1-reference', tree)
        (tree / 'README.md').write_text('changed\n', encoding='utf-8')
        code, result = self.judge('S1', tree)
        self.assertEqual((code, result['status'], result['hidden']), (0, 'FAIL', None))
        self.assertIn('README.md', result['reason'])

    def test_r1_exact_reconstruction_passes_and_missing_submission_fails(self):
        from verify_analysis_live import PREDICTION_INPUTS, binary_reference
        import inspect
        source = inspect.getsource(binary_reference).replace('def binary_reference(', 'def recover(')
        predictions = [binary_reference(bytes.fromhex(value)) for value in PREDICTION_INPUTS]
        text = f'```python\n{source}```\n```json\n{json.dumps({"predictions": predictions})}\n```\n'
        code, result = self.judge('R1', BASE / 'analysis-live/R1/base/project', text)
        self.assertEqual((code, result['status'], result['hidden'], result['predictions']),
                         (0, 'PASS', {'passed': 28, 'total': 28}, {'passed': 3, 'total': 3}))
        shutil.rmtree(self.tmp); self.tmp.mkdir()
        code, result = self.judge('R1', None, 'no code here')
        self.assertEqual((code, result['status'], result['hidden']), (0, 'FAIL', None))

    def test_h2_recorded_submission_reproduces_48_of_128(self):
        packets = [json.loads(line) for line in
                   (BASE / 'reverse-holdout-live-v1/H2-D/host.stdout.native.jsonl').read_text(encoding='utf-8').splitlines()
                   if line.strip()]
        text = next(packet for packet in reversed(packets) if packet.get('kind') == 'result')['text']
        code, result = self.judge('H2', BASE / 'reverse-holdout-stage/public-template', text)
        self.assertEqual((code, result['status'], result['hidden'], result['predictions']),
                         (0, 'FAIL', {'passed': 48, 'total': 128}, {'passed': 0, 'total': 4}))

    def test_s3_recorded_candidates_reproduce_their_judgements(self):
        for tree, status, hidden in (('full-core-s3-stage-v3/independent-final/sealed-source', 'PASS', 48),
                                     ('coding-stream-live-v1/S3-base/sealed-source', 'FAIL', 4)):
            with self.subTest(tree=tree):
                code, result = self.judge('S3', BASE / tree)
                self.assertEqual((code, result['status'], result['hidden']),
                                 (0, status, {'passed': hidden, 'total': 48}))
                self.assertEqual(result['public']['total'], 10)
                shutil.rmtree(self.tmp); self.tmp.mkdir()


if __name__ == '__main__':
    unittest.main()
