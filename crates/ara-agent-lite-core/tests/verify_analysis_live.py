"""Fixed S1/S2/R1 paired capability trials. Preparation never calls a model.

--prepare creates original fixtures and validates independent oracles. --live
performs exactly ONE task/variant, once, with a previously built business_host.
Run base before new; preserve every failure. Private material is not placed in
model cwd or its parents. This is not an OS sandbox or an unreadability claim.
R1 uses Plan on both sides: base can read the identical hex bytes; new additionally
has static binary_inspect. No variant may execute the specimen. Six Runs provide
task-specific evidence, not statistical proof of general improvement.
"""
import argparse
import ast
import json
import os
from pathlib import Path
import random
import re
import shutil
import subprocess
import sys
import time
import urllib.parse

from verify_coding_live import Relay, digest, execution_feedback, inventory, require, scan_actual_token, write_json

PRIVATE = Path('D:/WorkSpace/tmp/ara-core-capability-20261022/analysis-private')
ROUTE = Path('D:/WorkSpace/tmp/ara-core-capability-20261022/live-coding/result.json')
TASKS = ('S1', 'S2', 'R1')
VARIANTS = ('base', 'new')
SOURCES = {
    'include/model.hpp': '''#pragma once
#include <cstdint>
#include <string>
#include <vector>
namespace ledger { struct Record { std::string id; std::int32_t value; }; }
''',
    'include/wire.hpp': '''#pragma once
#include <cstdint>
namespace wire { std::int32_t decode(std::uint32_t bits); }
''',
    'include/reader.hpp': '''#pragma once
#include "model.hpp"
#include <istream>
namespace ledger { std::vector<Record> read(std::istream& input); }
''',
    'include/options.hpp': '''#pragma once
namespace ledger { enum class Duplicate { All, First, Last };
struct Options { Duplicate duplicates = Duplicate::All; };
Options parse(int argc, char** argv); }
''',
    'include/report.hpp': '''#pragma once
#include "model.hpp"
#include "options.hpp"
namespace ledger { std::vector<Record> select(const std::vector<Record>& rows, Options options);
std::int64_t total(const std::vector<Record>& rows); }
''',
    'include/display.hpp': '''#pragma once
#include "model.hpp"
namespace display { std::string decode(const ledger::Record& row); }
''',
    'src/wire.cpp': '''#include "wire.hpp"
namespace wire {
std::int32_t decode(std::uint32_t bits) {
    return static_cast<std::int32_t>(bits & 0x7fffffu);
}
}
''',
    'src/reader.cpp': '''#include "reader.hpp"
#include "wire.hpp"
#include <sstream>
#include <stdexcept>
namespace ledger {
std::vector<Record> read(std::istream& input) {
    std::vector<Record> rows;
    std::string line;
    while (std::getline(input, line)) {
        if (line.empty()) continue;
        std::istringstream parser(line);
        std::string id, hex, extra;
        if (!(parser >> id >> hex) || (parser >> extra) || hex.size() != 6)
            throw std::runtime_error("record must contain id and six hex digits");
        std::size_t end = 0;
        auto bits = std::stoul(hex, &end, 16);
        if (end != hex.size() || bits > 0xffffffu) throw std::runtime_error("invalid 24-bit record");
        rows.push_back({id, wire::decode(static_cast<std::uint32_t>(bits))});
    }
    return rows;
}
}
''',
    'src/options.cpp': '''#include "options.hpp"
#include <stdexcept>
#include <string>
namespace ledger {
Options parse(int argc, char** argv) {
    Options options;
    for (int i = 1; i < argc; ++i) {
        std::string arg(argv[i]);
        if (arg == "--duplicates=all") options.duplicates = Duplicate::All;
        else throw std::runtime_error("unsupported duplicates option");
    }
    return options;
}
}
''',
    'src/report.cpp': '''#include "report.hpp"
namespace ledger {
std::vector<Record> select(const std::vector<Record>& rows, Options options) {
    (void) options;
    return rows;
}
std::int64_t total(const std::vector<Record>& rows) {
    std::int64_t sum = 0;
    for (const auto& row : rows) sum += row.value;
    return sum;
}
}
''',
    'src/display.cpp': '''#include "display.hpp"
namespace display {
std::string decode(const ledger::Record& row) { return row.id + "=" + std::to_string(row.value); }
}
''',
    'src/main.cpp': '''#include "reader.hpp"
#include "report.hpp"
#include "display.hpp"
#include <iostream>
#include <exception>
int main(int argc, char** argv) {
    try {
        auto options = ledger::parse(argc, argv);
        auto rows = ledger::select(ledger::read(std::cin), options);
        for (const auto& row : rows) std::cout << display::decode(row) << "\\n";
        std::cout << "TOTAL=" << ledger::total(rows) << "\\n";
        return 0;
    } catch (const std::exception& error) { std::cerr << error.what() << "\\n"; return 2; }
}
''',
}
FIXED_WIRE = '''#include "wire.hpp"
namespace wire {
std::int32_t decode(std::uint32_t bits) {
    bits &= 0xffffffu;
    return bits >= 0x800000u ? static_cast<std::int32_t>(bits) - 0x1000000 : static_cast<std::int32_t>(bits);
}
}
'''
FEATURE_OPTIONS = SOURCES['src/options.cpp'].replace(
    'else throw', 'else if (arg == "--duplicates=first") options.duplicates = Duplicate::First;\n'
    '        else if (arg == "--duplicates=last") options.duplicates = Duplicate::Last;\n        else throw')
