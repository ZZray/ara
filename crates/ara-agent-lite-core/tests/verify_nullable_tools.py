"""Native nullable optional-argument contracts with a scripted loopback provider.

Uses an already built business_host and explicitly selected real Ctags/GNU
backends. GCC compiles only an owned object fixture; that object is never run.
Every tool call crosses the actual AgentLoop schema validator. This verifies
wire/runtime contracts, not model intelligence, upstream strict conversion,
remote provider behavior or an OS sandbox. Fresh evidence only; no retries.
"""
import argparse
import json
import os
from pathlib import Path
import re
import subprocess
import time

from verify_versioned_edits import Provider, TOKEN, MODEL, digest, require, write_json
from verify_process_capture import same_target


TEXT = b'first\r\nsecond\nthird\n'
SOURCE = b'int nullable_add(int x) { return x * 7 + 3; }\nint nullable_beta(void) { return nullable_add(4); }\n'
CASES = ('read-null-positive', 'analysis-null-positive', 'schema-negative',
         'nonnull-conflicts', 'pagination-guards', 'null-edge-rejections')
READ_OPTIONAL = ('snapshot', 'offset', 'limit', 'tail')
ANALYSIS_OPTIONAL = ('expected_sha256', 'expected_page_token', 'offset', 'limit')


def nulls(keys, **values):
    return {**dict.fromkeys(keys), **values}


def step(tool, args, check, error=None):
    return {'tool': tool, 'args': args, 'check': check, 'error': error}


def program(name, text, source, obj, unknown):
    read = lambda **kw: nulls(READ_OPTIONAL, path=str(text), **kw)
    binary = lambda action, **kw: nulls(ANALYSIS_OPTIONAL + ('start_vma', 'stop_vma'),
                                       path=str(obj), action=action, **kw)
    symbols = lambda **kw: nulls(ANALYSIS_OPTIONAL + ('language',), path=str(source), **kw)
    if name == 'read-null-positive':
        return [step('read_file', read(), 'text'),
                step('read_file', read(snapshot=True), 'snapshot'),
                step('read_file', read(offset=2, limit=1), 'range'),
                step('read_file', read(tail=1), 'tail'),
                step('read_file', read(snapshot=False), 'text'),
                step('read_file', read(snapshot='false', offset='2', limit='1'), 'range')]
    if name == 'analysis-null-positive':
        return [step('binary_inspect', binary('headers'), 'headers'),
                step('binary_inspect', binary('symbols'), 'binary-symbols'),
                step('binary_inspect', binary('disassemble', start_vma='0x0', stop_vma='0x10000'), 'disassemble'),
                step('source_symbols', symbols(), 'source-symbols'),
                step('source_symbols', symbols(language='C'), 'source-symbols'),
                step('binary_inspect', binary('headers', offset='0', limit='100'), 'headers')]
    if name == 'schema-negative':
        return [step('read_file', {**read(), 'unknown': None}, 'error', 'Validation failed'),
                step('read_file', {**read(), 'path': None}, 'error'),
                step('binary_inspect', {**binary('headers'), 'unknown': None}, 'error', 'Validation failed'),
                step('binary_inspect', {**binary('headers'), 'action': None}, 'error', 'Validation failed'),
                step('source_symbols', {**symbols(), 'path': None}, 'error'),
                step('source_symbols', {**symbols(), 'unknown': None}, 'error', 'Validation failed')]
    if name == 'nonnull-conflicts':
        return [step('read_file', read(snapshot=True, offset=1, limit=2000, tail=1),
                     'error', 'snapshot cannot combine'),
                step('read_file', read(snapshot=False, offset=1, limit=2000, tail=1),
                     'error', 'tail cannot be combined'),
                step('binary_inspect', binary('headers', start_vma='0x0', stop_vma='0x0',
                     expected_sha256='0' * 64, expected_page_token='0' * 64, offset=0, limit=100),
                     'error', 'VMA arguments require disassemble'),
                step('binary_inspect', binary('symbols', start_vma='0x0', stop_vma='0x0'),
                     'error', 'VMA arguments require disassemble')]
    if name == 'pagination-guards':
        return [step('binary_inspect', binary('headers', limit=1), 'page-anchor'),
                step('binary_inspect', binary('headers', offset=1, limit=1,
                     expected_sha256='@page_sha', expected_page_token='@page_token'), 'page-next'),
                step('binary_inspect', binary('headers', offset=1, limit=1),
                     'error', 'offset>0 requires expected_sha256'),
                step('binary_inspect', binary('headers', offset=1, limit=1, expected_sha256='@page_sha'),
                     'error', 'offset>0 requires expected_page_token'),
                step('binary_inspect', binary('headers', offset=1, limit=1,
                     expected_sha256='0' * 64, expected_page_token='@page_token'),
                     'error', 'input SHA256 differs'),
                step('binary_inspect', binary('headers', offset=1, limit=1,
                     expected_sha256='@page_sha', expected_page_token='0' * 64),
                     'error', 'analysis query/backend differs'),
                step('binary_inspect', binary('symbols', offset=1, limit=1,
                     expected_sha256='@page_sha', expected_page_token='@page_token'),
                     'error', 'analysis query/backend differs')]
    require(name == 'null-edge-rejections', 'unknown harness case')
    return [step('binary_inspect', binary('disassemble'), 'error', 'start_vma must be a string'),
            step('binary_inspect', binary('disassemble', start_vma='0x0'), 'error', 'stop_vma must be a string'),
            step('source_symbols', {**symbols(), 'path': str(unknown)},
                 'error', 'unsupported extension'),
            step('source_symbols', symbols(language='NO_SUCH_NULLABLE_LANGUAGE'),
                 'error', 'unsupported ctags language'),
            step('read_file', read(offset=0), 'error', 'Validation failed'),
            step('binary_inspect', binary('headers', limit=0), 'error', 'Validation failed')]


