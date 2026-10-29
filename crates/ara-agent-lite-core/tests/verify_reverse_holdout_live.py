"""Two fixed, one-shot H2 holdout trials. Preparation/selftest call no models.

H2-D and H2-G differ in both model and policy; they are not a controlled causal
model comparison. Static Plan tools have no Host-provided facts. Hidden oracle
evaluation follows a sealed submission and never feeds results back to a model.
There is no OS sandbox or claim that private material is physically unreadable.
"""
import argparse
import ast
import http.server
import json
import os
from pathlib import Path
import re
import shutil
import subprocess
import sys
import tempfile
import threading
import time
import urllib.error
import urllib.request

from verify_analysis_live import ROUTE, route_from
from verify_coding_live import Relay, digest, inventory, require, scan_actual_token, write_json
from verify_reverse_policy_live import call_phases, wire_observation

BASE = Path('D:/WorkSpace/tmp/ara-core-capability-20261022')
STAGE = BASE / 'reverse-holdout-stage'
OUTPUT = BASE / 'reverse-holdout-live-v1'
CATALOG = BASE / 'reverse-policy-live-v1/A/catalog.response.native.bin'
ARMS = (
    {'arm': 'H2-D', 'model': 'deepseek-v4.1-flash', 'policy': 'thinking_type_disabled', 'output': 8192},
    {'arm': 'H2-G', 'model': 'gpt-6.1-sol', 'policy': 'provider_default', 'output': 8192},
)
OBJECTIVE = ('Read README.md and reconstruct the supplied specimen using static evidence only. '
             'Check the actual return value of each validation branch, every byte offset, '
             'and each arithmetic width before writing the implementation. '
             'Prioritize the complete recover(data: bytes) Python block and predictions JSON block. '
             'Then explain the required instruction-address evidence in at most 1200 characters outside code blocks. '
             'Do not execute the specimen or modify the three supplied files. '
             'The complete input and output contract is in README.md.')


def load(path):
    return json.loads(path.read_text(encoding='utf-8-sig'))


def file_hash(path):
    return digest(path.read_bytes())


def core():
    return Path(__file__).resolve().parents[1]


def helper_pins():
    return {name: file_hash(Path(__file__).with_name(name)) for name in (
        'verify_analysis_live.py', 'verify_coding_live.py', 'verify_reverse_policy_live.py',
        'verify_reverse_holdout_live.py')}


def verify_stage(stage, expected=None):
    manifest_path = stage / 'manifest.json'
    require(file_hash(manifest_path) == (stage / 'manifest.sha256').read_text(encoding='ascii').strip(),
            'H2 manifest seal changed')
    manifest = load(manifest_path)
    if expected is not None:
        require(file_hash(manifest_path) == expected, 'H2 manifest differs from frozen plan')
    require(manifest['oracle']['cases'] == 128 and manifest['oracle']['valid'] == 80 and
            manifest['oracle']['invalid'] == 48, 'unexpected H2 oracle inventory')
    require(len(manifest['private_pins']) == 67 and len(manifest['delivered_source_pins']) == 41,
            'H2 author/runtime pin inventory differs')
    for relative, value in manifest['private_pins'].items():
        require(file_hash(stage / relative) == value, 'H2 author artifact changed: ' + relative)
    require(file_hash(stage / 'prepare_h2.py') == manifest['generator_sha256'], 'H2 generator changed')
    require(file_hash(stage / 'checks.json') == manifest['checks_sha256'], 'H2 author checks changed')
    require(file_hash(Path(manifest['oracle']['path'])) == manifest['oracle']['sha256'], 'H2 oracle changed')
    for relative, value in manifest['delivered_source_pins'].items():
        require(file_hash(core() / Path(relative)) == value, 'delivered runtime/helper changed: ' + relative)
    require(file_hash(Path(manifest['host_path'])) == manifest['host_sha256'], 'v4 Host changed')
    require(inventory(Path(manifest['public_template'])) == manifest['public_pins'], 'H2 public files changed')
    require(set(manifest['public_pins']) == {'README.md', 'specimen.exe', 'sample.hex'}, 'H2 public allowlist differs')
    return manifest


def select_catalog(catalog, model):
    require(isinstance(catalog.get('data'), list), 'catalog must contain a data array')
    rows = [row for row in catalog['data'] if isinstance(row, dict) and row.get('id') == model]
    require(len(rows) == 1, 'selected model missing or duplicated in actual catalog: ' + model)
    row = rows[0]
    protocols = row.get('protocols')
    require(protocols is None or (isinstance(protocols, list) and
            all(isinstance(value, str) and value.strip() == value and value for value in protocols)),
            'invalid catalog protocols')
    require(not protocols or 'openai-completions' in protocols, 'selected model lacks the Host protocol')
    return row


def catalog_caps(row):
    """Mirror positive/minimum directory declarations; absent output uses 8192.

    This is checked against the native model_capabilities event on each Run;
    unknown directory context and usage remain unknown.
    """
    omp = row.get('omp') or {}
    require(isinstance(omp, dict), 'invalid omp metadata')

    def positive(top, nested):
        values = []
        for value in (row.get(top), omp.get(nested)):
            require(value is None or type(value) is int and 0 <= value <= (1 << 64) - 1,
                    'catalog capacity is not a nonnegative integer')
            if value:
                values.append(value)
        return min(values) if values else None

    def modalities(value):
        require(value is None or isinstance(value, list) and len(value) <= 32 and all(
            isinstance(v, str) and v.strip() == v and 0 < len(v) <= 64 for v in value), 'invalid catalog input')
        return list(dict.fromkeys(value)) if value is not None else None

    top, nested = modalities(row.get('input')), modalities(omp.get('input'))
    inputs = [v for v in top if v in nested] if top is not None and nested is not None else top if top is not None else nested
    return {'context_window': positive('context_window', 'contextWindow'),
            'max_tokens': positive('max_tokens', 'maxTokens') or 8192, 'input': inputs}


