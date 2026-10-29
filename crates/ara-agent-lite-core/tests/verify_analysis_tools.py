"""Native static-analysis contracts: real backends and controlled backend faults.

Compiles only self-owned C fixtures using an existing GCC. Never executes the
analysis samples. The separate fault backend is executable test infrastructure.
Direct Host queries and scripted-provider Plan Runs exercise different paths;
neither proves autonomous model coding or reverse-engineering competence.
No installation, Cargo build, external provider or user credential is used.
"""
import argparse
import json
import os
from pathlib import Path
import re
import shutil
import subprocess
import time

from verify_versioned_edits import Provider, TOKEN, MODEL, digest, require, write_json
from verify_process_capture import process_probe, same_target


def analysis_json(text):
    # AgentError::Tool adds this typed presentation prefix to failure content.
    prefix = 'tool error: '
    return json.loads(text[len(prefix):] if text.startswith(prefix) else text)


SAMPLE = b'''__attribute__((noinline)) int capture_add(int x) { return x * 7 + 3; }
int alpha(void) { return capture_add(4); }
int beta(void) { return alpha() + 2; }
int main(void) { return beta(); }
'''
FAULT_SOURCE = r'''
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#ifdef _WIN32
#include <windows.h>
#define PID() ((unsigned long)GetCurrentProcessId())
#define SLEEP() Sleep(30000)
#else
#include <unistd.h>
#define PID() ((unsigned long)getpid())
#define SLEEP() sleep(30)
#endif
static void tag(unsigned n) {
  printf("{\"_type\":\"tag\",\"name\":\"fixture_%06u\",\"path\":\"input.c\",\"kind\":\"function\",\"line\":1}\n", n);
}
int main(int argc, char **argv) {
  const char *mode = getenv("ARA_ANALYSIS_FAULT");
  const char *log = getenv("ARA_ANALYSIS_ARGV");
  int i; if (!mode) mode = "valid";
  if (log) { FILE *f = fopen(log,"ab"); if (!f) return 91;
    fputs("CALL\n",f); for(i=1;i<argc;i++) fprintf(f,"%s\n",argv[i]); fputs("END\n",f); fclose(f); }
  for(i=1;i<argc;i++) {
    if (!strcmp(argv[i],"--version")) {
      if (!strcmp(mode,"empty-version")) return 0;
      else if (strstr(argv[0],"objdump")) puts("GNU objdump (GNU Binutils) 2.46.1");
      else if (strstr(argv[0],"nm")) puts("GNU nm (GNU Binutils) 2.46.1");
      else puts("Universal Ctags 6.2.1, Copyright (C) 2015 Universal Ctags Team");
      return 0;
    }
    if (strstr(argv[i],"--list-languages")) { puts("C\nC++\nPython"); return 0; }
    if (strstr(argv[i],"--list-features")) { puts("json\ninteractive"); return 0; }
  }
  if (!strcmp(mode,"malformed")) { puts("{\"_type\":\"tag\",\"name\":"); return 0; }
  if (!strcmp(mode,"invalid-utf8")) { putchar(0xff); return 0; }
  if (!strcmp(mode,"unfittable-record")) {
    fputs("{\"_type\":\"tag\",\"name\":\"",stdout);
    for(i=0;i<1024;i++) fputs("\\u0000",stdout);
    fputs("\",\"kind\":\"function\",\"line\":1,\"scope\":\"",stdout);
    for(i=0;i<1024;i++) fputs("\\u0000",stdout);
    fputs("\",\"signature\":\"",stdout);
    for(i=0;i<1024;i++) fputs("\\u0000",stdout);
    fputs("\"}\n",stdout); return 0;
  }
  if (!strcmp(mode,"nonzero")) { fputs("fixture-backend-nonzero\n",stderr); return 17; }
  if (!strcmp(mode,"snapshot-mutation")) {
    FILE *f=fopen(argv[argc-1],"ab"); if(!f) return 93;
    fputs("\n/* CONTROLLED_SNAPSHOT_MUTATION */\n",f); fclose(f); tag(0); return 0;
  }
  if (!strcmp(mode,"timeout") || !strcmp(mode,"cancel")) {
    const char *path = getenv("ARA_ANALYSIS_PID");
    if(path) { FILE *f=fopen(path,"wb"); if(!f) return 92; fprintf(f,"{\"pid\":%lu}",PID()); fflush(f); fclose(f); }
    tag(0); fflush(stdout); fputs("fixture-before-wait\n",stderr); fflush(stderr); SLEEP(); return 0;
  }
  if (!strcmp(mode,"quota")) { for(i=0;i<180000;i++) tag((unsigned)i); return 0; }
  tag(0); tag(1); return 0;
}
'''


