"""Protocol fixture standing in for the sidecar where the actual one cannot misbehave.

`python fake_resume_sidecar.py <mode> <log>`. Modes: `unexpected` sends a
reconcile_request although the start frame has no `resume`; `twice` sends two
reconcile_requests. Every stdin line after the start frame is appended to <log>.
Exits 2 (a refused resume) on stdin EOF or after the first `cancel`.
"""
import json
import sys

mode, log = sys.argv[1], sys.argv[2]
start = json.loads(sys.stdin.readline())


def request(rpc_id):
    return {'kind': 'reconcile_request', 'version': 1, 'rpc_id': rpc_id, 'task_id': start['task']['task_id'],
            'run_id': start['run']['run_id'], 'review': {'task_id': start['task']['task_id'], 'fixture': mode}}


for rpc_id in (['rpc-1', 'rpc-2'] if mode == 'twice' else ['rpc-1']):
    sys.stdout.write(json.dumps(request(rpc_id)) + '\n')
    sys.stdout.flush()
with open(log, 'a', encoding='utf-8') as stream:
    for line in sys.stdin:
        stream.write(line)
        stream.flush()
        if json.loads(line).get('kind') == 'cancel':
            break
sys.exit(2)