def validate_actual_catalog(catalog, arm, planned_row):
    row = select_catalog(catalog, arm['model'])
    require(row == planned_row, 'selected live catalog row changed after preparation')
    require(catalog_caps(row)['max_tokens'] >= arm['output'], 'live directory cannot realize fixed output condition')
    return row


def checked_relay(endpoint, token, arm, folder, planned_row):
    """Retain the audited forwarding body; gate POST on settled native GET.

    The Host still receives and parses the original catalog bytes. The gate
    prevents spending a model call under changed catalog conditions, including
    a race where the Host posts before this GET thread has checked the receipt.
    """
    relay = Relay(endpoint, token, arm['model'], folder, max_calls=28)
    relay.catalog_gate = {'checked': False, 'accepted': False, 'blocked_posts': 0, 'error': None}
    ready = threading.Event()
    base = relay.server.RequestHandlerClass

    class Handler(base):
        def do_GET(self):
            try:
                super().do_GET()
                require(len(relay.catalogs) == 1 and relay.catalogs[0].get('status') == 'received' and
                        relay.catalogs[0].get('http_status') == 200, 'native catalog GET did not settle successfully')
                validate_actual_catalog(load(folder / 'catalog.response.native.bin'), arm, planned_row)
                relay.catalog_gate['accepted'] = True
            except Exception as error:
                relay.catalog_gate['error'] = relay.safe(type(error).__name__ + ': ' + str(error))
                relay.errors.append(relay.catalog_gate['error'])
            finally:
                relay.catalog_gate['checked'] = True
                ready.set()

        def do_POST(self):
            if not ready.wait(5) or not relay.catalog_gate['accepted']:
                with relay.lock:
                    relay.catalog_gate['blocked_posts'] += 1
                self.send_error(502, 'catalog admission rejected; inspect private native receipts')
                return
            super().do_POST()

    # No Host or model request exists until construction has completed.
    relay.server.RequestHandlerClass = Handler
    return relay


def prepare(args):
    stage, root = args.stage.resolve(), args.output.resolve()
    require(stage != root and stage not in root.parents and root not in stage.parents, 'private stage overlaps live root')
    manifest = verify_stage(stage)
    endpoint, original_model = route_from(args.route_result)
    catalog = load(args.catalog)
    rows = {arm['arm']: select_catalog(catalog, arm['model']) for arm in ARMS}
    for row in rows.values():
        require(catalog_caps(row)['max_tokens'] >= 8192, 'declared output cannot realize fixed 8192 condition')
    tested = load(args.selftest_record)
    require(tested['status'] == 'PASS' and tested['model_calls'] == 0 and
            tested['harness_sha256'] == file_hash(Path(__file__)), 'current offline selftest record required')
    previous = None
    if root.exists():
        require(args.amend_from is not None, 'existing plan requires explicit zero-call amendment archive')
        require(not list(root.rglob('STARTED')) and not list(root.rglob('native-session.json')) and
                not list(root.rglob('result.json')), 'cannot amend after any live arm starts or settles')
        previous = load(root / 'plan.json')
        require(file_hash(root / 'plan.json') == (root / 'plan.sha256').read_text().strip() and
                file_hash(root / 'plan.json') == file_hash(args.amend_from / 'plan.json') and
                file_hash(args.amend_from / 'verify_reverse_holdout_live.py') ==
                    previous['harness_helper_pins']['verify_reverse_holdout_live.py'],
                'prior failed PRE plan/harness archive differs')
    else:
        root.mkdir(parents=True, exist_ok=False)
    for arm in ARMS:
        work = root / arm['arm'] / 'project'
        if previous is None:
            work.mkdir(parents=True)
            for name in manifest['public_pins']:
                shutil.copyfile(Path(manifest['public_template']) / name, work / name)
        require(inventory(work) == manifest['public_pins'], 'H2 public copy differs')
    backends = {name: {k: manifest['compiler_backends'][name][k] for k in ('path', 'sha256')}
                for name in ('objdump', 'nm')}
    for backend in backends.values():
        require(file_hash(Path(backend['path'])) == backend['sha256'], 'static backend changed')
    plan = {'schema': 'ara-reverse-holdout-live-v1', 'status': 'PREPARED', 'arms': list(ARMS),
        'objective': OBJECTIVE, 'max_runs': 2, 'max_calls': 28, 'host_seconds': 720, 'outer_host_seconds': 725,
        'evaluation_seconds': 8, 'host_facts': False, 'ctags': False, 'protocol': 'openai-completions',
        'stage_path': str(stage), 'stage_manifest_sha256': file_hash(stage / 'manifest.json'),
        'stage_manifest_seal_sha256': file_hash(stage / 'manifest.sha256'),
        'stage_checks_sha256': file_hash(stage / 'checks.json'), 'author_pins': manifest['private_pins'],
        'public_pins': manifest['public_pins'], 'oracle_sha256': manifest['oracle']['sha256'],
        'oracle_cases': 128, 'prediction_inputs': manifest['prediction_inputs'],
        'runtime_source_pins': manifest['delivered_source_pins'], 'harness_helper_pins': helper_pins(),
        'selftest_record_path': str(args.selftest_record.resolve()), 'selftest_record_sha256': file_hash(args.selftest_record),
        'binary_path': manifest['host_path'], 'binary_sha256': manifest['host_sha256'], 'backends': backends,
        'original_route_path': str(args.route_result.resolve()), 'route_record_sha256': file_hash(args.route_result),
        'endpoint_sha256': digest(endpoint.encode()), 'original_route_model': original_model,
        'catalog_snapshot_path': str(args.catalog.resolve()), 'catalog_snapshot_sha256': file_hash(args.catalog),
        'catalog_ids': [row['id'] for row in catalog['data']], 'selected_catalog_rows': rows,
        'selected_capacity_expectations': {arm: catalog_caps(row) for arm, row in rows.items()},
        'limits': ['two models and two policies differ; no pure-model or policy causal attribution',
                   'catalog snapshot proves recorded presence; each native Run catalog is checked independently',
                   'model POST forwarding waits for the settled native GET and rejects changed selected metadata before its first model call',
                   'directory output/context absence uses native local fallback, never advertised provider capacity',
                   'same Plan/basic/binary tools, no ctags, no static facts supplied by Host',
                   'STARTED is exclusive; failure, timeout or unknown effects are never automatically replayed',
                   'submission is sealed before hidden inputs; evaluation occurs only after Host/model settlement',
                   'generated candidate execution is not an OS sandbox; private material not exposed is not unreadability',
                   'oracle correctness, tool receipt use and address explanation are reported separately',
                   '139 author disassembly rows include byte continuation rows; not an exact machine instruction count',
                   'Relay retains existing 45-second socket and 50-second response checks for both arms',
                   'all raw responses and private reasoning remain private; no raw reasoning in summary reports']}
    if previous is not None:
        mutable = {'harness_helper_pins', 'selftest_record_path', 'selftest_record_sha256', 'amendments'}
        require({k: v for k, v in previous.items() if k not in mutable} ==
                {k: v for k, v in plan.items() if k not in mutable}, 'amendment unexpectedly changes fixed trial conditions')
        plan['amendments'] = previous.get('amendments', []) + [{
            'reason': 'PRE found response.model alias equality incorrectly blocked H2 oracle; retain raw echoes and report identity UNVERIFIED',
            'prior_plan_sha256': file_hash(root / 'plan.json'),
            'prior_harness_sha256': previous['harness_helper_pins']['verify_reverse_holdout_live.py'],
            'archive_path': str(args.amend_from.resolve()), 'model_calls_before_amendment': 0,
            'new_harness_sha256': file_hash(Path(__file__))}]
    write_json(root / 'plan.json', plan)
    (root / 'plan.sha256').write_text(file_hash(root / 'plan.json') + '\n', encoding='ascii')
    return {'status': 'PREPARED', 'plan_sha256': file_hash(root / 'plan.json'), 'arms': [a['arm'] for a in ARMS],
            'model_calls': 0, 'catalog_network_calls': 0, 'catalog_snapshot_ids': len(plan['catalog_ids'])}


