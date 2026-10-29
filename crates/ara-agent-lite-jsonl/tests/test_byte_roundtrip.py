"""Property 36: JSONL cross-language byte round trip (requirement 26.3), Python side.

Generated strings follow Property 35: Unicode scalar values only, 4-1024 code
points, each with at least one U+0000, one U+10000-U+10FFFF character and one
base + U+0300-U+036F sequence, no U+000A or U+000D. The controlled provider
sends them in batches as `echo_bytes` tool calls; a real Python client process
(`byte_roundtrip_driver.py`) decodes each `tool_call`, records the UTF-8 bytes
and returns the strings unchanged in its `tool_reply`. `frame_tap.mjs` records
the raw frame bytes in both directions.

Checked for every string (at least 100 per direction, plus the fixed sample):
the client decoded the exact bytes (sidecar -> client); the tool result the
sidecar sent to the provider holds the exact bytes (client -> sidecar); every
U+0000 is a `\\u0000` escape in the frame, no frame has a raw 00 byte, and each
frame is one complete line. Separate single-sample runs use a real Python
client process and a real Node client process. Seed: ARA_BYTE_SEED (default 36).
Controlled loopback provider only; no actual model.
"""
from __future__ import annotations

import base64
import json
import os
from pathlib import Path
import random
import shutil
import subprocess
import sys
import tempfile
import time
import unittest

HERE = Path(__file__).resolve().parent
sys.path[:0] = [str(HERE.parent / 'clients/python'), str(HERE)]
from controlled_provider import ControlledProvider, call, text
from test_jsonl_resume import BINARY, child_env

NODE = shutil.which('node')
TAP = HERE / 'frame_tap.mjs'
DRIVER = HERE / 'byte_roundtrip_driver.py'
CLIENT = HERE.parent / 'clients/node/client.mjs'
SAMPLE = 'header\x00中文😀'
SAMPLE_HEX = '68656164657200e4b8ade69687f09f9880'
SEED = int(os.environ.get('ARA_BYTE_SEED', '36'))
CASES = 100
BATCH_STRINGS, BATCH_BYTES = 8, 16 * 1024
SPECIALS = [*range(0x00, 0x0A), 0x0B, 0x0C, *range(0x0E, 0x20), 0x22, 0x5C, 0x7F, *range(0x80, 0xA0),
            0x2028, 0x2029, 0xFEFF, 0xFFFD, 0xFFFE, 0xFFFF, 0x10FFFF]


def scalar(rng: random.Random) -> str:
    """Any Unicode scalar except LF and CR, weighted toward the escape-relevant ranges."""
    while True:
        roll = rng.random()
        if roll < 0.3:
            point = rng.randrange(0x00, 0x80)
        elif roll < 0.5:
            point = rng.randrange(0x80, 0x10000)
        elif roll < 0.65:
            point = rng.randrange(0x10000, 0x110000)
        elif roll < 0.75:
            point = rng.randrange(0x300, 0x370)
        elif roll < 0.9:
            point = rng.choice(SPECIALS)
        else:
            point = rng.randrange(0, 0x110000)
        if point not in (0x0A, 0x0D) and not 0xD800 <= point <= 0xDFFF:
            return chr(point)


def generated(rng: random.Random) -> str:
    length = rng.randint(4, 1024)
    units = ['\x00', chr(rng.randrange(0x10000, 0x110000)),
             rng.choice('aeouAEOUnz') + chr(rng.randrange(0x300, 0x370))]
    units += [scalar(rng) for _ in range(length - 4)]
    rng.shuffle(units)
    value = ''.join(units)
    assert 4 <= len(value) <= 1024
    return value


def cases(count=CASES, seed=SEED):
    rng = random.Random(seed)
    return [SAMPLE] + [generated(rng) for _ in range(count)]


def batches(strings):
    out, current, size = [], [], 0
    for value in strings:
        length = len(value.encode('utf-8'))
        if current and (len(current) == BATCH_STRINGS or size + length > BATCH_BYTES):
            out.append(current)
            current, size = [], 0
        current.append(value)
        size += length
    return out + ([current] if current else [])


def nul_escapes(raw: bytes) -> int:
    """Count `\\u0000` JSON escapes, scanning escapes exactly (an escaped backslash is not one)."""
    count, index = 0, 0
    while (index := raw.find(b'\\', index)) >= 0:
        if raw[index + 1:index + 2] == b'u':
            count += raw[index + 2:index + 6] == b'0000'
            index += 6
        else:
            index += 2
    return count


def tool_text(message) -> str:
    content = message['content']
    return content if isinstance(content, str) else ''.join(part['text'] for part in content if part.get('type') == 'text')