def pins():
    core = Path(__file__).resolve().parents[1]
    paths = [p for p in (core / 'src').rglob('*') if p.is_file()]
    paths += [core / 'Cargo.toml', core / 'Cargo.lock', core / 'examples/business_host.rs',
              Path(__file__), Path(__file__).with_name('verify_versioned_edits.py'),
              Path(__file__).with_name('verify_process_capture.py')]
    return {str(p.relative_to(core)): digest(p.read_bytes()) for p in sorted(paths) if p.is_file()}


def command(args, folder, name, timeout=45, env=None):
    started = time.monotonic()
    try:
        p = subprocess.run([str(x) for x in args], capture_output=True, timeout=timeout, env=env)
        code, out, err = p.returncode, p.stdout, p.stderr
    except subprocess.TimeoutExpired as error:
        code, out, err = None, error.stdout or b'', error.stderr or b''
    (folder / (name + '.stdout')).write_bytes(out)
    (folder / (name + '.stderr')).write_bytes(err)
    row = {'argv': [str(x) for x in args], 'exit_code': code,
           'elapsed_seconds': round(time.monotonic() - started, 3),
           'stdout_sha256': digest(out), 'stderr_sha256': digest(err)}
    write_json(folder / (name + '.command.json'), row)
    require(code is not None, name + ': process timeout; retain evidence, do not replay')
    return code, out, err


