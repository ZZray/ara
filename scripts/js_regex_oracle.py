"""Capture native Bun 1.4.0 RegExp behavior with lossless UTF-16 inputs/outputs."""
from __future__ import annotations

import argparse
import datetime as dt
import hashlib
import json
from pathlib import Path
import subprocess
import uuid

BUN_VERSION = '1.4.0'
EXPECTED_COUNTS = {
    'source': 10, 'flags': 2, 'invalid': 16, 'fold': 18, 'line': 22, 'capture': 5,
    'lookaround': 4, 'unicode-set': 12, 'modifier': 3, 'state': 11, 'empty': 9, 'surrogate': 18,
}


def sha256(data: bytes) -> str:
    return hashlib.sha256(data).hexdigest()


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--bun', required=True, type=Path)
    parser.add_argument('--output', required=True, type=Path)
    args = parser.parse_args()
    stamp = dt.datetime.now(dt.timezone.utc).strftime('%Y%m%dT%H%M%SZ')
    run = args.output.resolve() / f'run-{stamp}-{uuid.uuid4().hex[:8]}'
    run.mkdir(parents=True, exist_ok=False)
    try:
        bun = args.bun.resolve(strict=True)
        version = subprocess.run([str(bun), '--version'], capture_output=True, timeout=15, check=True)
        if version.stdout.decode().strip() != BUN_VERSION:
            raise RuntimeError(f'Expected Bun {BUN_VERSION}')
        runner = Path(__file__).with_suffix('.mjs').read_bytes()
        copied = run / 'runner.mjs'
        copied.write_bytes(runner)
        manifest = {'schemaVersion': 1, 'runtime': {'name': 'bun', 'version': BUN_VERSION,
                    'executableSha256': sha256(bun.read_bytes())}, 'runnerSha256': sha256(runner)}
        manifest_path = run / 'runtime-manifest.json'
        manifest_path.write_text(json.dumps(manifest, indent=2) + '\n', encoding='utf-8')
        result = subprocess.run([str(bun), str(copied), str(manifest_path)], cwd=run,
                                capture_output=True, timeout=120, check=False)
        (run / 'stdout.txt').write_bytes(result.stdout)
        (run / 'stderr.txt').write_bytes(result.stderr)
        if result.returncode:
            raise RuntimeError(f'Bun oracle exit {result.returncode}; see stderr.txt')
        artifact = run / 'oracle.json'
        oracle = json.loads(artifact.read_text(encoding='utf-8'))
        if any(oracle[key] != manifest[key] for key in ('schemaVersion', 'runtime', 'runnerSha256')):
            raise RuntimeError('executed runtime/runner receipt mismatch')
        cases = oracle['cases']
        if len({case['id'] for case in cases}) != len(cases):
            raise RuntimeError('duplicate case IDs')
        counts = {}
        for case in cases:
            counts[case['family']] = counts.get(case['family'], 0) + 1
            for value in [case['sourceUnits'], *(step['inputUnits'] for step in case['steps'])]:
                if any(type(unit) is not int or not 0 <= unit <= 65535 for unit in value):
                    raise RuntimeError('invalid UTF-16 input units')
            expected = case['expected']
            if expected['status'] == 'ok' and len(expected['steps']) != len(case['steps']):
                raise RuntimeError('incomplete sequential exec output')
        if counts != oracle['counts'] or counts != EXPECTED_COUNTS:
            raise RuntimeError('family inventory mismatch')
        summary = {'status': 'PASS', 'oracle': str(artifact), 'oracleSha256': sha256(artifact.read_bytes()),
                   'cases': len(cases), 'counts': counts, **manifest}
        (run / 'receipt.json').write_text(json.dumps(summary, indent=2) + '\n', encoding='utf-8')
        print(json.dumps(summary))
        return 0
    except Exception as error:
        failure = {'status': 'FAIL', 'error': str(error), 'run': str(run)}
        if isinstance(error, subprocess.TimeoutExpired):
            (run / 'stdout.txt').write_bytes(error.stdout or b'')
            (run / 'stderr.txt').write_bytes(error.stderr or b'')
        (run / 'failure.json').write_text(json.dumps(failure) + '\n', encoding='utf-8')
        print(json.dumps(failure))
        return 1


if __name__ == '__main__':
    raise SystemExit(main())
