"""One-shot S3 coding trials and independent fresh CMake/CLI evaluation.

Both arms use the same fixed Host/model/policy. Only explicit source_symbols
registration differs. Tool use is observed separately from task correctness.
Private fixture data is used only after model settlement, never as a brief.
This controlled small project is not an OS sandbox or a general coding benchmark.
"""
import argparse
import json
import os
from pathlib import Path
import re
import runpy
import shutil
import subprocess
import sys
import tempfile
import time
import xml.etree.ElementTree as ET

from verify_analysis_live import ROUTE, new_tool_evidence, route_from
from verify_coding_live import digest, execution_feedback, inventory, require, scan_actual_token, write_json
from verify_reverse_holdout_live import checked_relay, select_catalog, catalog_caps
from verify_reverse_policy_live import wire_observation

BASE = Path('D:/WorkSpace/tmp/ara-core-capability-20261022')
STAGE = BASE / 'coding-stream-stage'
OUTPUT = BASE / 'coding-stream-live-v1'
BUILD_RECEIPT = BASE / 'analysis-stage/v6-checks.json'
HOST = BASE / 'analysis-stage/v6-business_host.exe'
HOST_HASH = 'ff527818486911969f3afacf0f71099c2911e433930a910b0e8a6a7ceb5c505f'
FIXTURE_REVIEW = BASE / 'coding-stream-independent-pre-v1.json'
CATALOG = BASE / 'reverse-policy-live-v1/A/catalog.response.native.bin'
ARMS = (
    {'arm': 'S3-base', 'model': 'deepseek-v4.1-flash', 'policy': 'thinking_type_disabled', 'output': 8192, 'symbols': False},
    {'arm': 'S3-symbols', 'model': 'deepseek-v4.1-flash', 'policy': 'thinking_type_disabled', 'output': 8192, 'symbols': True},
)
OBJECTIVE = ('Repair this stream CSV project according to README.md. Inspect the actual files, '
             'keep report and follow using the shared pump, and actually build and test the result. '
             'You may edit the fourteen original .cpp/.hpp files. Keep README.md, CMakeLists.txt, '
             'tests/check_cli.py and tests/cases.json unchanged. Put temporary tests, scripts and '
             'build outputs in this arm\'s scratch/build directories given below. '
             'Report the changes, actual validation results and remaining uncertainty.')


def load(path):
    return json.loads(path.read_text(encoding='utf-8-sig'))


def sha(path):
    return digest(path.read_bytes())


def core():
    return Path(__file__).resolve().parents[1]


def helpers():
    return {name: sha(Path(__file__).with_name(name)) for name in (
        'verify_stream_coding_live.py', 'verify_reverse_holdout_live.py', 'verify_reverse_policy_live.py',
        'verify_analysis_live.py', 'verify_coding_live.py')}


def fixture(stage):
    manifest = load(stage / 'manifest.json')
    require(sha(stage / 'manifest.json') == (stage / 'manifest.sha256').read_text(encoding='ascii').strip(), 'S3 manifest seal changed')
    require(manifest['hidden_cases'] == 48 and manifest['public_tests'] == 10 and
            manifest['public_source_header_files'] == 14, 'unexpected S3 inventory')
    for relative, value in manifest['artifact_pins'].items():
        require(sha(stage / relative) == value, 'S3 private author artifact changed: ' + relative)
    require(sha(stage / 'checks.json') == manifest['checks_sha256'], 'S3 author checks changed')
    require(sha(stage / 'prepare_s3.py') == manifest['generator_sha256'], 'S3 generator changed')
    require(sha(Path(manifest['private_oracle'])) == manifest['oracle_sha256'], 'S3 oracle changed')
    require(inventory(Path(manifest['public_template'])) == manifest['public_pins'], 'S3 public template changed')
    review = load(FIXTURE_REVIEW)
    require(review['manifest_sha256'] == sha(stage / 'manifest.json') and
            review['decision'].startswith('APPROVE_SCOPED_FIXTURE_PRE'), 'independent S3 fixture PRE required')
    for tool in manifest['tools'].values():
        require(sha(Path(tool['path'])) == tool['sha256'], 'S3 compiler/build tool changed')
    return manifest


def check_build(receipt_path, binary):
    receipt = load(receipt_path)
    require(receipt['inputs_unchanged'] and len(receipt['inputs']) == 38 and
            all(row['exit_code'] == 0 for row in receipt['results']), 'fixed v6 checks not all accepted')
    require(receipt['binary_sha256'] == HOST_HASH and sha(binary) == HOST_HASH, 'fixed v6 Host identity differs')
    for relative, value in receipt['inputs'].items():
        require(sha(core() / relative) == value, 'v6 delivered source changed: ' + relative)
    return receipt


def original_projection(work, initial):
    # Build/test scratch artifacts are retained but excluded from the 18-file
    # product snapshot. They never replace the independent fresh build.
    all_files = inventory(work)
    require(all(name in all_files for name in initial), 'an original source/contract file was deleted')
    editable = {name for name in initial if name.endswith(('.cpp', '.hpp'))}
    require(len(editable) == 14 and len(initial) == 18, 'original edit/contract inventory differs')
    protected = set(initial) - editable
    require(all(all_files[name] == initial[name] for name in protected), 'protected contract/public test changed')
    return {name: all_files[name] for name in initial}, {name: value for name, value in all_files.items() if name not in initial}


def command(argv, cwd, folder, name, timeout=120):
    started = time.monotonic(); code = None; out = err = b''
    timed_out = False
    try:
        process = subprocess.run(list(map(str, argv)), cwd=cwd, capture_output=True, timeout=timeout)
        code, out, err = process.returncode, process.stdout, process.stderr
    except subprocess.TimeoutExpired as error:
        out, err, timed_out = error.stdout or b'', error.stderr or b'', True
    (folder / (name + '.stdout.native.bin')).write_bytes(out)
    (folder / (name + '.stderr.native.bin')).write_bytes(err)
    record = {'argv': list(map(str, argv)), 'cwd': str(cwd), 'exit_code': code, 'timeout': timed_out,
        'effects_unknown': timed_out, 'seconds': round(time.monotonic() - started, 3),
        'stdout_sha256': digest(out), 'stderr_sha256': digest(err)}
    write_json(folder / (name + '.command.json'), record)
    return record


