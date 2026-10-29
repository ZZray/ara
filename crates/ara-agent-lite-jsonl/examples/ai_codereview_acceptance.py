"""Fixed Host acceptance over actual Git display and revision-source pipelines.

Usage: PRODUCT_PYTHON -X utf8 -B ai_codereview_acceptance.py
       --workspace ABS --evidence FRESH_ABS --source-sha256 HEX

The Host launches this checker with its existing bounded_process and owns the
deadline, cancellation, minimal environment and receipt. No HTTP request is sent.
Top-level imports and constants are safe for the Host to import independently.
"""
from __future__ import annotations

import argparse
from collections import Counter
from contextlib import ExitStack
import hashlib
import json
import os
from pathlib import Path
import re
import subprocess
import sys
import time

VERSION = 1
CASE_IDS = (
    'valid_diff_text_preserved',
    'valid_source_context_preserved',
    'non_utf8_diff_wire_safe',
    'non_utf8_source_context_wire_safe',
    'raw_git_blobs_unchanged',
    'revision_source_stays_strict',
)
MAX_REPORT_BYTES = 16384
SOURCE_PATH = 'app/services/git_service.py'


def sha(raw):
    return hashlib.sha256(raw).hexdigest()


def file_sha(path):
    return sha(Path(path).read_bytes())


def save_new(path, value, *, limit=MAX_REPORT_BYTES):
    raw = (json.dumps(value, ensure_ascii=True, sort_keys=True) + '\n').encode('utf-8')
    if limit is not None and len(raw) > limit:
        raise ValueError('checker report exceeds fixed byte limit')
    with Path(path).open('xb') as stream:
        stream.write(raw)
        stream.flush()
        os.fsync(stream.fileno())


class SemanticMismatch(Exception):
    def __init__(self, message, **details):
        super().__init__(message)
        self.details = details


def require(condition, message, **details):
    if not condition:
        raise SemanticMismatch(message, **details)


def wire(value, httpx):
    encoded = json.dumps(value, ensure_ascii=False).encode('utf-8', 'strict')
    decoded = json.loads(encoded)
    request = httpx.Request('POST', 'http://127.0.0.1/unused', json=value)
    require(json.loads(request.content) == decoded, 'HTTP request encoding changed the JSON response')
    return {'json_utf8_sha256': sha(encoded), 'http_body_sha256': sha(request.content)}


def blob_snapshot(repo, revision, paths):
    result = {}
    for path in paths:
        blob = repo.commit(revision).tree[path]
        raw = blob.data_stream.read()
        result[path] = {'oid': blob.hexsha, 'sha256': sha(raw), 'bytes': len(raw)}
    return result


def make_fixture(root, contents, *, Repo, Actor):
    root.mkdir()
    repo = Repo.init(root / 'repo')
    with repo.config_writer() as writer:
        writer.set_value('core', 'autocrlf', 'false')
        writer.set_value('core', 'quotepath', 'false')
        writer.set_value('core', 'hooksPath', (root / 'unused-hooks').as_posix())
        writer.set_value('commit', 'gpgsign', 'false')
    # -text preserves blob CRLF bytes; diff forces NUL-containing .cpp files
    # through Git's text diff. No external diff driver or executable is selected.
    (root / 'repo/.gitattributes').write_bytes(b'*.cpp -text diff\n')
    actor = Actor('Host Semantic Fixture', 'host-semantic@example.invalid')
    dates = dict(author=actor, committer=actor, author_date='2000-01-01T00:00:00 +0000',
                 commit_date='2000-01-01T00:00:00 +0000')
    for path, (before, _) in contents.items():
        (root / 'repo' / path).write_bytes(before)
    repo.index.add(['.gitattributes', *contents])
    before = repo.index.commit('owned fixture baseline', **dates).hexsha
    for path, (_, after) in contents.items():
        (root / 'repo' / path).write_bytes(after)
    repo.index.add(list(contents))
    after = repo.index.commit('owned fixture changed text', **dates).hexsha
    snapshots = {revision: blob_snapshot(repo, revision, contents) for revision in (before, after)}
    # Fixture setup is independent of candidate behavior. A setup failure is
    # Unknown, never feedback that asks the model to change its source.
    for revision, offset in ((before, 0), (after, 1)):
        for path, values in contents.items():
            if snapshots[revision][path]['sha256'] != sha(values[offset]):
                raise ValueError('Git fixture altered its original blob bytes')
    save_new(root / 'fixture.json', {'before': before, 'after': after, 'snapshots': snapshots,
                                   'attributes': '*.cpp -text diff'})
    raw_diff = repo.git.diff(before, after, '--no-color', '--no-ext-diff', '-M', stdout_as_string=False)
    if not isinstance(raw_diff, bytes) or not raw_diff or len(raw_diff) > 20000:
        raise ValueError('Git raw diff fixture precondition failed')
    (root / 'raw-diff.bin').write_bytes(raw_diff)
    raw_show = {}
    for path in contents:
        raw_show[path] = repo.git.show(f'{after}:{path}', stdout_as_string=False)
        if not isinstance(raw_show[path], bytes):
            raise ValueError('Git raw show fixture did not produce bytes')
    return {'repo': repo, 'root': root, 'before': before, 'after': after,
            'contents': contents, 'snapshots': snapshots, 'raw_diff': raw_diff, 'raw_show': raw_show}