class Harness:
    def __init__(self, binary, root, backends):
        self.binary, self.root, self.backends = binary, root, backends
        self.sequence = 0
        self.config = {name: str(path) for name, path in backends.items() if name in ('ctags', 'objdump', 'nm')}
        self.fixtures = root / 'fixtures'
        self.fixtures.mkdir()
        self.source = self.fixtures / '-中文 @ sample.c'
        self.source.write_bytes(SAMPLE)
        self.pe = self.fixtures / 'owned-sample.exe'
        self.obj = self.fixtures / 'owned-sample.o'
        fault_c = self.fixtures / 'fault-backend.c'
        fault_c.write_text(FAULT_SOURCE, encoding='utf-8', newline='\n')
        self.fake = self.fixtures / 'fixture-ctags.exe'
        for name, args in (
                ('compile-owned-pe', [backends['gcc'], '-O0', '-g0', '-fno-inline', self.source, '-o', self.pe]),
                ('compile-owned-object', [backends['gcc'], '-c', '-O0', '-g0', self.source, '-o', self.obj]),
                ('compile-fault-backend', [backends['gcc'], '-O0', fault_c, '-o', self.fake])):
            code, _, _ = command(args, self.fixtures, name)
            require(code == 0, name + ': compiler failed')
        self.fixture_pins = {p.name: digest(p.read_bytes()) for p in self.fixtures.iterdir() if p.is_file()}

    def query(self, folder, tool, args, *, config=None, fault=None, cancel_after_ms=None, expected='completed'):
        self.sequence += 1
        q = folder / ('query-%02d' % self.sequence)
        q.mkdir()
        cfg = dict(self.config if config is None else config)
        cfg['artifact_root'] = str(q / 'artifacts')
        (q / 'artifacts').mkdir()  # Host prepares its explicitly owned retention directory.
        request = {'id': q.name, 'objective': 'Controlled direct analysis contract',
            'action': 'analysis_query', 'cwd': str(folder),
            'session_path': str(q / 'session.json'), 'environment_state': str(q / 'environment'),
            'analysis_config': cfg, 'analysis_tool': tool, 'analysis_args': args}
        if cancel_after_ms is not None:
            request['cancel_after_ms'] = cancel_after_ms
        write_json(q / 'request.json', request)
        environment = os.environ.copy()
        for key in ('ARA_ANALYSIS_FAULT', 'ARA_ANALYSIS_ARGV', 'ARA_ANALYSIS_PID'):
            environment.pop(key, None)
        if fault:
            environment.update(ARA_ANALYSIS_FAULT=fault, ARA_ANALYSIS_ARGV=str(q / 'backend-argv.txt'),
                               ARA_ANALYSIS_PID=str(q / 'backend-pid.json'))
        code, out, _ = command([self.binary, q / 'request.json'], q, 'host', timeout=55, env=environment)
        packets = [json.loads(line) for line in out.decode('utf-8').splitlines() if line.strip()]
        require(code == 0 and packets and packets[-1].get('kind') == 'analysis_result',
                'analysis Host did not return its explicit result packet')
        packet = packets[-1]
        require(packet.get('tool') == tool and packet.get('status') == expected,
                'actual tool identity/result state differs: ' + repr(packet))
        text = packet.get('output') if expected == 'completed' else packet.get('error')
        require(isinstance(text, str), 'analysis result lacks original output/error text')
        require(len(text.encode('utf-8')) <= 16384, 'analysis output exceeds 16 KiB UTF-8')
        parsed = None
        json_start = text.find('{')
        if json_start >= 0:
            parsed, _ = json.JSONDecoder().raw_decode(text[json_start:])
            require(parsed.get('schema') == 'ara.analysis.v1' and parsed.get('tool') == tool,
                    'structured analysis result identity differs')
            if expected == 'failed':
                require(parsed.get('status') == 'failed', 'error JSON claims successful analysis')
        if expected == 'completed':
            require(parsed and parsed.get('status') in ('completed', 'partial'), 'successful result is not structured')
            target = Path(args['path'])
            require(same_target(parsed['input']['path'], target), 'analysis input path differs')
            require(parsed['input']['sha256'] == digest(target.read_bytes()) and
                    parsed['input']['bytes'] == target.stat().st_size, 'analysis input identity differs')
            snapshot = Path(parsed['input']['snapshot_path'])
            require(snapshot.is_file() and snapshot.read_bytes() == target.read_bytes(),
                    'retained analysis snapshot differs from actual input')
            backend = parsed['backend']
            require(Path(backend['path']).is_file() and backend['sha256'] == digest(Path(backend['path']).read_bytes()),
                    'analysis backend hash differs from actual executable')
            items = parsed.get('symbols') if tool == 'source_symbols' else parsed.get('lines')
            require(isinstance(items, list) and parsed['page']['returned'] == len(items),
                    'page count differs from actual returned records')
            require(parsed['page']['offset'] == args.get('offset', 0), 'page offset differs')
        if fault in ('timeout', 'cancel'):
            pid_bytes = (q / 'backend-pid.json').read_bytes()
            probe = process_probe(json.loads(pid_bytes)['pid'])
            write_json(q / 'backend-process-probe.json', probe)
            require(probe['running'] is False, 'failed analysis backend remains running or unknown')
        write_json(q / 'checked-result.json', {'packet': packet, 'parsed': parsed})
        return parsed, text, q

    def case(self, name, body):
        folder = self.root / name
        folder.mkdir()
        row = {'name': name, 'status': 'FAIL'}
        started = time.monotonic()
        try:
            body(folder, row)
            row['status'] = 'PASS'
        except Exception as error:
            row['error'] = type(error).__name__ + ': ' + str(error)
        row['elapsed_seconds'] = round(time.monotonic() - started, 3)
        write_json(folder / 'case.json', row)
        return row

    def source_real(self, folder, row):
        before = self.source.read_bytes()
        first, _, _ = self.query(folder, 'source_symbols', {'path': str(self.source), 'language': 'C', 'limit': 1})
        require(first['analysis']['complete'] is True, 'small real source backend output is partial')
        require(first['page']['next_offset'] is not None, 'real outline lacks expected next page')
        second, _, _ = self.query(folder, 'source_symbols', {'path': str(self.source), 'language': 'C',
            'expected_sha256': first['input']['sha256'], 'expected_page_token': first['page']['page_token'],
            'offset': first['page']['next_offset'], 'limit': 100})
        symbols = first['symbols'] + second['symbols']
        require({'capture_add', 'alpha', 'beta', 'main'} <= {s['name'] for s in symbols}, 'real source outline incomplete')
        require(all(type(s['line']) is int and s['line'] > 0 for s in symbols), 'source lines invalid')
        require(self.source.read_bytes() == before, 'source analysis changed input')
        self.query(folder, 'source_symbols', {'path': str(self.source), 'language': 'C',
            'expected_sha256': first['input']['sha256'], 'expected_page_token': '0' * 64,
            'offset': first['page']['next_offset'], 'limit': 1}, expected='failed')
        row.update(symbol_names=[s['name'] for s in symbols], source_sha256=digest(before))

    def isolated_config(self, folder, row):
        (folder / '.ctags').write_text('--output-format=unsupported-fixture-format\n', encoding='utf-8')
        d = folder / '.ctags.d'
        d.mkdir()
        (d / 'poison.ctags').write_text('--output-format=unsupported-fixture-format\n', encoding='utf-8')
        result, _, _ = self.query(folder, 'source_symbols', {'path': str(self.source), 'language': 'C'})
        require('capture_add' in {s['name'] for s in result['symbols']}, 'project configuration affected real ctags')
        fake, _, q = self.query(folder, 'source_symbols', {'path': str(self.source), 'language': 'C'},
                                config={'ctags': str(self.fake)}, fault='valid')
        calls = (q / 'backend-argv.txt').read_text(encoding='utf-8').split('CALL\n')[1:]
        analysis_calls = [x.splitlines()[:-1] for x in calls if '--version' not in x and '--list-' not in x]
        require(analysis_calls and all(a[0] == '--options=NONE' for a in analysis_calls),
                'ctags did not disable external options as its first argument')
        require(all('-f' in a and a[a.index('-f') + 1] == '-' for a in analysis_calls), 'ctags may write a tags file')
        require(not (folder / 'tags').exists(), 'ctags produced an unexpected project output file')
        row['fake_symbol_count'] = len(fake['symbols'])

    def no_symbols(self, folder, row):
        source = folder / 'empty.c'
        source.write_text('/* An intentional translation unit with no definitions. */\n', encoding='utf-8')
        result, _, _ = self.query(folder, 'source_symbols', {'path': str(source), 'language': 'C'})
        require(result['symbols'] == [] and result['analysis']['complete'] is True,
                'real no-symbol source contract differs')

    def binary_real(self, folder, row):
        headers, _, _ = self.query(folder, 'binary_inspect', {'path': str(self.pe), 'action': 'headers', 'limit': 100})
        require(headers['lines'] and any('file format' in line.lower() for line in headers['lines']),
                'actual PE headers missing format evidence')
        self.query(folder, 'binary_inspect', {'path': str(self.pe), 'action': 'symbols', 'offset': 1,
            'expected_sha256': headers['input']['sha256'],
            'expected_page_token': headers['page']['page_token']}, expected='failed')
        symbols, _, _ = self.query(folder, 'binary_inspect', {'path': str(self.obj), 'action': 'symbols', 'limit': 100})
        require(any('capture_add' in line for line in symbols['lines']), 'actual object symbol missing')
        code, out, _ = command([self.backends['nm'], '--defined-only', '--numeric-sort', self.pe], folder, 'nm-oracle')
        require(code == 0, 'independent real nm oracle failed')
        match = re.search(rb'^([0-9a-fA-F]+)\s+[A-Za-z]\s+_?capture_add\s*$', out, re.MULTILINE)
        require(match, 'independent PE address oracle cannot locate capture_add')
        start = int(match[1], 16)
        dis, _, _ = self.query(folder, 'binary_inspect', {'path': str(self.pe), 'action': 'disassemble',
            'start_vma': hex(start), 'stop_vma': hex(start + 64), 'limit': 100})
        require(any('capture_add' in line for line in dis['lines']), 'disassembly lacks independently located function')
        require(any(re.search(r'^\s*[0-9a-fA-F]+:', line) for line in dis['lines']), 'disassembly contains no instruction rows')
        row.update(owned_pe_sha256=digest(self.pe.read_bytes()), independently_located_vma=hex(start),
                   sample_executed=False)

    def fault(self, folder, row, mode):
        kwargs = {'config': {'ctags': str(self.fake)}, 'fault': mode}
        if mode == 'cancel':
            kwargs['cancel_after_ms'] = 1000
        result, text, _ = self.query(folder, 'source_symbols', {'path': str(self.source), 'language': 'C'},
            expected='completed' if mode == 'quota' else 'failed', **kwargs)
        if mode == 'quota':
            require(result['status'] == 'partial' and result['analysis']['complete'] is False,
                    'backend raw truncation claims complete analysis')
        elif mode == 'nonzero':
            require('17' in text or 'nonzero' in text.lower(), 'backend failure exit evidence missing')
        elif mode == 'snapshot-mutation':
            require('snapshot' in text.lower() and any(x in text.lower() for x in ('changed', 'mismatch', 'verification')),
                    'backend snapshot mutation was not specifically rejected')
            changed = [p for p in folder.rglob('input.source') if b'CONTROLLED_SNAPSHOT_MUTATION' in p.read_bytes()]
            require(changed and self.source.read_bytes() == SAMPLE,
                    'controlled backend did not mutate only retained snapshot evidence')
            row['mutated_snapshot_evidence'] = {str(p): digest(p.read_bytes()) for p in changed}
        elif mode == 'unfittable-record':
            require(any(x in text.lower() for x in ('record', 'row')) and
                    any(x in text.lower() for x in ('budget', 'fit', 'size')),
                    'unfittable record did not return a useful explicit budget failure')
        row['fault'] = mode


    def stripped(self, folder, row):
        stripped = folder / 'owned-stripped.exe'
        code, _, _ = command([self.backends['strip'], '--strip-all', '-o', stripped, self.pe], folder, 'strip-owned-sample')
        require(code == 0, 'strip self-owned sample failed')
        code, out, err = command([self.backends['nm'], '--format=posix', '--no-demangle', '--print-size',
                                 '--numeric-sort', stripped], folder, 'nm-stripped-oracle')
        require(not out.strip() and (code == 0 or (code == 1 and b': no symbols' in err)),
                'actual stripped sample did not produce the expected no-symbol contract')
        result, _, _ = self.query(folder, 'binary_inspect', {'path': str(stripped), 'action': 'symbols'})
        require(result['lines'] == [] and result['analysis']['complete'] is True,
                'real no-symbol binary was presented as incomplete or invented symbols')
        row.update(nm_oracle_exit=code, stripped_sha256=digest(stripped.read_bytes()), sample_executed=False)


