"""Execute unchanged fixed OMP discovery sources in isolated Bun processes.

The resulting corpus is evidence of the explicitly inventoried scenarios only.
No network credentials are read and no original source assertions are rewritten.
"""
from __future__ import annotations
import argparse
import datetime as dt
import hashlib
import json
from pathlib import Path
import subprocess
import shutil
import uuid
from ctx_skill_invocation_oracle import UPSTREAM_COMMIT, BUN_VERSION, command, export_blob, git


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--upstream', required=True, type=Path)
    parser.add_argument('--bun', required=True, type=Path)
    parser.add_argument('--output', required=True, type=Path)
    parser.add_argument('--fixture', action='append', type=Path, default=[])
    parser.add_argument('--families', default='')
    parser.add_argument('--source-run', type=Path, help='Reuse an earlier byte-verified source export')
    args = parser.parse_args()
    stamp = dt.datetime.now(dt.timezone.utc).strftime('%Y%m%dT%H%M%SZ')
    run = args.output.resolve() / f'run-{stamp}-{uuid.uuid4().hex[:8]}'
    run.mkdir(parents=True, exist_ok=False)
    try:
        upstream, bun = args.upstream.resolve(strict=True), args.bun.resolve(strict=True)
        if git(upstream, 'rev-parse', f'{UPSTREAM_COMMIT}^{{commit}}').decode().strip() != UPSTREAM_COMMIT:
            raise RuntimeError('fixed upstream commit unavailable')
        if command([str(bun), '--version']).decode().strip() != BUN_VERSION:
            raise RuntimeError('wrong Bun version')
        if args.source_run:
            previous = args.source_run.resolve(strict=True)
            prior = json.loads((previous / 'source-manifest.json').read_text(encoding='utf-8'))
            if prior['upstreamCommit'] != UPSTREAM_COMMIT:
                raise RuntimeError('reused source commit differs')
            entries, licenses = prior['sourceManifest'], prior['licenses']
            fixed_tree = git(upstream, 'ls-tree', '-r', '--format=%(objectname) %(path)',
                             UPSTREAM_COMMIT, 'packages/catalog/src', 'packages/catalog/test',
                             'packages/omptype/src', 'packages/utils/src', 'packages/utils/package.json',
                             'LICENSE', 'packages/catalog/LICENSE', 'packages/omptype/LICENSE',
                             'packages/utils/LICENSE').decode().splitlines()
            fixed_blobs = dict((line.split(' ', 1)[1], line.split(' ', 1)[0]) for line in fixed_tree)
            if {entry['path'] for entry in entries + licenses} != set(fixed_blobs):
                raise RuntimeError('reused export omits fixed source inventory')
            for entry in entries + licenses:
                content = (previous / 'upstream' / entry['path']).read_bytes()
                object_bytes = b'blob ' + str(len(content)).encode('ascii') + b'\0' + content
                if (fixed_blobs[entry['path']] != entry['gitBlob']
                        or hashlib.sha256(content).hexdigest() != entry['sha256']
                        or hashlib.sha1(object_bytes).hexdigest() != entry['gitBlob']):
                    raise RuntimeError(f'reused source bytes differ: {entry["path"]}')
            shutil.copytree(previous / 'upstream', run / 'upstream')
        else:
            paths = git(upstream, 'ls-tree', '-r', '--name-only', UPSTREAM_COMMIT,
                        'packages/catalog/src', 'packages/catalog/test', 'packages/omptype/src',
                        'packages/utils/src').decode().splitlines()
            paths += ['packages/utils/package.json']
            entries = [export_blob(upstream, run, path) for path in paths]
            licenses = [export_blob(upstream, run, path) for path in (
                'LICENSE', 'packages/catalog/LICENSE', 'packages/omptype/LICENSE', 'packages/utils/LICENSE')]
        runner = Path(__file__).with_suffix('.mjs').read_bytes()
        (run / 'runner.mjs').write_bytes(runner)
        adapters = []
        for index, source in enumerate(args.fixture):
            content = source.resolve(strict=True).read_bytes()
            target = f'fixture-{index}.mjs'
            (run / target).write_bytes(content)
            adapters.append({'path': target, 'sha256': hashlib.sha256(content).hexdigest(), 'source': str(source.resolve())})
        manifest = {'schemaVersion': 1, 'upstreamCommit': UPSTREAM_COMMIT, 'bunVersion': BUN_VERSION,
                    'bunSha256': hashlib.sha256(bun.read_bytes()).hexdigest(),
                    'runnerSha256': hashlib.sha256(runner).hexdigest(),
                    'sourceManifest': entries, 'licenses': licenses, 'adapters': adapters,
                    'families': [value for value in args.families.split(',') if value]}
        manifest_path = run / 'source-manifest.json'
        manifest_path.write_text(json.dumps(manifest, indent=2) + '\n', encoding='utf-8')
        result = subprocess.run([str(bun), str(run / 'runner.mjs'), str(manifest_path)], cwd=run,
                                capture_output=True, timeout=1200, check=False)
        (run / 'stdout.txt').write_bytes(result.stdout)
        (run / 'stderr.txt').write_bytes(result.stderr)
        if result.returncode:
            raise RuntimeError(f'Bun oracle exit {result.returncode}; see stderr.txt')
        artifact = run / 'oracle.json'
        oracle = json.loads(artifact.read_text(encoding='utf-8'))
        for key in ('schemaVersion', 'upstreamCommit', 'bunVersion', 'bunSha256', 'runnerSha256', 'sourceManifest'):
            if oracle[key] != manifest[key]:
                raise RuntimeError(f'provenance mismatch: {key}')
        cases = oracle['cases']
        if not cases or len({case['id'] for case in cases}) != len(cases):
            raise RuntimeError('empty or duplicate case inventory')
        counts = {}
        for case in cases:
            counts[case['family']] = counts.get(case['family'], 0) + 1
            if 'expected' not in case:
                raise RuntimeError(f'missing result: {case["id"]}')
        if counts != oracle['counts'] or oracle['isolation']['processCount'] != len(cases):
            raise RuntimeError('incomplete scenario execution')
        receipt = {'status': 'PASS', 'oracle': str(artifact), 'cases': len(cases), 'counts': counts,
                   'oracleSha256': hashlib.sha256(artifact.read_bytes()).hexdigest(),
                   'runnerSha256': manifest['runnerSha256'], 'originalTestAssertionsExecuted': False,
                   'scope': 'Explicit scenario corpus; not a whole-discovery acceptance claim'}
        (run / 'receipt.json').write_text(json.dumps(receipt, indent=2) + '\n', encoding='utf-8')
        print(json.dumps(receipt))
        return 0
    except Exception as error:
        if isinstance(error, subprocess.TimeoutExpired):
            (run / 'stdout.txt').write_bytes(error.stdout or b'')
            (run / 'stderr.txt').write_bytes(error.stderr or b'')
        failure = {'status': 'FAIL', 'error': str(error), 'run': str(run)}
        (run / 'failure.json').write_text(json.dumps(failure) + '\n', encoding='utf-8')
        print(json.dumps(failure))
        return 1


if __name__ == '__main__':
    raise SystemExit(main())
