"""Native Host generation-policy contract with a controlled local provider.

This exercises real native admission, requests, tool execution and failure
settlement. Scripted responses prove neither model intelligence nor a remote
provider's internal thinking behavior. Evidence roots are fresh; no builds,
external credentials, automatic retries or failed-case replays are performed.
"""
import argparse
import json
import os
from pathlib import Path
import subprocess
import time

from verify_versioned_edits import Provider, TOKEN, MODEL, digest, require, write_json


REASONING_SENTINEL = 'PRIVATE_GENERATION_THINKING_SENTINEL'
BRIEF_SENTINEL = 'CONTROLLED_HOST_BRIEF_GENERATION_POLICY'
MARKER = b'GENERATION_POLICY_NATIVE_WRITE_VERIFIED\n'
CASES = (
    {'name': 'legacy-omitted', 'effective': 2048},
    {'name': 'explicit-default', 'policy': 'provider_default', 'max_output': 4096, 'effective': 4096},
    {'name': 'disabled-multiround', 'policy': 'thinking_type_disabled',
     'max_output': 4096, 'effective': 4096, 'brief': BRIEF_SENTINEL},
    {'name': 'disabled-8192', 'policy': 'thinking_type_disabled', 'max_output': 8192, 'effective': 8192},
    {'name': 'default-catalog-cap', 'policy': 'provider_default',
     'max_output': 8192, 'effective': 1024, 'catalog_max': 1024},
    {'name': 'disabled-catalog-cap', 'policy': 'thinking_type_disabled',
     'max_output': 8192, 'effective': 1024, 'catalog_max': 1024},
    {'name': 'disabled-reasoning-length', 'policy': 'thinking_type_disabled',
     'max_output': 4096, 'effective': 4096, 'failure': 'length'},
    {'name': 'unknown-policy', 'policy': 'not_a_policy', 'failure': 'config'},
)


class GenerationProvider(Provider):
    def __init__(self, folder, spec, target):
        self.spec = spec
        self.first_tools = None
        super().__init__(folder, spec['name'], target)
        owner = self
        base = self.server.RequestHandlerClass

        class Handler(base):
            def do_GET(self):
                try:
                    self.authorize()
                    require(self.path == '/v1/models', 'wrong catalog path')
                    owner.catalogs.append(self.path)
                    body = json.dumps({'data': [{'id': MODEL, 'protocols': ['openai-completions'],
                        'context_window': 128000, 'max_tokens': spec.get('catalog_max', 16384),
                        'omp': {'supportsTools': True, 'input': ['text'], 'reasoning': False}}]}).encode()
                    self.send_response(200)
                    self.send_header('Content-Type', 'application/json')
                    self.send_header('Content-Length', str(len(body)))
                    self.end_headers()
                    self.wfile.write(body)
                except Exception as error:
                    owner.errors.append(type(error).__name__ + ': ' + str(error))
                    self.send_error(500)

        # Construction precedes launching the Host, so no request can race this.
        self.server.RequestHandlerClass = Handler

    def next_action(self, request):
        step = len(self.requests)
        require(step <= 2, 'unexpected model retry or extra model call')
        require(request.get('stream') is True and request.get('model') == MODEL,
                'generation policy changed model identity or streaming')
        require(request.get('max_tokens') == self.spec['effective'], 'wire output budget differs')
        forbidden = {'reasoning_effort', 'reasoning', 'extra_body', 'max_completion_tokens'}
        require(not forbidden.intersection(request), 'unexpected reasoning or output override')
        disabled = self.spec.get('policy') == 'thinking_type_disabled'
        if disabled:
            require(request.get('thinking') == {'type': 'disabled'}, 'fixed disabled policy absent or altered')
        else:
            require('thinking' not in request, 'provider-default unexpectedly adds thinking control')
        require(isinstance(request.get('messages'), list), 'wire messages missing')
        if step == 1:
            text = json.dumps(request['messages'], ensure_ascii=False)
            require('Controlled generation policy acceptance: ' + self.name in text, 'objective lost')
            if self.spec.get('brief'):
                require(self.spec['brief'] in text, 'explicit Host brief missing from model projection')
            self.first_tools = request.get('tools')
            advertised = {item['function']['name'] for item in self.first_tools or []}
            require({'read_file', 'write_file'} <= advertised, 'native tool contracts missing')
            require(not self.target.exists(), 'marker existed before native tool execution')
            if self.spec.get('failure') == 'length':
                self.finished = True
                return {'reasoning_content': REASONING_SENTINEL}, 'length'
            return self.call('write_file', {'path': self.target.name, 'content': MARKER.decode('ascii')})
        require(request.get('tools') == self.first_tools, 'policy changed tool schemas between model calls')
        tools = [message for message in request['messages'] if message.get('role') == 'tool']
        require(bool(self.previous(tools)), 'native write_file receipt missing')
        require(self.target.read_bytes() == MARKER, 'native write effect differs from marker bytes')
        self.finished = True
        return {'content': 'GENERATION_POLICY_CASE_VERIFIED'}, 'stop'


