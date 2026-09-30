"""Execute the unchanged fixed OMP catalog policy, preserving source receipts."""
from __future__ import annotations

import argparse
import datetime as dt
import hashlib
import json
from pathlib import Path
import subprocess
import uuid

from ctx_skill_invocation_oracle import UPSTREAM_COMMIT, BUN_VERSION, command, export_blob, git

CATALOG_FILES = (
    'build.ts', 'effort.ts', 'hosts.ts', 'model-tokenizer.ts', 'models.ts', 'models.json', 'utils.ts',
    'types.ts', 'compat/types.ts', 'compat/revision.ts', 'compat/cascade.ts', 'compat/taxonomy.ts',
    'compat/resolve.ts', 'compat/axes.ts', 'compat/apply.ts', 'compat/anthropic.ts', 'compat/openai.ts',
    'compat/behavior.ts', 'compat/auth.ts', 'compat/rules.json', 'identity/id.ts', 'identity/dialect.ts',
    'identity/reference.ts', 'identity/bundled.ts', 'identity/metrics.ts', 'identity/priority.ts',
    'provider-models/bundled-references.ts',
)
UTILITY_FILES = ('type-guards.ts', 'tls-fetch.ts', 'env.ts', 'dirs.ts', 'fs-error.ts', 'worker-host.ts', 'path.ts')
FIXTURE_FILES = (
    'compat-cascade.test.ts', 'compat-taxonomy.test.ts', 'compat-conformance.test.ts', 'compat-parity.test.ts',
    'build.test.ts', 'model-tokenizer.test.ts', 'hosts.test.ts', 'model-id-affixes.test.ts',
    'model-provider-priority.test.ts', 'gateway-reference.test.ts', 'catalog-metrics-index.test.ts',
    'canonical-limit-fallback.test.ts', 'issue-4297-repro.test.ts', 'issue-9345-repro.test.ts',
)


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
            raise RuntimeError(f'Expected Bun {BUN_VERSION}')
        paths = [f'packages/catalog/src/{p}' for p in CATALOG_FILES]
        paths += [f'packages/utils/src/{p}' for p in UTILITY_FILES] + ['packages/utils/package.json']
        fixtures = [f'packages/catalog/test/{p}' for p in FIXTURE_FILES]
        entries = [export_blob(upstream, run, p) for p in paths + fixtures]
        licenses = [export_blob(upstream, run, p) for p in ('LICENSE', 'packages/catalog/LICENSE')]
        runner = Path(__file__).with_suffix('.mjs').read_bytes()
        copied = run / 'runner.mjs'
        copied.write_bytes(runner)
        manifest = {'upstreamCommit': UPSTREAM_COMMIT, 'bunVersion': BUN_VERSION,
                    'sourceManifest': entries, 'licenses': licenses, 'fixturePaths': fixtures,
                    'runnerSha256': hashlib.sha256(runner).hexdigest()}
        receipt = run / 'source-manifest.json'
        receipt.write_text(json.dumps(manifest, indent=2) + '\n', encoding='utf-8')
        result = subprocess.run([str(bun), str(copied), str(receipt)], cwd=run,
                                capture_output=True, timeout=120, check=False)
        (run / 'stdout.txt').write_bytes(result.stdout)
        (run / 'stderr.txt').write_bytes(result.stderr)
        if result.returncode:
            raise RuntimeError(f'Bun oracle exit {result.returncode}; see {run / "stderr.txt"}')
        oracle = json.loads((run / 'oracle.json').read_text(encoding='utf-8'))
        if (oracle['sourceManifest'] != entries or oracle['upstreamCommit'] != UPSTREAM_COMMIT
                or oracle['bunVersion'] != BUN_VERSION or oracle['runnerSha256'] != manifest['runnerSha256']):
            raise RuntimeError('executed source receipt mismatch')
        if oracle['catalogRowCount'] != 4776 or len(oracle['catalogKeys']) != 4776:
            raise RuntimeError('incomplete bundled catalogue')
        summary = {'status': 'PASS', 'oracle': str(run / 'oracle.json'), 'cases': len(oracle['cases']),
                   'counts': oracle['counts'], 'behaviorPairs': oracle['behaviorPairs'], 'sources': len(entries)}
        (run / 'receipt.json').write_text(json.dumps(summary, indent=2) + '\n', encoding='utf-8')
        print(json.dumps(summary))
        return 0
    except Exception as error:
        failure = {'status': 'FAIL', 'error': str(error)}
        (run / 'failure.json').write_text(json.dumps(failure) + '\n', encoding='utf-8')
        print(json.dumps(failure))
        return 1


if __name__ == '__main__':
    raise SystemExit(main())