FEATURE_REPORT = '''#include "report.hpp"
#include <unordered_map>
namespace ledger {
std::vector<Record> select(const std::vector<Record>& rows, Options options) {
    if (options.duplicates == Duplicate::All) return rows;
    std::vector<Record> result;
    std::unordered_map<std::string, std::size_t> positions;
    for (const auto& row : rows) {
        auto found = positions.find(row.id);
        if (found == positions.end()) { positions[row.id] = result.size(); result.push_back(row); }
        else if (options.duplicates == Duplicate::Last) result[found->second] = row;
    }
    return result;
}
std::int64_t total(const std::vector<Record>& rows) {
    std::int64_t sum = 0;
    for (const auto& row : rows) sum += row.value;
    return sum;
}
}
'''
COMPILE_SOURCES = ['src/main.cpp', 'src/wire.cpp', 'src/reader.cpp', 'src/options.cpp', 'src/report.cpp', 'src/display.cpp']
CONTRACT = '''# Ledger records
Build with: g++ -std=c++17 -Iinclude src/main.cpp src/wire.cpp src/reader.cpp src/options.cpp src/report.cpp src/display.cpp -o ledger.exe
Read stdin lines: an id without whitespace, then six hexadecimal digits encoding
a SIGNED two's-complement 24-bit value. Ignore empty lines. Emit each selected
record as id=decimal_value, then TOTAL=sum in signed 64-bit arithmetic, with
newline after every line. No arguments and --duplicates=all retain all rows.
Malformed input or invalid options must exit 2; successful processing exits 0.
Example input: alpha 000005\nExample output: alpha=5 then TOTAL=5, each on its own line.
Keep this contract, source data, public interfaces and unrelated behavior.
'''
FEATURE_CONTRACT = '''
Implement --duplicates=first and --duplicates=last. Both emit each distinct id
once, in order of its FIRST appearance. first selects its first value; last
selects its last value without moving its position. The existing all/default
mode must remain unchanged. Reject any other --duplicates value with exit 2.
'''
BINARY_C = r'''#include <stdint.h>
#include <stddef.h>
uint32_t analyze(const unsigned char* p, size_t n) {
    if (n < 6) return 0xe0000001u;
    if (p[0] != 0xa7) return 0xe0000002u;
    if (p[1] != 1 && p[1] != 2) return 0xe0000003u;
    uint32_t count = p[2] | ((uint32_t)p[3] << 8);
    if (n != 6 + count * 2) return 0xe0000004u;
    uint32_t key = p[4] | ((uint32_t)p[5] << 8);
    uint32_t state = 0x13579bdfu ^ key;
    for (uint32_t i = 0; i < count; ++i) {
        uint32_t value = (p[6+i*2] | ((uint32_t)p[7+i*2] << 8)) ^ key;
        int32_t signed_value = value >= 0x8000u ? (int32_t)value - 65536 : (int32_t)value;
        state += (uint32_t)signed_value;
        state = ((state << 5) | (state >> 27)) ^ (0x9e3779b9u + i);
    }
    return p[1] == 2 ? state ^ 0xa5a5a5a5u : state;
}
#ifdef VALIDATE
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
int main(int argc, char** argv) {
    if (argc != 2 || strlen(argv[1]) > 4096 || strlen(argv[1]) % 2) return 2;
    unsigned char bytes[2048];
    size_t size = strlen(argv[1])/2;
    for (size_t i=0; i<size; ++i) { char part[3]={argv[1][i*2],argv[1][i*2+1],0}; bytes[i]=(unsigned char)strtoul(part,0,16); }
    printf("%u\n", analyze(bytes,size)); return 0;
}
#endif
'''
PREDICTION_INPUTS = ['a701020034120100ffff', 'a702010000800080', 'a70100000000']