def check_schema(request):
    tools = {t['function']['name']: t['function'] for t in request.get('tools', [])}
    for name, optional, required in (
            ('read_file', READ_OPTIONAL, ['path']),
            ('source_symbols', ANALYSIS_OPTIONAL + ('language',), ['path']),
            ('binary_inspect', ANALYSIS_OPTIONAL + ('start_vma', 'stop_vma'), ['path', 'action'])):
        require(name in tools, 'native tool absent: ' + name)
        schema = tools[name]['parameters']
        require(schema.get('required') == required, 'required fields changed: ' + name)
        require(schema.get('additionalProperties') is False, 'unknown fields permitted: ' + name)
        properties = schema['properties']
        for key in optional:
            types = properties[key].get('type')
            expected = ('boolean' if key == 'snapshot' else
                        'integer' if key in ('offset', 'limit', 'tail') else 'string')
            require(isinstance(types, list) and set(types) == {expected, 'null'} and len(types) == 2,
                    'optional field is not precisely nullable: ' + name + '.' + key)
        for key in required:
            require(properties[key].get('type') == 'string', 'required field became nullable')
    edit = tools['edit_file']['parameters']['properties']
    require(edit['expected_sha256']['type'] == 'string' and edit['dry_run']['type'] == 'boolean',
            'edit_file strict optional types changed')
    require(tools['binary_inspect']['parameters']['properties']['action']['enum'] ==
            ['headers', 'symbols', 'disassemble'], 'binary action enum changed')
    return tools


def verify_analysis(value, target, backends, check):
    require(value.get('schema') == 'ara.analysis.v1' and value.get('status') == 'completed',
            'small real analysis did not complete')
    require(same_target(value['input']['path'], target), 'analysis target identity differs')
    require(value['input']['sha256'] == digest(target.read_bytes()), 'analysis target hash differs')
    require(Path(value['input']['snapshot_path']).read_bytes() == target.read_bytes(),
            'actual retained snapshot differs')
    backend = value['backend']
    require(backend['name'] in backends and
            same_target(backend['path'], backends[backend['name']]) and
            backend['sha256'] == digest(backends[backend['name']].read_bytes()), 'backend identity differs')
    analysis = value['analysis']
    require(analysis['complete'] and analysis['backend_complete'] and analysis['exit_code'] == 0,
            'actual backend process failed or is partial')
    capture = analysis['capture']
    require(capture['complete'], 'backend raw capture incomplete')
    for stream in ('stdout', 'stderr'):
        row = capture[stream]
        raw = Path(row['path']).read_bytes()
        require(row['complete'] and row['eof'] and len(raw) == row['stored_bytes'] == row['observed_bytes']
                and digest(raw) == row['sha256'], 'backend raw stream evidence differs')
    records = value.get('symbols', value.get('lines'))
    require(isinstance(records, list) and value['page']['returned'] == len(records), 'page count differs')
    require(not value['page']['fields_truncated'], 'small fixture fields unexpectedly truncated')
    if check == 'headers':
        require(records and any('file format' in line for line in records), 'real GNU headers absent')
    elif check == 'binary-symbols':
        require(any('nullable_add' in line for line in records) and
                any('nullable_beta' in line for line in records), 'real GNU symbols absent')
    elif check == 'disassemble':
        require(any(re.match(r'\s*[0-9a-f]+:', line) for line in records), 'real disassembly addresses absent')
    elif check == 'source-symbols':
        require({'nullable_add', 'nullable_beta'} <= {row['name'] for row in records}, 'real Ctags definitions absent')
        require(value['language'] == 'C' and all(row['line'] > 0 for row in records), 'source language/lines differ')