def receipt_projection(folder):
    paths = list(folder.glob('call-*.request.native.json')) + list(folder.glob('call-*.response.native.bin'))
    paths += [folder / name for name in ('native-session.json', 'catalog.response.native.bin',
              'host.stdout.native.jsonl', 'host.stderr.native.txt', 'request.json')]
    result = {path.relative_to(folder).as_posix(): {'sha256': sha(path), 'bytes': path.stat().st_size}
              for path in paths if path.is_file()}
    for name in ('native-session.journal', 'model-analysis'):
        directory = folder / name
        if directory.exists():
            result.update({name + '/' + relative: value for relative, value in inventory(directory).items()})
    return result


def sealed_intact(folder, seal, seal_hash):
    return (sha(folder / 'source-seal.json') == seal_hash and
        inventory(folder / 'sealed-source') == seal['source_pins'] and
        sha(folder / 'final-submission.txt') == seal['final_text_sha256'] and
        receipt_projection(folder) == seal['model_receipt_pins'])


def seal_sources(work, folder, initial, final_text):
    projection, extra = original_projection(work, initial)
    sealed = folder / 'sealed-source'; sealed.mkdir()
    for name in initial:
        target = sealed / name; target.parent.mkdir(parents=True, exist_ok=True)
        target.write_bytes((work / name).read_bytes())
    require(inventory(sealed) == projection, 'source changed while sealing')
    (folder / 'final-submission.txt').write_text(final_text, encoding='utf-8', newline='\n')
    record = {'source_pins': projection, 'final_text_sha256': sha(folder / 'final-submission.txt'),
        'model_receipt_pins': receipt_projection(folder),
        'ignored_extra_files': extra, 'protected_files_unchanged': True,
        'hidden_inputs_evaluated': False, 'scope': '14 editable originals and4 protected originals only'}
    write_json(folder / 'source-seal.json', record)
    return record, sha(folder / 'source-seal.json')


def fresh_evaluate(folder, manifest, seal, seal_hash):
    """Never calls the private generator/reference or trusts model binaries."""
    result = {'status': 'FAIL', 'public_status': 'NOT_RUN', 'hidden_status': 'NOT_RUN',
              'effects_unknown': False, 'source_seal_sha256': seal_hash}
    require(sealed_intact(folder, seal, seal_hash), 'sealed candidate/model receipts changed before fresh evaluation')
    evaluation = folder / 'independent-evaluation'; evaluation.mkdir()
    source, build = evaluation / 'source', evaluation / 'build'
    shutil.copytree(folder / 'sealed-source', source)
    tools = manifest['tools']
    configure = command([tools['cmake']['path'], '-S', source, '-B', build, '-G', 'Ninja',
        '-DCMAKE_BUILD_TYPE=Release', '-DCMAKE_CXX_COMPILER=' + tools['g++']['path'],
        '-DCMAKE_MAKE_PROGRAM=' + tools['ninja']['path'], '-DPython3_EXECUTABLE=' + sys.executable],
        evaluation, evaluation, 'configure')
    result['configure'] = configure
    if configure['exit_code'] == 0:
        compiled = command([tools['cmake']['path'], '--build', build, '--parallel', '4'], evaluation, evaluation, 'build')
        result['build'] = compiled
        if compiled['exit_code'] == 0:
            binary = build / 'csv_import.exe'
            require(binary.is_file(), 'fresh compiler did not produce expected CLI')
            binary_hash = sha(binary)
            result.update(binary_path=str(binary), binary_sha256=binary_hash,
                          compiled_source_pins=inventory(source))
            ctest = command([tools['ctest']['path'], '--test-dir', build, '--output-on-failure',
                             '--output-junit', evaluation / 'ctest.xml'], evaluation, evaluation, 'ctest')
            result['ctest'] = ctest
            try:
                cases = list(ET.parse(evaluation / 'ctest.xml').getroot().iter('testcase'))
                public_ok = len(cases) == 10 and all(c.find('failure') is None and c.find('skipped') is None for c in cases)
            except (ET.ParseError, OSError):
                cases, public_ok = [], False
            result['public_test_count'] = len(cases)
            result['public_status'] = 'PASS' if ctest['exit_code'] == 0 and public_ok else 'FAIL'
            # The native model process is already settled and the complete
            # candidate is sealed before this private oracle is loaded.
            require(sha(Path(manifest['private_oracle'])) == manifest['oracle_sha256'], 'private48 oracle changed')
            hidden = load(Path(manifest['private_oracle']))
            require(len(hidden) == 48, 'private48 inventory differs')
            driver = Path(manifest['public_template']) / 'tests/check_cli.py'
            require(sha(driver) == manifest['public_pins']['tests/check_cli.py']['sha256'], 'fixed evaluator driver changed')
            run_case = runpy.run_path(str(driver))['run_case']
            results = [run_case(binary, case, evaluation / ('hidden-' + case['id'])) for case in hidden]
            result['hidden_passed'] = sum(case['status'] == 'PASS' for case in results)
            result['hidden_total'] = len(results)
            result['hidden_status'] = 'PASS' if result['hidden_passed'] == 48 else 'FAIL'
            result['hidden_failed_ids'] = [case['id'] for case in results if case['status'] != 'PASS']
            result['fresh_binary_unchanged'] = sha(binary) == binary_hash
            require(result['fresh_binary_unchanged'], 'candidate CLI modified its compiled binary during evaluation')
    result['effects_unknown'] = any(value.get('effects_unknown') for value in result.values() if isinstance(value, dict))
    result['source_integrity_after_evaluation'] = sealed_intact(folder, seal, seal_hash) and inventory(source) == seal['source_pins']
    result['oracle_unchanged_after_evaluation'] = sha(Path(manifest['private_oracle'])) == manifest['oracle_sha256']
    result['status'] = ('PASS' if result['public_status'] == result['hidden_status'] == 'PASS' and
        result['source_integrity_after_evaluation'] and result['oracle_unchanged_after_evaluation'] and
        not result['effects_unknown'] else 'FAIL')
    write_json(evaluation / 'result.json', result)
    return result