def source_pins():
    core = Path(__file__).resolve().parents[1]
    paths = sorted(path for path in (core / 'src').rglob('*') if path.is_file())
    paths += [core / 'Cargo.toml', core / 'Cargo.lock', core / 'examples' / 'business_host.rs',
              Path(__file__).resolve(), Path(__file__).with_name('verify_versioned_edits.py').resolve()]
    return {str(path.relative_to(core)): digest(path.read_bytes()) for path in paths if path.is_file()}


def verify_policy_event(events, spec):
    policies = [event for event in events if event.get('type') == 'generation_policy']
    if spec.get('policy') != 'thinking_type_disabled':
        require(not policies, 'legacy/default config emitted a new policy event')
        return
    require(len(policies) == 1, 'explicit policy admission event missing or duplicated')
    policy = policies[0]
    expected = {'source': 'host_model_policy', 'model': MODEL, 'protocol': 'openai-completions',
        'policy': 'thinking_type_disabled', 'requested_output_tokens': spec['max_output'],
        'effective_output_tokens': spec['effective'], 'provider_effect_verified': False}
    require(all(policy.get(key) == value for key, value in expected.items()), 'policy admission evidence differs')
    require(policy.get('output_budget_rule') ==
            'min(host request, local 8192 ceiling, parsed directory maximum with local fallback)',
            'policy event lost the effective-budget rule')


