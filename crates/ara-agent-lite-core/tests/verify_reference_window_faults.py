"""Public business_host reference-window faults; real processes, controlled SSE."""
from __future__ import annotations

import argparse
import asyncio
import json
import os
from pathlib import Path
from types import SimpleNamespace
import time
import traceback

import verify_reference_windows as windows


CASES = ('mid_scalar_offset', 'limit_zero', 'limit_over_max', 'limit_non_integer',
         'eof_empty_window', 'superseded_source', 'revoked_source')
RETIRED_MARKER = 'retired-source-identity-Q91'


def tool_message(request, name):
    names = {item['id']: item['function']['name']
             for message in request['messages'] for item in message.get('tool_calls', [])}
    for message in reversed(request['messages']):
        if message.get('role') == 'tool' and names.get(message['tool_call_id']) == name:
            return message
    raise AssertionError('actual next HTTP request lacks native tool receipt: ' + name)


def receipt_text(receipt):
    return '\n'.join(item.get('text', '') for item in receipt['content'] if isinstance(item, dict))


class FaultFixture(windows.Fixture):
    def __init__(self, args, case):
        super().__init__(SimpleNamespace(binary=args.binary, evidence=str(Path(args.evidence) / case), expect='windows'))
        self.case, self.received, self.provider_errors = case, {}, []
        windows.require(windows.sha(self.binary.read_bytes()) == args.expected_binary_sha256.lower(),
                        'business_host does not match the explicitly frozen binary hash')
        windows.save(self.root / 'fault-identity.json', {
            'case': case, 'script_sha256': windows.sha(Path(__file__).read_bytes()),
            'helper_sha256': windows.sha(Path(windows.__file__).read_bytes()),
            'expected_binary_sha256': args.expected_binary_sha256.lower(),
            'fixture_type': 'actual public Core API and business_host; deterministic provider only'})

    async def invoke(self, endpoint, phase, objective, calls):
        """Keep raw evidence and terminate/join the exact process if it times out."""
        config = {'id': phase + '-run', 'objective': objective, 'cwd': str(self.root),
            'session_path': str(self.session), 'environment_state': str(self.root / 'state/environment'),
            'mode': 'execute', 'endpoint': endpoint, 'model': 'jsonl-fixture',
            'token_env': 'ARA_AGENT_MODEL_TOKEN', 'enable_local_tools': False,
            'max_calls': calls, 'max_seconds': 30, 'max_output': 512}
        request = self.root / (phase + '-request.json')
        windows.save(request, config)
        options = {'creationflags': 0x08000000} if os.name == 'nt' else {}
        started = time.monotonic()
        process = await asyncio.create_subprocess_exec(str(self.binary), str(request),
            stdout=asyncio.subprocess.PIPE, stderr=asyncio.subprocess.PIPE,
            env={**os.environ, 'ARA_AGENT_MODEL_TOKEN': windows.TOKEN}, **options)
        communication = asyncio.create_task(process.communicate())
        timed_out = False
        try:
            stdout, stderr = await asyncio.wait_for(asyncio.shield(communication), timeout=45)
        except TimeoutError:
            timed_out = True
            if process.returncode is None:
                process.kill()
            stdout, stderr = await asyncio.wait_for(communication, timeout=5)
        finally:
            if process.returncode is None:
                process.kill()
                await process.wait()
        (self.root / (phase + '-stdout.jsonl')).write_bytes(stdout)
        (self.root / (phase + '-stderr.bin')).write_bytes(stderr)
        frames = [json.loads(row) for row in stdout.splitlines()]
        results = [row for row in frames if row['kind'] == 'result']
        events = [row['event'] for row in frames if row['kind'] == 'event']
        record = {'phase': phase, 'pid': process.pid, 'exit_code': process.returncode,
            'timed_out': timed_out, 'elapsed_seconds': time.monotonic() - started,
            'result': results[0] if len(results) == 1 else None,
            'reserved_calls': sum(row['type'] == 'model_call' for row in events),
            'stdout_sha256': windows.sha(stdout), 'stderr_sha256': windows.sha(stderr)}
        self.records.append(record)
        windows.save(self.root / (phase + '-process.json'), record)
        windows.require(not timed_out and len(results) == 1, 'native process timed out or did not produce one result')
        return record, events

    def safe_reply(self, callback):
        def reply(body):
            try:
                return callback(body)
            except Exception:
                self.provider_errors.append(traceback.format_exc())
                return windows.text('Controlled fixture assertion failed; inspect preserved receipts.')
        return reply

    def read_request(self, body):
        tool = next(row['function'] for row in body['tools'] if row['function']['name'] == 'reference_read')
        properties = tool['parameters']['properties']
        windows.require(properties['offset']['type'] == properties['limit']['type'] == 'integer',
                        'actual provider request did not advertise the new integer window schema')
        windows.require(properties['limit']['minimum'] == 1 and properties['limit']['maximum'] == 16384,
                        'actual provider request lost explicit page bounds')
        args = {'id': self.target['id']}
        if self.case == 'mid_scalar_offset':
            args.update(offset=1, limit=64)
        elif self.case == 'limit_zero':
            args['limit'] = 0
        elif self.case == 'limit_over_max':
            args['limit'] = 16385
        elif self.case == 'limit_non_integer':
            args['limit'] = 1.5
        elif self.case == 'eof_empty_window':
            args.update(offset=len(self.source.encode()), limit=256)
        else:
            args['limit'] = 64
        self.received['read_arguments'] = args
        self.received['advertised_schema'] = tool
        return windows.call('fault-read', 'reference_read', args)

    def after_read(self, body):
        self.received['read_http_receipt'] = tool_message(body, 'reference_read')
        if self.case in ('superseded_source', 'revoked_source'):
            query = RETIRED_MARKER if self.case == 'superseded_source' else windows.KEY
            self.received['search_query'] = query
            return windows.call('retired-search', 'reference_search', {'query': query})
        return windows.text('Controlled terminal after the actual native read receipt; no business acceptance claim.')

    def after_search(self, body):
        self.received['search_http_receipt'] = tool_message(body, 'reference_search')
        self.received['search_value'] = windows.actual_tool_value(body, 'reference_search')
        return windows.text('Controlled terminal after the actual search receipt; no business acceptance claim.')

    async def manage(self, provider):
        seed, _ = await self.invoke(provider.endpoint, 'seed',
            'Register original source ' + RETIRED_MARKER + '; a correction follows through the public management API.', 1)
        windows.require(seed['exit_code'] == seed['result']['code'] == 0, 'public seed failed')
        old = json.loads(self.context.read_bytes())['references'][0]
        raw_before = json.loads(self.session.read_bytes())['messages']
        corrected, _ = await self.invoke(provider.endpoint, 'correct',
            '/reference correct ' + old['id'] + ' ' + self.source, 1)
        windows.require(corrected['exit_code'] == corrected['result']['code'] == 0, 'public correction failed')
        windows.require(corrected['reserved_calls'] == 0, 'management correction unexpectedly called a model')
        raw_after = json.loads(self.session.read_bytes())['messages']
        windows.require(raw_before == raw_after, 'management correction fabricated a raw transcript turn')
        windows.require(windows.KEY not in json.dumps(raw_after), 'corrected document already leaked into native history')
        refs = json.loads(self.context.read_bytes())['references']
        current = next(row for row in refs if row['status'] == 'active')
        superseded = next(row for row in refs if row['id'] == old['id'])
        windows.require(superseded['status'] == 'superseded' and current['supersedes'] == old['id']
                        and current['version'] == 2, 'correction lost source provenance')
        windows.require(current['text'] == self.source and current['hash'] == self.source_hash,
                        'public correction changed the original source')
        self.target = superseded if self.case == 'superseded_source' else current
        if self.case == 'revoked_source':
            revoked, _ = await self.invoke(provider.endpoint, 'revoke', '/reference revoke ' + current['id'], 1)
            windows.require(revoked['exit_code'] == revoked['result']['code'] == 0
                            and revoked['reserved_calls'] == 0, 'public revocation failed or used a model')
            windows.require(json.loads(self.session.read_bytes())['messages'] == raw_after,
                            'revocation fabricated a native history turn')
            self.target = next(row for row in json.loads(self.context.read_bytes())['references']
                               if row['id'] == current['id'])
            windows.require(self.target['status'] == 'revoked', 'public revocation did not persist')
        self.before = json.loads(self.context.read_bytes())
        windows.save(self.root / 'context-before-read.json', self.before)
        windows.save(self.root / 'management-proof.json', {'target': self.target,
            'source_sha256': self.source_hash, 'native_messages_unchanged': True,
            'native_history_contains_corrected_document': False})

    def verify(self, record, events, provider):
        windows.require(not self.provider_errors, 'provider callback failed: ' + repr(self.provider_errors))
        windows.require(record['exit_code'] == record['result']['code'] == 0,
                        'controlled terminal did not settle after the tool receipt')
        expected_calls = 3 if self.case in ('superseded_source', 'revoked_source') else 2
        windows.require(record['reserved_calls'] == expected_calls and len(provider.requests) == expected_calls + 1,
                        'fault case used an unexpected call count or management made HTTP requests')
        windows.require(not any(row['type'] == 'model_call' and row.get('phase') == 'summary' for row in events),
                        'unexpected summary confounds the fault check')
        session = json.loads(self.session.read_bytes())
        receipts = [row for row in session['messages']
                    if row.get('toolName') == 'reference_read' and row.get('toolCallId') == 'fault-read']
        windows.require(len(receipts) == 1, 'exact actual native reference_read receipt missing')
        receipt = receipts[0]
        output = receipt_text(receipt)
        windows.require(output == self.received['read_http_receipt']['content'],
                        'next HTTP request did not contain the actual native receipt bytes')
        ends = [row for row in events if row['type'] == 'tool_end' and row.get('title') == 'reference_read']
        windows.require(len(ends) == 1, 'actual read terminal event missing')
        if self.case == 'eof_empty_window':
            windows.require(receipt['isError'] is False and ends[0]['status'] == 'completed', 'EOF was treated as an error')
            page = json.loads(output)
            for key, value in self.target.items():
                windows.require(page[key] == ('' if key == 'text' else value), 'EOF lost original metadata: ' + key)
            size = len(self.source.encode())
            windows.require(page['offset'] == page['next_offset'] == page['bytes'] == size,
                            'EOF offsets/whole-source size incorrect')
            windows.require(page['truncated'] is True and page['hash_scope'] == 'full_source_text_utf8'
                            and 'not authorization or adopted memory' in page['boundary'],
                            'EOF lost whole-source hash scope or authority boundary')
        else:
            windows.require(receipt['isError'] is True and ends[0]['status'] == 'failed',
                            'illegal read was not represented as a real tool error')
            expected_error = ('UTF-8 boundary' if self.case == 'mid_scalar_offset' else
                              'active reference not found' if self.case in ('superseded_source', 'revoked_source')
                              else 'Validation failed for tool "reference_read"')
            windows.require(expected_error in output, 'tool failed for a different reason: ' + output)
            if self.case.startswith('limit_'):
                windows.require('limit' in output, 'schema rejection did not identify the invalid page argument')
        if self.case in ('superseded_source', 'revoked_source'):
            search = self.received['search_value']
            windows.require(search['matches'] == [] and search['coverage'] == 'active references only',
                            'superseded/revoked source remained searchable')
            searches = [row for row in session['messages'] if row.get('toolName') == 'reference_search'
                        and row.get('toolCallId') == 'retired-search']
            windows.require(len(searches) == 1 and searches[0]['isError'] is False,
                            'search exclusion was not backed by a real successful native receipt')
            windows.require(json.loads(receipt_text(searches[0])) == search, 'actual search receipt differs from observed HTTP')
        after = json.loads(self.context.read_bytes())
        for reference in self.before['references']:
            current = next(row for row in after['references'] if row['id'] == reference['id'])
            windows.require(current == reference, 'read/search mutated original source or management metadata')
            windows.require(windows.sha(current['text'].encode()) == current['hash'], 'retained source hash differs')
        windows.require(after['watermark'] == self.before['watermark'] and after['summary'] == self.before['summary']
                        and after['memories'] == self.before['memories'], 'read/search changed derived context state')
        windows.require(session['pending_run'] is None, 'controlled terminal left a pending Run')
        windows.require(len({row['pid'] for row in self.records}) == len(self.records), 'native process identities reused')
        windows.save(self.root / 'context-after-read.json', after)
        windows.save(self.root / 'native-read-receipt.json', receipt)
        windows.save(self.root / 'observed-http-receipts.json', self.received)
        windows.save(self.root / 'result.json', {'status': 'PASS', 'case': self.case,
            'target_id': self.target['id'], 'target_status': self.target['status'],
            'tool_is_error': receipt['isError'], 'read_calls': record['reserved_calls'],
            'all_http_requests': len(provider.requests), 'native_pids': [row['pid'] for row in self.records],
            'source_before_after_equal': True, 'actual_model': False, 'business_acceptance': 'NOT_CLAIMED'})

    async def execute(self):
        replies = [windows.text('Original source registered.'), self.safe_reply(self.read_request),
                   self.safe_reply(self.after_read), self.safe_reply(self.after_search)]
        with windows.DirectoryProvider(replies) as provider:
            try:
                await self.manage(provider)
                calls = 3 if self.case in ('superseded_source', 'revoked_source') else 2
                record, events = await self.invoke(provider.endpoint, 'read',
                    'Exercise the requested reference window and record native receipt data, then stop.', calls)
                self.verify(record, events, provider)
            finally:
                windows.save(self.root / 'provider-requests.json', provider.requests)
                windows.save(self.root / 'provider-errors.json', self.provider_errors)
        print(json.dumps({'status': 'PASS', 'case': self.case, 'evidence': str(self.root)}), flush=True)