def evaluate_submission(text, folder, oracle_path, oracle_hash, prediction_inputs, timeout=8):
    python = re.findall(r'```python\s*\n(.*?)```', text, re.DOTALL | re.IGNORECASE)
    prediction = re.findall(r'```json\s*\n(.*?)```', text, re.DOTALL | re.IGNORECASE)
    if len(python) != 1 or len(prediction) != 1:
        return {'status': 'NOT_RUN', 'reason': 'missing/duplicate/incomplete Python or JSON block', 'planned_cases': 128}
    try:
        tree = ast.parse(python[0]); parsed = json.loads(prediction[0])
        require(any(isinstance(node, ast.FunctionDef) and node.name == 'recover' for node in tree.body),
                'complete recover definition missing')
        require(isinstance(parsed, dict) and set(parsed) == {'predictions'} and
                isinstance(parsed['predictions'], list), 'invalid prediction object')
    except (SyntaxError, ValueError, AssertionError):
        return {'status': 'NOT_RUN', 'reason': 'invalid submitted syntax/function/predictions', 'planned_cases': 128}
    artifact, predictions_path, seal = folder / 'recovered.py', folder / 'predictions.json', folder / 'sealed-submission.json'
    for path in (artifact, predictions_path, seal):
        require(not path.exists(), 'submission destination already exists; never overwrite evaluation')
    artifact.write_text(python[0], encoding='utf-8', newline='\n')
    predictions_path.write_text(prediction[0], encoding='utf-8', newline='\n')
    sealed = {'code_sha256': file_hash(artifact), 'predictions_sha256': file_hash(predictions_path),
              'predictions': parsed['predictions'], 'hidden_inputs_supplied': False}
    write_json(seal, sealed)
    seal_hash = file_hash(seal)
    # The complete code and raw prediction JSON are durable before reading any
    # hidden expected outputs or sending hidden inputs to a candidate process.
    require(file_hash(oracle_path) == oracle_hash, 'sealed H2 oracle changed before evaluation')
    cases = load(oracle_path)
    require(len(cases) == 128 and len({item['hex'] for item in cases}) == 128, 'H2 case inventory differs')
    mapping = {item['hex']: item['expected'] for item in cases}
    require(len(prediction_inputs) == 4 and all(value in mapping for value in prediction_inputs),
            'public predictions are not uniquely covered by H2 oracle')
    expected = [item['expected'] for item in cases]
    expected_predictions = [mapping[value] for value in prediction_inputs]
    worker = ('import json,sys,runpy\nns=runpy.run_path(sys.argv[1])\nfn=ns["recover"]\n'
              'print(json.dumps([fn(bytes.fromhex(x)) for x in json.load(sys.stdin)]))\n')
    stdin = json.dumps([item['hex'] for item in cases]).encode()
    started = time.monotonic(); code = None; out = err = b''; timed_out = False
    try:
        process = subprocess.run([sys.executable, '-I', '-B', '-c', worker, str(artifact)], cwd=folder,
                                 input=stdin, capture_output=True, timeout=timeout)
        code, out, err = process.returncode, process.stdout, process.stderr
    except subprocess.TimeoutExpired as error:
        out, err, timed_out = error.stdout or b'', error.stderr or b'', True
    (folder / 'evaluation.stdout.native.bin').write_bytes(out)
    (folder / 'evaluation.stderr.native.bin').write_bytes(err)
    integrity = (artifact.is_file() and predictions_path.is_file() and seal.is_file() and
        file_hash(artifact) == sealed['code_sha256'] and file_hash(predictions_path) == sealed['predictions_sha256'] and
        file_hash(seal) == seal_hash and file_hash(oracle_path) == oracle_hash)
    try:
        actual = json.loads(out)
    except ValueError:
        actual = None
    outputs_ok = isinstance(actual, list) and len(actual) == 128 and all(
        type(a) is int and a == b for a, b in zip(actual, expected))
    predicted = parsed['predictions']
    predictions_ok = len(predicted) == 4 and all(type(a) is int and a == b for a, b in zip(predicted, expected_predictions))
    matched = sum(type(a) is int and a == b for a, b in zip(actual, expected)) if isinstance(actual, list) else 0
    result = {'status': 'PASS' if code == 0 and integrity and outputs_ok and predictions_ok else 'FAIL',
        'sealed_submission': sealed, 'seal_sha256': seal_hash, 'seal_integrity_after_evaluation': integrity,
        'hidden_case_count': 128, 'hidden_outputs_correct': outputs_ok, 'hidden_outputs_matched': matched,
        'predictions_correct': predictions_ok, 'prediction_source': 'exact public-input lookup in sealed H2 oracle',
        'execution': {'argv': [sys.executable, '-I', '-B', '-c', worker, str(artifact)], 'cwd': str(folder),
            'stdin_sha256': digest(stdin), 'exit_code': code, 'timeout': timed_out,
            'seconds': round(time.monotonic() - started, 3), 'stdout_sha256': digest(out), 'stderr_sha256': digest(err)},
        'effects_unknown': timed_out, 'generated_code_is_not_sandboxed': True}
    write_json(folder / 'evaluation.json', result)
    return result


