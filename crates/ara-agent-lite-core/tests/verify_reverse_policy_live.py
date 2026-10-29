"""Five preregistered, one-shot R1 mechanism diagnostics; NOT a new benchmark.

Reuse the original public specimen and 28 private oracle cases, never an old
Session. --prepare makes a new immutable-by-convention plan without model calls.
--live --arm A..E performs one new Run, in that order, once per evidence root.
All arms have identical Plan tools. Only policy, output cap and Host-provided
static observations differ. The specimen is never executed. No OS sandbox or
hidden-material unreadability is claimed. Raw evidence remains private.
"""
import argparse
import ast
import json
import math
import os
from pathlib import Path
import re
import shutil
import subprocess
import tempfile
import time

from verify_analysis_live import PRIVATE, ROUTE, pins, reverse_oracle, route_from
from verify_coding_live import Relay, digest, inventory, require, scan_actual_token, write_json

ORIGINAL = Path('D:/WorkSpace/tmp/ara-core-capability-20261022/analysis-live')
ARMS = (
    {'arm': 'A', 'policy': 'provider_default', 'output': 4096, 'host_facts': False},
    {'arm': 'B', 'policy': 'thinking_type_disabled', 'output': 4096, 'host_facts': False},
    {'arm': 'C', 'policy': 'provider_default', 'output': 4096, 'host_facts': True},
    {'arm': 'D', 'policy': 'thinking_type_disabled', 'output': 4096, 'host_facts': True},
    {'arm': 'E', 'policy': 'provider_default', 'output': 8192, 'host_facts': False},
)
OBJECTIVE = ('Read README.md and reconstruct the supplied specimen using static evidence only. '
             'Do not execute it. Return the required complete Python and JSON blocks, plus instruction-address evidence.')


def current_pins():
    result = pins()
    result['tests/verify_reverse_policy_live.py'] = digest(Path(__file__).read_bytes())
    return result


def load(path):
    return json.loads(path.read_text(encoding='utf-8-sig'))


def prepare(args):
    root, original, private = args.output.resolve(), args.original.resolve(), args.private.resolve()
    require(root != original and original not in root.parents and root not in original.parents, 'new evidence must be separate from old Runs')
    require(private != root and private not in root.parents and root not in private.parents, 'oracle root must not overlap public project parents')
    old = load(original / 'plan.json')
    fixture = old['fixtures']['R1']
    source = original / 'R1' / 'new' / 'project'
    require(inventory(source) == fixture['public_pins'], 'original public R1 bytes changed')
    oracle = private / 'R1-oracle.json'
    require(digest(oracle.read_bytes()) == fixture['oracle_sha256'] and len(load(oracle)) == 28, 'original 28-case oracle identity differs')
    binary = args.binary.resolve(); require(binary.is_file(), 'prepare requires an existing new business_host binary')
    endpoint, model = route_from(args.route_result)
    backends = {}
    for name in ('objdump', 'nm'):
        value = getattr(args, name) or shutil.which(name)
        require(value and Path(value).is_file(), 'explicit static backend unavailable: ' + name)
        path = Path(value).resolve()
        backends[name] = {'path': str(path), 'sha256': digest(path.read_bytes())}
    root.mkdir(parents=True, exist_ok=False)
    for arm in ARMS:
        work = root / arm['arm'] / 'project'
        work.mkdir(parents=True)
        for name in fixture['public_pins']:
            target = work / name; target.parent.mkdir(parents=True, exist_ok=True)
            shutil.copyfile(source / name, target)
        require(inventory(work) == fixture['public_pins'], 'diagnostic copy differs')
    plan = {'schema': 'ara-reverse-policy-diagnostic-v1', 'arms': list(ARMS),
        'objective': OBJECTIVE, 'max_runs': 5, 'max_calls': 28, 'total_seconds_per_arm': 720,
        'facts': {'max_queries': 3, 'actions': ['headers', 'symbols', 'disassemble'],
                  'disassembly_bytes_from_header_entry': 4096, 'rows_per_query': 100, 'brief_bytes': 32768,
                  'source': 'actual analysis_query output only; never private algorithm/source'},
        'fixture': fixture, 'original_plan_sha256': digest((original / 'plan.json').read_bytes()),
        'binary_path': str(binary), 'binary_sha256': digest(binary.read_bytes()),
        'backends': backends, 'source_pins': current_pins(),
        'model': model, 'protocol': 'openai-completions', 'route_sha256': digest(endpoint.encode()),
        'route_record_sha256': digest(args.route_result.read_bytes()),
        'limits': ['same original specimen/oracle: mechanism diagnosis, not generalization or resumed old Runs',
                   'facts are Host observations, not model-autonomous tool use',
                   'default policy omits the policy field; other defaults remain unchanged',
                   'all arms have the same Plan/static tools; no specimen execution',
                   'no OS sandbox; oracle not exposed, not proven unreadable',
                   '720s covers Host facts plus model admission budget; bounded cleanup/oracle overhead is additionally measured',
                   'relay retains existing 45s socket/50s response checks equally for all arms']}
    write_json(root / 'plan.json', plan)
    plan_hash = digest((root / 'plan.json').read_bytes())
    (root / 'plan.sha256').write_text(plan_hash + '\n', encoding='ascii')
    return {'status': 'PREPARED', 'arms': [a['arm'] for a in ARMS], 'plan_sha256': plan_hash,
            'model_calls': 0, 'has_token': bool(os.environ.get('CAS_API_KEY'))}