def phases40(events, actual):
    require(0 < actual <= 40, 'shared40 call budget differs')
    found = {}
    for event in events:
        if event.get('type') != 'model_call': continue
        n, phase = event.get('call'), event.get('phase')
        require(type(n) is int and n not in found and phase in ('agent', 'summary'), 'invalid/duplicate native phase')
        found[n] = phase
    require(set(found) == set(range(1, actual + 1)) and 'agent' in found.values(), 'native phases do not match40 HTTP call IDs')
    return found


def tool_effect_settlement(events):
    """Classify only native event structure and known Core error envelopes.

    Command text and arbitrary stdout are never searched for timeout keywords.
    A settled nonzero compiler exit remains usable feedback. This cannot prove
    that all external effects of arbitrary programs are observable or isolated.
    """
    starts, ends, unknown, known = {}, {}, [], []
    effect_tools = {'exec', 'run_script', 'edit_file', 'write_file', 'download_file'}
    process_tools = {'exec', 'run_script'}
    for index, event in enumerate(events):
        kind, title, identity = event.get('type'), event.get('title'), event.get('step_id')
        if kind not in ('tool_start', 'tool_end'): continue
        if event.get('effects_unknown') is True:
            unknown.append({'step_id': identity, 'reason': 'native_event_effects_unknown'})
        if title in effect_tools:
            target = starts if kind == 'tool_start' else ends
            target.setdefault(identity, []).append((index, event))
        if kind != 'tool_end': continue
        output = event.get('output')
        if not isinstance(output, str): continue
        payload = output[len('tool error: '):] if output.startswith('tool error: ') else output
        if event.get('status') == 'failed':
            # file_edits.rs137 uses this typed prefix for a failed commit/readback.
            if payload.startswith('effects_unknown:'):
                unknown.append({'step_id': identity, 'reason': 'native_edit_commit_unknown'})
            # A structured tool receipt may state unknown effects explicitly.
            try: structured = json.loads(payload)
            except ValueError: structured = None
            if isinstance(structured, dict) and structured.get('effects_unknown') is True:
                unknown.append({'step_id': identity, 'reason': 'native_structured_effects_unknown'})
        if title not in process_tools or event.get('status') != 'failed': continue
        # rpi wraps Abort separately; its phase is not sufficient to prove that
        # no process started. Explicit before-start errors below remain known.
        if payload in ('operation aborted', 'Operation aborted'):
            unknown.append({'step_id': identity, 'reason': 'native_abort_phase_unobservable'}); continue
        head = payload.splitlines()[0] if payload else ''
        if title == 'run_script':
            head = re.sub(r'^run (?:python|python3|powershell|pwsh|cmd\.exe|sh) script \(no interpreter fallback\): ', '', head)
        if (re.match(r'^(?:command|script) exited with code -?[0-9]+$', head) or
                head.startswith(('command canceled before start', 'script canceled before start',
                                 'capture preparation failed; process not_started', 'start shell; process not_started'))):
            known.append({'step_id': identity, 'reason': 'native_nonzero_or_not_started'}); continue
        prefixes = ('command timed out; partial effects may remain', 'command canceled; partial effects may remain',
                    'process started but tree admission failed; partial effects may remain',
                    'process resume failed; partial effects may remain',
                    'output capture failed after process settlement (')
        cleanup = re.match(r'^process exit code (?:Some\(-?[0-9]+\)|None); stdout: .*; stderr: .*; partial effects may remain$', head)
        if head.startswith(prefixes) or cleanup:
            unknown.append({'step_id': identity, 'reason': 'native_process_partial_effects_error'})
    for identity in set(starts) | set(ends):
        begin, finish = starts.get(identity, []), ends.get(identity, [])
        if (not identity or len(begin) != 1 or len(finish) != 1 or
                begin[0][1].get('title') != finish[0][1].get('title') or begin[0][0] >= finish[0][0]):
            unknown.append({'step_id': identity, 'reason': 'unpaired_or_ambiguous_native_local_effect_lifecycle'})
        elif finish[0][1].get('status') not in ('completed', 'failed'):
            unknown.append({'step_id': identity, 'reason': 'native_local_effect_terminal_status_missing'})
    return {'status': 'UNKNOWN' if unknown else 'NO_EXPLICIT_UNKNOWN_OBSERVED', 'effects_unknown': bool(unknown),
        'unknown_receipts': unknown, 'known_nonzero_or_not_started': known,
        'limit': 'five local process/file lifecycles and known native error/status envelopes; other environment/external effects are not exhaustively observed; no OS isolation proof'}


def evaluation_admission(events, host_settled):
    settlement = tool_effect_settlement(events)
    terminal = [event for event in events if event.get('type') == 'agent_end']
    run_unknown = []
    # This Host always emits agent_end before its business result. Its absence
    # makes the lifecycle evidence incomplete even when the result was printed.
    if host_settled and len(terminal) != 1:
        run_unknown.append('native_agent_end_missing_or_duplicated')
    unknown_details = {
        'run deadline exceeded; partial effects may remain',
        'run canceled; partial effects may remain',
        'run canceled internally; partial effects may remain',
        'run did not settle after cancellation; effects may be unknown',
    }
    for end in terminal:
        if end.get('effects_unknown') is True or end.get('detail') in unknown_details:
            run_unknown.append('native_agent_end_explicit_partial_or_unknown')
    eligible = host_settled and not settlement['effects_unknown'] and not run_unknown
    return {'status': 'ELIGIBLE' if eligible else 'NOT_RUN', 'effects_unknown': not eligible,
            'tool_effect_settlement': settlement,
            'native_run_end': [{k: end.get(k) for k in ('status', 'detail', 'code')} for end in terminal],
            'native_run_unknown_reasons': run_unknown,
            'reason': None if eligible else 'Host incomplete or native tool effects explicitly unknown; no fresh evaluation'}