def observe_wire(folder, number, arm, phase, first):
    request = load(folder / f'call-{number:02d}.request.native.json')
    require(request.get('model') == arm['model'] and request.get('stream') is True, 'actual wire model/stream differs')
    observation = wire_observation(folder, number, arm, '', phase, first, arm['model'])
    observation['request_model'] = arm['model']
    observation['response_model_discrepancy'] = bool(observation['response_models']) and observation['response_models'] != [arm['model']]
    observation['alias_identity'] = 'UNVERIFIED'
    observation['backend_weight_identity'] = 'UNKNOWN'
    observation['identity_limit'] = 'request/catalog/endpoint are checked; response echoes are retained without alias normalization or backend identity inference'
    if phase == 'agent':
        names = {item['function']['name'] for item in request.get('tools', [])}
        require('binary_inspect' in names and 'source_symbols' not in names, 'Plan binary-only registration differs')
        require(not {'exec', 'run_script', 'write_file', 'edit_file'}.intersection(names), 'Plan exposes effect tools')
        observation['tool_schema_sha256'] = digest(json.dumps(request.get('tools', []), sort_keys=True).encode())
    return observation


def tool_evidence(events, text, sample_hash, backends):
    starts = {event.get('step_id'): event for event in events if event.get('type') == 'tool_start'}
    receipts, errors, addresses = [], [], set()
    for end in events:
        if end.get('type') != 'tool_end' or end.get('title') != 'binary_inspect':
            continue
        if end.get('status') != 'completed':
            errors.append({'step_id': end.get('step_id'), 'status': end.get('status')}); continue
        try:
            require(starts.get(end.get('step_id'), {}).get('title') == 'binary_inspect', 'unmatched tool receipt')
            value = json.loads(end['output']); item, backend = value['input'], value['backend']
            require(value.get('schema') == 'ara.analysis.v1' and value.get('tool') == 'binary_inspect', 'wrong analysis schema')
            snapshot = Path(item['snapshot_path'])
            require(item['sha256'] == sample_hash and snapshot.is_file() and file_hash(snapshot) == sample_hash,
                    'binary snapshot identity differs')
            require(any(backend['sha256'] == b['sha256'] and Path(backend['path']).samefile(b['path'])
                        for b in backends.values()), 'analysis backend differs from frozen tools')
            for line in value.get('lines', []):
                match = re.match(r'\s*([0-9a-fA-F]+):', line if isinstance(line, str) else '')
                if match: addresses.add(int(match[1], 16))
            receipts.append({'step_id': end['step_id'], 'action': value.get('action'),
                'receipt_sha256': digest(end['output'].encode()), 'input_sha256': item['sha256'],
                'analysis': value.get('analysis'), 'page': value.get('page')})
        except (ValueError, KeyError, TypeError, OSError, AssertionError) as error:
            errors.append({'step_id': end.get('step_id'), 'error': type(error).__name__ + ': ' + str(error)})
    explanation = re.sub(r'```.*?```', '', text, flags=re.DOTALL).strip()
    cited = {int(value, 16) for value in re.findall(r'0x([0-9a-fA-F]+)', explanation)}
    return {'completed_binary_receipts_verified': len(receipts), 'receipts': receipts, 'errors': errors,
        'observed_disassembly_addresses': len(addresses), 'cited_observed_addresses': [hex(v) for v in sorted(addresses & cited)],
        'explanation_characters': len(explanation), 'explanation_within_1200': len(explanation) <= 1200,
        'independent_semantic_review': 'PENDING',
        'limit': 'matched instruction addresses prove citation identity; algorithm explanation needs independent review'}