def verify_query(value, sample_hash):
    require(value.get('schema') == 'ara.analysis.v1' and value.get('tool') == 'binary_inspect', 'analysis identity differs')
    item = value['input']; snapshot = Path(item['snapshot_path'])
    require(item.get('sha256') == sample_hash and snapshot.is_file() and digest(snapshot.read_bytes()) == sample_hash,
            'Host observation specimen/snapshot hash differs')
    backend = value['backend']; executable = Path(backend['path'])
    require(executable.is_file() and digest(executable.read_bytes()) == backend['sha256'], 'backend receipt hash differs')
    return value.get('status') == 'completed' and value.get('analysis', {}).get('complete') is True \
        and value.get('page', {}).get('complete') is True


def static_facts(plan, folder, work, started, token):
    query_root = folder / 'host-static'; query_root.mkdir()
    observations, errors = [], []
    entry = None
    sample_hash, sample = plan['fixture']['sample_sha256'], plan['fixture']['binary_name']
    for index, action in enumerate(('headers', 'symbols', 'disassemble'), 1):
        if action == 'disassemble' and entry is None:
            observations.append({'action': action, 'status': 'NOT_RUN', 'reason': 'no entry address in actual headers'})
            break
        remaining = 720 - (time.monotonic() - started)
        if remaining <= 1:
            errors.append('total arm deadline exhausted before static observation'); break
        query = query_root / str(index); query.mkdir(); (query / 'artifacts').mkdir()
        config = {'artifact_root': str(query / 'artifacts'), **{k: v['path'] for k, v in plan['backends'].items()}}
        arguments = {'path': sample, 'action': action, 'expected_sha256': sample_hash, 'limit': 100}
        if action == 'disassemble': arguments.update(start_vma=hex(entry), stop_vma=hex(entry + 4096))
        request = {'id': 'host-static-' + action, 'objective': 'Read-only Host static observation',
            'action': 'analysis_query', 'cwd': str(work), 'session_path': str(query / 'unused-session.json'),
            'environment_state': str(query / 'unused-environment'), 'analysis_config': config,
            'analysis_tool': 'binary_inspect', 'analysis_args': arguments}
        write_json(query / 'request.json', request)
        environment = os.environ.copy(); environment.pop('CAS_API_KEY', None)
        begin = time.monotonic(); code = None; out = err = b''
        try:
            process = subprocess.run([plan['binary_path'], str(query / 'request.json')], cwd=work,
                env=environment, capture_output=True, timeout=min(40, remaining))
            code, out, err = process.returncode, process.stdout, process.stderr
            packets = [json.loads(line) for line in out.decode('utf-8').splitlines() if line.strip()]
            packet = packets[-1]
            require(code == 0 and packet.get('kind') == 'analysis_result' and packet.get('status') == 'completed', 'analysis_query failed')
            value = json.loads(packet['output'])
            require(value.get('action') == action, 'static query action differs')
            complete = verify_query(value, sample_hash)
            row = {'action': action, 'status': 'COMPLETE' if complete else 'PARTIAL', 'receipt': value}
            if action == 'headers':
                for line in value.get('lines', []):
                    found = re.search(r'\bstart address\s+(?:0x)?([0-9a-fA-F]+)', line)
                    if found: entry = int(found.group(1), 16); break
            observations.append(row)
        except subprocess.TimeoutExpired as error:
            out, err = error.stdout or b'', error.stderr or b''
            errors.append(action + ': static Host timeout; completion unknown')
            observations.append({'action': action, 'status': 'UNKNOWN'}); break
        except Exception as error:
            errors.append((action + ': ' + type(error).__name__ + ': ' + str(error)).replace(token, '[REDACTED]'))
            observations.append({'action': action, 'status': 'FAILED'})
        finally:
            (query / 'host.stdout.native.jsonl').write_bytes(out)
            (query / 'host.stderr.native.txt').write_bytes(err)
            write_json(query / 'settlement.json', {'exit_code': code, 'seconds': time.monotonic() - begin,
                                                  'stdout_sha256': digest(out), 'stderr_sha256': digest(err)}, token)
    payload = {'origin': 'Host static observations, not autonomous model tool use', 'sample_sha256': sample_hash,
               'entry_from_headers': hex(entry) if entry is not None else None, 'queries': observations, 'errors': errors,
               'coverage': 'only retained pages/address range; no full-program completeness claim'}
    raw = json.dumps(payload, ensure_ascii=False)
    # Keep identity/partial status while reducing only displayed line arrays.
    while len(raw.encode()) > 32768:
        rows = [r for r in observations if len(r.get('receipt', {}).get('lines', [])) > 1]
        require(rows, 'static metadata alone exceeds brief budget')
        row = max(rows, key=lambda r: len(r['receipt']['lines']))
        lines = row['receipt']['lines']; keep = max(1, len(lines) // 2)
        row['brief_omitted_lines'] = row.get('brief_omitted_lines', 0) + len(lines) - keep
        row['receipt']['lines'] = lines[:keep]; row['status'] = 'PARTIAL'
        raw = json.dumps(payload, ensure_ascii=False)
    status = 'COMPLETE' if len(observations) == 3 and not errors and all(r['status'] == 'COMPLETE' for r in observations) else 'PARTIAL'
    write_json(query_root / 'observations.json', payload, token)
    (query_root / 'brief.txt').write_text(raw, encoding='utf-8', newline='\n')
    return raw, {'status': status, 'queries': sum(r['status'] != 'NOT_RUN' for r in observations), 'errors': errors,
                 'effects_unknown': any(r['status'] == 'UNKNOWN' for r in observations),
                 'brief_sha256': digest(raw.encode()), 'brief_bytes': len(raw.encode()),
                 'seconds': round(time.monotonic() - started, 3)}


def call_phases(events, actual_calls):
    require(0 < actual_calls <= 28, 'actual shared call budget differs')
    phases = {}
    for event in events:
        if event.get('type') != 'model_call': continue
        number, phase = event.get('call'), event.get('phase')
        require(type(number) is int and number not in phases and phase in ('agent', 'summary'),
                'invalid, duplicate or unknown native call phase')
        phases[number] = phase
    require(set(phases) == set(range(1, actual_calls + 1)), 'native call IDs do not match actual HTTP calls')
    require('agent' in phases.values(), 'no actual agent call')
    return phases


def wire_observation(folder, number, arm, brief, phase, first_agent_call, model):
    request_path = folder / f'call-{number:02d}.request.native.json'
    response_path = folder / f'call-{number:02d}.response.native.bin'
    request = load(request_path)
    require(phase in ('agent', 'summary'), 'unknown wire phase')
    # summary_model clones the selected policy and applies the existing exact
    # deepseek-v4.1-flash/OpenAI summary override (agent.rs); it is not arm policy.
    disabled = arm['policy'] == 'thinking_type_disabled' or (phase == 'summary' and model == 'deepseek-v4.1-flash')
    expected = {'type': 'disabled'} if disabled else None
    require(request.get('thinking') == expected and ('thinking' in request) == (expected is not None), 'wire thinking root field differs')
    require('reasoning_effort' not in request and 'extra_body' not in request, 'unexpected reasoning/SDK wrapper wire field')
    output = arm['output'] if phase == 'agent' else min(arm['output'], 4096)
    require(request.get('max_tokens') == output and 'max_completion_tokens' not in request, 'wire output budget differs')
    if phase == 'summary':
        require(request.get('response_format') == {'type': 'json_object'}, 'summary JSON response format differs')
        require(request.get('tools', []) == [] and 'tool_choice' not in request, 'summary unexpectedly exposes tools')
    verify_brief = brief and phase == 'agent' and number == first_agent_call
    if verify_brief:
        contents = [m.get('content', '') for m in request.get('messages', [])]
        texts = [v for v in contents if isinstance(v, str)]
        texts += [b.get('text', '') for v in contents if isinstance(v, list) for b in v if isinstance(b, dict)]
        require(any(brief in text for text in texts), 'actual first provider request omitted or changed Host static brief')
    counts = {'content_chars': 0, 'content_bytes': 0, 'reasoning_chars': 0, 'reasoning_bytes': 0, 'tool_call_deltas': 0}
    reasons, models, usage = [], set(), []
    for line in response_path.read_bytes().splitlines():
        if not line.startswith(b'data: ') or line[6:].strip() == b'[DONE]': continue
        value = json.loads(line[6:])
        if isinstance(value.get('model'), str): models.add(value['model'])
        if isinstance(value.get('usage'), dict): usage.append(value['usage'])
        for choice in value.get('choices', []):
            delta = choice.get('delta', {})
            text = delta.get('content') or ''
            reasoning = delta.get('reasoning_content') or delta.get('reasoning') or delta.get('reasoning_text') or ''
            require(isinstance(text, str) and isinstance(reasoning, str), 'unexpected non-text delta')
            counts['content_chars'] += len(text); counts['content_bytes'] += len(text.encode())
            counts['reasoning_chars'] += len(reasoning); counts['reasoning_bytes'] += len(reasoning.encode())
            counts['tool_call_deltas'] += len(delta.get('tool_calls') or [])
            if choice.get('finish_reason'): reasons.append(choice['finish_reason'])
    return {'call': number, 'phase': phase, 'request_sha256': digest(request_path.read_bytes()),
            'response_sha256': digest(response_path.read_bytes()), 'thinking': expected,
            'max_tokens': request['max_tokens'], 'counts': counts, 'finish_reasons': reasons,
            'host_brief_sha256_verified': digest(brief.encode()) if verify_brief else None,
            'response_models': sorted(models), 'usage': usage[-1] if usage else None}


def evaluate_submission(text, folder, oracle_path, oracle_hash):
    python = re.findall(r'```python\s*\n(.*?)```', text, re.DOTALL | re.IGNORECASE)
    prediction = re.findall(r'```json\s*\n(.*?)```', text, re.DOTALL | re.IGNORECASE)
    if len(python) != 1 or len(prediction) != 1:
        return {'status': 'NOT_RUN', 'reason': 'missing/incomplete required submission blocks', 'planned_cases': 28}
    try:
        ast.parse(python[0]); value = json.loads(prediction[0])
        require(isinstance(value, dict) and isinstance(value.get('predictions'), list), 'invalid prediction object')
    except (SyntaxError, ValueError, AssertionError):
        return {'status': 'NOT_RUN', 'reason': 'invalid/incomplete submitted code or predictions', 'planned_cases': 28}
    require(digest(oracle_path.read_bytes()) == oracle_hash, 'old oracle changed before evaluation')
    # reverse_oracle seals exact submitted code and prediction JSON before any
    # hidden input is sent to the candidate process, and rechecks both afterward.
    return reverse_oracle(text, folder, load(oracle_path))


def live(args):
    root, private = args.output.resolve(), args.private.resolve()
    plan_path = root / 'plan.json'; plan_hash = digest(plan_path.read_bytes())
    require(plan_hash == (root / 'plan.sha256').read_text(encoding='ascii').strip(), 'preregistered plan changed')
    plan = load(plan_path); require(plan['arms'] == list(ARMS), 'diagnostic condition matrix changed')
    require(current_pins() == plan['source_pins'], 'delivered source/helper snapshot changed after planning')
    binary = Path(plan['binary_path']); require(digest(binary.read_bytes()) == plan['binary_sha256'], 'new Host binary changed')
    endpoint, model = route_from(args.route_result)
    require(model == plan['model'] and digest(endpoint.encode()) == plan['route_sha256']
            and digest(args.route_result.read_bytes()) == plan['route_record_sha256'], 'fixed route changed')
    for backend in plan['backends'].values(): require(digest(Path(backend['path']).read_bytes()) == backend['sha256'], 'pinned static backend changed')
    arm = next(a for a in ARMS if a['arm'] == args.arm)
    index = [a['arm'] for a in ARMS].index(args.arm)
    require(all((root / a['arm'] / 'result.json').is_file() for a in ARMS[:index]), 'finish preceding arms in fixed order, including failures')
    folder, work = root / args.arm, root / args.arm / 'project'
    require(private != work and private not in work.parents and work not in private.parents, 'private oracle overlaps model project/parents')
    require(inventory(work) == plan['fixture']['public_pins'], 'initial public specimen changed')
    token = os.environ.get('CAS_API_KEY', ''); require(token, 'CAS_API_KEY is required')
    (folder / 'STARTED').open('x').close()
    started = time.monotonic(); relay = None; facts = {'status': 'NOT_APPLICABLE', 'seconds': 0}
    result = {'schema': plan['schema'], 'arm': args.arm, 'condition': arm, 'plan_sha256': plan_hash,
        'status': 'FAIL', 'effects_unknown': True, 'oracle_status': 'NOT_RUN', 'has_token': True,
        'model': model, 'binary_sha256': plan['binary_sha256'], 'same_sample_mechanism_diagnostic': True,
        'model_observation_causality_or_generalization_proven': False}
    try:
        brief = ''
        if arm['host_facts']: brief, facts = static_facts(plan, folder, work, started, token)
        result['host_facts'] = facts
        remaining = 720 - math.ceil(time.monotonic() - started)
        require(remaining >= 1, 'total arm budget exhausted before model admission')
        artifacts = folder / 'model-analysis'; artifacts.mkdir()
        relay = Relay(endpoint, token, model, folder, max_calls=28)
        request = {'id': 'reverse-policy-' + args.arm, 'objective': plan['objective'], 'cwd': str(work),
            'session_path': str(folder / 'native-session.json'), 'environment_state': str(folder / 'environment-state'),
            'endpoint': relay.local_endpoint, 'model': model, 'token_env': 'CAS_API_KEY', 'mode': 'plan',
            'enable_local_tools': True, 'max_calls': 28, 'max_seconds': remaining, 'max_output': arm['output'],
            'analysis_config': {'artifact_root': str(artifacts), **{k: v['path'] for k, v in plan['backends'].items()}}}
        if arm['policy'] != 'provider_default': request['generation_policy'] = arm['policy']
        if brief: request['brief'] = brief
        write_json(folder / 'request.json', request)
        result['model_seconds_remaining_after_host_facts'] = remaining
        code = None; stdout = stderr = b''
        try:
            process = subprocess.run([str(binary), str(folder / 'request.json')], cwd=work,
                                     capture_output=True, timeout=remaining + 5)
            code, stdout, stderr = process.returncode, process.stdout, process.stderr
        except subprocess.TimeoutExpired as error:
            stdout, stderr = error.stdout or b'', error.stderr or b''
        finally:
            relay.close()
            (folder / 'host.stdout.native.jsonl').write_bytes(stdout)
            (folder / 'host.stderr.native.txt').write_bytes(stderr)
        packets = [json.loads(line) for line in stdout.decode('utf-8').splitlines() if line.strip()]
        events = [p['event'] for p in packets if p.get('kind') == 'event']
        final = next((p for p in reversed(packets) if p.get('kind') == 'result'), None)
        result.update(host_exit=code, effects_unknown=code is None or final is None or facts.get('effects_unknown', False),
            native_result_code=final.get('code') if final else None,
            native_policy_events=[e for e in events if e.get('type') in ('generation_policy', 'model_generation_policy')],
            model_capabilities=[e for e in events if e.get('type') == 'model_capabilities'])
        phases = call_phases(events, len(relay.calls))
        first_agent_call = min(n for n, phase in phases.items() if phase == 'agent')
        wire = [wire_observation(folder, n, arm, brief, phases[n], first_agent_call, model)
                for n in range(1, len(relay.calls) + 1)]
        result['wire_observations'] = wire
        result['agent_wire_observations'] = [w for w in wire if w['phase'] == 'agent']
        result['summary_wire_observations'] = [w for w in wire if w['phase'] == 'summary']
        require(not relay.errors, 'upstream relay failed; retained raw receipts')
        forbidden = [e for e in events if e.get('type') == 'tool_start' and e.get('title') in ('exec', 'run_script', 'edit_file', 'write_file')]
        require(not forbidden, 'Plan trial attempted execution/mutation; diagnostic invalid')
        require(inventory(work) == plan['fixture']['public_pins'], 'specimen/contract changed')
        result['autonomous_binary_tool_ends'] = sum(e.get('type') == 'tool_end' and e.get('title') == 'binary_inspect' for e in events)
        text = final.get('text') if final else ''
        oracle = evaluate_submission(text if isinstance(text, str) else '', folder,
                                     private / 'R1-oracle.json', plan['fixture']['oracle_sha256'])
        write_json(folder / 'oracle.json', oracle, token)
        result.update(oracle_status=oracle['status'], task_success=code == 0 and final is not None
                      and final.get('code') == 0 and oracle['status'] == 'PASS')
        result['conditions_realized'] = not arm['host_facts'] or facts['status'] == 'COMPLETE'
        result['status'] = ('PASS' if result['conditions_realized'] else 'PARTIAL') if result['task_success'] else 'FAIL'
    except Exception as error:
        result['error'] = (type(error).__name__ + ': ' + str(error)).replace(token, '[REDACTED]')
    finally:
        if relay is not None:
            if relay.thread.is_alive(): relay.close()
            write_json(folder / 'provider-receipts.json', {'calls': relay.calls, 'catalogs': relay.catalogs, 'errors': relay.errors}, token)
        result['total_wall_seconds_including_host_facts_and_oracle'] = round(time.monotonic() - started, 3)
        result['source_pins_unchanged'] = current_pins() == plan['source_pins']
        result['binary_unchanged'] = digest(binary.read_bytes()) == plan['binary_sha256']
        result['secret_scan'] = scan_actual_token(folder, token)
        result['raw_evidence_private'] = True
        if not result['source_pins_unchanged'] or not result['binary_unchanged'] or result['secret_scan']['status'] != 'CLEAN': result['status'] = 'FAIL'
        write_json(folder / 'result.json', result, token)
    return {k: result.get(k) for k in ('arm', 'status', 'oracle_status', 'task_success', 'has_token', 'effects_unknown',
                                     'autonomous_binary_tool_ends', 'total_wall_seconds_including_host_facts_and_oracle')}


def selftest():
    passed = []
    def rejects(name, operation):
        try: operation()
        except AssertionError: passed.append(name)
        else: raise AssertionError('selftest failed to reject: ' + name)
    model = 'deepseek-v4.1-flash'
    with tempfile.TemporaryDirectory(prefix='ara-phase-wire-selftest-') as temporary:
        folder = Path(temporary)
        def observe(arm, phase, request, number=1, brief='', first=1, selected_model=model):
            write_json(folder / f'call-{number:02d}.request.native.json', request)
            (folder / f'call-{number:02d}.response.native.bin').write_bytes(
                b'data: {"choices":[{"delta":{"content":"ok"},"finish_reason":"stop"}]}\n\ndata: [DONE]\n')
            return wire_observation(folder, number, arm, brief, phase, first, selected_model)
        for arm in ARMS:
            summary = {'thinking': {'type': 'disabled'}, 'max_tokens': 4096,
                       'response_format': {'type': 'json_object'}, 'messages': []}
            value = observe(arm, 'summary', summary)
            require(value['phase'] == 'summary' and value['max_tokens'] == 4096, 'valid summary selftest failed')
            passed.append(arm['arm'] + '-valid-summary')
            agent = {'max_tokens': arm['output'], 'messages': [{'role': 'user', 'content': 'Host brief'}]}
            if arm['policy'] == 'thinking_type_disabled': agent['thinking'] = {'type': 'disabled'}
            value = observe(arm, 'agent', agent, number=2, brief='Host brief', first=2)
            require(value['host_brief_sha256_verified'] == digest(b'Host brief'), 'first agent brief not verified')
            passed.append(arm['arm'] + '-valid-agent-after-summary')
            broken = dict(agent)
            if arm['policy'] == 'thinking_type_disabled': broken.pop('thinking')
            else: broken['thinking'] = {'type': 'disabled'}
            rejects(arm['arm'] + '-agent-wrong-thinking', lambda: observe(arm, 'agent', broken))
        arm = ARMS[4]
        rejects('E-agent-wrong-output', lambda: observe(arm, 'agent', {'max_tokens': 4096}))
        summary = {'thinking': {'type': 'disabled'}, 'max_tokens': 4096, 'response_format': {'type': 'json_object'}}
        for name, key, value in [('output', 'max_tokens', 8192), ('JSON', 'response_format', {}),
                                 ('tools', 'tools', [{}]), ('thinking', 'thinking', None)]:
            rejects('summary-wrong-' + name, lambda k=key, v=value: observe(arm, 'summary', {**summary, k: v}))
        rejects('first-agent-missing-brief', lambda: observe(ARMS[2], 'agent', {'max_tokens': 4096},
                                                          number=2, brief='Host brief', first=2))
        # A nonlegacy model inherits the selected policy; no generic model-name mapping.
        observe(ARMS[0], 'summary', {'max_tokens': 4096, 'response_format': {'type': 'json_object'}}, selected_model='other')
        observe(ARMS[1], 'summary', summary, selected_model='other')
        passed.append('summary-inherited-policy')
        valid = [{'type': 'model_call', 'call': 1, 'phase': 'summary'},
                 {'type': 'model_call', 'call': 2, 'phase': 'agent'}]
        require(call_phases(valid, 2) == {1: 'summary', 2: 'agent'}, 'phase binding selftest failed')
        passed.append('native-call-phase-binding')
        for name, events, count in [
            ('duplicate', valid + [valid[0]], 2), ('missing', valid[:1], 2),
            ('unknown', [{**valid[0], 'phase': 'unknown'}, valid[1]], 2),
            ('wrong-id', [{**valid[0], 'call': 3}, valid[1]], 2),
            ('no-agent', valid[:1], 1), ('shared-budget', valid, 29)]:
            rejects('phase-' + name, lambda e=events, c=count: call_phases(e, c))
        budget = [{'type': 'model_call', 'call': n, 'phase': 'summary' if n % 2 else 'agent'} for n in range(1, 29)]
        require(len(call_phases(budget, 28)) == 28, 'shared budget boundary selftest failed')
        passed.append('shared-28-call-boundary')
    return {'status': 'PASS', 'model_calls': 0, 'cargo_invocations': 0, 'checks': len(passed), 'passed': passed}


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    mode = parser.add_mutually_exclusive_group(required=True)
    mode.add_argument('--prepare', action='store_true'); mode.add_argument('--live', action='store_true')
    mode.add_argument('--selftest', action='store_true')
    parser.add_argument('--output', type=Path)
    parser.add_argument('--original', type=Path, default=ORIGINAL)
    parser.add_argument('--private', type=Path, default=PRIVATE)
    parser.add_argument('--route-result', type=Path, default=ROUTE)
    parser.add_argument('--binary', type=Path)
    parser.add_argument('--arm', choices=[a['arm'] for a in ARMS])
    parser.add_argument('--objdump'); parser.add_argument('--nm')
    args = parser.parse_args()
    require(args.selftest or args.output, '--prepare/--live require --output')
    require(not args.prepare or args.binary, '--prepare requires the newly built --binary')
    require(not args.live or args.arm, '--live requires exactly one --arm')
    result = selftest() if args.selftest else prepare(args) if args.prepare else live(args)
    print(json.dumps(result, ensure_ascii=False, indent=2))
    require(result['status'] in ('PREPARED', 'PASS'), 'fixed diagnostic failed/partial; preserve evidence, never replay this arm')


if __name__ == '__main__':
    main()