def process_feedback(events, folder):
    result = execution_feedback(events)
    requests = [load(path) for path in sorted(folder.glob('call-*.request.native.json'))]
    starts = {e.get('step_id'): e for e in events if e.get('type') == 'tool_start' and e.get('title') in ('exec', 'run_script')}
    receipts = []
    for end in events:
        start = starts.get(end.get('step_id'))
        if end.get('type') != 'tool_end' or not start or end.get('title') != start.get('title'): continue
        output = end.get('output')
        if not isinstance(output, str) or not output.strip(): continue
        detail = start.get('detail')
        try: args = json.loads(detail) if isinstance(detail, str) else detail
        except ValueError: args = None
        text = json.dumps(args, ensure_ascii=False) if isinstance(args, dict) else str(detail)
        kind = 'build_or_test' if re.search(r'cmake|ctest|g\+\+|ninja|check_cli|csv_import', text, re.I) else 'other_process'
        seen = []
        for n, request in enumerate(requests, 1):
            for message in request.get('messages', []):
                if message.get('role') != 'tool' or message.get('tool_call_id') != end['step_id']: continue
                content = message.get('content')
                observed = content if isinstance(content, str) else ''.join(
                    part.get('text', '') for part in content or [] if isinstance(part, dict))
                if output in observed: seen.append(n)
        receipts.append({'step_id': end['step_id'], 'tool': end['title'], 'status': end.get('status'),
            'output_sha256': digest(output.encode()), 'output_bytes': len(output.encode()),
            'classification': kind, 'subsequent_provider_calls_with_output': seen})
    result.update(all_matched_nonempty_process_receipts=receipts,
        build_or_test_feedback_observed=any(r['classification'] == 'build_or_test' for r in receipts))
    return result


def wire(folder, n, arm, phase, first):
    request = load(folder / f'call-{n:02d}.request.native.json')
    require(request.get('model') == arm['model'] and request.get('stream') is True, 'wire request model/stream differs')
    observed = wire_observation(folder, n, arm, '', phase, first, arm['model'])
    observed.update(response_model_discrepancy=bool(observed['response_models']) and observed['response_models'] != [arm['model']],
                    alias_identity='UNVERIFIED', backend_weight_identity='UNKNOWN')
    if phase == 'agent':
        tools = {item['function']['name']: item['function'] for item in request.get('tools', [])}
        require({'exec', 'run_script', 'read_file', 'edit_file'} <= set(tools), 'execute local tool set differs')
        require(('source_symbols' in tools) == arm['symbols'] and 'binary_inspect' not in tools, 'arm analysis registrations differ')
        observed['common_tool_schema_sha256'] = digest(json.dumps({k: v for k, v in tools.items() if k != 'source_symbols'}, sort_keys=True).encode())
    return observed


