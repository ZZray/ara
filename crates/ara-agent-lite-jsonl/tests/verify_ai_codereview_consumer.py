"""Actual product Workspace + public Host: empty submissions must get feedback.

The two controlled model replies deliberately remain wrong. This verifies
known rejection and bounded continuation, never a successful coding task.
"""
import argparse
import hashlib
import json
import os
from pathlib import Path
import subprocess
import time

from controlled_provider import ControlledProvider, text


def require(value, message):
    if not value:
        raise AssertionError(message)


def save(path, value):
    with path.open('x', encoding='utf-8') as stream:
        json.dump(value, stream, ensure_ascii=False, indent=2)
        stream.write('\n')


def sha(path):
    return hashlib.sha256(path.read_bytes()).hexdigest()


def main(args):
    root = Path(args.evidence)
    require(root.is_absolute() and not root.exists(), 'fresh absolute evidence directory required')
    root.mkdir(parents=True)
    repo = Path(__file__).resolve().parents[3]
    example = repo / 'crates/ara-agent-lite-jsonl/examples/ai_codereview_host.py'
    binary = Path(args.binary).resolve(strict=True)
    fixture = Path(args.source_fixture).resolve(strict=True)
    revision = subprocess.check_output(['git', '-C', str(fixture), 'rev-parse', 'HEAD']).decode().strip()
    require(revision == 'd5d5a8bc97f74fecfa6776bad2d2e0690389cb40', 'historical fixture identity changed')
    require(sha(binary) == args.expected_binary_sha256.lower(), 'ordinary image identity changed')
    save(root / 'identity.json', {'binary': str(binary), 'binary_sha256': sha(binary),
        'example_sha256': sha(example), 'driver_sha256': sha(Path(__file__)),
        'fixture_head': revision, 'profile': args.profile, 'actual_model': False,
        'product_operations': 'isolated Workspace preparation only; no checks test subprocess, production DB/service or release'})
    with ControlledProvider([text('Submitted without a patch.'), text('Submitted again without a patch.')]) as provider:
        run_root = root / 'consumer'
        config = {'binary': str(binary), 'run_root': str(run_root),
            'product_root': args.product_root, 'source_fixture': str(fixture),
            'start': {'kind': 'start_task', 'version': 1,
                'task': {'task_id': 'empty-submission-' + args.profile, 'criteria_revision': 'seven-tests-unmodified',
                    'authorization_revision': 'isolated-one-file-no-release', 'kind': 'Coding',
                    'evidence_dir': str(run_root / 'harness'), 'max_runs': 2, 'max_calls': 2,
                    'deadline_unix_ms': int(time.time() * 1000) + 45000},
                'run': {'run_id': 'empty-run-1', 'objective': 'Repair the authorized Python source and submit an actual patch.',
                    'cwd': str(fixture), 'session_path': str(run_root / 'state/session.json'),
                    'environment_state': str(run_root / 'state/environment'), 'device_fp': 'acr-empty-host',
                    'session_key': 'acr-empty-host', 'mode': 'execute', 'endpoint': provider.endpoint,
                    'model': provider.model, 'max_calls': 2, 'max_seconds': 45, 'max_output': 512,
                    'model_resolution': {'kind': 'host_declared_loopback', 'capabilities': {
                        'context_window': 131072, 'max_tokens': 8192, 'input': ['text']}},
                    'generation_policy': 'thinking_type_disabled', 'tools': []},
                'cancel_grace_ms': 1000, 'emit_events': False}}
        path = root / 'configuration.json'
        save(path, config)
        env = {**os.environ, 'ARA_AGENT_MODEL_TOKEN': 'controlled-empty-submission-no-provider-grant',
               'PYTHONDONTWRITEBYTECODE': '1'}
        started = time.time()
        options = {'creationflags': 0x08000000} if os.name == 'nt' else {}
        process = subprocess.Popen([args.product_python, str(example), str(path)],
            stdout=subprocess.PIPE, stderr=subprocess.PIPE, env=env, **options)
        try:
            stdout, stderr = process.communicate(timeout=65)
        except subprocess.TimeoutExpired:
            process.kill()
            stdout, stderr = process.communicate(timeout=10)
            (root / 'stdout.bin').write_bytes(stdout)
            (root / 'stderr.bin').write_bytes(stderr)
            raise AssertionError('original consumer exceeded watchdog; effects require original receipt inspection')
        (root / 'stdout.bin').write_bytes(stdout)
        (root / 'stderr.bin').write_bytes(stderr)
        save(root / 'process.json', {'pid': process.pid, 'exit_code': process.returncode,
            'elapsed_seconds': time.time() - started})
        save(root / 'provider-requests.json', provider.requests)
        require(process.returncode != 0, 'two wrong submissions must not produce consumer success')
        require(not list((run_root / 'host').glob('operation-*.json')), 'fixture unexpectedly invoked a business tool')
        require(not (run_root / 'host/work/validation.json').exists(), 'test subprocess unexpectedly started')
        reviews = sorted((run_root / 'host').glob('review-*-committed.json'))
        if args.profile == 'baseline':
            require(len(provider.requests) == 1 and not reviews, 'baseline did not fail in first Host verification')
            require(b'ReconciliationRequired' in stderr, 'baseline failed for another reason')
            require(not (run_root / 'harness/released.json').exists(), 'unknown original callback was released')
            outcome_path = run_root / 'harness/outcome.json'
        else:
            require(len(provider.requests) == 2 and len(reviews) == 2, 'Host did not continue exactly once')
            receipts = [json.loads(path.read_bytes()) for path in reviews]
            require(all(row['committed'] and row['settlement'] == 'Settled'
                        and 'NeedsChanges' in row['verdict'] for row in receipts), 'empty candidate got an invalid review')
            require('No source patch exists' in json.dumps(provider.requests[1]), 'actual failed-check feedback missing from next HTTP')
            require(all(request['thinking'] == {'type': 'disabled'} for request in provider.requests), 'fixed policy changed in continuation')
            for number in (1, 2):
                review = json.loads((run_root / f'host/review-{number}.json').read_bytes())
                require(review['test_count'] is None and review['validation']['test_subprocess_started'] is False,
                        'unexecuted tests were reported as executed')
            outcome_path = run_root / 'harness/released.json'
        outcome = json.loads(outcome_path.read_bytes())
        require(outcome['task_accepted'] is False, 'wrong task was accepted')
        if args.profile == 'fixed':
            require(outcome['stop'] == 'BudgetExhausted' and outcome['runs'] == outcome['observed_calls'] == 2,
                    'original Task run/call budget was not enforced')
        save(root / 'result.json', {'assertions': 'PASS', 'profile': args.profile,
            'consumer_task': 'FAILED_BY_DESIGN_EMPTY_SUBMISSIONS', 'model_http_requests': len(provider.requests),
            'committed_reviews': len(reviews), 'actual_outcome': outcome, 'actual_model': False})
    print(json.dumps({'assertions': 'PASS', 'profile': args.profile, 'evidence': str(root)}))


if __name__ == '__main__':
    parser = argparse.ArgumentParser()
    parser.add_argument('--binary', required=True)
    parser.add_argument('--expected-binary-sha256', required=True)
    parser.add_argument('--product-python', required=True)
    parser.add_argument('--product-root', required=True)
    parser.add_argument('--source-fixture', required=True)
    parser.add_argument('--evidence', required=True)
    parser.add_argument('--profile', choices=('baseline', 'fixed'), required=True)
    main(parser.parse_args())
