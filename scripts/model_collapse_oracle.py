"""Run unchanged fixed OMP collapse exports; preserve native state and wire values."""
from __future__ import annotations
import argparse
import datetime as dt
import hashlib
import json
from pathlib import Path
import subprocess
import uuid
from ctx_skill_invocation_oracle import UPSTREAM_COMMIT, BUN_VERSION, command, export_blob, git
from model_policy_oracle import CATALOG_FILES, UTILITY_FILES, FIXTURE_FILES

EXPECTED_COUNTS = {
    'reviewed-table': 11, 'reviewed-family': 1642, 'stale-family': 67, 'extra-alias': 2,
    'reviewed-alias': 67, 'reviewed-template': 18, 'template-first': 8, 'default-repair': 10, 'provider-alias': 17,
    'thinking-pair': 12, 'thinking-token': 6, 'cursor-gate': 25, 'alias-state': 5,
    'retarget': 1, 'caller-contract': 6, 'custom-table': 8, 'custom-template': 4,
    'lossless-mandatory': 15, 'inherited-marker': 3, 'catalog-built': 1, 'catalog-provider': 67,
}


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--upstream', required=True, type=Path)
    parser.add_argument('--bun', required=True, type=Path)
    parser.add_argument('--output', required=True, type=Path)
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
        paths = [f'packages/catalog/src/{p}' for p in (*CATALOG_FILES, 'compat/collapse.ts')]
        paths += [f'packages/utils/src/{p}' for p in UTILITY_FILES] + ['packages/utils/package.json']
        fixtures = [f'packages/catalog/test/{p}' for p in (*FIXTURE_FILES, 'compat-collapse.test.ts')]
        entries = [export_blob(upstream, run, p) for p in paths + fixtures]
        licenses = [export_blob(upstream, run, p) for p in ('LICENSE', 'packages/catalog/LICENSE')]
        runner = Path(__file__).with_suffix('.mjs').read_bytes()
        (run / 'runner.mjs').write_bytes(runner)
        manifest = {'schemaVersion': 1, 'upstreamCommit': UPSTREAM_COMMIT, 'bunVersion': BUN_VERSION,
                    'bunSha256': hashlib.sha256(bun.read_bytes()).hexdigest(),
                    'sourceManifest': entries, 'licenses': licenses, 'fixturePaths': fixtures,
                    'runnerSha256': hashlib.sha256(runner).hexdigest()}
        manifest_path = run / 'source-manifest.json'
        manifest_path.write_text(json.dumps(manifest, indent=2) + '\n', encoding='utf-8')
        result = subprocess.run([str(bun), str(run / 'runner.mjs'), str(manifest_path)], cwd=run,
                                capture_output=True, timeout=240, check=False)
        (run / 'stdout.txt').write_bytes(result.stdout)
        (run / 'stderr.txt').write_bytes(result.stderr)
        if result.returncode:
            raise RuntimeError(f'Bun oracle exit {result.returncode}; see {run / "stderr.txt"}')
        artifact = run / 'oracle.json'
        oracle = json.loads(artifact.read_text(encoding='utf-8'))
        for key in ('schemaVersion', 'upstreamCommit', 'bunVersion', 'bunSha256', 'runnerSha256', 'sourceManifest'):
            if oracle[key] != manifest[key]:
                raise RuntimeError(f'provenance mismatch: {key}')
        if oracle['catalogRowCount'] != 4776 or len(set(oracle['catalogKeys'])) != 4776:
            raise RuntimeError('incomplete catalog')
        cases = oracle['cases']
        if len({case['id'] for case in cases}) != len(cases):
            raise RuntimeError('duplicate sequence IDs')
        counts = {}
        for case in cases:
            counts[case['family']] = counts.get(case['family'], 0) + 1
            if len(case['steps']) != len(case['expected']):
                raise RuntimeError('incomplete sequence')
        if counts != oracle['counts'] or counts != EXPECTED_COUNTS or len(oracle['exportCounts']) != 8 or not all(oracle['exportCounts'].values()):
            raise RuntimeError('incomplete export/family inventory')
        if oracle['reviewedFamilyCount'] != 67 or oracle['reviewedTemplateCount'] != 2:
            raise RuntimeError('reviewed table inventory mismatch')
        isolation = oracle['isolation']
        collapse_source = next(source for source in entries if source['path'] == 'packages/catalog/src/compat/collapse.ts')
        if isolation != {'strategy': 'byte-identical-sibling-modules', 'moduleCount': len(cases),
                         'namespaceChecks': len(cases), 'functionChecks': len(cases) * 8,
                         'tableChecks': len(cases), 'sourceSha256': collapse_source['sha256']}:
            raise RuntimeError('sequence runtime isolation not verified')
        if not any(case['family'] == 'lossless-mandatory' for case in cases):
            raise RuntimeError('missing mandatory lossless domain')
        for case in cases:
            if case['family'] != 'inherited-marker' and any(item['status'] != 'ok' for item in case['expected']):
                raise RuntimeError(f'Unexpected original runtime failure: {case["id"]}')
            if case['id'].startswith('lossless/new-build-cost/'):
                if case['expected'][2]['inputRefs'] != [-1]:
                    raise RuntimeError('special-number built path did not create a new model')
        for mandatory in ('lossless/lone-id', 'lossless/lowercase-expansion-id'):
            case = next(case for case in cases if case['id'] == mandatory)
            if not any(step['op'] == 'collapseBuiltVariants' for step in case['steps']):
                raise RuntimeError('missing mandatory lossless build call')
        policy_cases = oracle['losslessPolicyCases']
        policy_ids = {f'{label}/{op}' for label in (
            'proto-native-own', 'xai-map-own-undefined', 'deepseek-extra-own-undefined', 'thinking-map-own-undefined',
            'ambiguous-mistral-mixtral', 'ambiguous-claude-haiku-fable', 'ambiguous-minimax-m1-minimax-m2',
            'lone-url-deepseek', 'lone-url-loopback', 'deepseek-extra-authored-thinking',
            'ambiguous-lone-mistral-mixtral', 'ambiguous-lone-claude-haiku-fable', 'ambiguous-lone-minimax-m1-minimax-m2'
        ) for op in ('resolveModelPolicy', 'buildModel')}
        if (len(policy_cases) != 26 or oracle['losslessPolicyCaseCount'] != 26
                or {case['id'] for case in policy_cases} != policy_ids
                or any(case['id'].split('/')[-1] != case['op']
                       or case['expected']['status'] != ('error' if case['id'].startswith('ambiguous-') else 'ok')
                       or (case['id'].startswith('ambiguous-') and case['expected']['name'] != 'AmbiguousIdentityError')
                       for case in policy_cases)):
            raise RuntimeError('incomplete/failed original lossless policy probes')
        summary = {'status': 'PASS', 'oracle': str(artifact), 'cases': len(cases), 'counts': counts,
                   'exportCounts': oracle['exportCounts'], 'catalogRows': oracle['catalogRowCount'],
                   'oracleSha256': hashlib.sha256(artifact.read_bytes()).hexdigest(),
                   'runnerSha256': manifest['runnerSha256'], 'isolation': isolation,
                   'losslessPolicyCases': len(policy_cases),
                   'originalTestAssertionsExecuted': False}
        (run / 'receipt.json').write_text(json.dumps(summary, indent=2) + '\n', encoding='utf-8')
        print(json.dumps(summary))
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