def prepare(args):
    stage, root = args.stage.resolve(), args.output.resolve()
    require(stage != root and stage not in root.parents and root not in stage.parents, 'private stage overlaps model root')
    manifest = fixture(stage); receipt = check_build(args.build_receipt, args.binary)
    tested = load(args.selftest_record)
    require(tested['status'] == 'PASS' and tested['harness_sha256'] == sha(Path(__file__)) and tested['model_calls'] == 0,
            'current controlled selftest required')
    endpoint, route_model = route_from(args.route_result)
    require(route_model == ARMS[0]['model'], 'original authorized route model differs')
    catalog = load(args.catalog); row = select_catalog(catalog, ARMS[0]['model'])
    require(catalog_caps(row)['max_tokens'] >= 8192, 'directory cannot realize fixed8192 budget')
    ctags = Path(args.ctags or shutil.which('ctags') or '').resolve()
    require(ctags.is_file(), 'existing ctags executable required')
    previous = None
    if root.exists():
        require(args.amend_from is not None, 'existing plan needs explicit zero-call PRE amendment')
        require(not list(root.rglob('STARTED')) and not list(root.rglob('native-session.json')) and
                not list(root.rglob('call-*.request.native.json')) and not list(root.rglob('result.json')),
                'cannot amend after any live marker/Session/request/result')
        previous = load(root / 'plan.json')
        require(sha(root / 'plan.json') == (root / 'plan.sha256').read_text().strip() and
                sha(root / 'plan.json') == sha(args.amend_from / 'plan.json') and
                sha(args.amend_from / 'verify_stream_coding_live.py') == previous['harness_helper_pins']['verify_stream_coding_live.py'],
                'prior failed PRE archive identity differs')
        version = previous['ctags']['version_receipt']
    else:
        root.mkdir(parents=True, exist_ok=False)
        version = command([ctags, '--version'], root, root, 'ctags-version', 20)
        require(version['exit_code'] == 0, 'ctags version failed')
    for arm in ARMS:
        folder = root / arm['arm']
        if previous is None:
            folder.mkdir(); shutil.copytree(manifest['public_template'], folder / 'project')
            (folder / 'scratch').mkdir(); (folder / 'build').mkdir()
        require(inventory(folder / 'project') == manifest['public_pins'], 'initial public18 copy differs')
    plan = {'schema': 'ara-stream-coding-live-v1', 'status': 'PREPARED', 'arms': list(ARMS), 'objective': OBJECTIVE,
        'max_calls': 40, 'host_seconds': 900, 'outer_host_seconds': 905, 'max_runs': 2,
        'stage_path': str(stage), 'stage_manifest_sha256': sha(stage / 'manifest.json'),
        'fixture_review_path': str(FIXTURE_REVIEW), 'fixture_review_sha256': sha(FIXTURE_REVIEW),
        'public_pins': manifest['public_pins'], 'oracle_sha256': manifest['oracle_sha256'],
        'private_author_pins': manifest['artifact_pins'], 'tools': manifest['tools'],
        'host_path': str(args.binary.resolve()), 'host_sha256': sha(args.binary),
        'build_receipt_path': str(args.build_receipt.resolve()), 'build_receipt_sha256': sha(args.build_receipt),
        'runtime_source_pins': receipt['inputs'], 'harness_helper_pins': helpers(),
        'ctags': {'path': str(ctags), 'sha256': sha(ctags), 'version_receipt': version},
        'route_path': str(args.route_result.resolve()), 'route_record_sha256': sha(args.route_result), 'endpoint_sha256': digest(endpoint.encode()),
        'catalog_snapshot_path': str(args.catalog.resolve()), 'catalog_snapshot_sha256': sha(args.catalog), 'selected_catalog_row': row,
        'selftest_record_path': str(args.selftest_record.resolve()), 'selftest_record_sha256': sha(args.selftest_record),
        'actual_seed_core_lines': 428, 'core_source_header_files': 14, 'protected_files':
            ['README.md', 'CMakeLists.txt', 'tests/check_cli.py', 'tests/cases.json'],
        'limits': ['controlled small authored14-file project; no large-repository/general capability claim',
            'fresh CMake/build/public10/private48 establish bounded correctness; model self-report is not the oracle',
            'navigation and process feedback are separate observations, not mandatory source_symbols-use gates',
            'both arms same model/policy/Host/budget, only explicit ctags registration differs',
            'temporary build/tests may use per-arm scratch/build; extra files never replace original18 in fresh evaluator',
            'response alias echoes are retained UNVERIFIED; actual backend weights are UNKNOWN',
            'unknown model/tool effects block evaluation; settled failed Runs may retain partial correctness evidence without task acceptance',
            'relay has existing45s socket/50s response checks; cleanup/fresh evaluator costs are measured separately',
            'private evidence not exposed to model is not an OS unreadability/sandbox claim']}
    if previous is not None:
        mutable = {'harness_helper_pins', 'selftest_record_path', 'selftest_record_sha256', 'amendments'}
        require({k: v for k, v in previous.items() if k not in mutable} ==
                {k: v for k, v in plan.items() if k not in mutable}, 'PRE fix changed frozen trial conditions')
        plan['amendments'] = previous.get('amendments', []) + [{
            'reason': 'PRE found incomplete native unknown-effects admission; gate fresh evaluation on native agent_end and paired local process/file terminal receipts without changing trial conditions',
            'prior_plan_sha256': sha(root / 'plan.json'), 'prior_harness_sha256': previous['harness_helper_pins']['verify_stream_coding_live.py'],
            'new_harness_sha256': sha(Path(__file__)), 'archive_path': str(args.amend_from.resolve()), 'model_calls_before_amendment': 0}]
    write_json(root / 'plan.json', plan); (root / 'plan.sha256').write_text(sha(root / 'plan.json') + '\n', encoding='ascii')
    return {'status': 'PREPARED', 'plan_sha256': sha(root / 'plan.json'), 'model_calls': 0, 'arms': [a['arm'] for a in ARMS]}


def frozen(args):
    root = args.output.resolve(); plan = load(root / 'plan.json')
    require(sha(root / 'plan.json') == (root / 'plan.sha256').read_text().strip(), 'S3 trial plan changed')
    require(plan['arms'] == list(ARMS) and plan['objective'] == OBJECTIVE and plan['max_calls'] == 40 and
            plan['host_seconds'] == 900, 'fixed S3 conditions changed')
    manifest = fixture(Path(plan['stage_path']))
    require(sha(Path(plan['stage_path']) / 'manifest.json') == plan['stage_manifest_sha256'], 'frozen S3 fixture differs')
    require(helpers() == plan['harness_helper_pins'], 'harness/helper changed')
    require(sha(FIXTURE_REVIEW) == plan['fixture_review_sha256'], 'fixture PRE changed')
    require(sha(Path(plan['build_receipt_path'])) == plan['build_receipt_sha256'], 'v6 build receipt changed')
    check_build(Path(plan['build_receipt_path']), Path(plan['host_path']))
    require(sha(Path(plan['ctags']['path'])) == plan['ctags']['sha256'], 'ctags backend changed')
    require(sha(Path(plan['selftest_record_path'])) == plan['selftest_record_sha256'], 'selftest record changed')
    endpoint, model = route_from(Path(plan['route_path']))
    require(sha(Path(plan['route_path'])) == plan['route_record_sha256'] and digest(endpoint.encode()) == plan['endpoint_sha256'] and
            model == ARMS[0]['model'], 'fixed route changed')
    require(sha(Path(plan['catalog_snapshot_path'])) == plan['catalog_snapshot_sha256'], 'catalog snapshot changed')
    return root, plan, manifest, endpoint


