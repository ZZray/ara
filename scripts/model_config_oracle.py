"""Execute unchanged fixed OMP models schema/config with pinned Bun, retaining receipts."""
from __future__ import annotations

import argparse
import datetime as dt
import hashlib
import json
from pathlib import Path
import subprocess
import uuid

from ctx_skill_invocation_oracle import UPSTREAM_COMMIT, BUN_VERSION, command, export_blob, git


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
        upstream = args.upstream.resolve(strict=True)
        bun = args.bun.resolve(strict=True)
        if command([str(bun), '--version']).decode().strip() != BUN_VERSION:
            raise RuntimeError(f'Expected Bun {BUN_VERSION}')
        sources = [
            'packages/coding-agent/src/config/config-file.ts',
            'packages/coding-agent/src/config/models-config.ts',
            'packages/coding-agent/src/config/models-config-schema.ts',
            'packages/coding-agent/src/config/models-config-schema-bundle.ts',
        ]
        sources += [p for p in git(upstream, 'ls-tree', '-r', '--name-only', UPSTREAM_COMMIT,
                                  'packages/omptype/src').decode().splitlines() if p.endswith('.ts')]
        entries = [export_blob(upstream, run, p) for p in sources]
        license_entry = export_blob(upstream, run, 'LICENSE')
        runner = Path(__file__).with_suffix('.mjs').read_bytes()
        copied = run / 'runner.mjs'
        copied.write_bytes(runner)
        manifest = {'upstreamCommit': UPSTREAM_COMMIT, 'bunVersion': BUN_VERSION,
                    'sourceManifest': entries, 'license': license_entry,
                    'runnerSha256': hashlib.sha256(runner).hexdigest()}
        receipt = run / 'source-manifest.json'
        receipt.write_text(json.dumps(manifest, indent=2) + '\n', encoding='utf-8')
        result = subprocess.run([str(bun), str(copied), str(receipt)], cwd=run,
                                capture_output=True, timeout=60, check=False)
        (run / 'stdout.txt').write_bytes(result.stdout)
        (run / 'stderr.txt').write_bytes(result.stderr)
        if result.returncode:
            raise RuntimeError(f'Bun oracle exit {result.returncode}; see {run / "stderr.txt"}')
        oracle = json.loads((run / 'oracle.json').read_text(encoding='utf-8'))
        if oracle['sourceManifest'] != entries or oracle['upstreamCommit'] != UPSTREAM_COMMIT:
            raise RuntimeError('Executed source receipt mismatch')
        print(json.dumps({'status': 'PASS', 'oracle': str(run / 'oracle.json'),
                          'cases': len(oracle['cases']), 'sources': len(entries)}))
        return 0
    except Exception as error:
        failure = {'status': 'FAIL', 'error': str(error)}
        (run / 'failure.json').write_text(json.dumps(failure) + '\n', encoding='utf-8')
        print(json.dumps(failure))
        return 1


if __name__ == '__main__':
    raise SystemExit(main())