class NullableProvider(Provider):
    def __init__(self, folder, name, text, source, obj, unknown, backends):
        self.steps = program(name, text, source, obj, unknown)
        self.source, self.obj, self.backends = source, obj, backends
        self.anchor, self.first_tools, self.checked = None, None, []
        super().__init__(folder, name, text)

    def next_action(self, request):
        count = len(self.requests)
        require(count <= len(self.steps) + 1, 'unexpected retry or extra model call')
        require(request.get('stream') is True, 'streaming wire contract changed')
        check_schema(request)
        if self.first_tools is None:
            self.first_tools = request['tools']
        else:
            require(request['tools'] == self.first_tools, 'tool schemas changed within Session')
        tools = [m for m in request['messages'] if m.get('role') == 'tool']
        previous = self.previous(tools)
        require(self.target.read_bytes() == TEXT, 'read/negative edit modified owned text fixture')
        if count > 1:
            spec = self.steps[count - 2]
            check = spec['check']
            if check == 'error':
                require(bool(previous), 'native rejection has no evidence')
                if spec['error'] is not None:
                    require(spec['error'] in previous, 'expected rejection absent: ' + repr(previous))
            elif check == 'text':
                require(previous.encode('utf8') == TEXT, 'optional nulls changed path-only text')
            else:
                value = json.loads(previous)
                if check == 'snapshot':
                    require(value['content'].encode('utf8') == TEXT and value['sha256'] == digest(TEXT)
                            and value['bytes'] == len(TEXT) and value['truncated'] is False,
                            'actual null-compatible snapshot differs')
                elif check in ('range', 'tail'):
                    require(value['mode'] == check and len(value['lines']) == 1, 'read selection differs')
                    row = value['lines'][0]
                    if check == 'range':
                        require(row['line'] == 2 and row['text'] == 'second', 'range content/line differs')
                    else:
                        require(row['line'] is None and row['line_from_end'] == 1 and row['text'] == 'third',
                                'tail content/line differs')
                else:
                    verify_analysis(value, self.source if spec['tool'] == 'source_symbols' else self.obj,
                                    self.backends, check)
                    if check == 'page-anchor':
                        require(value['page']['offset'] == 0 and value['page']['next_offset'] == 1,
                                'anchor does not expose actual next page')
                        self.anchor = value
                    elif check == 'page-next':
                        require(value['page']['offset'] == 1 and value['page']['page_token'] == self.anchor['page']['page_token'],
                                'nullable-compatible guarded pagination differs')
            self.checked.append({'step': count - 1, 'tool': spec['tool'], 'check': check, 'passed': True,
                                 'tool_response_sha256': digest(previous.encode('utf8')),
                                 'rejection_text': previous if check == 'error' else None,
                                 'rejection_stage': ('AgentLoop schema validation' if 'Validation failed' in previous
                                                     else 'tool execution') if check == 'error' else None})
        if count == len(self.steps) + 1:
            self.finished = True
            return {'content': 'NULLABLE_NATIVE_CASE_VERIFIED'}, 'stop'
        spec = self.steps[count - 1]
        args = dict(spec['args'])
        for key, val in list(args.items()):
            if val == '@page_sha':
                require(self.anchor is not None, 'page anchor not observed')
                args[key] = self.anchor['input']['sha256']
            elif val == '@page_token':
                require(self.anchor is not None, 'page anchor not observed')
                args[key] = self.anchor['page']['page_token']
        return self.call(spec['tool'], args)


