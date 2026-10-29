"""Shared entry point of the lite-eval Oracle adapters for S1, S2, S3, R1, H2.

Usage of every adapter:

    python tools/lite-eval/oracles/<task>.py --artifacts <dir> --final-text <file> --out <json>

<dir> is the Oracle-directory copy of the task workspace and <file> holds the
model's final response text. The adapters reuse the judging functions of the
existing live-trial scripts in crates/ara-agent-lite-core/tests and never call
a model. --out must not exist; scratch output goes to a new sibling directory
`<out stem>-work`. The JSON written to --out is:

    {"schema": "lite-eval-oracle-v1", "task_id": "S3",
     "status": "PASS" | "FAIL" | "ERROR",
     "public": {"passed": n, "total": m} | null,
     "hidden": {"passed": n, "total": m} | null,
     "predictions": {"passed": n, "total": m} | null,
     "reason": null | "...", "work_dir": "...", "detail": "<work_dir>/detail.json" | null}

Counts are present only when cases were actually executed. A FAIL without
counts means the candidate was rejected before execution (contract file
changed, build failed, submission missing). Exit status 0 means a judgement
(PASS or FAIL); 1 means an Oracle execution error (ERROR), whose traceback is
kept in `<work_dir>/oracle-error.txt`.
"""
import argparse
import ast
import json
import re
import shutil
import sys
import traceback
import xml.etree.ElementTree as ET
from pathlib import Path

REPO = Path(__file__).resolve().parents[3]
sys.path.insert(0, str(REPO / 'crates' / 'ara-agent-lite-core' / 'tests'))

SCHEMA = 'lite-eval-oracle-v1'


class Rejected(Exception):
    """The candidate fails without any case being executed."""


def counts(passed, total):
    return {'passed': passed, 'total': total}


def write_detail(work, value):
    path = work / 'detail.json'
    with path.open('x', encoding='utf-8', newline='\n') as handle:
        json.dump(value, handle, ensure_ascii=False, indent=2, default=str)
        handle.write('\n')
    return str(path)


def main(task_id, judge):
    parser = argparse.ArgumentParser(description=f'lite-eval Oracle adapter for {task_id}')
    parser.add_argument('--artifacts', type=Path, required=True)
    parser.add_argument('--final-text', type=Path, required=True)
    parser.add_argument('--out', type=Path, required=True)
    args = parser.parse_args()
    out = args.out.resolve()
    if out.exists():
        parser.error(f'{out} already exists; every evaluation writes a new result')
    work = out.with_name(out.stem + '-work')
    work.mkdir(parents=True, exist_ok=False)
    result = {'schema': SCHEMA, 'task_id': task_id, 'status': 'ERROR', 'public': None, 'hidden': None,
              'predictions': None, 'reason': None, 'work_dir': str(work), 'detail': None}
    code = 1
    try:
        text = args.final_text.read_text(encoding='utf-8')
        result.update(judge(args.artifacts.resolve(), text, work))
        code = 0
    except Rejected as error:
        result.update(status='FAIL', reason=str(error))
        code = 0
    except Exception as error:  # Oracle execution error: keep the evidence, never a pass.
        result['reason'] = f'{type(error).__name__}: {error}'
        (work / 'oracle-error.txt').write_text(traceback.format_exc(), encoding='utf-8', newline='\n')
    with out.open('x', encoding='utf-8', newline='\n') as handle:
        json.dump(result, handle, ensure_ascii=False, indent=2)
        handle.write('\n')
    return code


def cpp_judge(task):
    """S1/S2: verify_analysis_live.cpp_oracle on the candidate tree."""

    def judge(artifacts, text, work):
        from verify_analysis_live import PRIVATE, cpp_oracle
        from verify_coding_live import digest

        plan = json.loads((PRIVATE / 'private-plan.json').read_text(encoding='utf-8'))
        cases_path = PRIVATE / f'{task}-oracle.json'
        if digest(cases_path.read_bytes()) != plan['fixtures'][task]['oracle_sha256']:
            raise RuntimeError(f'{task} hidden oracle differs from the frozen plan')
        readme = artifacts / 'README.md'
        if not readme.is_file() or readme.read_bytes() != (PRIVATE / f'{task}-seed' / 'README.md').read_bytes():
            raise Rejected('README.md contract was changed or removed')
        compiler = shutil.which('g++')
        if compiler is None:
            raise RuntimeError('g++ is not on PATH')
        cases = json.loads(cases_path.read_text(encoding='utf-8'))
        result = cpp_oracle(artifacts, compiler, work, cases)
        detail = write_detail(work, result)
        if readme.read_bytes() != (PRIVATE / f'{task}-seed' / 'README.md').read_bytes():
            raise RuntimeError('README.md changed during evaluation')
        if result['build']['code'] != 0:
            return {'status': 'FAIL', 'reason': 'candidate build failed', 'detail': detail}
        passed = sum(1 for case in result['cases'] if case['passed'])
        return {'status': result['status'], 'hidden': counts(passed, len(cases)), 'detail': detail}

    return judge


