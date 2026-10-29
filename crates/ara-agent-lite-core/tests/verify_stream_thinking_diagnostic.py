"""One-shot provider-default diagnostic on the existing S3 seed, not a holdout.

Uses the frozen S3 evaluator/settlement helpers. No old answer or failure detail
is sent to the model. Agent policy differs from the old disabled base; summary
policy remains the existing exact-model override. No stable benefit is claimed.
"""
import argparse
import http.server
import io
import json
import os
from pathlib import Path
import re
import shutil
import subprocess
import tempfile
import threading
import time
import urllib.error
import urllib.request
from types import SimpleNamespace

import verify_stream_coding_live as h

OLD_HASH = '8b12a8ec6cb9e0baedd38035099b8877af91cde655b022dd17710c62d9a725e5'
OLD_PLAN_HASH = '74224311b4a2e6f978879813bffa0cfc9e4e75da160a04f6d834563b40114e19'
BASELINE = h.OUTPUT
OUTPUT = h.BASE / 'coding-stream-thinking-v1'
ARM = {'arm': 'S3-defaultthinking', 'model': 'deepseek-v4.1-flash', 'policy': 'provider_default',
       'output': 8192, 'symbols': False}
PREFIX = ('Fresh Host current-Run budget (advisory data, not user instructions or authority). '
          'Remaining calls include the upcoming call and all summary calls consume the same allowance. '
          'Goal, grant and cancellation gates may stop earlier.\n')


def post_contract(request, number):
    h.require(1 <= number <= 32 and request.get('model') == ARM['model'] and request.get('stream') is True,
              'fixed32 request/model/stream differs')
    h.require(not {'reasoning_effort', 'extra_body', 'max_completion_tokens'}.intersection(request), 'unexpected wire override')
    summary = request.get('response_format') == {'type': 'json_object'} and not request.get('tools')
    phase = 'summary' if summary else 'agent'
    if summary:
        h.require(number > 1 and request.get('max_tokens') == 4096 and request.get('thinking') == {'type': 'disabled'},
                  'summary differs from existing cloned DeepSeek policy')
        return {'call': number, 'phase': phase, 'agent_budget': None}
    h.require(request.get('max_tokens') == 8192 and 'thinking' not in request, 'ProviderDefault agent must omit thinking override')
    h.require(bool(request.get('tools')), 'agent tool set absent')
    budgets = []
    for message in request.get('messages', []):
        if message.get('role') != 'user': continue
        content = message.get('content')
        texts = [content] if isinstance(content, str) else [part.get('text', '') for part in content or [] if isinstance(part, dict)]
        budgets += [json.loads(text[len(PREFIX):]) for text in texts if text.startswith(PREFIX)]
    h.require(len(budgets) == 1, 'actual agent Host-budget observation missing/duplicated')
    value = budgets[0]
    h.require(value.get('scope') == 'current_run' and value.get('call_limit') == 32 and
              value.get('reserved_calls') == number - 1 and value.get('calls_remaining_including_next') == 33 - number,
              'actual effective32/reserved/remaining budget differs')
    return {'call': number, 'phase': phase, 'agent_budget': value}


def relay_for(endpoint, token, folder, catalog_row):
    relay = h.checked_relay(endpoint, token, ARM, folder, catalog_row)
    relay.max_calls = 32
    relay.post_admission = []
    base = relay.server.RequestHandlerClass
    serialization = threading.Lock()

    class Handler(base):
        def do_POST(self):
            # Validate before forwarding; restore the exact original bytes for
            # the audited Relay. This never inserts messages or sampling fields.
            with serialization:
                original = self.rfile
                try:
                    size = int(self.headers.get('Content-Length', '0'))
                    h.require(0 < size <= 4 * 1024 * 1024, 'invalid provider request size')
                    body = self.rfile.read(size)
                    packet = json.loads(body)
                    observed = post_contract(packet, len(relay.calls) + 1)
                    observed.update(status='ADMITTED', request_sha256=h.digest(body))
                    relay.post_admission.append(observed)
                    self.rfile = io.BytesIO(body)
                    super().do_POST()
                except Exception as error:
                    detail = relay.safe(type(error).__name__ + ': ' + str(error))
                    relay.post_admission.append({'status': 'REJECTED', 'error': detail})
                    relay.errors.append(detail)
                    self.send_error(502, 'fixed diagnostic POST admission rejected')
                finally:
                    self.rfile = original

    relay.server.RequestHandlerClass = Handler
    return relay