def load_plan(args):
    root = args.output.resolve(); path = root / 'plan.json'
    require(file_hash(path) == (root / 'plan.sha256').read_text(encoding='ascii').strip(), 'H2 plan seal changed')
    plan = load(path)
    require(plan['arms'] == list(ARMS) and plan['objective'] == OBJECTIVE and plan['max_calls'] == 28 and
            plan['host_seconds'] == 720 and plan['max_runs'] == 2, 'fixed conditions/budgets changed')
    require(helper_pins() == plan['harness_helper_pins'], 'harness/helper changed after freezing plan')
    require(file_hash(Path(plan['selftest_record_path'])) == plan['selftest_record_sha256'], 'offline selftest record changed')
    stage = Path(plan['stage_path']); manifest = verify_stage(stage, plan['stage_manifest_sha256'])
    require(file_hash(stage / 'manifest.sha256') == plan['stage_manifest_seal_sha256'], 'stage seal changed')
    require(manifest['private_pins'] == plan['author_pins'] and manifest['delivered_source_pins'] == plan['runtime_source_pins'],
            'author/runtime identity differs')
    endpoint, _ = route_from(Path(plan['original_route_path']))
    require(file_hash(Path(plan['original_route_path'])) == plan['route_record_sha256'] and
            digest(endpoint.encode()) == plan['endpoint_sha256'], 'original endpoint route changed')
    require(file_hash(Path(plan['catalog_snapshot_path'])) == plan['catalog_snapshot_sha256'], 'catalog snapshot changed')
    for backend in plan['backends'].values():
        require(file_hash(Path(backend['path'])) == backend['sha256'], 'static backend changed')
    return root, plan, manifest, endpoint