def start_config(root: Path, endpoint: str, calls: int):
    return {'kind': 'start_task', 'version': 1,
        'task': {'task_id': 'byte-roundtrip', 'criteria_revision': 'c1', 'authorization_revision': 'a1',
                 'kind': 'Coding', 'evidence_dir': str(root / 'evidence'), 'max_runs': 1, 'max_calls': calls,
                 'deadline_unix_ms': int(time.time() * 1000) + 240_000},
        'run': {'run_id': 'run-1', 'objective': 'Echo every string with the Host tool.', 'cwd': str(root),
                'session_path': str(root / 'session.json'), 'environment_state': str(root / 'environment'),
                'device_fp': 'byte-roundtrip', 'session_key': 'byte/roundtrip', 'mode': 'execute',
                'endpoint': endpoint, 'model': 'jsonl-fixture', 'max_calls': calls, 'max_seconds': 180,
                'max_output': 512, 'model_resolution': {'kind': 'catalog'},
                'tools': [{'name': 'echo_bytes', 'description': 'Host echo of the given strings.',
                           'parameters': {'type': 'object', 'properties': {'items': {'type': 'array',
                                          'items': {'type': 'string'}}}, 'required': ['items']}}]},
        'cancel_grace_ms': 1000}


def first_difference(expected: str, actual: str) -> str:
    left, right = expected.encode('utf-8'), actual.encode('utf-8')
    at = next((i for i, (a, b) in enumerate(zip(left, right)) if a != b), min(len(left), len(right)))
    return (f'expected {len(left)} bytes, got {len(right)}; first difference at byte {at}: '
            f'expected {left[max(0, at - 8):at + 8].hex(" ")} got {right[max(0, at - 8):at + 8].hex(" ")}; '
            f'full expected {left.hex()}')