def live(args):
    root, plan, manifest, endpoint = frozen(args)
    arm = next(a for a in ARMS if a['arm'] == args.arm)
    preceding = ARMS[:[a['arm'] for a in ARMS].index(args.arm)]
    require(all((root / a['arm'] / 'result.json').is_file() for a in preceding), 'preceding arm must settle first, including failure')
    folder = root / args.arm; work = folder / 'project'
    require(inventory(work) == plan['public_pins'] and not (folder / 'native-session.json').exists(), 'fresh original18 and new Session required')
    token = os.environ.get('CAS_API_KEY', ''); require(token, 'CAS_API_KEY required')
    (folder / 'STARTED').open('x').close(); started = time.monotonic(); relay = None
    result = {'schema': plan['schema'], 'arm': arm['arm'], 'condition': arm, 'status': 'FAIL',
        'plan_sha256': sha(root / 'plan.json'), 'oracle_status': 'NOT_RUN', 'task_success': False,
        'effects_unknown': True, 'host_sha256': plan['host_sha256'], 'independent_task_review': 'PENDING'}
    try:
        relay = checked_relay(endpoint, token, arm, folder, plan['selected_catalog_row'])
        relay.max_calls = 40  # Existing Relay reads this field for every POST, before forwarding.
        objective = OBJECTIVE + '\nScratch directory: ' + str(folder / 'scratch') + '\nBuild directory: ' + str(folder / 'build')
        request = {'id': 'stream-coding-' + arm['arm'], 'objective': objective, 'cwd': str(work),
            'session_path': str(folder / 'native-session.json'), 'environment_state': str(folder / 'environment-state'),
            'endpoint': relay.local_endpoint, 'model': arm['model'], 'token_env': 'CAS_API_KEY', 'mode': 'execute',
            'enable_local_tools': True, 'max_calls': 40, 'max_seconds': 900, 'max_output': 8192,
            'generation_policy': 'thinking_type_disabled'}
        if arm['symbols']:
            artifacts = folder / 'model-analysis'; artifacts.mkdir()
            request['analysis_config'] = {'artifact_root': str(artifacts), 'ctags': plan['ctags']['path']}
        write_json(folder / 'request.json', request)
        code = None; stdout = stderr = b''
        try:
            process = subprocess.run([plan['host_path'], str(folder / 'request.json')], cwd=work,
                                     capture_output=True, timeout=905)
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
        settled = code is not None and final is not None
        admission = evaluation_admission(events, settled)
        result.update(host_exit=code, native_result_code=final.get('code') if final else None,
            effects_unknown=admission['effects_unknown'], host_wall_seconds=round(time.monotonic() - started, 3),
            evaluation_admission=admission)
        result['process_feedback'] = process_feedback(events, folder)
        adopted = any(e.get('type') == 'tool_start' and e.get('title') == 'source_symbols' for e in events)
        navigation = new_tool_evidence('S1', events, final.get('text', '') if final else '', work, None)
        result['source_navigation'] = {'status': 'NOT_OBSERVED' if not adopted else 'OBSERVED_VERIFIED' if navigation['verified'] else 'PARTIAL_OR_FAILED',
                                       'evidence': navigation, 'required_for_correctness': False}
        require(relay.catalog_gate['accepted'] and not relay.catalog_gate['blocked_posts'], 'actual native directory gate rejected admission')
        row = select_catalog(load(folder / 'catalog.response.native.bin'), arm['model'])
        caps = catalog_caps(row)
        declared = [e for e in events if e.get('type') == 'model_capabilities']
        require(len(declared) == 1 and declared[0].get('declared') == caps and declared[0].get('model') == arm['model'], 'native catalog capacity differs')
        result['model_capabilities'] = declared
        policies = [e for e in events if e.get('type') == 'generation_policy']
        result['generation_policy_events'] = policies
        require(len(policies) == 1 and all(policies[0].get(k) == v for k, v in {
            'model': arm['model'], 'protocol': 'openai-completions', 'policy': 'thinking_type_disabled',
            'requested_output_tokens': 8192, 'effective_output_tokens': 8192,
            'provider_effect_verified': False}.items()), 'actual Host generation policy admission differs')
        phases = phases40(events, len(relay.calls)); first = min(n for n, phase in phases.items() if phase == 'agent')
        observed = [wire(folder, n, arm, phases[n], first) for n in range(1, len(relay.calls) + 1)]
        result['wire_observations'] = observed
        common = {r['common_tool_schema_sha256'] for r in observed if r['phase'] == 'agent'}
        require(len(common) == 1, 'common tool schema changed within arm'); result['common_tool_schema_sha256'] = next(iter(common))
        if preceding:
            prior = load(root / preceding[-1]['arm'] / 'result.json')
            if prior.get('common_tool_schema_sha256'):
                require(prior['common_tool_schema_sha256'] == result['common_tool_schema_sha256'], 'common tool schema differs between arms')
        require(not relay.errors and all(c.get('status') == 'received' and c.get('http_status') == 200 for c in relay.calls),
                'provider/relay failed; preserve original failure without automatic replay')
        if admission['status'] == 'ELIGIBLE':
            seal, seal_hash = seal_sources(work, folder, plan['public_pins'], final.get('text', ''))
            result['source_seal_sha256'] = seal_hash
            evaluation = fresh_evaluate(folder, manifest, seal, seal_hash)
            result['fresh_evaluation'] = evaluation
            result['oracle_status'] = evaluation['status']
            result['effects_unknown'] |= evaluation['effects_unknown']
            result['task_success'] = code == 0 and final.get('code') == 0 and evaluation['status'] == 'PASS'
            after, _ = original_projection(work, plan['public_pins'])
            require(after == seal['source_pins'], 'model source changed after submission seal')
            result['status'] = 'PASS' if result['task_success'] and not result['effects_unknown'] else 'FAIL'
        else:
            result['evaluation_not_run_reason'] = admission['reason']
    except Exception as error:
        result['error'] = (type(error).__name__ + ': ' + str(error)).replace(token, '[REDACTED]')
    finally:
        if relay is not None:
            if relay.thread.is_alive(): relay.close()
            write_json(folder / 'provider-receipts.json', {'calls': relay.calls, 'catalogs': relay.catalogs,
                'catalog_gate': relay.catalog_gate, 'errors': relay.errors}, token)
        try:
            frozen(args); result['all_frozen_inputs_unchanged'] = True
        except Exception as error:
            result['all_frozen_inputs_unchanged'] = False; result['integrity_error'] = str(error).replace(token, '[REDACTED]')
        result['total_wall_seconds_with_fresh_evaluation'] = round(time.monotonic() - started, 3)
        result['secret_scan'] = scan_actual_token(folder, token); result['raw_evidence_private'] = True
        if not result['all_frozen_inputs_unchanged'] or result['secret_scan']['status'] != 'CLEAN': result['status'] = 'FAIL'
        write_json(folder / 'result.json', result, token)
    return {k: result.get(k) for k in ('arm', 'status', 'oracle_status', 'task_success', 'effects_unknown',
                                     'total_wall_seconds_with_fresh_evaluation')}