def run(command, cwd, data=None, timeout=20):
    """Only own compiler/fixture/candidate processes; never the stripped specimen."""
    environment = os.environ.copy()
    environment.pop('CAS_API_KEY', None)
    started = time.monotonic()
    try:
        value = subprocess.run([str(x) for x in command], cwd=cwd, input=data, env=environment,
                               capture_output=True, timeout=timeout)
        return {'code': value.returncode, 'stdout': value.stdout.decode('utf-8', errors='replace'),
                'stderr': value.stderr.decode('utf-8', errors='replace'),
                'seconds': round(time.monotonic() - started, 3)}
    except subprocess.TimeoutExpired:
        return {'code': None, 'stdout': '', 'stderr': 'own fixture/candidate process timeout',
                'seconds': round(time.monotonic() - started, 3)}


def tree(root, files):
    root.mkdir(parents=True, exist_ok=False)
    for name, content in files.items():
        target = root / name
        target.parent.mkdir(parents=True, exist_ok=True)
        target.write_text(content, encoding='utf-8', newline='\n')


def cpp_build(work, compiler, output):
    return run([compiler, '-std=c++17', '-O1', '-Iinclude', *COMPILE_SOURCES, '-o', output], work, timeout=60)


def selected(rows, mode):
    if mode == 'all':
        return rows
    result, positions = [], {}
    for name, value in rows:
        if name not in positions:
            positions[name] = len(result)
            result.append((name, value))
        elif mode == 'last':
            result[positions[name]] = (name, value)
    return result


def cpp_cases(task):
    rng = random.Random(0xA621)
    groups = [[], [('alpha', -1), ('beta', -8388608), ('alpha', 8388607)],
              [('中文', 0), ('中文', -17), ('z', 99), ('中文', 23)]]
    for _ in range(8):
        groups.append([(rng.choice(('red', 'blue', 'green', '黄')), rng.randrange(-8388608, 8388608))
                       for _ in range(rng.randrange(1, 19))])
    cases = []
    for rows in groups:
        for mode in (('all',) if task == 'S1' else ('all', 'first', 'last')):
            values = selected(rows, mode)
            cases.append({'args': [] if mode == 'all' else ['--duplicates=' + mode],
                'input': ''.join(f'{name} {value & 0xffffff:06x}\n' for name, value in rows),
                'expected': ''.join(f'{name}={value}\n' for name, value in values) +
                            f'TOTAL={sum(value for _, value in values)}\n', 'code': 0})
    cases += [{'args': ['--duplicates=invalid'], 'input': '', 'expected': None, 'code': 2},
              {'args': [], 'input': 'malformed\n', 'expected': None, 'code': 2}]
    return cases


def cpp_oracle(work, compiler, folder, cases):
    binary = folder / ('candidate.exe' if os.name == 'nt' else 'candidate')
    build = cpp_build(work, compiler, binary)
    outcomes = []
    if build['code'] == 0:
        for case in cases:
            observed = run([binary, *case['args']], work, case['input'].encode(), timeout=5)
            outcomes.append({'passed': observed['code'] == case['code'] and
                             (case['expected'] is None or observed['stdout'].replace('\r\n', '\n') == case['expected']),
                             'observed': observed})
    return {'status': 'PASS' if build['code'] == 0 and outcomes and all(x['passed'] for x in outcomes) else 'FAIL',
            'build': build, 'cases': outcomes}


def binary_reference(data):
    if len(data) < 6: return 0xE0000001
    if data[0] != 0xA7: return 0xE0000002
    if data[1] not in (1, 2): return 0xE0000003
    count = int.from_bytes(data[2:4], 'little')
    if len(data) != 6 + count * 2: return 0xE0000004
    key = int.from_bytes(data[4:6], 'little')
    state = 0x13579BDF ^ key
    for i in range(count):
        value = int.from_bytes(data[6+i*2:8+i*2], 'little') ^ key
        value = value - 65536 if value >= 32768 else value
        state = (state + value) & 0xFFFFFFFF
        state = (((state << 5) | (state >> 27)) ^ (0x9E3779B9 + i)) & 0xFFFFFFFF
    return state ^ (0xA5A5A5A5 if data[1] == 2 else 0)