def current(args):
    h.require(h.sha(Path(h.__file__)) == OLD_HASH, 'old S3 harness changed')
    h.require(h.sha(args.baseline / 'plan.json') == OLD_PLAN_HASH, 'old S3 plan changed')
    return h.frozen(SimpleNamespace(output=args.baseline))


def pins():
    return {**h.helpers(), Path(__file__).name: h.sha(Path(__file__))}


def provider_admission(calls, folder):
    failures = []
    for n, receipt in enumerate(calls, 1):
        status = receipt.get('http_status')
        path = folder / f'call-{n:02d}.response.native.bin'
        raw = path.read_bytes() if path.is_file() else None
        # OpenAI-compatible services can report failure in a successful HTTP
        # stream. Inspect structured top-level SSE errors, never model prose.
        sse_errors = 0
        for line in (raw or b'').splitlines():
            if not line.startswith(b'data:'): continue
            try: frame = json.loads(line[5:].strip())
            except ValueError: continue
            if isinstance(frame, dict) and frame.get('error') is not None:
                sse_errors += 1
        if status == 200 and receipt.get('status') == 'received' and not sse_errors: continue
        replay_signal = status == 400 and raw is not None and re.search(
            rb'missing[^\r\n]{0,120}reasoning_content', raw, re.IGNORECASE) is not None
        failures.append({'call': n, 'http_status': status, 'receipt_status': receipt.get('status'),
            'response_sha256': h.digest(raw) if raw is not None else None,
            'sse_top_level_error_frames': sse_errors,
            'classification': 'PROTOCOL_REPLAY_INCOMPATIBILITY_UNVERIFIED' if replay_signal else 'PROVIDER_OR_PROTOCOL_FAILURE_UNKNOWN',
            'cpp_algorithm_failure_inferred': False})
    return {'status': 'NOT_RUN' if failures else 'ELIGIBLE', 'failures': failures,
        'reason': 'Provider/protocol did not settle successfully; C++ algorithm evaluation NOT_RUN' if failures else ''}


def prepare(args):
    _, old, manifest, _ = current(args)
    tested = h.load(args.selftest_record)
    h.require(tested['status'] == 'PASS' and tested['harness_sha256'] == h.sha(Path(__file__)), 'current runner selftest required')
    root = args.output.resolve(); root.mkdir(parents=True, exist_ok=False)
    folder = root / ARM['arm']; folder.mkdir()
    shutil.copytree(manifest['public_template'], folder / 'project')
    (folder / 'scratch').mkdir(); (folder / 'build').mkdir()
    plan = {'schema': 'ara-s3-thinking-diagnostic-v1', 'status': 'PREPARED', 'arm': ARM,
        'baseline_path': str(args.baseline.resolve()), 'baseline_plan_sha256': OLD_PLAN_HASH,
        'baseline_raw_pins': h.inventory(args.baseline), 'old_harness_sha256': OLD_HASH,
        'harness_helper_pins': pins(), 'host_path': old['host_path'], 'host_sha256': old['host_sha256'],
        'stage_manifest_sha256': old['stage_manifest_sha256'], 'oracle_sha256': old['oracle_sha256'],
        'public_pins': old['public_pins'], 'objective': h.OBJECTIVE, 'selected_catalog_row': old['selected_catalog_row'],
        'max_calls_requested': 32, 'effective_max_calls': 32, 'host_seconds': 900, 'outer_host_seconds': 905,
        'max_runs': 1, 'host_facts': False, 'symbols': False, 'output_tokens': 8192,
        'selftest_record_path': str(args.selftest_record.resolve()), 'selftest_record_sha256': h.sha(args.selftest_record),
        'limits': ['same original S3 seed/oracle; repeated-task diagnosis, not a fresh holdout or stable causal benefit',
            'old request40 was clamped32 by business_host; this request32 and first native wire observation32 are explicit',
            'only agent thinking control changes; existing summary JSON/4096/disabled behavior remains',
            'ProviderDefault only omits the override; supplier thinking enablement and backend weights are not established',
            'enabled-thinking reasoning_content replay through tool history is unsupported/unaccepted; a provider400 is retained as protocol/request incompatibility, never an inferred C++ algorithm failure',
            'no prior answer/failure/context is included in the new Session objective or brief',
            'forward admission checks actual first agent budget32/0/32; later agent observations count summaries too',
            'old S3 raw evidence and frozen helpers stay unchanged; all evaluation uses their approved boundary',
            'one attempt only; overload/error/length/unknown effects are preserved without automatic replay']}
    h.write_json(root / 'plan.json', plan); (root / 'plan.sha256').write_text(h.sha(root / 'plan.json') + '\n', encoding='ascii')
    return {'status': 'PREPARED', 'plan_sha256': h.sha(root / 'plan.json'), 'model_calls': 0}