class PlanProvider(Provider):
    def __init__(self, folder, name, target, registered, changed_backend=None):
        super().__init__(folder, name, target)
        self.registered = registered
        self.changed_backend = changed_backend

    def next_action(self, request):
        step = len(self.requests)
        tools = [m for m in request['messages'] if m.get('role') == 'tool']
        previous = self.previous(tools)
        if step == 1:
            names = {t['function']['name'] for t in request.get('tools', [])}
            require(('source_symbols' in names) is self.registered, 'Plan custom tool advertisement differs')
            if self.changed_backend:
                before = digest(self.changed_backend.read_bytes())
                with self.changed_backend.open('ab') as changed:
                    changed.write(b'CONTROLLED_BACKEND_IDENTITY_CHANGE')
                self.observations.append({'actor': 'controlled-fixture-after-registration',
                    'before_sha256': before, 'after_sha256': digest(self.changed_backend.read_bytes())})
            return self.call('source_symbols', {'path': str(self.target), 'language': 'C'})
        require(step == 2 and previous, 'Plan Run did not return the exact tool result')
        if self.changed_backend:
            value = analysis_json(previous)
            require(value['status'] == 'failed' and 'backend' in previous.lower() and
                    any(word in previous.lower() for word in ('changed', 'identity', 'sha256')),
                    'changed backend identity was not rejected')
        elif self.registered:
            value = json.loads(previous)
            require(value['schema'] == 'ara.analysis.v1' and value['tool'] == 'source_symbols' and
                    value['analysis']['complete'] is True, 'Plan analysis result differs')
            require('capture_add' in {s['name'] for s in value['symbols']}, 'Plan native outline missed sample definition')
            require(value['input']['sha256'] == digest(self.target.read_bytes()), 'Plan input hash differs')
        else:
            require('not found' in previous.lower() or 'denied' in previous.lower() or 'blocked' in previous.lower(),
                    'unregistered forged tool call did not fail explicitly')
        self.finished = True
        return {'content': 'STATIC_ANALYSIS_PLAN_CONTRACT_VERIFIED'}, 'stop'


