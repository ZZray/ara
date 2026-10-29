"""Isolated product-Workspace consumer. No queue, production DB or release tool.

Usage: PRODUCT_PYTHON ai_codereview_host.py CONFIG.json
The caller provides an owned historical source fixture, a fresh run_root, the
shared runtime binary and one fixed model endpoint. Grant stays in environment.
"""
import asyncio
import hashlib
import json
import os
from pathlib import Path
import re
import sys
import time
from types import SimpleNamespace

sys.path.insert(0, str(Path(__file__).resolve().parents[1] / 'clients/python'))
from ara_agent_lite import run_task
sys.path.insert(0, str(Path(__file__).resolve().parent))
from ai_codereview_acceptance import CASE_IDS, VERSION as ACCEPTANCE_VERSION


def save_new(path, value):
    with Path(path).open('x', encoding='utf-8') as stream:
        json.dump(value, stream, ensure_ascii=False, indent=2)
        stream.write('\n')
        stream.flush()
        os.fsync(stream.fileno())


def digest(path):
    return hashlib.sha256(Path(path).read_bytes()).hexdigest()


def limited_json(path):
    with Path(path).open('rb') as stream:
        body = stream.read(16385)
    if len(body) > 16384:
        raise ValueError('semantic report byte limit exceeded')
    value = json.loads(body)
    if not isinstance(value, dict):
        raise ValueError('semantic report must be an object')
    return value, hashlib.sha256(body).hexdigest()


def semantic_checks(workspace, evidence, checker_sha, source_sha, deadline_unix_ms, cancelled):
    """Run fixed Host checks; model output and stdout do not decide acceptance."""
    from app.services.agent_runtime_service import bounded_process
    checker = Path(__file__).with_name('ai_codereview_acceptance.py')
    source = workspace.path / 'app/services/git_service.py'
    result = {'status': 'UNKNOWN', 'process': None, 'report': None}
    try:
        if digest(checker) != checker_sha or digest(source) != source_sha or evidence.exists():
            raise ValueError('semantic launch source or evidence changed')
        timeout = min(30.0, (deadline_unix_ms - time.time() * 1000) / 1000)
        if timeout <= 0 or cancelled.is_set():
            raise RuntimeError('semantic launch canceled or deadline elapsed')
        env = {key: value for key, value in os.environ.items() if key.upper() in
               ('PATH', 'SYSTEMROOT', 'WINDIR', 'TEMP', 'TMP', 'HOME', 'USERPROFILE', 'LANG')}
        env.update(APP_DB_PATH=str(evidence / 'test.db'), APP_DATABASE_URL='',
                   APP_REPO_ROOT=str(evidence / 'repos'), APP_CRASH_ROOT=str(evidence / 'crashes'),
                   APP_AUTH_ENABLED='false', PYTHONDONTWRITEBYTECODE='1', PYTHONUTF8='1')
        argv = [sys.executable, '-X', 'utf8', '-B', str(checker), '--workspace', str(workspace.path),
                '--evidence', str(evidence), '--source-sha256', source_sha]
        result['timeout_seconds'] = timeout
        result['process'] = bounded_process(argv, workspace.path, env, timeout=timeout,
                                             cancelled=cancelled.is_set)
        started, started_sha = limited_json(evidence / 'started.json')
        report, report_sha = limited_json(evidence / 'result.json')
        result.update(started=started, report=report,
                      started_sha256=started_sha, report_sha256=report_sha)
        for record in (started, report):
            if type(record.get('version')) is not int or record['version'] != ACCEPTANCE_VERSION:
                raise ValueError('semantic report version differs')
            if type(record.get('worker_pid')) is not int or record['worker_pid'] <= 0:
                raise ValueError('semantic worker identity missing')
            if record.get('checker_sha256') != checker_sha or Path(record['workspace']).resolve() != workspace.path.resolve():
                raise ValueError('semantic checker or Workspace identity differs')
            state = record['source']
            if Path(state['path']).resolve() != source.resolve() or any(
                    state.get(key) != source_sha for key in ('expected_sha256', 'before_sha256')):
                raise ValueError('semantic report source differs')
        if started['worker_pid'] != report['worker_pid'] or report['source'].get('after_sha256') != source_sha:
            raise ValueError('semantic process or final source differs')
        if digest(checker) != checker_sha or digest(source) != source_sha or cancelled.is_set() or time.time() * 1000 >= deadline_unix_ms:
            raise ValueError('semantic sources changed or verification canceled')
        cases = report.get('cases')
        if not isinstance(cases, list) or len(cases) != len(CASE_IDS):
            raise ValueError('semantic cases incomplete')
        ids = [case.get('case_id') for case in cases]
        if len(set(ids)) != len(CASE_IDS) or set(ids) != set(CASE_IDS):
            raise ValueError('semantic case identities differ')
        if any(case.get('status') not in ('PASS', 'FAIL') for case in cases):
            raise ValueError('semantic case did not settle')
        passed = all(case['status'] == 'PASS' for case in cases)
        expected = ('PASS', 0) if passed else ('FAIL', 1)
        code = result['process'].get('code')
        if report.get('complete') is not True or type(code) is not int or (report.get('status'), code) != expected:
            raise ValueError('semantic process and report disagree')
        result['status'] = expected[0]
        return result
    except Exception as error:
        # Model-written code may cause arbitrary exception strings. Preserve
        # fixed categories here; original bounded files/output remain evidence.
        result['error_category'] = type(error).__name__
        return result