def blocks(text):
    """The one python and one json block both reverse tasks require, or Rejected."""
    python = re.findall(r'```python\s*\n(.*?)```', text, flags=re.DOTALL | re.IGNORECASE)
    prediction = re.findall(r'```json\s*\n(.*?)```', text, flags=re.DOTALL | re.IGNORECASE)
    if len(python) != 1 or len(prediction) != 1:
        raise Rejected('submission needs exactly one python and one json block')
    try:
        ast.parse(python[0])
        parsed = json.loads(prediction[0])
    except (SyntaxError, ValueError) as error:
        raise Rejected(f'submission does not parse: {type(error).__name__}') from None
    if not isinstance(parsed, dict) or not isinstance(parsed.get('predictions'), list):
        raise Rejected('json block must be an object with a predictions list')
    return parsed['predictions']


def matches(actual, expected):
    return sum(1 for a, b in zip(actual, expected) if type(a) is int and a == b) if isinstance(actual, list) else 0


def r1_judge(artifacts, text, work):
    """R1: verify_analysis_live.reverse_oracle on the final response text."""
    from verify_analysis_live import PREDICTION_INPUTS, PRIVATE, binary_reference, reverse_oracle
    from verify_coding_live import digest

    plan = json.loads((PRIVATE / 'private-plan.json').read_text(encoding='utf-8'))
    cases_path = PRIVATE / 'R1-oracle.json'
    if digest(cases_path.read_bytes()) != plan['fixtures']['R1']['oracle_sha256']:
        raise RuntimeError('R1 hidden oracle differs from the frozen plan')
    predicted = blocks(text)
    cases = json.loads(cases_path.read_text(encoding='utf-8'))
    result = reverse_oracle(text, work, cases)
    detail = write_detail(work, result)
    try:
        actual = json.loads(result['execution']['stdout'])
    except ValueError:
        actual = None
    expected_predictions = [binary_reference(bytes.fromhex(value)) for value in PREDICTION_INPUTS]
    return {'status': result['status'], 'hidden': counts(matches(actual, [c['expected'] for c in cases]), len(cases)),
            'predictions': counts(matches(predicted, expected_predictions), len(expected_predictions)),
            'detail': detail}


def h2_judge(artifacts, text, work):
    """H2: verify_reverse_holdout_live.evaluate_submission on the final response text."""
    from verify_coding_live import digest
    from verify_reverse_holdout_live import STAGE, evaluate_submission, load

    seal = (STAGE / 'manifest.sha256').read_text(encoding='ascii').strip()
    if digest((STAGE / 'manifest.json').read_bytes()) != seal:
        raise RuntimeError('H2 stage manifest seal changed')
    manifest = load(STAGE / 'manifest.json')
    oracle_path, oracle_hash = Path(manifest['oracle']['path']), manifest['oracle']['sha256']
    inputs = manifest['prediction_inputs']
    predicted = blocks(text)
    result = evaluate_submission(text, work, oracle_path, oracle_hash, inputs)
    detail = write_detail(work, result)
    if result['status'] == 'NOT_RUN':
        raise Rejected(result['reason'])
    mapping = {item['hex']: item['expected'] for item in load(oracle_path)}
    return {'status': result['status'], 'hidden': counts(result['hidden_outputs_matched'], result['hidden_case_count']),
            'predictions': counts(matches(predicted, [mapping[value] for value in inputs]), len(inputs)),
            'detail': detail}


def s3_judge(artifacts, text, work):
    """S3: verify_stream_coding_live.seal_sources + fresh_evaluate (fresh cmake/ctest build, private 48)."""
    from verify_stream_coding_live import STAGE, fixture, fresh_evaluate, original_projection, seal_sources

    manifest = fixture(STAGE)
    try:
        original_projection(artifacts, manifest['public_pins'])
    except AssertionError as error:
        raise Rejected(f'contract violation: {error}') from None
    seal, seal_hash = seal_sources(artifacts, work, manifest['public_pins'], text)
    result = fresh_evaluate(work, manifest, seal, seal_hash)
    detail = write_detail(work, result)
    if 'ctest' not in result:
        return {'status': 'FAIL', 'reason': 'candidate configure or build failed', 'detail': detail}
    try:
        cases = list(ET.parse(work / 'independent-evaluation' / 'ctest.xml').getroot().iter('testcase'))
    except (ET.ParseError, OSError):
        cases = []
    public = sum(1 for c in cases if c.find('failure') is None and c.find('skipped') is None)
    return {'status': result['status'], 'public': counts(public, manifest['public_tests']),
            'hidden': counts(result['hidden_passed'], result['hidden_total']), 'detail': detail}