def run_case(binary, root, spec):
    folder = root / spec['name']
    folder.mkdir()
    work = folder / 'project'
    work.mkdir()
    target = work / 'marker.txt'
    provider = GenerationProvider(folder, spec, target)
    config = {'id': spec['name'], 'objective': 'Controlled generation policy acceptance: ' + spec['name'],
        'cwd': str(work), 'session_path': str(folder / 'session.json'),
        'environment_state': str(folder / 'environment-state'), 'endpoint': provider.endpoint,
        'model': MODEL, 'token_env': 'ARA_GENERATION_POLICY_FIXTURE_KEY',
        'mode': 'execute', 'enable_local_tools': True, 'max_calls': 3, 'max_seconds': 45}
    for source, destination in [('policy', 'generation_policy'), ('max_output', 'max_output'), ('brief', 'brief')]:
        if source in spec:
            config[destination] = spec[source]
    write_json(folder / 'request.json', config)
    environment = os.environ.copy()
    environment.update(ARA_GENERATION_POLICY_FIXTURE_KEY=TOKEN, NO_PROXY='127.0.0.1,localhost')
    case = {'name': spec['name'], 'status': 'FAIL', 'spec': spec}
    started = time.monotonic()
    try:
        try:
            process = subprocess.run([str(binary), str(folder / 'request.json')], cwd=work, env=environment,
                                     capture_output=True, timeout=55)
            code, stdout, stderr = process.returncode, process.stdout, process.stderr
        except subprocess.TimeoutExpired as error:
            code, stdout, stderr = None, error.stdout or b'', error.stderr or b''
            case.update(timeout=True, effects_unknown=True)
        (folder / 'host.stdout.jsonl').write_bytes(stdout)
        (folder / 'host.stderr.txt').write_bytes(stderr)
        case['host_process_exit'] = code
        require(code is not None, 'native process timed out; retain evidence and do not replay')
        packets = [json.loads(line) for line in stdout.decode('utf-8').splitlines() if line.strip()]
        events = [packet['event'] for packet in packets if packet.get('kind') == 'event']
        write_json(folder / 'host-events.json', events)
        starts = [event for event in events if event.get('type') == 'tool_start']
        ends = [event for event in events if event.get('type') == 'tool_end']
        calls = [event for event in events if event.get('type') == 'model_call']
        require(not provider.errors, 'controlled provider assertions failed: ' + repr(provider.errors))
        if spec.get('failure') == 'config':
            require(code != 0 and 'unknown variant' in stderr.decode('utf-8'), 'unknown enum was not rejected by config parsing')
            require(not provider.catalogs and not provider.requests, 'invalid enum reached catalog or model HTTP')
            require(not packets and not starts and not ends, 'invalid enum reached native event/tool execution')
            require(not (folder / 'session.json').exists() and not (folder / 'environment-state').exists(),
                    'invalid enum created persistent Run state')
            require(not target.exists(), 'invalid enum produced a tool effect')
        else:
            verify_policy_event(events, spec)
            require(len(provider.catalogs) == 1, 'unexpected catalog lookup count')
            require(provider.finished, 'native loop did not reach all controlled checks')
            require(len(calls) == len(provider.requests), 'native/provider model call count differs')
            require(len(starts) == len(ends) == len(provider.actions), 'native tool event count differs')
            require(packets and packets[-1].get('kind') == 'result', 'native final result missing')
            if spec.get('failure') == 'length':
                require(code != 0 and packets[-1].get('code') != 0, 'length-only reply incorrectly succeeded')
                require('assistant stopped with length' in json.dumps(packets[-1]), 'length failure reason lost')
                require(len(provider.requests) == 1 and not starts and not ends and not target.exists(),
                        'length-only reply retried or executed a tool')
                require(REASONING_SENTINEL.encode() not in stdout + stderr,
                        'private reasoning leaked into ordinary Host output')
            else:
                require(code == 0 and packets[-1].get('code') == 0, 'positive native Run failed')
                require(len(provider.requests) == 2 and len(ends) == 1, 'native multi-round sequence differs')
                action = provider.actions[0]
                for event in (starts[0], ends[0]):
                    require(event.get('step_id') == action['id'] and event.get('title') == 'write_file',
                            'native tool identity differs from provider action')
                require(ends[0].get('status') == 'completed' and target.read_bytes() == MARKER,
                        'native write_file failed or final marker bytes differ')
            saved = json.loads((folder / 'session.json').read_text(encoding='utf-8'))
            require(set(saved.get('route', {})) == {'model', 'protocol', 'binding', 'decision'},
                    'generation policy altered persisted Route schema')
            require(saved['route']['model'] == MODEL and saved['route']['protocol'] == 'openai-completions',
                    'persisted route identity differs')
        case.update(status='PASS', native_model_calls=len(calls), native_tool_ends=len(ends),
                    provider_requests=len(provider.requests), catalog_requests=len(provider.catalogs))
    except Exception as error:
        case['error'] = type(error).__name__ + ': ' + str(error)
    finally:
        provider.close()
        write_json(folder / 'provider-requests.json', provider.requests)
        write_json(folder / 'provider-errors.json', provider.errors)
        write_json(folder / 'scripted-actions.json', provider.actions)
        case.update(elapsed_seconds=round(time.monotonic() - started, 3),
                    final_marker_sha256=digest(target.read_bytes()) if target.exists() else None)
        if provider.requests:
            # These fields must remain identical across default/disabled/budget
            # variants. Only the explicit thinking field and budget may differ.
            stable = {key: value for key, value in provider.requests[0].items()
                      if key not in {'messages', 'thinking', 'max_tokens'}}
            write_json(folder / 'stable-wire-contract.json', stable)
            case['stable_wire_contract_sha256'] = digest(json.dumps(stable, sort_keys=True).encode())
        write_json(folder / 'case.json', case)
    return case


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--binary', type=Path, required=True)
    parser.add_argument('--output', type=Path, required=True)
    args = parser.parse_args()
    binary, root = args.binary.resolve(), args.output.resolve()
    require(binary.is_file(), '--binary must already exist; this harness never builds it')
    root.mkdir(parents=True, exist_ok=False)
    before = source_pins()
    result = {'schema': 'ara-lite-generation-policy-native-v1', 'status': 'FAIL',
        'scope': 'controlled provider; native admission, fixed wire policy, capped budget, tools and failures',
        'limits': 'not real-model intelligence, remote thinking proof or OS sandbox acceptance',
        'binary_path': str(binary), 'binary_sha256': digest(binary.read_bytes()),
        'harness_sha256': digest(Path(__file__).read_bytes()), 'source_pins': before,
        'budget_per_case': {'host_model_calls': 3, 'expected_provider_calls_at_most': 2,
                            'host_seconds': 45, 'outer_seconds': 55}, 'cases': []}
    started = time.monotonic()
    try:
        for spec in CASES:
            result['cases'].append(run_case(binary, root, spec))
        contracts = {case['stable_wire_contract_sha256'] for case in result['cases']
                     if 'stable_wire_contract_sha256' in case}
        result['stable_wire_contracts_equal'] = len(contracts) == 1
        result['source_pins_unchanged'] = before == source_pins()
        result['binary_unchanged'] = result['binary_sha256'] == digest(binary.read_bytes())
        result['status'] = ('PASS' if result['stable_wire_contracts_equal'] and result['source_pins_unchanged']
                            and result['binary_unchanged'] and all(case['status'] == 'PASS' for case in result['cases'])
                            else 'FAIL')
    finally:
        result['elapsed_seconds'] = round(time.monotonic() - started, 3)
        write_json(root / 'result.json', result)
        print(json.dumps(result, ensure_ascii=False, indent=2))
    require(result['status'] == 'PASS', 'native generation-policy acceptance failed; inspect retained evidence')


if __name__ == '__main__':
    main()