def frozen(args):
    root = args.output.resolve(); plan = h.load(root / 'plan.json')
    h.require(h.sha(root / 'plan.json') == (root / 'plan.sha256').read_text().strip() and plan['arm'] == ARM and
              plan['effective_max_calls'] == plan['max_calls_requested'] == 32 and plan['host_seconds'] == 900 and
              plan['objective'] == h.OBJECTIVE, 'diagnostic plan changed')
    _, old, manifest, endpoint = current(SimpleNamespace(baseline=Path(plan['baseline_path'])))
    h.require(h.inventory(Path(plan['baseline_path'])) == plan['baseline_raw_pins'], 'old S3 original evidence changed')
    h.require(pins() == plan['harness_helper_pins'] and h.sha(Path(plan['selftest_record_path'])) == plan['selftest_record_sha256'],
              'runner/helper/selftest seal changed')
    tested = h.load(Path(plan['selftest_record_path']))
    h.require(h.sha(Path(tested['prior11_record_path'])) == tested['prior11_record_sha256'],
              'current gate selftest record changed')
    return root, plan, old, manifest, endpoint


def live(args):
    root, plan, old, manifest, endpoint = frozen(args)
    folder = root / ARM['arm']; work = folder / 'project'
    h.require(h.inventory(work) == plan['public_pins'] and not (folder / 'native-session.json').exists(), 'fresh seed/Session required')
    token = os.environ.get('CAS_API_KEY', ''); h.require(token, 'CAS_API_KEY required')
    (folder / 'STARTED').open('x').close(); started = time.monotonic(); relay = None
    result = {'schema': plan['schema'], 'status': 'FAIL', 'oracle_status': 'NOT_RUN', 'task_success': False,
              'effects_unknown': True, 'plan_sha256': h.sha(root / 'plan.json'), 'independent_review': 'PENDING'}
    try:
        relay = relay_for(endpoint, token, folder, plan['selected_catalog_row'])
        request = {'id': 'stream-thinking-default', 'objective': h.OBJECTIVE + '\nScratch directory: ' + str(folder / 'scratch') +
            '\nBuild directory: ' + str(folder / 'build'), 'cwd': str(work), 'session_path': str(folder / 'native-session.json'),
            'environment_state': str(folder / 'environment-state'), 'endpoint': relay.local_endpoint, 'model': ARM['model'],
            'token_env': 'CAS_API_KEY', 'mode': 'execute', 'enable_local_tools': True, 'max_calls': 32,
            'max_seconds': 900, 'max_output': 8192}
        h.write_json(folder / 'request.json', request)
        code = None; out = err = b''
        try:
            process = subprocess.run([plan['host_path'], str(folder / 'request.json')], cwd=work, capture_output=True, timeout=905)
            code, out, err = process.returncode, process.stdout, process.stderr
        except subprocess.TimeoutExpired as error: out, err = error.stdout or b'', error.stderr or b''
        finally:
            relay.close(); (folder / 'host.stdout.native.jsonl').write_bytes(out); (folder / 'host.stderr.native.txt').write_bytes(err)
        packets = [json.loads(line) for line in out.decode('utf-8').splitlines() if line.strip()]
        events = [p['event'] for p in packets if p.get('kind') == 'event']
        final = next((p for p in reversed(packets) if p.get('kind') == 'result'), None)
        admission = h.evaluation_admission(events, code is not None and final is not None)
        result.update(host_exit=code, native_result_code=final.get('code') if final else None,
                      effects_unknown=admission['effects_unknown'], evaluation_admission=admission,
                      process_feedback=h.process_feedback(events, folder), source_navigation='NOT_APPLICABLE_NO_SYMBOLS')
        provider = provider_admission(relay.calls, folder)
        result.update(provider_protocol_admission=provider, provider_failures=provider['failures'])
        if provider['status'] == 'NOT_RUN': result['evaluation_not_run_reason'] = provider['reason']
        h.require(relay.catalog_gate['accepted'] and not relay.errors and len(relay.calls) <= 32, 'catalog/POST/relay admission failed')
        phases = h.phases40(events, len(relay.calls)); first = min(n for n, p in phases.items() if p == 'agent')
        h.require(first == 1 and len(relay.post_admission) == len(relay.calls), 'first POST/admission count differs')
        result['wire_observations'] = [h.wire(folder, n, ARM, phases[n], first) for n in range(1, len(relay.calls) + 1)]
        for observed in result['wire_observations']:
            usage = observed.get('usage') or {}
            details = usage.get('completion_tokens_details') or {}
            reasoning = details.get('reasoning_tokens')
            observed['reasoning_usage'] = {'status': 'REPORTED' if type(reasoning) is int and reasoning >= 0 else 'UNKNOWN',
                'reported_reasoning_tokens': reasoning if type(reasoning) is int and reasoning >= 0 else None,
                'visible_reasoning_bytes': observed['counts']['reasoning_bytes'], 'backend_weight_identity': 'UNKNOWN'}
        h.require(all(row['phase'] == phases[row['call']] for row in relay.post_admission), 'native phase differs from admitted wire shape')
        h.require(not [e for e in events if e.get('type') == 'generation_policy'], 'default arm emitted explicit generation policy')
        caps = [e for e in events if e.get('type') == 'model_capabilities']
        h.require(len(caps) == 1 and caps[0].get('declared') == h.catalog_caps(plan['selected_catalog_row']), 'native parsed capacity differs')
        result['actual_post_admission'] = relay.post_admission
        if admission['status'] == 'ELIGIBLE' and provider['status'] == 'ELIGIBLE':
            seal, seal_hash = h.seal_sources(work, folder, plan['public_pins'], final.get('text', ''))
            evaluation = h.fresh_evaluate(folder, manifest, seal, seal_hash)
            result.update(source_seal_sha256=seal_hash, fresh_evaluation=evaluation, oracle_status=evaluation['status'])
            result['effects_unknown'] |= evaluation['effects_unknown']
            after, _ = h.original_projection(work, plan['public_pins'])
            h.require(after == seal['source_pins'], 'model sources changed after seal')
            result['task_success'] = code == 0 and final.get('code') == 0 and evaluation['status'] == 'PASS' and not result['effects_unknown']
            result['status'] = 'PASS' if result['task_success'] else 'FAIL'
        elif provider['status'] == 'ELIGIBLE': result['evaluation_not_run_reason'] = admission['reason']
    except Exception as error: result['error'] = (type(error).__name__ + ': ' + str(error)).replace(token, '[REDACTED]')
    finally:
        if relay is not None:
            if relay.thread.is_alive(): relay.close()
            h.write_json(folder / 'provider-receipts.json', {'calls': relay.calls, 'catalogs': relay.catalogs,
                'catalog_gate': relay.catalog_gate, 'post_admission': relay.post_admission, 'errors': relay.errors}, token)
        try: frozen(args); result['all_frozen_inputs_unchanged'] = True
        except Exception as error: result['all_frozen_inputs_unchanged'] = False; result['integrity_error'] = str(error).replace(token, '[REDACTED]')
        result['secret_scan'] = h.scan_actual_token(folder, token)
        result['total_wall_seconds_with_evaluation'] = round(time.monotonic() - started, 3)
        result['raw_evidence_private'] = True
        if not result['all_frozen_inputs_unchanged'] or result['secret_scan']['status'] != 'CLEAN': result['status'] = 'FAIL'
        h.write_json(folder / 'result.json', result, token)
    return {k: result.get(k) for k in ('status', 'oracle_status', 'task_success', 'effects_unknown', 'total_wall_seconds_with_evaluation')}