def selftest(args):
    manifest = fixture(args.stage.resolve()); check_build(args.build_receipt, args.binary)
    root = args.selftest_root.resolve(); root.mkdir(parents=True, exist_ok=False)
    passed = []
    def rejects(name, operation):
        try: operation()
        except AssertionError: passed.append(name)
        else: raise AssertionError('did not reject: ' + name)
    events = [{'type': 'model_call', 'call': n, 'phase': 'summary' if n % 3 == 0 else 'agent'} for n in range(1, 41)]
    require(len(phases40(events, 40)) == 40, 'shared40 boundary failed'); passed.append('shared40-agent-summary-boundary')
    rejects('shared41-rejected', lambda: phases40(events, 41))
    rejects('phase-duplicate-rejected', lambda: phases40(events + [events[0]], 40))
    def process_events(output, status='failed', title='exec', extra=None):
        return [{'type': 'tool_start', 'title': title, 'step_id': 'process', 'detail': '{"cmd":"arbitrary"}'},
                {'type': 'tool_end', 'title': title, 'step_id': 'process', 'status': status, 'output': output, **(extra or {})},
                {'type': 'agent_end', 'status': 'completed', 'detail': '', 'code': 0}]
    for name, data in (
        ('native-timeout', process_events('tool error: command timed out; partial effects may remain')),
        ('native-cancel', process_events('tool error: command canceled; partial effects may remain')),
        ('native-script-timeout', process_events('tool error: run python script (no interpreter fallback): command timed out; partial effects may remain', title='run_script')),
        ('native-tree-admission', process_events('tool error: process started but tree admission failed; partial effects may remain: controlled')),
        ('native-resume', process_events('tool error: process resume failed; partial effects may remain: controlled')),
        ('native-cleanup', process_events('tool error: process exit code Some(0); stdout: Some(cleanup); stderr: None; partial effects may remain')),
        ('native-abort', process_events('operation aborted', title='run_script')),
        ('explicit-unknown-field', process_events('settled text', status='completed', extra={'effects_unknown': True})),
        ('explicit-unknown-receipt', process_events('{"effects_unknown":true}')),
        ('native-edit-unknown', process_events('tool error: effects_unknown: edit commit or readback failed (controlled)', title='edit_file')),
        ('unpaired-start', process_events('unused')[:1]), ('unpaired-end', process_events('unused')[1:]),
    ):
        value = evaluation_admission(data, True)
        require(value['status'] == 'NOT_RUN' and value['effects_unknown'], name + ' admitted fresh evaluation')
        passed.append(name + '-blocks-fresh-evaluation')
    for name, data in (
        ('nonzero-compile', process_events('tool error: command exited with code 1\ncompiler test: command timed out; partial effects may remain')),
        ('nonzero-test', process_events('tool error: script exited with code 7\nstdout:\nunit timeout regression failed', title='run_script')),
        ('ordinary-stdout-timeout-word', process_events('command timed out; partial effects may remain', status='completed')),
        ('not-started-cancel', process_events('tool error: script canceled before start', title='run_script')),
        ('not-started-shell', process_events('tool error: start shell; process not_started: controlled')),
    ):
        require(evaluation_admission(data, True)['status'] == 'ELIGIBLE', name + ' incorrectly blocked settled feedback')
        passed.append(name + '-remains-eligible')
    for detail in (
        'run deadline exceeded; partial effects may remain',
        'run canceled; partial effects may remain',
        'run canceled internally; partial effects may remain',
        'run did not settle after cancellation; effects may be unknown',
    ):
        value = evaluation_admission([{'type': 'agent_end', 'status': 'failed', 'detail': detail, 'code': -1}], True)
        require(value['status'] == 'NOT_RUN' and value['effects_unknown'], 'native run terminal partial error admitted evaluation')
        passed.append('agent-end-' + detail.split(';')[0].replace(' ', '-') + '-blocks-evaluation')
    for title in ('edit_file', 'write_file', 'download_file'):
        data = [{'type': 'tool_start', 'title': title, 'step_id': title},
                {'type': 'agent_end', 'status': 'completed', 'detail': '', 'code': 0}]
        require(evaluation_admission(data, True)['status'] == 'NOT_RUN', 'unpaired mutating file tool escaped')
        passed.append('unpaired-' + title + '-blocks-evaluation')
    require(evaluation_admission(process_events('ordinary', status='completed')[:-1], True)['status'] == 'NOT_RUN',
            'missing native agent_end admitted evaluation')
    passed.append('missing-agent-end-with-business-final-blocks-evaluation')
    safe = [{'type': 'tool_start', 'title': 'read_file', 'step_id': 'readonly'},
            {'type': 'agent_end', 'status': 'completed', 'detail': '', 'code': 0}]
    require(evaluation_admission(safe, True)['status'] == 'ELIGIBLE', 'readonly missing end incorrectly generalized')
    passed.append('readonly-missing-end-not-generalized-to-unknown')
    ordinary = [{'type': 'agent_end', 'status': 'failed', 'detail': 'assistant stopped with length', 'code': -1}]
    require(evaluation_admission(ordinary, True)['status'] == 'ELIGIBLE', 'settled length failure incorrectly classified unknown')
    passed.append('settled-length-failure-not-unknown-task-success-still-false')
    work = root / 'project'; shutil.copytree(manifest['public_template'], work)
    original_projection(work, manifest['public_pins']); passed.append('original18-projection')
    (work / 'build').mkdir(); (work / 'build/temp.log').write_text('controlled temporary output')
    original_projection(work, manifest['public_pins']); passed.append('temporary-build-artifact-not-task-failure')
    original = (work / 'README.md').read_bytes(); (work / 'README.md').write_bytes(b'altered contract')
    rejects('protected-contract-mutation', lambda: original_projection(work, manifest['public_pins']))
    (work / 'README.md').write_bytes(original)
    receipt_folder = root / 'seal-receipt-test'; receipt_folder.mkdir()
    write_json(receipt_folder / 'call-01.request.native.json', {'model': 'controlled'})
    receipt_seal, receipt_hash = seal_sources(work, receipt_folder, manifest['public_pins'], 'controlled receipt seal')
    require(sealed_intact(receipt_folder, receipt_seal, receipt_hash), 'fresh receipt seal invalid')
    write_json(receipt_folder / 'call-01.request.native.json', {'model': 'changed'})
    require(not sealed_intact(receipt_folder, receipt_seal, receipt_hash), 'model request mutation escaped submission seal')
    passed.append('native-model-receipt-mutation-invalidates-seal')
    for arm in ARMS:
        for phase in ('agent', 'summary'):
            folder = root / (arm['arm'] + '-' + phase); folder.mkdir()
            request = {'model': arm['model'], 'stream': True, 'max_tokens': 8192 if phase == 'agent' else 4096,
                       'thinking': {'type': 'disabled'}}
            if phase == 'agent':
                names = ['exec', 'run_script', 'read_file', 'edit_file'] + (['source_symbols'] if arm['symbols'] else [])
                request['tools'] = [{'function': {'name': name}} for name in names]
            else: request['response_format'] = {'type': 'json_object'}
            write_json(folder / 'call-01.request.native.json', request)
            response = {'model': 'controlled-alias', 'choices': [{'delta': {'content': 'fixture'}, 'finish_reason': 'stop'}]}
            (folder / 'call-01.response.native.bin').write_bytes(('data: ' + json.dumps(response) + '\n\ndata: [DONE]\n').encode())
            observed = wire(folder, 1, arm, phase, 1)
            require(observed['alias_identity'] == 'UNVERIFIED' and observed['response_model_discrepancy'], 'alias incorrectly normalized/rejected')
            passed.append(arm['arm'] + '-' + phase + '-wire-alias')
            if phase == 'agent':
                request['tools'].append({'function': {'name': 'binary_inspect'}}); write_json(folder / 'call-01.request.native.json', request)
                rejects(arm['arm'] + '-unplanned-binary-tool', lambda f=folder, a=arm: wire(f, 1, a, 'agent', 1))
    feedback_folder = root / 'feedback'; feedback_folder.mkdir()
    write_json(feedback_folder / 'call-01.request.native.json', {'messages': [{'role': 'tool', 'tool_call_id': 'build', 'content': 'compiled12 targets'}]})
    feedback = process_feedback([{'type': 'tool_start', 'step_id': 'build', 'title': 'exec', 'detail': '{"cmd":"cmake --build build"}'},
        {'type': 'tool_end', 'step_id': 'build', 'title': 'exec', 'status': 'completed', 'output': 'compiled12 targets'}], feedback_folder)
    require(feedback['build_or_test_feedback_observed'] and feedback['all_matched_nonempty_process_receipts'][0]['subsequent_provider_calls_with_output'] == [1], 'native feedback-nextrequest chain failed')
    passed.append('matched-build-feedback-returned-to-model')
    lock = root / 'STARTED'; lock.open('x').close()
    try: lock.open('x').close()
    except FileExistsError: passed.append('STARTED-exclusive-replay-rejected')
    else: raise AssertionError('replay not rejected')
    # Controlled private reference is used only in this offline selftest. It is
    # never copied to prepare/live projects or sent through a model request.
    for name, expected in (('reference', 'PASS'), ('seed', 'FAIL')):
        folder = root / ('fresh-' + name); folder.mkdir()
        source = root / ('source-' + name)
        shutil.copytree(args.stage / ('author/reference' if name == 'reference' else 'public-template'), source)
        seal, seal_hash = seal_sources(source, folder, manifest['public_pins'], 'controlled offline ' + name)
        evaluated = fresh_evaluate(folder, manifest, seal, seal_hash)
        require(evaluated['status'] == expected, 'fresh evaluator rejected/accepted wrong controlled source')
        passed.append('fresh-' + name + '-10ctest-48cli-' + expected)
    result = {'status': 'PASS', 'checks': len(passed), 'passed': passed, 'model_calls': 0,
        'external_catalog_calls': 0, 'native_host_runs': 0, 'cargo_invocations': 0,
        'controlled_fresh_cpp_builds': 2, 'harness_sha256': sha(Path(__file__)), 'evidence_root': str(root)}
    write_json(root / 'result.json', result)
    return result


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    mode = parser.add_mutually_exclusive_group(required=True)
    mode.add_argument('--prepare', action='store_true'); mode.add_argument('--live', action='store_true'); mode.add_argument('--selftest', action='store_true')
    parser.add_argument('--output', type=Path, default=OUTPUT); parser.add_argument('--stage', type=Path, default=STAGE)
    parser.add_argument('--binary', type=Path, default=HOST); parser.add_argument('--build-receipt', type=Path, default=BUILD_RECEIPT)
    parser.add_argument('--route-result', type=Path, default=ROUTE); parser.add_argument('--catalog', type=Path, default=CATALOG)
    parser.add_argument('--ctags'); parser.add_argument('--arm', choices=[a['arm'] for a in ARMS])
    parser.add_argument('--selftest-root', type=Path, default=BASE / 'coding-stream-harness-selftest-v1')
    parser.add_argument('--selftest-record', type=Path, default=BASE / 'coding-stream-harness-selftest-v1/result.json')
    parser.add_argument('--amend-from', type=Path, help='explicit failed PRE archive; only while both live arms remain unstarted')
    args = parser.parse_args(); require(not args.live or args.arm, '--live requires exactly one fixed arm')
    result = selftest(args) if args.selftest else prepare(args) if args.prepare else live(args)
    print(json.dumps(result, ensure_ascii=False, indent=2))
    require(result['status'] in ('PASS', 'PREPARED'), 'S3 trial failed/incomplete; retain all receipts and never replay arm')


if __name__ == '__main__': main()