def live(args):
    root, plan, manifest, endpoint = load_plan(args)
    arm = next(a for a in ARMS if a['arm'] == args.arm)
    prior = ARMS[:[a['arm'] for a in ARMS].index(args.arm)]
    require(all((root / a['arm'] / 'result.json').is_file() for a in prior), 'preceding one-shot arm must settle first')
    folder = root / args.arm; work = folder / 'project'
    require(inventory(work) == plan['public_pins'], 'public project changed before Run')
    require(not (folder / 'native-session.json').exists(), 'fresh native Session required')
    token = os.environ.get('CAS_API_KEY', ''); require(token, 'CAS_API_KEY is required')
    (folder / 'STARTED').open('x').close()
    started = time.monotonic(); relay = None
    result = {'schema': plan['schema'], 'arm': args.arm, 'condition': arm, 'plan_sha256': file_hash(root / 'plan.json'),
        'status': 'FAIL', 'effects_unknown': True, 'oracle_status': 'NOT_RUN', 'task_success': False,
        'model': arm['model'], 'binary_sha256': plan['binary_sha256'], 'host_facts': False,
        'pure_model_or_policy_causal_attribution': False, 'independent_semantic_review': 'PENDING'}
    try:
        artifacts = folder / 'model-analysis'; artifacts.mkdir()
        relay = checked_relay(endpoint, token, arm, folder, plan['selected_catalog_rows'][args.arm])
        request = {'id': 'reverse-holdout-' + args.arm, 'objective': OBJECTIVE, 'cwd': str(work),
            'session_path': str(folder / 'native-session.json'), 'environment_state': str(folder / 'environment-state'),
            'endpoint': relay.local_endpoint, 'model': arm['model'], 'token_env': 'CAS_API_KEY', 'mode': 'plan',
            'enable_local_tools': True, 'max_calls': 28, 'max_seconds': 720, 'max_output': 8192,
            'analysis_config': {'artifact_root': str(artifacts), **{k: b['path'] for k, b in plan['backends'].items()}}}
        if arm['policy'] != 'provider_default': request['generation_policy'] = arm['policy']
        write_json(folder / 'request.json', request)
        code = None; stdout = stderr = b''
        try:
            process = subprocess.run([plan['binary_path'], str(folder / 'request.json')], cwd=work,
                                     capture_output=True, timeout=725)
            code, stdout, stderr = process.returncode, process.stdout, process.stderr
        except subprocess.TimeoutExpired as error:
            stdout, stderr = error.stdout or b'', error.stderr or b''
        finally:
            relay.close()
            (folder / 'host.stdout.native.jsonl').write_bytes(stdout)
            (folder / 'host.stderr.native.txt').write_bytes(stderr)
        packets = [json.loads(line) for line in stdout.decode('utf-8').splitlines() if line.strip()]
        events = [packet['event'] for packet in packets if packet.get('kind') == 'event']
        final = next((packet for packet in reversed(packets) if packet.get('kind') == 'result'), None)
        text = final.get('text', '') if final else ''
        require(isinstance(text, str), 'native final text is not text')
        result.update(host_exit=code, native_result_code=final.get('code') if final else None,
                      effects_unknown=code is None or final is None,
                      host_wall_seconds=round(time.monotonic() - started, 3))
        require(len(relay.catalogs) == 1 and relay.catalogs[0].get('status') == 'received' and
                relay.catalogs[0].get('http_status') == 200, 'native directory lookup did not settle exactly once')
        native_catalog = load(folder / 'catalog.response.native.bin')
        row = select_catalog(native_catalog, arm['model']); caps = catalog_caps(row)
        result['actual_selected_catalog_row'] = row
        result['actual_catalog_sha256'] = file_hash(folder / 'catalog.response.native.bin')
        validate_actual_catalog(native_catalog, arm, plan['selected_catalog_rows'][args.arm])
        require(relay.catalog_gate['accepted'] and not relay.catalog_gate['blocked_posts'], 'native catalog gate rejected model admission')
        native_caps = [e for e in events if e.get('type') == 'model_capabilities']
        result['model_capabilities'] = native_caps
        require(len(native_caps) == 1 and native_caps[0].get('model') == arm['model'] and
                native_caps[0].get('declared') == caps, 'native parsed directory capacity differs')
        policies = [e for e in events if e.get('type') == 'generation_policy']; result['policy_events'] = policies
        if arm['policy'] == 'thinking_type_disabled':
            require(len(policies) == 1 and all(policies[0].get(k) == v for k, v in {
                'policy': arm['policy'], 'model': arm['model'], 'protocol': 'openai-completions',
                'requested_output_tokens': 8192, 'effective_output_tokens': 8192,
                'provider_effect_verified': False}.items()), 'Host policy admission event differs')
        else:
            require(not policies, 'default arm emitted explicit policy event')
        phases = call_phases(events, len(relay.calls))
        first = min(n for n, phase in phases.items() if phase == 'agent')
        wire = [observe_wire(folder, n, arm, phases[n], first) for n in range(1, len(relay.calls) + 1)]
        result['wire_observations'] = wire
        tool_hashes = {w['tool_schema_sha256'] for w in wire if w['phase'] == 'agent'}
        require(len(tool_hashes) == 1, 'agent tool schemas changed within Run')
        result['agent_tool_schema_sha256'] = next(iter(tool_hashes))
        if prior:
            previous = load(root / prior[-1]['arm'] / 'result.json')
            if previous.get('agent_tool_schema_sha256'):
                require(previous['agent_tool_schema_sha256'] == result['agent_tool_schema_sha256'], 'arm tool schemas differ')
        require(not relay.errors and all(c.get('status') == 'received' and c.get('http_status') == 200 for c in relay.calls),
                'provider/relay call did not settle successfully; retain receipts')
        forbidden = [e for e in events if e.get('type') == 'tool_start' and e.get('title') in
                     ('exec', 'run_script', 'write_file', 'edit_file', 'source_symbols')]
        require(not forbidden, 'Plan trial attempted execution/mutation/ctags')
        require(inventory(work) == plan['public_pins'], 'public specimen/contract changed during Run')
        result['tool_and_explanation_evidence'] = tool_evidence(events, text,
            plan['public_pins']['specimen.exe']['sha256'], plan['backends'])
        if code == 0 and final is not None and final.get('code') == 0:
            oracle = evaluate_submission(text, folder, Path(manifest['oracle']['path']),
                plan['oracle_sha256'], plan['prediction_inputs'])
        else:
            oracle = {'status': 'NOT_RUN', 'reason': 'native Run timed out/failed/incomplete; no partial-answer evaluation',
                      'planned_cases': 128}
        write_json(folder / 'oracle.json', oracle, token)
        result.update(oracle_status=oracle['status'], task_success=oracle['status'] == 'PASS',
                      effects_unknown=result['effects_unknown'] or oracle.get('effects_unknown', False))
        result['status'] = 'PASS' if result['task_success'] and not result['effects_unknown'] else 'FAIL'
    except Exception as error:
        result['error'] = (type(error).__name__ + ': ' + str(error)).replace(token, '[REDACTED]')
    finally:
        if relay is not None:
            if relay.thread.is_alive(): relay.close()
            write_json(folder / 'provider-receipts.json', {'calls': relay.calls, 'catalogs': relay.catalogs,
                'catalog_gate': relay.catalog_gate, 'errors': relay.errors}, token)
        result['total_wall_seconds_with_relay_cleanup_and_evaluation'] = round(time.monotonic() - started, 3)
        try:
            load_plan(args); result['all_frozen_inputs_unchanged'] = True
        except Exception as error:
            result['all_frozen_inputs_unchanged'] = False
            result['integrity_error'] = (type(error).__name__ + ': ' + str(error)).replace(token, '[REDACTED]')
        result['secret_scan'] = scan_actual_token(folder, token)
        result['public_project_unchanged_after_evaluation'] = inventory(work) == plan['public_pins']
        result['raw_evidence_private'] = True
        if (not result['all_frozen_inputs_unchanged'] or not result['public_project_unchanged_after_evaluation'] or
                result['secret_scan']['status'] != 'CLEAN'): result['status'] = 'FAIL'
        write_json(folder / 'result.json', result, token)
    return {k: result.get(k) for k in ('arm', 'status', 'oracle_status', 'task_success', 'effects_unknown',
                                     'total_wall_seconds_with_relay_cleanup_and_evaluation')}