async def main(configuration):
    root = Path(configuration['run_root'])
    if not root.is_absolute() or root.exists() or not root.parent.is_dir():
        raise ValueError('run_root must be a fresh absolute directory')
    root.mkdir()
    private = root / 'host'
    private.mkdir()
    state = root / 'state'
    state.mkdir()
    # Importing app.config may create directories; explicitly isolate all roots.
    os.environ.update(APP_DB_PATH=str(private / 'control.db'), APP_DATABASE_URL='',
                      APP_REPO_ROOT=str(private / 'repos'), APP_CRASH_ROOT=str(private / 'crashes'),
                      APP_AUTH_ENABLED='false', PYTHONDONTWRITEBYTECODE='1')
    sys.path.insert(0, configuration['product_root'])
    from app.services.agent_workspace import Workspace
    from app.services.agent_runtime_service import tool_schemas

    workspace = Workspace(Path(configuration['source_fixture']), private / 'work',
                          ['app/services/git_service.py'], writable=True)
    # Git checkout filters can change LF to CRLF on Windows. Snapshot the actual
    # isolated test bytes before the Agent starts, then require those exact bytes
    # to remain unchanged through every Run.
    await asyncio.to_thread(workspace.prepare_readonly)
    original_test_hash = digest(workspace.path / 'tests/test_git_diff_chunks.py')
    checker_sha = digest(Path(__file__).with_name('ai_codereview_acceptance.py'))
    schemas = [entry for entry in tool_schemas(SimpleNamespace(allow_code=True, allow_release=False))
               if entry['name'] in ('source_read', 'source_search', 'source_replace', 'workspace_diff', 'run_checks')]
    spec = configuration['start']
    spec['task']['evidence_dir'] = str(root / 'harness')
    spec['run'].update(cwd=str(workspace.root), session_path=str(state / 'session.json'),
                       environment_state=str(state / 'environment'), tools=schemas)
    objective = spec['run']['objective']
    schemas_by_name = {entry['name']: entry['parameters'] for entry in schemas}
    lock = asyncio.Lock()

    class ProductHost:
        async def tool_call(self, request, cancelled):
            name, args = request['tool_name'], request['arguments']
            schema = schemas_by_name[name]
            if not isinstance(args, dict) or set(args) - set(schema['properties']) or not set(schema['required']) <= set(args):
                raise ValueError('tool arguments exceed the original product contract')
            operation = private / ('operation-' + hashlib.sha256(request['rpc_id'].encode()).hexdigest() + '.json')
            async with lock:
                if cancelled.is_set():
                    save_new(operation, {'status': 'not_started', 'rpc_id': request['rpc_id']})
                    return {'value': 'Host cancellation before invocation', 'is_error': True,
                            'settlement': 'Settled', 'operation_receipt': str(operation)}
                if name == 'source_read':
                    value = await asyncio.to_thread(workspace.read, **args)
                elif name == 'source_search':
                    value = await asyncio.to_thread(workspace.search, **args)
                elif name == 'source_replace':
                    value = await asyncio.to_thread(workspace.replace, **args)
                elif name == 'workspace_diff':
                    value = await asyncio.to_thread(workspace.diff)
                elif name == 'run_checks':
                    before = await asyncio.to_thread(workspace.diff)
                    if not before['files']:
                        # The real Workspace requires a patch before it can
                        # start checks. Establish this exact precondition before
                        # invoking it; do not classify arbitrary ValueError or
                        # process failures as known no-effect errors.
                        value = {'category': 'checks_require_actual_patch', 'subprocess_started': False,
                                 'message': 'No source patch exists. Read and correct the authorized source first; the Host runs the fixed regressions after submission.',
                                 'patch_digest': before['digest']}
                        save_new(operation, {'rpc_id': request['rpc_id'], 'task_id': request['task_id'],
                                 'run_id': request['run_id'], 'tool': name, 'status': 'known_not_started',
                                 'args_sha256': request['args_sha256'], 'result': value})
                        return {'value': value, 'is_error': True, 'settlement': 'Settled',
                                'operation_receipt': str(operation)}
                    value = await asyncio.to_thread(workspace.checks, **args, cancelled=cancelled.is_set)
                else:
                    raise ValueError('unregistered product operation')
                save_new(operation, {'rpc_id': request['rpc_id'], 'task_id': request['task_id'],
                         'run_id': request['run_id'], 'native_call_id': request['native_call_id'],
                         'args_sha256': request['args_sha256'], 'tool': name, 'status': 'completed', 'result': value})
            return {'value': value, 'is_error': False, 'settlement': 'Settled', 'operation_receipt': str(operation)}

        async def verify(self, candidate, cancelled):
            return await self._verify_workspace(candidate['basis'], cancelled)

        async def verify_budget_snapshot(self, snapshot, cancelled):
            termination = snapshot['termination']
            if termination != {'version': 1, 'cause': 'call_budget_exhausted',
                               'native_round_closed': True, 'model_final_stop': False}:
                raise ValueError('unsupported native budget snapshot')
            return await self._verify_workspace(snapshot['basis'], cancelled, termination)

        async def _verify_workspace(self, basis, cancelled, termination=None):
            async with lock:
                current = workspace.diff()
                original_patch = current['digest']
                source_sha = digest(workspace.path / 'app/services/git_service.py')
                semantic = None
                if not current['files']:
                    # This known precondition precedes Workspace.checks and its
                    # test subprocess. Preserve a failed candidate as feedback;
                    # unexpected check errors still propagate for reconciliation.
                    result = {'status': 'failed', 'category': 'checks_require_actual_patch',
                              'test_subprocess_started': False, 'executed_modules': [],
                              'digest': current['digest'],
                              'output': 'No source patch exists. The checks test subprocess was not started. '
                                        'Read and correct the authorized source, preserve the fixed tests, then submit the actual patch.'}
                    feedback = result['output']
                else:
                    result = await asyncio.to_thread(workspace.checks, ['tests.test_git_diff_chunks'],
                                                     cancelled=cancelled.is_set)
                    feedback = ('Actual fixed seven-test suite failed. Preserve tests and raw Git blobs.\n'
                                + result['output'][-5000:])
                current = workspace.diff()
                count = re.search(r'Ran (\d+) tests?', result['output'])
                test_count = int(count.group(1)) if count else None
                tests_unchanged = digest(workspace.path / 'tests/test_git_diff_chunks.py') == original_test_hash
                stable = current['digest'] == original_patch and digest(workspace.path / 'app/services/git_service.py') == source_sha
                regression_passed = result['status'] == 'passed' and test_count == 7 and tests_unchanged and stable
                if regression_passed and not cancelled.is_set():
                    semantic = await asyncio.to_thread(semantic_checks, workspace,
                        private / f"semantic-{basis['run_number']}", checker_sha, source_sha,
                        spec['task']['deadline_unix_ms'], cancelled)
                    current = workspace.diff()
                    tests_unchanged = digest(workspace.path / 'tests/test_git_diff_chunks.py') == original_test_hash
                    stable = current['digest'] == original_patch and digest(workspace.path / 'app/services/git_service.py') == source_sha
                    if semantic['status'] == 'FAIL':
                        failures = [{'case_id': case['case_id'], 'failures': case.get('failures')}
                                    for case in semantic['report']['cases'] if case['status'] == 'FAIL']
                        feedback = ('Fixed seven regressions passed, but actual Git display-path checks failed. '
                                    'Preserve valid Unicode including U+0000, raw Git blobs and strict revision reading. '
                                    'Display malformed UTF-8 bytes using UTF-8 replacement U+FFFD.\n'
                                    + json.dumps(failures, ensure_ascii=True)[:5000])
                passed = regression_passed and semantic is not None and semantic['status'] == 'PASS' and stable and tests_unchanged
                deadline_expired = time.time() * 1000 >= spec['task']['deadline_unix_ms']
                unknown = cancelled.is_set() or deadline_expired or not stable or not tests_unchanged or (semantic is not None and semantic['status'] == 'UNKNOWN')
                report = private / f"review-{basis['run_number']}.json"
                save_new(report, {'basis': basis, 'validation': result, 'current_patch': current,
                         'test_count': test_count, 'test_source_unchanged': tests_unchanged,
                         'review_kind': 'closed_budget_snapshot' if termination else 'ordinary_candidate',
                         'native_termination': termination,
                         'semantic_checks': semantic, 'acceptance_checker_sha256': checker_sha,
                         'source_sha256_before': source_sha, 'source_and_patch_stable': stable,
                         'deadline_expired': deadline_expired,
                         'scope': 'actual product Workspace, seven fixed regressions and six real Git semantic checks; no model review or release'})
                receipt = {'basis': basis, 'committed': True,
                           'settlement': 'Unknown' if unknown else 'Settled',
                           'verdict': 'Unknown' if unknown else 'Verified' if passed else
                                      {'NeedsChanges': {'feedback': feedback}},
                           'evidence': [str(report) + '#sha256=' + digest(report)]}
                save_new(private / f"review-{basis['run_number']}-committed.json", receipt)
                return receipt

        async def prepare(self, continuation, cancelled):
            return self._prepare(continuation, cancelled)

        async def prepare_budget_stop_continuation(self, continuation, cancelled):
            if continuation['previous']['termination'].get('model_final_stop') is not False:
                raise ValueError('budget continuation requires the original native snapshot')
            return self._prepare(continuation, cancelled)

        def _prepare(self, continuation, cancelled):
            if cancelled.is_set():
                raise RuntimeError('cancelled; no continuation')
            feedback = continuation['receipt']['verdict']['NeedsChanges']['feedback']
            return {'run_id_next': f"acr-harness-run-{continuation['next_run_number']}",
                    'objective': objective + '\nIndependent Host check feedback:\n' + feedback}

    save_new(private / 'configuration.json', {'start': spec, 'product_root': configuration['product_root'],
             'source_fixture': configuration['source_fixture'], 'workspace_base': workspace.base,
             'semantic_acceptance_version': ACCEPTANCE_VERSION, 'semantic_case_ids': CASE_IDS,
             'acceptance_checker_sha256': checker_sha,
             'native_core_pin_modified': False, 'production_or_release': False})
    outcome = await run_task(configuration['binary'], spec, ProductHost(),
                             token=os.environ['ARA_AGENT_MODEL_TOKEN'], operation_directory=root / 'host-operations')
    save_new(private / 'consumer-outcome.json', outcome)
    print(json.dumps({'stop': outcome['stop'], 'runs': outcome['runs'], 'calls': outcome['observed_calls'],
                      'task_accepted': outcome['task_accepted'], 'root': str(root)}))
    if outcome['stop'] != 'AwaitingHostAcceptance':
        raise RuntimeError('isolated repair did not pass; inspect original evidence before any replay')


if __name__ == '__main__':
    if len(sys.argv) != 2:
        raise SystemExit('usage: ai_codereview_host.py CONFIG.json')
    with Path(sys.argv[1]).open(encoding='utf-8') as stream:
        configuration = json.load(stream)
    asyncio.run(main(configuration))