def binary_cases():
    rng = random.Random(0xB621)
    values = ['', 'a7', '000100000000', 'a70300000000', 'a70101000000', *PREDICTION_INPUTS]
    for version in (1, 2):
        for count in (0, 1, 2, 7, 31):
            key = rng.randrange(65536)
            blob = bytes((0xA7, version)) + count.to_bytes(2, 'little') + key.to_bytes(2, 'little')
            blob += bytes(rng.randrange(256) for _ in range(count * 2))
            values += [blob.hex(), (blob + b'\x00').hex()]
    return [{'hex': value, 'expected': binary_reference(bytes.fromhex(value))} for value in values]


def pins():
    core = Path(__file__).resolve().parents[1]
    paths = list((core / 'src').rglob('*')) + [core / 'Cargo.toml', core / 'Cargo.lock',
            core / 'examples/business_host.rs', Path(__file__), Path(__file__).with_name('verify_coding_live.py')]
    return {str(p.relative_to(core)): digest(p.read_bytes()) for p in sorted(paths) if p.is_file()}


def prepare(root, private, compiler, cc):
    root.mkdir(parents=True, exist_ok=False)
    private.mkdir(parents=True, exist_ok=False)
    require(private != root and private not in root.parents and root not in private.parents,
            'private oracle and public root must be separate, non-nested directories')
    plan = {'schema': 'ara-analysis-paired-v1', 'status': 'PREPARED', 'task_order': list(TASKS),
            'variant_order': list(VARIANTS), 'max_runs': 6, 'fixtures': {},
            'scope': 'fixed tasks; same model; no sandbox; hidden material not exposed, not proven unreadable',
            'harness_sha256': digest(Path(__file__).read_bytes())}
    for task in ('S1', 'S2'):
        files = dict(SOURCES)
        files['README.md'] = CONTRACT + (FEATURE_CONTRACT if task == 'S2' else '\nRepair incorrect signed-record handling; verify the actual program.\n')
        if task == 'S2': files['src/wire.cpp'] = FIXED_WIRE
        seed = private / (task + '-seed')
        tree(seed, files)
        cases = cpp_cases(task)
        write_json(private / (task + '-oracle.json'), cases)
        before_dir = private / (task + '-before'); before_dir.mkdir()
        before = cpp_oracle(seed, compiler, before_dir, cases)
        require(before['build']['code'] == 0, task + ' original fixture did not compile')
        require(before['status'] == 'FAIL', task + ' original must fail its independent oracle')
        reference = dict(files); reference['src/wire.cpp'] = FIXED_WIRE
        if task == 'S2': reference.update({'src/options.cpp': FEATURE_OPTIONS, 'src/report.cpp': FEATURE_REPORT})
        reference_dir = private / (task + '-reference'); tree(reference_dir, reference)
        after_dir = private / (task + '-reference-check'); after_dir.mkdir()
        after = cpp_oracle(reference_dir, compiler, after_dir, cases)
        require(after['status'] == 'PASS', task + ' oracle rejected the original reference implementation')
        write_json(private / (task + '-oracle-selftest.json'), {'before': before, 'reference': after})
        for variant in VARIANTS: tree(root / task / variant / 'project', files)
        plan['fixtures'][task] = {'public_pins': inventory(root / task / 'base' / 'project'),
                                  'oracle_sha256': digest((private / (task + '-oracle.json')).read_bytes())}
    source = private / 'specimen.c'; source.write_text(BINARY_C, encoding='utf-8', newline='\n')
    sample = private / ('specimen.exe' if os.name == 'nt' else 'specimen.elf')
    flags = ['-O1', '-g0', '-nostdlib', '-fno-stack-protector', '-Wl,-e,analyze', '-s']
    if os.name != 'nt': flags += ['-fno-pie', '-no-pie']
    build = run([cc, *flags, source, '-o', sample], private, timeout=60)
    require(build['code'] == 0 and sample.is_file(), 'own stripped fixture compile failed: ' + build['stderr'])
    validator = private / ('reference-validator.exe' if os.name == 'nt' else 'reference-validator')
    validation_build = run([cc, '-O1', '-DVALIDATE', source, '-o', validator], private, timeout=60)
    require(validation_build['code'] == 0, 'own independent C oracle build failed')
    cases = binary_cases()
    validation = [run([validator, item['hex']], private, timeout=5) for item in cases]
    require(all(row['code'] == 0 and row['stdout'].strip() == str(item['expected'])
                for row, item in zip(validation, cases)), 'C implementation and independent Python oracle disagree')
    write_json(private / 'R1-oracle.json', cases)
    write_json(private / 'R1-build.json', {'specimen': build, 'validation_build': validation_build,
                                        'validation': validation, 'source_sha256': digest(source.read_bytes())})
    binary_name = sample.name
    description = ('# Static reconstruction task\nThe supplied ' + binary_name + ' is an original stripped x64 specimen.\n'
        'Its entry address is the function to reconstruct: uint32_t analyze(const unsigned char*, size_t).\n'
        'Calling convention follows the platform ABI. Do NOT execute the specimen or attach to a process.\n'
        'sample.hex contains exactly the same bytes, one hex-encoded file, for either tool variant.\n'
        'Recover all buffer validation and result computation. Return one complete fenced python block\n'
        'defining recover(data: bytes) -> int, without external dependencies. Also return one fenced json\n'
        'object {"predictions":[...]} for these hex inputs, in order: ' + json.dumps(PREDICTION_INPUTS) + '.\n'
        'Explain architecture, link-time entry address, key branches and arithmetic with instruction addresses.\n'
        'No expected outputs are available during this Run. Do not modify the supplied files.\n')
    for variant in VARIANTS:
        work = root / 'R1' / variant / 'project'; tree(work, {'README.md': description, 'sample.hex': sample.read_bytes().hex() + '\n'})
        shutil.copyfile(sample, work / binary_name)
    plan['fixtures']['R1'] = {'public_pins': inventory(root / 'R1' / 'base' / 'project'),
        'oracle_sha256': digest((private / 'R1-oracle.json').read_bytes()), 'binary_name': binary_name,
        'sample_sha256': digest(sample.read_bytes()), 'predictions': PREDICTION_INPUTS}
    write_json(private / 'private-plan.json', plan)
    write_json(root / 'plan.json', plan)
    return {'status': 'PREPARED', 'tasks': list(TASKS), 'model_calls': 0,
            'oracle_selftests': 'original rejected; reference accepted; C/Python reverse oracle agree'}