def relay_selftest(root):
    """Local scripted HTTP only; tests catalog admission before forwarding."""
    passed = []
    token = 'h2-controlled-relay-fixture-key'
    for valid in (True, False):
        folder = root / ('relay-valid' if valid else 'relay-changed'); folder.mkdir()
        arm = ARMS[0]; planned = {'id': arm['model']}
        row = planned if valid else {**planned, 'max_tokens': 4096}
        counters = {'posts': 0}

        class Handler(http.server.BaseHTTPRequestHandler):
            def log_message(self, *unused):
                pass

            def do_GET(self):
                body = json.dumps({'data': [row]}).encode()
                self.send_response(200); self.send_header('Content-Type', 'application/json')
                self.send_header('Content-Length', str(len(body))); self.end_headers(); self.wfile.write(body)

            def do_POST(self):
                counters['posts'] += 1
                self.rfile.read(int(self.headers['Content-Length']))
                body = ('data: ' + json.dumps({'model': arm['model'], 'choices': [
                    {'delta': {'content': 'CONTROLLED_FIXTURE'}, 'finish_reason': 'stop'}]}) + '\n\ndata: [DONE]\n').encode()
                self.send_response(200); self.send_header('Content-Type', 'text/event-stream')
                self.send_header('Content-Length', str(len(body))); self.end_headers(); self.wfile.write(body)

        server = http.server.ThreadingHTTPServer(('127.0.0.1', 0), Handler)
        thread = threading.Thread(target=server.serve_forever, daemon=True); thread.start()
        relay = checked_relay('http://127.0.0.1:%d/v1' % server.server_port, token, arm, folder, planned)
        opener = urllib.request.build_opener(urllib.request.ProxyHandler({}))
        headers = {'Authorization': 'Bearer ' + token, 'Content-Type': 'application/json'}
        try:
            with opener.open(urllib.request.Request(relay.local_endpoint + '/models', headers=headers), timeout=5) as response:
                json.loads(response.read())
            request = urllib.request.Request(relay.local_endpoint + '/chat/completions',
                data=json.dumps({'model': arm['model'], 'stream': True, 'max_tokens': 8192}).encode(), headers=headers)
            status = None
            try:
                with opener.open(request, timeout=5) as response:
                    response.read(); status = response.status
            except urllib.error.HTTPError as error:
                status = error.code; error.read()
        finally:
            relay.close(); server.shutdown(); server.server_close(); thread.join(timeout=2)
        if valid:
            require(status == 200 and counters['posts'] == 1 and len(relay.calls) == 1 and
                    relay.catalog_gate['accepted'] and not relay.errors, 'valid catalog gate did not forward once')
            passed.append('controlled-native-catalog-get-before-single-post-forward')
        else:
            require(status == 502 and counters['posts'] == 0 and not relay.calls and
                    relay.catalog_gate['blocked_posts'] == 1 and not relay.catalog_gate['accepted'],
                    'changed catalog reached scripted upstream POST')
            passed.append('controlled-changed-catalog-blocks-first-post')
    return passed