def plan_case(harness, folder, row, registered, change_backend=False):
    backend_copy = folder / 'changing-ctags.exe' if change_backend else None
    if backend_copy:
        shutil.copyfile(harness.fake, backend_copy)
    provider = PlanProvider(folder, row['name'], harness.source, registered, backend_copy)
    request = {'id': row['name'], 'objective': 'Controlled static-analysis Plan acceptance', 'cwd': str(folder),
        'session_path': str(folder / 'session.json'), 'environment_state': str(folder / 'environment'),
        'endpoint': provider.endpoint, 'model': MODEL, 'token_env': 'ARA_ANALYSIS_PLAN_FIXTURE_KEY',
        'mode': 'plan', 'enable_local_tools': False}
    artifacts = folder / 'plan-artifacts'
    if registered:
        artifacts.mkdir()
        cfg = {'ctags': str(backend_copy)} if backend_copy else harness.config
        request['analysis_config'] = dict(cfg, artifact_root=str(artifacts))
    write_json(folder / 'request.json', request)
    env = os.environ.copy()
    env.update(ARA_ANALYSIS_PLAN_FIXTURE_KEY=TOKEN, NO_PROXY='127.0.0.1,localhost')
    try:
        code, out, _ = command([harness.binary, folder / 'request.json'], folder, 'host', timeout=55, env=env)
        packets = [json.loads(line) for line in out.decode('utf-8').splitlines() if line.strip()]
        events = [p['event'] for p in packets if p.get('kind') == 'event']
        ends = [e for e in events if e.get('type') == 'tool_end']
        require(code == 0 and packets[-1].get('kind') == 'result' and packets[-1].get('code') == 0,
                'actual Plan Run did not settle')
        require(not provider.errors and provider.finished and len(provider.requests) == 2,
                'controlled Plan provider checks incomplete: ' + repr(provider.errors))
        require(len(ends) == 1 and ends[0].get('step_id') == provider.actions[0]['id'] and
                ends[0].get('title') == 'source_symbols' and
                ends[0].get('status') == ('completed' if registered and not change_backend else 'failed'),
                'native Plan tool identity/state differs')
        if registered:
            require(artifacts.is_dir(), 'Plan analysis artifact owner missing')
            event_result = analysis_json(ends[0]['output'])
            if not change_backend:
                require(event_result['input']['sha256'] == digest(harness.source.read_bytes()),
                        'Plan event/result input identity differs')
            else:
                require(event_result['status'] == 'failed', 'Plan event accepted a changed backend')
        else:
            require(not artifacts.exists(), 'default unregistered tool created analysis artifacts')
        row.update(native_model_calls=len(provider.requests), registered=registered, real_model=False)
    finally:
        provider.close()
        write_json(folder / 'provider-errors.json', provider.errors)
        write_json(folder / 'scripted-actions.json', provider.actions)
        write_json(folder / 'fixture-observations.json', provider.observations)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--binary', type=Path, required=True)
    parser.add_argument('--output', type=Path, required=True)
    args = parser.parse_args()
    binary, root = args.binary.resolve(), args.output.resolve()
    require(binary.is_file(), 'use an existing business_host binary; no implicit Cargo build')
    root.mkdir(parents=True, exist_ok=False)
    backends = {}
    for name in ('gcc', 'ctags', 'objdump', 'nm', 'strip'):
        found = shutil.which(name)
        require(found, 'required existing backend not found: ' + name)
        backends[name] = Path(found).resolve()
    source_before = pins()
    result = {'schema': 'ara-analysis-native-v1', 'status': 'FAIL', 'binary_path': str(binary),
        'binary_sha256': digest(binary.read_bytes()), 'harness_sha256': digest(Path(__file__).read_bytes()),
        'source_pins': source_before, 'cases': [],
        'scope': 'owned samples; real static backends; controlled faults and scripted-provider Plan; no autonomous intelligence claim',
        'analysis_samples_executed': False, 'raw_evidence': 'private fixture material; no user secrets',
        'backends': {n: {'path': str(p), 'sha256': digest(p.read_bytes())} for n, p in backends.items()}}
    started = time.monotonic()
    try:
        for name, backend in backends.items():
            code, out, _ = command([backend, '--version'], root, name + '-version')
            require(code == 0, name + ': backend version query failed')
            result['backends'][name]['version'] = out.decode('utf-8', errors='strict').splitlines()[0]
        h = Harness(binary, root, backends)
        result['fixture_pins'] = h.fixture_pins
        for name, method in [('source-real-paging-unicode', h.source_real), ('ctags-config-isolation', h.isolated_config),
                             ('source-no-symbols', h.no_symbols), ('binary-real-static', h.binary_real),
                             ('binary-real-stripped-empty', h.stripped)]:
            result['cases'].append(h.case(name, method))
        bad_cases = (
            ('stale-source', 'source_symbols', {'path': str(h.source), 'language': 'C', 'expected_sha256': '0' * 64}, None),
            ('unsupported-language', 'source_symbols', {'path': str(h.source), 'language': 'NOT_A_REAL_LANGUAGE'}, None),
            ('unbound-page', 'source_symbols', {'path': str(h.source), 'offset': 1, 'limit': 1}, None),
            ('reversed-disassembly-range', 'binary_inspect', {'path': str(h.pe), 'action': 'disassemble', 'start_vma': '0x20', 'stop_vma': '0x10'}, None),
            ('huge-disassembly-range', 'binary_inspect', {'path': str(h.pe), 'action': 'disassemble', 'start_vma': '0x0', 'stop_vma': '0x10001'}, None),
            ('missing-backend', 'source_symbols', {'path': str(h.source), 'language': 'C'}, {}),
        )
        for name, tool, query, config in bad_cases:
            result['cases'].append(h.case(name, lambda f, r, t=tool, a=query, c=config:
                h.query(f, t, a, config=c, expected='failed')))
        for mode in ('malformed', 'invalid-utf8', 'nonzero', 'quota', 'timeout', 'cancel', 'empty-version',
                     'snapshot-mutation', 'unfittable-record'):
            result['cases'].append(h.case('backend-' + mode, lambda f, r, m=mode: h.fault(f, r, m)))
        for registered in (True, False):
            result['cases'].append(h.case('plan-' + ('explicit-registration' if registered else 'default-denial'),
                lambda f, r, yes=registered: plan_case(h, f, r, yes)))
        result['cases'].append(h.case('plan-changed-backend', lambda f, r: plan_case(h, f, r, True, True)))
        result['source_pins_unchanged'] = source_before == pins()
        result['binary_unchanged'] = result['binary_sha256'] == digest(binary.read_bytes())
        result['samples_unchanged'] = h.source.read_bytes() == SAMPLE and all(
            digest((h.fixtures / name).read_bytes()) == value for name, value in h.fixture_pins.items())
        result['status'] = ('PASS' if all(row['status'] == 'PASS' for row in result['cases']) and
            result['source_pins_unchanged'] and result['binary_unchanged'] and result['samples_unchanged'] else 'FAIL')
    finally:
        result['elapsed_seconds'] = round(time.monotonic() - started, 3)
        write_json(root / 'result.json', result)
        print(json.dumps(result, ensure_ascii=False, indent=2))
    require(result['status'] == 'PASS', 'analysis acceptance failed; retain evidence, do not blindly replay')


if __name__ == '__main__':
    main()