def pins():
    core = Path(__file__).resolve().parents[1]
    files = sorted(p for p in (core / 'src').rglob('*') if p.is_file())
    files += [core / 'Cargo.toml', core / 'Cargo.lock', core / 'examples/business_host.rs',
              Path(__file__).resolve(), Path(__file__).with_name('verify_versioned_edits.py'),
              Path(__file__).with_name('verify_process_capture.py')]
    return {str(p.relative_to(core)): digest(p.read_bytes()) for p in files if p.is_file()}


def run_case(binary, root, name, fixtures, backends):
    folder = root / name
    folder.mkdir()
    artifacts = folder / 'analysis-artifacts'
    artifacts.mkdir()
    text, source, obj, unknown = fixtures
    provider = NullableProvider(folder, name, text, source, obj, unknown, backends)
    config = {'id': name, 'objective': 'Controlled nullable tool contract: ' + name,
              'cwd': str(folder), 'session_path': str(folder / 'session.json'),
              'environment_state': str(folder / 'environment-state'), 'endpoint': provider.endpoint,
              'model': MODEL, 'token_env': 'ARA_NULLABLE_FIXTURE_KEY', 'mode': 'execute',
              'enable_local_tools': True, 'max_calls': len(provider.steps) + 1,
              'max_seconds': 90, 'analysis_config': {**{k: str(v) for k, v in backends.items()},
                                                   'artifact_root': str(artifacts)}}
    write_json(folder / 'request.json', config)
    environment = os.environ.copy()
    for key in ('ARA_ANALYSIS_FAULT', 'ARA_ANALYSIS_ARGV', 'ARA_ANALYSIS_PID'):
        environment.pop(key, None)
    environment.update(ARA_NULLABLE_FIXTURE_KEY=TOKEN, NO_PROXY='127.0.0.1,localhost')
    started = time.monotonic()
    row = {'name': name, 'status': 'FAIL', 'effects_unknown': False}
    try:
        try:
            process = subprocess.run([str(binary), str(folder / 'request.json')], cwd=folder,
                                     env=environment, capture_output=True, timeout=100)
            code, stdout, stderr = process.returncode, process.stdout, process.stderr
        except subprocess.TimeoutExpired as error:
            code, stdout, stderr = None, error.stdout or b'', error.stderr or b''
            row.update(timeout=True, effects_unknown=True)
        (folder / 'host.stdout.native.jsonl').write_bytes(stdout)
        (folder / 'host.stderr.native.txt').write_bytes(stderr)
        row.update(host_exit=code, stdout_sha256=digest(stdout), stderr_sha256=digest(stderr))
        require(code is not None, 'native Host timeout; retain evidence, do not replay')
        packets = [json.loads(line) for line in stdout.decode('utf8').splitlines() if line.strip()]
        events = [p['event'] for p in packets if p.get('kind') == 'event']
        write_json(folder / 'host-events.json', events)
        require(not provider.errors, 'controlled provider assertions failed: ' + repr(provider.errors))
        require(code == 0 and packets[-1].get('kind') == 'result' and packets[-1].get('code') == 0,
                'native controlled Run did not settle successfully')
        require(provider.finished and len(provider.checked) == len(provider.steps), 'scripted checks incomplete')
        require(len(provider.requests) == len(provider.steps) + 1 and len(provider.catalogs) == 1,
                'unexpected catalog/model call count')
        ends = {e['step_id']: e for e in events if e.get('type') == 'tool_end'}
        require(len(ends) == len(provider.steps), 'native tool-end count differs')
        for action, spec in zip(provider.actions, provider.steps):
            event = ends.get(action['id'])
            expected = 'failed' if spec['check'] == 'error' else 'completed'
            require(event and event['title'] == action['name'] and event['status'] == expected,
                    'actual native tool status differs: ' + action['id'])
        require(text.read_bytes() == TEXT, 'fixture changed after settlement')
        row.update(status='PASS', expected_tools=len(provider.steps), checked=provider.checked,
                   model_calls=len(provider.requests), tool_ends=len(ends),
                   first_wire_tools_sha256=digest(json.dumps(provider.first_tools, sort_keys=True).encode()))
    except Exception as error:
        row['error'] = type(error).__name__ + ': ' + str(error)
    finally:
        provider.close()
        write_json(folder / 'provider-errors.json', provider.errors)
        write_json(folder / 'scripted-actions.json', provider.actions)
        write_json(folder / 'checked-tool-feedback.json', provider.checked)
        row['elapsed_seconds'] = round(time.monotonic() - started, 3)
        write_json(folder / 'case.json', row)
    return row


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--binary', type=Path, required=True)
    parser.add_argument('--output', type=Path, required=True)
    for name in ('gcc', 'ctags', 'objdump', 'nm'):
        parser.add_argument('--' + name, type=Path, required=True)
    args = parser.parse_args()
    binary, root = args.binary.resolve(), args.output.resolve()
    backends = {name: getattr(args, name).resolve() for name in ('ctags', 'objdump', 'nm')}
    require(all(p.is_file() for p in [binary, args.gcc, *backends.values()]), 'explicit executable missing')
    root.mkdir(parents=True, exist_ok=False)
    fixture = root / 'fixtures'
    fixture.mkdir()
    text, source, obj, unknown = (fixture / p for p in ('lines.txt', 'owned.c', 'owned.o', 'owned.unknown'))
    text.write_bytes(TEXT)
    source.write_bytes(SOURCE)
    unknown.write_bytes(SOURCE)
    before = pins()
    result = {'schema': 'ara-nullable-tools-native-v1', 'status': 'FAIL', 'cases': [],
              'scope': 'scripted loopback responses through real business_host AgentLoop and selected real backends',
              'limits': 'not model intelligence, strict-schema conversion, remote provider or OS sandbox acceptance',
              'binary_path': str(binary), 'binary_sha256': digest(binary.read_bytes()),
              'source_pins': before, 'harness_sha256': digest(Path(__file__).read_bytes()),
              'backends': {k: {'path': str(p), 'sha256': digest(p.read_bytes())} for k, p in backends.items()},
              'budget_per_case': {'model_calls_at_most': 8, 'host_seconds': 90, 'outer_seconds': 100},
              'sample_executions': 0, 'external_model_calls': 0, 'automatic_retries': 0}
    result['edit_file_null_boundary'] = (
        'Wire edit_file optional fields remain nonnullable. Direct LocalTool null rejection is covered by '
        'the existing Rust versioned-edit tests, not claimed from this AgentLoop fixture: pinned rpi may '
        'drop unsupported optional nulls before execution. No edit_file call is sent by this harness.')
    started = time.monotonic()
    try:
        argv = [str(args.gcc.resolve()), '-c', '-O0', '-g0', str(source), '-o', str(obj)]
        try:
            built = subprocess.run(argv, capture_output=True, timeout=30)
        except subprocess.TimeoutExpired as error:
            (fixture / 'compile.stdout').write_bytes(error.stdout or b'')
            (fixture / 'compile.stderr').write_bytes(error.stderr or b'')
            result['effects_unknown'] = True
            raise AssertionError('owned fixture compile timed out; do not replay') from error
        (fixture / 'compile.stdout').write_bytes(built.stdout)
        (fixture / 'compile.stderr').write_bytes(built.stderr)
        result['fixture_compile'] = {'argv': argv, 'exit_code': built.returncode,
            'stdout_sha256': digest(built.stdout), 'stderr_sha256': digest(built.stderr),
            'gcc_sha256': digest(args.gcc.read_bytes())}
        require(built.returncode == 0 and obj.is_file(), 'owned object fixture compile failed')
        fixture_before = {p.name: digest(p.read_bytes()) for p in (text, source, obj, unknown)}
        result['fixture_pins'] = fixture_before
        for name in CASES:
            result['cases'].append(run_case(binary, root, name, (text, source, obj, unknown), backends))
        require(pins() == before, 'runtime/harness source changed during acceptance')
        require(all(digest((fixture / name).read_bytes()) == h for name, h in fixture_before.items()),
                'owned fixtures changed during acceptance')
        require(all(digest(p.read_bytes()) == result['backends'][k]['sha256'] for k, p in backends.items()),
                'selected backend executable changed during acceptance')
        require(digest(binary.read_bytes()) == result['binary_sha256'], 'Host executable changed during acceptance')
        require(len(result['cases']) == len(CASES) and all(c['status'] == 'PASS' for c in result['cases']),
                'one or more nullable native cases failed; retain original evidence')
        result['status'] = 'PASS'
    except Exception as error:
        result['error'] = type(error).__name__ + ': ' + str(error)
    result['elapsed_seconds'] = round(time.monotonic() - started, 3)
    write_json(root / 'result.json', result)
    print(json.dumps({'status': result['status'], 'cases': len(result['cases']), 'evidence': str(root / 'result.json')}))
    return 0 if result['status'] == 'PASS' else 1


if __name__ == '__main__':
    raise SystemExit(main())