def arguments(event):
    value = event.get('detail', {})
    if isinstance(value, str):
        try: return json.loads(value)
        except ValueError: return {}
    return value if isinstance(value, dict) else {}


def new_tool_evidence(task, events, final_text, work, sample_hash):
    starts = {e.get('step_id'): e for e in events if e.get('type') == 'tool_start'}
    completed_reads = {e.get('step_id'): e for e in events if e.get('type') == 'tool_end'
                       and e.get('title') == 'read_file' and e.get('status') == 'completed'
                       and isinstance(e.get('output'), str) and e['output'].strip()}
    receipts, navigated, addresses = [], False, set()
    for index, end in enumerate(events):
        tool = 'binary_inspect' if task == 'R1' else 'source_symbols'
        if end.get('type') != 'tool_end' or end.get('title') != tool or end.get('status') != 'completed': continue
        start = starts.get(end.get('step_id'))
        if not start or start.get('title') != tool: continue
        try:
            value = json.loads(end.get('output', ''))
            require(value.get('schema') == 'ara.analysis.v1', 'analysis receipt schema differs')
            item = value['input']; snapshot = Path(item['snapshot_path'])
            require(snapshot.is_file() and digest(snapshot.read_bytes()) == item['sha256'], 'analysis snapshot hash differs')
            if task == 'R1':
                require(item['sha256'] == sample_hash, 'analysis used another specimen')
                if value.get('action') == 'disassemble':
                    lines = value.get('lines', [])
                    for line in lines:
                        match = re.match(r'\s*([0-9a-fA-F]+):', line if isinstance(line, str) else '')
                        if match: addresses.add(int(match.group(1), 16))
            else:
                original = Path(item['path'])
                if not original.is_absolute(): original = work / original
                symbol_lines = {row['line'] for row in value.get('symbols', []) if isinstance(row.get('line'), int)}
                for later in events[index + 1:]:
                    if later.get('type') != 'tool_start' or later.get('title') != 'read_file': continue
                    read_end = completed_reads.get(later.get('step_id'))
                    if not read_end: continue
                    args = arguments(later); path = Path(args.get('path', ''))
                    if not path.is_absolute(): path = work / path
                    if path.is_file() and original.is_file() and path.samefile(original):
                        # Accept bytes actually returned from the read, never a
                        # requested range or a failed/empty tool_start alone.
                        source_text = snapshot.read_text(encoding='utf-8')
                        source_lines = source_text.splitlines()
                        observed = read_end['output']
                        if not any(k in args for k in ('offset', 'limit', 'tail', 'snapshot')):
                            navigated |= observed == source_text and any(
                                1 <= line <= len(source_lines) for line in symbol_lines)
                        else:
                            try: read_value = json.loads(observed)
                            except ValueError: continue
                            if args.get('snapshot'):
                                navigated |= read_value.get('sha256') == item['sha256'] and \
                                    read_value.get('content') == source_text and bool(symbol_lines)
                            elif not args.get('tail'):
                                navigated |= any(isinstance(row, dict) and row.get('line') in symbol_lines
                                    and type(row.get('line')) is int and 1 <= row['line'] <= len(source_lines)
                                    and not row.get('truncated') and row.get('text') == source_lines[row['line'] - 1]
                                    for row in read_value.get('lines', []))
            receipts.append({'step_id': end['step_id'], 'input_sha256': item['sha256'],
                             'receipt_sha256': digest(end['output'].encode()), 'action': value.get('action')})
        except (KeyError, TypeError, ValueError, OSError, AssertionError):
            continue
    cited = {int(value, 16) for value in re.findall(r'0x([0-9a-fA-F]+)', final_text)}
    return {'verified': bool(receipts) and (bool(addresses & cited) if task == 'R1' else navigated),
            'receipts': receipts, 'symbol_then_source_read': navigated,
            'disassembly_address_cited': bool(addresses & cited),
            'limit': 'mechanism use and artifact identity; semantic explanation still needs independent review'}