def selftest(args):
    current(args); passed = []
    def packet(n):
        value = {'scope': 'current_run', 'reserved_calls': n - 1, 'call_limit': 32, 'calls_remaining_including_next': 33 - n}
        return {'model': ARM['model'], 'stream': True, 'max_tokens': 8192, 'tools': [{'function': {'name': 'exec'}}],
                'messages': [{'role': 'user', 'content': PREFIX + json.dumps(value)}]}
    def rejects(name, function):
        try: function()
        except AssertionError: passed.append(name)
        else: raise AssertionError('did not reject ' + name)
    for n in (1, 2, 32): post_contract(packet(n), n); passed.append('agent-effective32-call-' + str(n))
    wrong = packet(1); wrong['messages'][0]['content'] = PREFIX + json.dumps({'scope': 'current_run', 'reserved_calls': 0, 'call_limit': 40, 'calls_remaining_including_next': 40})
    rejects('old-request40-budget-rejected', lambda: post_contract(wrong, 1))
    override = packet(1); override['thinking'] = {'type': 'disabled'}
    rejects('default-agent-thinking-override-rejected', lambda: post_contract(override, 1))
    rejects('call33-rejected', lambda: post_contract(packet(33), 33))
    summary = {'model': ARM['model'], 'stream': True, 'max_tokens': 4096, 'thinking': {'type': 'disabled'}, 'response_format': {'type': 'json_object'}}
    h.require(post_contract(summary, 2)['phase'] == 'summary', 'existing summary contract rejected'); passed.append('summary-no-budget-existing-policy')
    rejects('summary-cannot-be-first', lambda: post_contract(summary, 1))
    terminal = [{'type': 'agent_end', 'status': 'completed', 'detail': '', 'code': 0}]
    h.require(h.evaluation_admission(terminal, True)['status'] == 'ELIGIBLE', 'approved helper admission changed'); passed.append('approved-settlement-helper-reused')
    with tempfile.TemporaryDirectory(prefix='s3-default-post-selftest-') as temporary:
        folder = Path(temporary); calls = {'posts': 0}; token = 'controlled-default-fixture-key'; row = {'id': ARM['model']}
        class Upstream(http.server.BaseHTTPRequestHandler):
            def log_message(self, *unused): pass
            def do_GET(self):
                body = json.dumps({'data': [row]}).encode(); self.send_response(200); self.end_headers(); self.wfile.write(body)
            def do_POST(self):
                calls['posts'] += 1; self.rfile.read(int(self.headers['Content-Length']))
                self.send_response(200); self.end_headers(); self.wfile.write(b'data: [DONE]\n\n')
        server = http.server.ThreadingHTTPServer(('127.0.0.1', 0), Upstream)
        thread = threading.Thread(target=server.serve_forever, daemon=True); thread.start()
        relay = relay_for('http://127.0.0.1:%d/v1' % server.server_port, token, folder, row)
        opener = urllib.request.build_opener(urllib.request.ProxyHandler({})); headers = {'Authorization': 'Bearer ' + token}
        try:
            with opener.open(urllib.request.Request(relay.local_endpoint + '/models', headers=headers), timeout=5) as response: response.read()
            for candidate in (wrong, override, packet(1)):
                status = None
                try:
                    with opener.open(urllib.request.Request(relay.local_endpoint + '/chat/completions', data=json.dumps(candidate).encode(), headers=headers), timeout=5) as response:
                        response.read(); status = response.status
                except urllib.error.HTTPError as error: status = error.code; error.read()
                h.require(status == (200 if candidate is not wrong and candidate is not override else 502), 'controlled POST status differs')
                h.require(calls['posts'] == (1 if status == 200 else 0), 'bad POST reached upstream before admission')
            passed.append('bad-first-budget-and-thinking-blocked-before-upstream')
            h.require(len(relay.calls) == 1 and relay.max_calls == 32, 'effective relay32 admission differs'); passed.append('valid-first32-forwarded-once')
        finally: relay.close(); server.shutdown(); server.server_close(); thread.join(timeout=2)
    result = {'status': 'PASS', 'checks': len(passed), 'passed': passed, 'model_calls': 0, 'native_host_runs': 0,
        'old44_selftests_rerun': False, 'controlled_loopback_posts': 1, 'harness_sha256': h.sha(Path(__file__))}
    h.require(not args.selftest_record.exists(), 'retain existing selftest; choose a fresh record')
    h.write_json(args.selftest_record, result)
    return result