def prepare_fixtures(evidence, Repo, Actor):
    prefix = ''.join(f'// padding_{i:03d}_' + 'x' * 65 + '\n' for i in range(32))
    representatives = '\x01\t\x1b\x7f\u0085\ud7ff\ue000\ufdd0\uffff\U00010000\U0010ffff'
    valid_after = (prefix + '// 中文😀 e\u0301\u200d' + representatives + '\0tail\r\n'
                   + '// CRLF 中文😀\r\n// final valid line\n').encode('utf-8')
    if valid_after.index(b'\0') <= 2200:
        raise ValueError('NUL must be beyond the published source-context binary probe')
    valid = make_fixture(evidence / 'valid', {
        'valid.cpp': ((prefix + '// old valid line\n').encode(), valid_after)}, Repo=Repo, Actor=Actor)
    # A separate NUL-free fixture distinguishes invalid UTF-8 from binary-file
    # rejection by the strict revision-source reader.
    good = '// good 中文😀\r\nint known_value = 7;\r\n'.encode('utf-8')
    bad_before = b'// old \xa1\nint old_value = 0;\n'
    bad_after = ('// 中文😀\r\n'.encode('utf-8') + b'// invalid \xff\xa1\r\n'
                 + b'int first_value = 1;\nint last_value = 249;\n')
    invalid = make_fixture(evidence / 'non-utf8', {
        'bad.cpp': (bad_before, bad_after), 'good.cpp': (good, good)}, Repo=Repo, Actor=Actor)
    # Raw API outputs are the oracle, including GitPython's own newline behavior;
    # the checker never invents a trim rule or a source-context page header.
    valid['raw_diff'].decode('utf-8', 'strict')
    valid['raw_show']['valid.cpp'].decode('utf-8', 'strict')
    try:
        invalid['raw_diff'].decode('utf-8', 'strict')
    except UnicodeDecodeError:
        pass
    else:
        raise ValueError('non-UTF-8 raw diff fixture is not discriminating')
    for fixture in (valid, invalid):
        save_new(fixture['root'] / 'raw-shows.json', {
            path: {'sha256': sha(raw), 'bytes': len(raw)} for path, raw in fixture['raw_show'].items()})
    return valid, invalid