def selftest(args):
    manifest = verify_stage(args.stage.resolve())
    oracle_path = Path(manifest['oracle']['path']); oracle_hash = manifest['oracle']['sha256']
    mapping = {item['hex']: item['expected'] for item in load(oracle_path)}
    expected = [mapping[value] for value in manifest['prediction_inputs']]
    reference = (args.stage / 'author/reference.py').read_text(encoding='utf-8')
    passed = []
    def submission(code, predictions=expected):
        return '```python\n' + code + '\n```\n```json\n' + json.dumps({'predictions': predictions}) + '\n```'
    with tempfile.TemporaryDirectory(prefix='ara-h2-harness-selftest-') as temporary:
        root = Path(temporary)
        def evaluate(name, text, timeout=8):
            folder = root / name; folder.mkdir()
            return evaluate_submission(text, folder, oracle_path, oracle_hash, manifest['prediction_inputs'], timeout)
        value = evaluate('correct-h2', submission(reference)); require(value['status'] == 'PASS', 'H2 oracle rejected correct reference')
        passed.append('correct-H2-all128-and-four-predictions')
        value = evaluate('bad-predictions', submission(reference, [True] * 4)); require(value['status'] == 'FAIL' and
            value['hidden_outputs_correct'] and not value['predictions_correct'], 'Boolean prediction incorrectly accepted')
        passed.append('prediction-bool-rejected-with-correct-code')
        for name, text in [('missing-block', 'no submission'), ('duplicate-block', submission(reference) + submission(reference)),
                           ('invalid-python', submission('def recover(:')),
                           ('missing-function', submission('value = 0'))]:
            require(evaluate(name, text)['status'] == 'NOT_RUN', name + ' did not stay NOT_RUN'); passed.append(name)
        value = evaluate('old-style-wrong', submission('def recover(data):\n    return 0xe0000002'))
        require(value['status'] == 'FAIL' and value['hidden_outputs_matched'] == 0, 'old error-family candidate passed')
        passed.append('old-R1-family-rejected')
        value = evaluate('timeout', submission('def recover(data):\n    import time\n    time.sleep(2)\n    return 0'), 0.05)
        require(value['status'] == 'FAIL' and value['effects_unknown'] and value['execution']['timeout'], 'evaluator timeout lost unknown state')
        passed.append('evaluation-timeout-no-retry')
        tamper = 'from pathlib import Path\nPath(__file__).write_text("changed")\n' + reference
        value = evaluate('tamper-sealed-source', submission(tamper))
        require(value['status'] == 'FAIL' and not value['seal_integrity_after_evaluation'], 'candidate seal mutation accepted')
        passed.append('sealed-code-mutation-rejected')
        def rejects(name, operation):
            try: operation()
            except AssertionError: passed.append(name)
            else: raise AssertionError('selftest did not reject ' + name)
        rejects('missing-catalog-model', lambda: select_catalog({'data': []}, ARMS[1]['model']))
        rejects('duplicate-catalog-model', lambda: select_catalog({'data': [{'id': ARMS[0]['model']}] * 2}, ARMS[0]['model']))
        rejects('wrong-catalog-protocol', lambda: select_catalog({'data': [{'id': ARMS[0]['model'], 'protocols': ['anthropic']}]}, ARMS[0]['model']))
        row = {'id': ARMS[0]['model']}
        require(validate_actual_catalog({'data': [row]}, ARMS[0], row) == row, 'valid catalog gate failed')
        rejects('changed-catalog-before-post', lambda: validate_actual_catalog(
            {'data': [{**row, 'max_tokens': 4096}]}, ARMS[0], row))
        rejects('insufficient-catalog-before-post', lambda: validate_actual_catalog(
            {'data': [{**row, 'max_tokens': 4096}]}, ARMS[0], {**row, 'max_tokens': 4096}))
        require(catalog_caps({'id': 'unknown'}) == {'context_window': None, 'max_tokens': 8192, 'input': None}, 'capacity fallback differs')
        require(catalog_caps({'max_tokens': 16384, 'omp': {'maxTokens': 8192}})['max_tokens'] == 8192, 'capacity did not use minimum')
        rejects('bool-catalog-capacity', lambda: catalog_caps({'max_tokens': True}))
        passed.append('unknown-capacity-explicit-local-fallback-and-minimum')
        phases = [{'type': 'model_call', 'call': 1, 'phase': 'summary'}, {'type': 'model_call', 'call': 2, 'phase': 'agent'}]
        require(call_phases(phases, 2) == {1: 'summary', 2: 'agent'}, 'mixed phases invalid')
        rejects('phase-duplicate', lambda: call_phases(phases + [phases[0]], 2))
        rejects('shared-call-budget', lambda: call_phases(phases, 29))
        for arm in ARMS:
            for phase in ('agent', 'summary'):
                folder = root / (arm['arm'] + phase); folder.mkdir()
                request = {'model': arm['model'], 'stream': True, 'max_tokens': 8192 if phase == 'agent' else 4096}
                if phase == 'agent':
                    request['tools'] = [{'function': {'name': 'binary_inspect'}}]
                else:
                    request['response_format'] = {'type': 'json_object'}
                if arm['policy'] == 'thinking_type_disabled' or phase == 'summary' and arm['model'] == 'deepseek-v4.1-flash':
                    request['thinking'] = {'type': 'disabled'}
                write_json(folder / 'call-01.request.native.json', request)
                response = {'model': arm['model'], 'choices': [{'delta': {'content': 'fixture'}, 'finish_reason': 'stop'}]}
                (folder / 'call-01.response.native.bin').write_bytes(('data: ' + json.dumps(response) + '\n\ndata: [DONE]\n').encode())
                observe_wire(folder, 1, arm, phase, 1); passed.append(arm['arm'] + '-' + phase + '-wire')
                alias = {**response, 'model': 'controlled-unverified-alias'}
                (folder / 'call-01.response.native.bin').write_bytes(('data: ' + json.dumps(alias) + '\n\ndata: [DONE]\n').encode())
                aliased = observe_wire(folder, 1, arm, phase, 1)
                require(aliased['response_models'] == ['controlled-unverified-alias'] and
                        aliased['response_model_discrepancy'] and aliased['alias_identity'] == 'UNVERIFIED',
                        'provider echo alias was normalized, inferred, or incorrectly blocked')
                passed.append(arm['arm'] + '-' + phase + '-alias-recorded-not-accepted-as-identity')
                wrong = dict(request); wrong['model'] = 'wrong'
                write_json(folder / 'call-01.request.native.json', wrong)
                rejects(arm['arm'] + '-' + phase + '-wrong-model', lambda f=folder, a=arm, p=phase: observe_wire(f, 1, a, p, 1))
        lock = root / 'STARTED'; lock.open('x').close()
        try: lock.open('x').close()
        except FileExistsError: passed.append('exclusive-STARTED-replay-rejected')
        else: raise AssertionError('STARTED replay accepted')
        passed.extend(relay_selftest(root))
    return {'status': 'PASS', 'checks': len(passed), 'passed': passed, 'model_calls': 0,
            'catalog_network_calls': 0, 'native_host_runs': 0, 'cargo_invocations': 0,
            'sample_executions': 0, 'controlled_generated_candidate_processes': 5,
            'controlled_loopback_upstream_posts': 1, 'harness_sha256': file_hash(Path(__file__)),
            'stage_manifest_sha256': file_hash(args.stage / 'manifest.json')}


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    mode = parser.add_mutually_exclusive_group(required=True)
    mode.add_argument('--prepare', action='store_true'); mode.add_argument('--live', action='store_true')
    mode.add_argument('--selftest', action='store_true')
    parser.add_argument('--output', type=Path, default=OUTPUT)
    parser.add_argument('--stage', type=Path, default=STAGE)
    parser.add_argument('--route-result', type=Path, default=ROUTE)
    parser.add_argument('--catalog', type=Path, default=CATALOG)
    parser.add_argument('--selftest-record', type=Path, default=BASE / 'reverse-holdout-selftest-v1.json')
    parser.add_argument('--amend-from', type=Path, help='existing failed PRE archive; only before any live STARTED marker')
    parser.add_argument('--arm', choices=[arm['arm'] for arm in ARMS])
    args = parser.parse_args()
    require(not args.live or args.arm, '--live requires exactly one fixed arm')
    result = selftest(args) if args.selftest else prepare(args) if args.prepare else live(args)
    if args.selftest:
        require(not args.selftest_record.exists(), 'selftest record exists; retain it and choose a fresh record path')
        write_json(args.selftest_record, result)
    print(json.dumps(result, ensure_ascii=False, indent=2))
    require(result['status'] in ('PASS', 'PREPARED'), 'holdout failed/incomplete; retain evidence and never replay arm')


if __name__ == '__main__':
    main()