def provider_selftest(args):
    current(args); passed = []
    previous = h.BASE / 'coding-stream-thinking-selftest-v3.json'
    prior = h.load(previous)
    h.require(prior['status'] == 'PASS' and prior['checks'] == 11 and prior['model_calls'] == 0 and
              prior['harness_sha256'] == h.sha(Path(__file__)), 'current11 gate evidence missing')
    with tempfile.TemporaryDirectory(prefix='s3-provider-classification-') as temporary:
        folder = Path(temporary); raw_path = folder / 'call-01.response.native.bin'
        for name, status, receipt_status, raw, expected in (
            ('400-explicit-missing-reasoning', 400, 'received', b'{"error":"missing required reasoning_content field"}', 'PROTOCOL_REPLAY_INCOMPATIBILITY_UNVERIFIED'),
            ('400-unspecified', 400, 'received', b'{"error":"bad request"}', 'PROVIDER_OR_PROTOCOL_FAILURE_UNKNOWN'),
            ('503-service', 503, 'received', b'service unavailable', 'PROVIDER_OR_PROTOCOL_FAILURE_UNKNOWN'),
            ('200-partial-receipt', 200, 'partial', b'data: partial', 'PROVIDER_OR_PROTOCOL_FAILURE_UNKNOWN'),
            ('200-SSE-error', 200, 'received', b'data: {"error":{"code":"server_is_overloaded","type":"service_unavailable_error"}}\n\ndata: [DONE]\n', 'PROVIDER_OR_PROTOCOL_FAILURE_UNKNOWN'),
        ):
            raw_path.write_bytes(raw)
            value = provider_admission([{'http_status': status, 'status': receipt_status}], folder)
            h.require(value['status'] == 'NOT_RUN' and value['failures'][0]['classification'] == expected and
                not value['failures'][0]['cpp_algorithm_failure_inferred'] and raw_path.read_bytes() == raw,
                name + ' failed protocol gate/raw retention')
            passed.append(name + '-blocks-algorithm-evaluation')
        raw_path.write_bytes(b'data: {"choices":[{"delta":{"content":"an error is discussed here"}}]}\n\ndata: [DONE]\n')
        h.require(provider_admission([{'http_status': 200, 'status': 'received'}], folder)['status'] == 'ELIGIBLE',
                  'successful receipt incorrectly blocked')
        passed.append('200-model-error-prose-remains-eligible')
    result = {'status': 'PASS', 'checks': len(passed), 'passed': passed, 'scope': 'pure-provider-classification-only',
        'model_calls': 0, 'native_host_runs': 0, 'controlled_loopback_posts': 0, 'old44_selftests_rerun': False,
        'prior11_selftests_rerun': True, 'prior11_record_path': str(previous), 'prior11_record_sha256': h.sha(previous),
        'harness_sha256': h.sha(Path(__file__))}
    h.require(not args.selftest_record.exists(), 'retain existing selftest; choose a fresh record')
    h.write_json(args.selftest_record, result)
    return result


def main():
    parser = argparse.ArgumentParser(description=__doc__); mode = parser.add_mutually_exclusive_group(required=True)
    mode.add_argument('--prepare', action='store_true'); mode.add_argument('--selftest', action='store_true'); mode.add_argument('--live', action='store_true')
    mode.add_argument('--selftest-provider', action='store_true')
    parser.add_argument('--baseline', type=Path, default=BASELINE); parser.add_argument('--output', type=Path, default=OUTPUT)
    parser.add_argument('--selftest-record', type=Path, default=h.BASE / 'coding-stream-thinking-selftest-v1.json')
    args = parser.parse_args(); result = selftest(args) if args.selftest else provider_selftest(args) if args.selftest_provider else prepare(args) if args.prepare else live(args)
    print(json.dumps(result, ensure_ascii=False, indent=2))
    h.require(result['status'] in ('PASS', 'PREPARED'), 'single diagnostic failed/incomplete; preserve original attempt, never auto-replay')


if __name__ == '__main__': main()