def evaluate(evidence, git_service, RevisionSourceReader, httpx, valid, invalid):
    cases = []

    def run(case_id, action):
        try:
            details = action()
        except (OSError, subprocess.TimeoutExpired):
            raise  # Environment/process faults stay Unknown.
        except Exception as error:
            cases.append({'case_id': case_id, 'status': 'FAIL', 'evidence': {},
                          'failures': [{'type': type(error).__name__, 'message': str(error)[:300],
                                        'details': getattr(error, 'details', {})}]})
        else:
            cases.append({'case_id': case_id, 'status': 'PASS', 'evidence': details, 'failures': []})

    def diff_case(fixture, expected):
        result = git_service.diff_for_range(fixture['repo'], fixture['before'], fixture['after'], max_chars=60000)
        require(isinstance(result.chunks, list) and result.chunks and all(isinstance(c, str) for c in result.chunks),
                'actual diff pipeline did not return text chunks')
        actual = ''.join(result.chunks)
        save_new(fixture['root'] / 'actual-diff.json', {'chunks': result.chunks, 'text': result.text})
        require(actual == expected, 'actual diff text differs from Git raw API display text',
                expected_sha256=sha(expected.encode()), actual_sha256=sha(actual.encode('utf-8', 'surrogatepass')),
                nul_expected=expected.count('\0'), nul_actual=actual.count('\0'))
        return {'raw_git_sha256': sha(fixture['raw_diff']), 'display_sha256': sha(actual.encode()),
                'chunks': len(result.chunks), **wire({'chunks': result.chunks, 'text': result.text}, httpx)}

    def context_case(fixture, path, expected):
        actual = git_service.read_paths(fixture['repo'], [path], max_total_chars=40000, per_file=20000, rev=fixture['after'])
        save_new(fixture['root'] / 'actual-context.json', {'context': actual})
        require(isinstance(actual, str), 'actual source-context pipeline did not return text')
        require(expected in actual, 'actual source context omitted or changed Git raw API body',
                expected_body_sha256=sha(expected.encode()), actual_sha256=sha(actual.encode('utf-8', 'surrogatepass')),
                nul_expected=expected.count('\0'), nul_actual=actual.count('\0'))
        return {'raw_git_sha256': sha(fixture['raw_show'][path]), 'expected_body_sha256': sha(expected.encode()),
                'body_offset': actual.index(expected), **wire({'context': actual}, httpx)}

    run(CASE_IDS[0], lambda: diff_case(valid, valid['raw_diff'].decode('utf-8')))
    run(CASE_IDS[1], lambda: context_case(valid, 'valid.cpp', valid['raw_show']['valid.cpp'].decode('utf-8')))
    run(CASE_IDS[2], lambda: diff_case(invalid, invalid['raw_diff'].decode('utf-8', 'replace')))
    run(CASE_IDS[3], lambda: context_case(invalid, 'bad.cpp', invalid['raw_show']['bad.cpp'].decode('utf-8', 'replace')))

    def unchanged():
        rows = []
        for fixture in (valid, invalid):
            repo, contents = fixture['repo'], fixture['contents']
            for revision in (fixture['before'], fixture['after']):
                actual = blob_snapshot(repo, revision, contents)
                require(actual == fixture['snapshots'][revision], 'actual pipeline changed original Git blobs')
                published = git_service.blob_oids(repo, revision, list(contents))
                require(published == {path: item['oid'] for path, item in actual.items()}, 'public blob receipt differs from actual Git object IDs')
                rows.append({'revision': revision, 'blobs': actual})
        return {'revisions': rows}
    run(CASE_IDS[4], unchanged)

    def strict_source():
        repo, revision = invalid['repo'], invalid['after']
        bad = RevisionSourceReader(repo, revision).gather({'file': 'bad.cpp', 'line_start': 1})
        good = RevisionSourceReader(repo, revision).gather({'file': 'good.cpp', 'line_start': 1})
        save_new(invalid['root'] / 'strict-source-packets.json', {'bad': bad, 'good': good})
        require(bad['revision'] == revision, 'strict reader did not pin requested revision')
        require('source_unavailable:bad.cpp' in bad['errors'] and not bad['text'] and not bad['lines'] and not bad['blobs'],
                'strict reader accepted undecodable source bytes', errors=bad['errors'])
        expected_lines = {i: line for i, line in enumerate(invalid['contents']['good.cpp'][1].decode('utf-8').splitlines(), 1)}
        require(good['revision'] == revision and not good['errors'], 'strict reader rejected valid UTF-8 source', errors=good['errors'])
        require(good['lines'] == {'good.cpp': expected_lines}, 'strict reader changed valid source lines')
        require(good['blobs'] == {'good.cpp': invalid['snapshots'][revision]['good.cpp']['oid']} and bool(good['text']),
                'strict reader did not bind valid source to its actual blob')
        return {'revision': revision, 'bad_errors': bad['errors'], 'good_line_count': len(expected_lines),
                'good_blobs': good['blobs'], **wire({'bad': bad, 'good': good}, httpx)}
    run(CASE_IDS[5], strict_source)
    return cases