async def execute_suite(args):
    root = Path(args.evidence)
    windows.require(root.is_absolute() and not root.exists(), 'fresh absolute suite evidence directory required')
    root.mkdir(parents=True)
    started = time.monotonic()
    results = []
    failures = []
    for case in CASES:
        try:
            fixture = FaultFixture(args, case)
            await fixture.execute()
            results.append({'case': case, 'status': 'PASS', 'directory': str(fixture.root)})
        except Exception:
            failure = traceback.format_exc()
            failures.append({'case': case, 'traceback': failure})
            windows.save(root / (case + '-failure.json'), failure)
            results.append({'case': case, 'status': 'FAIL'})
    files = [{'path': str(path.relative_to(root)), 'sha256': windows.sha(path.read_bytes()),
              'bytes': path.stat().st_size} for path in sorted(root.rglob('*')) if path.is_file()]
    windows.save(root / 'suite-result.json', {'status': 'FAIL' if failures else 'PASS', 'cases': results,
        'elapsed_seconds': time.monotonic() - started, 'binary_sha256': windows.sha(Path(args.binary).read_bytes()),
        'script_sha256': windows.sha(Path(__file__).read_bytes()), 'artifacts': files,
        'limits': ['controlled provider only', 'no actual model or product or database',
                   'tool error followed by controlled Stop is not business Task acceptance']})
    windows.require(not failures, 'fault suite failed; preserved evidence: ' + str(root))
    print(json.dumps({'status': 'PASS', 'cases': len(results), 'evidence': str(root)}), flush=True)


if __name__ == '__main__':
    parser = argparse.ArgumentParser()
    parser.add_argument('--binary', required=True)
    parser.add_argument('--expected-binary-sha256', required=True)
    parser.add_argument('--evidence', required=True)
    asyncio.run(execute_suite(parser.parse_args()))