def reverse_oracle(text, folder, cases):
    blocks = re.findall(r'```python\s*\n(.*?)```', text, flags=re.DOTALL | re.IGNORECASE)
    predictions = re.findall(r'```json\s*\n(.*?)```', text, flags=re.DOTALL | re.IGNORECASE)
    require(len(blocks) == 1 and len(predictions) == 1, 'R1 requires one complete python and one predictions JSON block')
    code = blocks[0]; ast.parse(code)
    artifact = folder / 'recovered.py'; artifact.write_text(code, encoding='utf-8', newline='\n')
    # Seal both deliverables before supplying hidden inputs. This is a fresh
    # evaluator process, not a security sandbox for generated Python code.
    predicted = json.loads(predictions[0]).get('predictions')
    sealed = {'code_sha256': digest(artifact.read_bytes()), 'predictions': predicted}
    write_json(folder / 'sealed-submission.json', sealed)
    seal_sha256 = digest((folder / 'sealed-submission.json').read_bytes())
    expected_predictions = [binary_reference(bytes.fromhex(value)) for value in PREDICTION_INPUTS]
    worker = ('import json,sys,runpy\nns=runpy.run_path(sys.argv[1])\nfn=ns["recover"]\n'
              'print(json.dumps([fn(bytes.fromhex(x)) for x in json.load(sys.stdin)]))\n')
    observed = run([sys.executable, '-I', '-B', '-c', worker, artifact], folder,
                   json.dumps([item['hex'] for item in cases]).encode(), timeout=8)
    require(digest(artifact.read_bytes()) == sealed['code_sha256'], 'R1 candidate changed its sealed source during evaluation')
    require(digest((folder / 'sealed-submission.json').read_bytes()) == seal_sha256, 'R1 submission seal changed during evaluation')
    try: actual = json.loads(observed['stdout'])
    except ValueError: actual = None
    expected = [item['expected'] for item in cases]
    matched = isinstance(actual, list) and len(actual) == len(expected) and all(
        type(a) is int and a == b for a, b in zip(actual, expected))
    predictions_ok = isinstance(predicted, list) and len(predicted) == len(expected_predictions) and all(
        type(a) is int and a == b for a, b in zip(predicted, expected_predictions))
    return {'status': 'PASS' if observed['code'] == 0 and matched and predictions_ok else 'FAIL',
            'sealed_submission': sealed, 'hidden_case_count': len(cases), 'predictions_correct': predictions_ok,
            'hidden_outputs_correct': matched, 'execution': observed}


def route_from(path):
    value = json.loads(path.read_text(encoding='utf-8-sig'))
    route = value.get('route', value)
    endpoint, model = route.get('endpoint', ''), route.get('model', '')
    parsed = urllib.parse.urlsplit(endpoint)
    require(endpoint and model and not any((parsed.username, parsed.password, parsed.query, parsed.fragment)), 'invalid explicit route')
    require(parsed.scheme == 'https' or (parsed.scheme == 'http' and parsed.hostname in ('127.0.0.1', '::1')), 'route must be HTTPS or literal loopback')
    require(route.get('protocol', 'openai-completions') == 'openai-completions', 'unsupported route protocol')
    return endpoint, model