def main(argv=None):
    parser = argparse.ArgumentParser()
    parser.add_argument('--workspace', type=Path, required=True)
    parser.add_argument('--evidence', type=Path, required=True)
    parser.add_argument('--source-sha256', required=True)
    args = parser.parse_args(argv)
    # A killed worker can leave no result. The Host treats every missing,
    # malformed, oversized or incomplete report as Unknown and never retries it.
    workspace = args.workspace.resolve(strict=True)
    evidence = args.evidence.resolve()
    if not args.workspace.is_absolute() or not args.evidence.is_absolute() or evidence.is_relative_to(workspace):
        raise ValueError('workspace/evidence must be absolute; evidence must be outside workspace')
    if evidence.exists() or not evidence.parent.is_dir():
        raise ValueError('evidence must be a fresh directory with an existing parent')
    if not re.fullmatch('[0-9a-f]{64}', args.source_sha256):
        raise ValueError('source-sha256 must be lowercase SHA256')
    evidence.mkdir()
    source = workspace / SOURCE_PATH
    report = {'version': VERSION, 'worker_pid': os.getpid(), 'workspace': str(workspace),
              'checker_sha256': file_sha(__file__), 'status': 'UNKNOWN', 'complete': False,
              'source': {'path': str(source), 'expected_sha256': args.source_sha256,
                         'before_sha256': None, 'after_sha256': None}, 'cases': [], 'error': None}
    started = time.monotonic()
    stage = 'source_identity'
    try:
        if not source.resolve(strict=True).is_relative_to(workspace) or source.stat().st_size > 2 * 1024 * 1024:
            raise ValueError('source escapes workspace or exceeds fixed source limit')
        report['source']['before_sha256'] = file_sha(source)
        save_new(evidence / 'started.json', {key: report[key] for key in ('version', 'worker_pid', 'workspace', 'checker_sha256', 'source')}, limit=MAX_REPORT_BYTES)
        if report['source']['before_sha256'] != args.source_sha256:
            raise ValueError('source identity differs from Host grant')
        stage = 'isolated_import'
        private = evidence / 'private'
        private.mkdir()
        os.environ.update(APP_DB_PATH=str(private / 'control.db'), APP_DATABASE_URL='',
                          APP_REPO_ROOT=str(private / 'repos'), APP_CRASH_ROOT=str(private / 'crashes'),
                          APP_AUTH_ENABLED='false', PYTHONDONTWRITEBYTECODE='1',
                          GIT_CONFIG_NOSYSTEM='1', GIT_CONFIG_GLOBAL=os.devnull, GIT_ATTR_NOSYSTEM='1',
                          GIT_TERMINAL_PROMPT='0', GCM_INTERACTIVE='never')
        sys.dont_write_bytecode = True
        sys.path.insert(0, str(workspace))
        import httpx
        from git import Repo, Actor
        from app.services import git_service, review_source_service
        if Path(git_service.__file__).resolve() != source.resolve():
            raise ValueError('candidate module was not loaded from the granted workspace')
        reader_source = workspace / 'app/services/review_source_service.py'
        if Path(review_source_service.__file__).resolve() != reader_source.resolve():
            raise ValueError('strict source reader was loaded outside the granted workspace')
        report['revision_reader_sha256'] = file_sha(reader_source)
        stage = 'fixture_setup'
        valid, invalid = prepare_fixtures(evidence, Repo, Actor)
        with ExitStack() as stack:
            stack.callback(valid['repo'].close)
            stack.callback(invalid['repo'].close)
            stage = 'case_execution'
            report['cases'] = evaluate(evidence, git_service, review_source_service.RevisionSourceReader, httpx, valid, invalid)
        stage = 'source_recheck'
        report['source']['after_sha256'] = file_sha(source)
        if report['source']['after_sha256'] != args.source_sha256:
            raise ValueError('source changed during semantic acceptance')
        if Counter(case['case_id'] for case in report['cases']) != Counter(CASE_IDS):
            raise ValueError('checker did not complete every fixed case exactly once')
        report['complete'] = True
        report['status'] = 'FAIL' if any(case['status'] != 'PASS' for case in report['cases']) else 'PASS'
    except BaseException as error:
        report.update(status='UNKNOWN', complete=False,
                      error={'stage': stage, 'type': type(error).__name__, 'message': str(error)[:300]})
        try:
            report['source']['after_sha256'] = file_sha(source)
        except OSError:
            pass
    report['elapsed_seconds'] = time.monotonic() - started
    save_new(evidence / 'result.json', report, limit=MAX_REPORT_BYTES)
    print(json.dumps({'status': report['status'], 'worker_pid': report['worker_pid'], 'result': str(evidence / 'result.json')}), flush=True)
    return {'PASS': 0, 'FAIL': 1, 'UNKNOWN': 2}[report['status']]


if __name__ == '__main__':
    try:
        exit_code = main()
    except (Exception, KeyboardInterrupt) as error:
        # Invalid startup paths or an unwritable/partial report cannot become a
        # completed semantic failure, even when no result can be committed.
        print('checker infrastructure failure: ' + type(error).__name__, file=sys.stderr, flush=True)
        exit_code = 2
    raise SystemExit(exit_code)