class ByteRoundTripTest(unittest.TestCase):
    def setUp(self):
        self.assertTrue(BINARY.is_file(), f'build the sidecar first: {BINARY}')
        self.assertIsNotNone(NODE, 'node is required for frame_tap.mjs and the Node client process')
        self.root = Path(tempfile.mkdtemp(prefix='ara-byte-roundtrip-'))

    def assert_round_trip(self, strings, log, decoded, provider, client):
        groups = batches(strings)
        # sidecar -> client: what the client decoded from each tool_call.
        self.assertEqual([entry['native_call_id'] for entry in decoded], [f'op-{i}' for i in range(len(groups))])
        for group, entry in zip(groups, decoded):
            for expected, actual_hex in zip(group, entry['hex'], strict=True):
                actual = bytes.fromhex(actual_hex).decode('utf-8')
                self.assertEqual(actual, expected, f'{client} decoded: ' + first_difference(expected, actual))
        # client -> sidecar: the ToolResult the sidecar sent upstream.
        for index, group in enumerate(groups):
            messages = [message for message in provider.requests[index + 1]['messages']
                        if message.get('role') == 'tool' and message.get('tool_call_id') == f'op-{index}']
            self.assertEqual(len(messages), 1)
            returned = json.loads(tool_text(messages[0]))['items']
            for expected, actual in zip(group, returned, strict=True):
                self.assertEqual(actual, expected, f'{client} -> sidecar: ' + first_difference(expected, actual))
        # Raw frames in both directions.
        rows = [json.loads(line) for line in log.read_text(encoding='utf-8').splitlines()]
        frames = {'sidecar': [], 'host': []}
        for row in rows:
            raw = base64.b64decode(row['base64'])
            self.assertTrue(raw.endswith(b'\n') and raw.count(b'\n') == 1, 'one complete line per frame')
            self.assertNotIn(b'\x00', raw, f"raw 00 byte in a {row['from']} frame")
            frame = json.loads(raw)
            if frame['kind'] in ('tool_call', 'tool_reply'):
                frames[row['from']].append((raw, frame))
        calls, replies = frames['sidecar'], frames['host']
        self.assertEqual([frame['kind'] for _, frame in calls], ['tool_call'] * len(groups))
        self.assertEqual([frame['kind'] for _, frame in replies], ['tool_reply'] * len(groups))
        for group, (call_raw, call_frame), (reply_raw, reply_frame) in zip(groups, calls, replies):
            nul = sum(value.count('\x00') for value in group)
            self.assertEqual(call_frame['arguments']['items'], group, 'tool_call frame not truncated')
            self.assertEqual(reply_frame['result']['value']['items'], group, 'tool_reply frame not truncated')
            self.assertEqual(nul_escapes(call_raw), nul, 'each U+0000 is a \\u0000 escape in tool_call')
            self.assertEqual(nul_escapes(reply_raw), nul, 'each U+0000 is a \\u0000 escape in tool_reply')

    def provider_for(self, strings):
        groups = batches(strings)
        return ControlledProvider([call(f'op-{i}', 'echo_bytes', {'items': group}) for i, group in enumerate(groups)]
                                  + [text('All strings echoed.')]), len(groups) + 1

    def python_process(self, name, strings):
        root = self.root / name
        root.mkdir()
        provider, calls = self.provider_for(strings)
        with provider:
            config = start_config(root, provider.endpoint, calls)
            (root / 'start.json').write_text(json.dumps(config), encoding='utf-8')
            log, result = root / 'tap.jsonl', root / 'result.json'
            completed = subprocess.run([sys.executable, str(DRIVER), str(BINARY), NODE, str(TAP), str(root / 'start.json'),
                                        str(log), str(root / 'ops'), str(result)], env=child_env(), cwd=root,
                                       capture_output=True, timeout=240)
            self.assertEqual(completed.returncode, 0, completed.stderr.decode('utf-8', 'replace')[-2000:])
            output = json.loads(result.read_text(encoding='utf-8'))
            self.assertEqual(output['stop'], 'AwaitingHostAcceptance')
            self.assert_round_trip(strings, log, output['decoded'], provider, 'Python client')
        return output

    def node_process(self, name, strings):
        root = self.root / name
        root.mkdir()
        provider, calls = self.provider_for(strings)
        with provider:
            config = start_config(root, provider.endpoint, calls)
            (root / 'start.json').write_text(json.dumps(config), encoding='utf-8')
            log, result = root / 'tap.jsonl', root / 'result.json'
            driver = root / 'driver.mjs'
            driver.write_text(f"""
import {{ promises as fs }} from 'node:fs';
import {{ runTask }} from {json.dumps(CLIENT.as_uri())};
const start = JSON.parse(await fs.readFile({json.dumps(str(root / 'start.json'))}, 'utf8'));
const decoded = [];
const callbacks = {{
  tool: async (frame) => {{
    const items = frame.arguments.items;
    decoded.push({{ native_call_id: frame.native_call_id, hex: items.map((item) => Buffer.from(item, 'utf8').toString('hex')) }});
    return {{ value: {{ items }}, is_error: false, settlement: 'Settled', operation_receipt: `echo-${{frame.rpc_id}}` }};
  }},
  verify: async (candidate) => ({{ basis: candidate.basis, committed: true, settlement: 'Settled', verdict: 'Verified',
    evidence: ['controlled-review-only'] }}),
  prepare: async () => {{ throw new Error('no continuation'); }} }};
const outcome = await runTask({{ binary: process.execPath, binaryArgs: [{json.dumps(str(TAP))}, {json.dumps(str(BINARY))},
  {json.dumps(str(log))}], start, modelToken: 'controlled-bytes-no-provider-grant',
  operationDirectory: {json.dumps(str(root / 'ops'))}, callbacks }});
await fs.writeFile({json.dumps(str(result))}, JSON.stringify({{ stop: outcome.outcome?.stop ?? null, fault: outcome.fault ?? null, decoded }}));
""", encoding='utf-8')
            completed = subprocess.run([NODE, str(driver)], env=child_env(), cwd=root, capture_output=True, timeout=240)
            self.assertEqual(completed.returncode, 0, completed.stderr.decode('utf-8', 'replace')[-2000:])
            output = json.loads(result.read_text(encoding='utf-8'))
            self.assertEqual(output['stop'], 'AwaitingHostAcceptance', output['fault'])
            self.assert_round_trip(strings, log, output['decoded'], provider, 'Node client')
        return output

    def test_generated_strings_round_trip_through_the_python_client(self):
        strings = cases()
        self.assertGreaterEqual(len(strings) - 1, 100)
        self.assertEqual(SAMPLE.encode('utf-8').hex(), SAMPLE_HEX)
        for value in strings[1:]:
            self.assertTrue('\x00' in value and any(ord(c) >= 0x10000 for c in value)
                            and any(0x300 <= ord(c) <= 0x36F for c in value) and '\n' not in value and '\r' not in value)
        self.python_process('python-property', strings)

    def test_sample_round_trips_through_real_python_and_node_client_processes(self):
        for client, run in (('python', self.python_process), ('node', self.node_process)):
            with self.subTest(client=client):
                output = run(f'{client}-sample', [SAMPLE])
                self.assertEqual(output['decoded'][0]['hex'], [SAMPLE_HEX])


if __name__ == '__main__':
    unittest.main()