def live(args):
    root, private = args.output.resolve(), args.private.resolve()
    plan = json.loads((root / 'plan.json').read_text(encoding='utf-8'))
    require(plan == json.loads((private / 'private-plan.json').read_text(encoding='utf-8')), 'public/private fixed plans differ')
    require(plan['harness_sha256'] == digest(Path(__file__).read_bytes()), 'harness changed after fixed plan preparation')
    task, variant = args.task, args.variant
    folder, work = root / task / variant, root / task / variant / 'project'
    require(inventory(work) == plan['fixtures'][task]['public_pins'], 'initial model project differs from frozen fixture')
    require(private != work and private not in work.parents and work not in private.parents, 'oracle location overlaps model cwd/parents')
    if variant == 'new': require((root / task / 'base' / 'result.json').is_file(), 'base must finish before new, including failed base')
    binary = args.binary.resolve(); require(binary.is_file(), 'supply an existing business_host binary')
    endpoint, model = route_from(args.route_result)
    token = os.environ.get('CAS_API_KEY', ''); require(token, 'CAS_API_KEY is required for --live')
    max_calls, seconds = (28, 720) if task == 'R1' else (24, 600)
    frozen = pins(); binary_hash = digest(binary.read_bytes())
    if variant == 'new':
        base = json.loads((root / task / 'base' / 'result.json').read_text(encoding='utf-8'))
        require(base.get('model') == model and base.get('binary_sha256') == binary_hash and base.get('source_pins') == frozen
                and base.get('route_sha256') == digest(endpoint.encode()), 'pair model, route, binary or source snapshot changed')
    cases_path = private / (task + '-oracle.json')
    require(digest(cases_path.read_bytes()) == plan['fixtures'][task]['oracle_sha256'], 'hidden oracle changed')
    cases = json.loads(cases_path.read_text(encoding='utf-8'))
    analysis = None
    if variant == 'new':
        artifact_root = folder / 'analysis-artifacts'
        analysis = {'artifact_root': str(artifact_root)}
        for name in ('ctags', 'objdump', 'nm'):
            value = getattr(args, name) or shutil.which(name)
            require(value and Path(value).is_file(), 'required analysis backend unavailable: ' + name)
            analysis[name] = str(Path(value).resolve())
        artifact_root.mkdir(exist_ok=False)
    # Never restart a failed/unknown operation under the same evidence identity.
    marker = folder / 'STARTED'; marker.open('x').close()
    relay = Relay(endpoint, token, model, folder, max_calls=max_calls)
    objective = ('Read README.md and complete its task. Inspect the real files and cite exact files/lines. '
                 'Use actual build/test feedback before finishing; keep README.md unchanged.') if task != 'R1' else (
                 'Read README.md and reconstruct the supplied specimen using static evidence only. '
                 'Do not execute it. Return the required complete Python and JSON blocks, plus instruction-address evidence.')
    request = {'id': 'analysis-' + task + '-' + variant, 'objective': objective, 'cwd': str(work),
        'session_path': str(folder / 'native-session.json'), 'environment_state': str(folder / 'environment-state'),
        'endpoint': relay.local_endpoint, 'model': model, 'token_env': 'CAS_API_KEY',
        'mode': 'plan' if task == 'R1' else 'execute', 'enable_local_tools': True,
        'max_calls': max_calls, 'max_seconds': seconds, 'max_output': 4096}
    if analysis is not None: request['analysis_config'] = analysis
    write_json(folder / 'request.json', request)
    result = {'schema': plan['schema'], 'task': task, 'variant': variant, 'status': 'FAIL',
        'model': model, 'protocol': 'openai-completions', 'route_sha256': digest(endpoint.encode()),
        'has_token': True, 'binary_path': str(binary), 'binary_sha256': binary_hash, 'source_pins': frozen,
        'budget': {'max_calls': max_calls, 'max_seconds': seconds, 'max_output': 4096},
        'limits': ['no OS sandbox; hidden material not exposed, not proven unreadable',
                   'R1 both Plan; base has identical hex bytes; new adds permitted static tools',
                   'relay socket 45s and response wall check 50s; same on both variants',
                   'one fixed Run per variant, no statistical improvement claim'], 'effects_unknown': True}
    started = time.monotonic(); code, stdout, stderr = None, b'', b''
    try:
        try:
            process = subprocess.run([str(binary), str(folder / 'request.json')], cwd=work,
                                     capture_output=True, timeout=seconds + 25)
            code, stdout, stderr = process.returncode, process.stdout, process.stderr
        except subprocess.TimeoutExpired as error:
            stdout, stderr = error.stdout or b'', error.stderr or b''
            result['effects_unknown'] = True
        finally: relay.close()
        (folder / 'host.stdout.native.jsonl').write_bytes(stdout)
        (folder / 'host.stderr.native.txt').write_bytes(stderr)
        packets = [json.loads(line) for line in stdout.decode('utf-8').splitlines() if line.strip()]
        events = [item['event'] for item in packets if item.get('kind') == 'event']
        final = next((item for item in reversed(packets) if item.get('kind') == 'result'), None)
        result['effects_unknown'] = code is None or final is None
        result.update(host_exit=code, final_result=final, usage_by_call=[item['usage'] for item in relay.calls],
                      calls=relay.calls, relay_errors=relay.errors,
                      model_capabilities=[e for e in events if e.get('type') == 'model_capabilities'])
        require(code is not None and final is not None, 'Host outcome unknown; retain evidence and do not replay')
        require(0 < len(relay.calls) <= max_calls and len(relay.calls) == sum(e.get('type') == 'model_call' for e in events), 'model call identity/budget differs')
        require(not relay.errors, 'provider relay failed')
        if task == 'R1':
            forbidden = [e for e in events if e.get('type') == 'tool_start' and e.get('title') in ('exec', 'run_script', 'write_file', 'edit_file')]
            require(not forbidden, 'R1 attempted a mutating/execution tool in Plan; trial invalid')
            require(inventory(work) == plan['fixtures'][task]['public_pins'], 'read-only specimen fixture changed')
            oracle = reverse_oracle(final.get('text', ''), folder, cases)
        else:
            inventory(work)
            require((work / 'README.md').read_bytes() == (private / (task + '-seed') / 'README.md').read_bytes(), 'project contract changed')
            oracle = cpp_oracle(work, args.cxx, folder, cases)
            inventory(work)
            require((work / 'README.md').read_bytes() == (private / (task + '-seed') / 'README.md').read_bytes(), 'candidate changed contract during oracle')
        write_json(folder / 'after-oracle.json', oracle, token)
        result['oracle_status'] = oracle['status']
        feedback = execution_feedback(events)
        result['execution_feedback'] = feedback
        evidence = new_tool_evidence(task, events, final.get('text', ''), work, plan['fixtures'].get('R1', {}).get('sample_sha256'))
        result['new_tool_evidence'] = evidence
        result['task_success'] = code == 0 and final.get('code') == 0 and oracle['status'] == 'PASS'
        feedback_ok = task == 'R1' or feedback['verified']
        result['status'] = 'PASS' if result['task_success'] and feedback_ok and (variant == 'base' or evidence['verified']) else 'FAIL'
    except Exception as error:
        result['error'] = (type(error).__name__ + ': ' + str(error)).replace(token, '[REDACTED]')
    finally:
        write_json(folder / 'provider-receipts.json', {'calls': relay.calls, 'catalogs': relay.catalogs,
                                                     'errors': relay.errors}, token)
        result['elapsed_seconds'] = round(time.monotonic() - started, 3)
        result['source_pins_unchanged'] = frozen == pins()
        result['binary_unchanged'] = binary_hash == digest(binary.read_bytes())
        result['secret_scan'] = scan_actual_token(folder, token)
        result['raw_evidence_private'] = True
        if not result['source_pins_unchanged'] or not result['binary_unchanged'] or result['secret_scan']['status'] != 'CLEAN': result['status'] = 'FAIL'
        write_json(folder / 'result.json', result, token)
    return {key: result.get(key) for key in ('task', 'variant', 'status', 'task_success', 'oracle_status', 'has_token', 'elapsed_seconds', 'effects_unknown')}


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    modes = parser.add_mutually_exclusive_group(required=True)
    modes.add_argument('--prepare', action='store_true'); modes.add_argument('--live', action='store_true')
    parser.add_argument('--output', type=Path, required=True)
    parser.add_argument('--private', type=Path, default=PRIVATE)
    parser.add_argument('--task', choices=TASKS); parser.add_argument('--variant', choices=VARIANTS)
    parser.add_argument('--binary', type=Path)
    parser.add_argument('--route-result', type=Path, default=ROUTE)
    parser.add_argument('--cxx', default=shutil.which('g++'))
    parser.add_argument('--cc', default=shutil.which('gcc'))
    for name in ('ctags', 'objdump', 'nm'): parser.add_argument('--' + name)
    args = parser.parse_args()
    if args.prepare:
        require(args.cxx and args.cc, 'own fixture preparation needs explicit available g++ and gcc')
        result = prepare(args.output.resolve(), args.private.resolve(), args.cxx, args.cc)
    else:
        require(args.task and args.variant and args.binary and args.cxx, '--live needs --task --variant --binary and g++')
        result = live(args)
    print(json.dumps(result, ensure_ascii=False, indent=2))
    require(result['status'] in ('PASS', 'PREPARED'), 'fixed trial failed; preserve evidence, do not replay')


if __name__ == '__main__':
    main()
