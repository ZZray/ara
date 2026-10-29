"""Python client process for test_byte_roundtrip.py (requirement 26.3).

`python byte_roundtrip_driver.py <sidecar> <node> <frame_tap.mjs> <start.json> <tap.log> <ops dir> <result.json>`
runs one Task through `run_task`; the sidecar is started behind `frame_tap.mjs`
so the test can read the raw frame bytes. The Host `echo_bytes` tool records
the UTF-8 hex of every string it decoded and returns the strings unchanged.
"""
from __future__ import annotations

import asyncio
import json
from pathlib import Path
import sys
from unittest import mock

HERE = Path(__file__).resolve().parent
sys.path.insert(0, str(HERE.parent / 'clients/python'))
import ara_agent_lite
from ara_agent_lite import run_task

TOKEN = 'controlled-bytes-no-provider-grant'


class EchoHost:
    def __init__(self):
        self.decoded = []

    async def tool_call(self, request, cancelled):
        items = request['arguments']['items']
        self.decoded.append({'native_call_id': request['native_call_id'],
                             'hex': [item.encode('utf-8').hex() for item in items]})
        return {'value': {'items': items}, 'is_error': False, 'settlement': 'Settled',
                'operation_receipt': 'echo-' + request['rpc_id']}

    async def verify(self, candidate, cancelled):
        return {'basis': candidate['basis'], 'committed': True, 'settlement': 'Settled', 'verdict': 'Verified',
                'evidence': ['controlled-review-only']}

    async def prepare(self, continuation, cancelled):
        raise AssertionError('no continuation expected')


async def main(sidecar, node, tap, start, log, operations, result):
    config = json.loads(Path(start).read_text(encoding='utf-8'))
    host = EchoHost()
    original = asyncio.create_subprocess_exec

    async def launch(program, *args, **kwargs):
        return await original(node, tap, sidecar, log, *args, **kwargs)

    with mock.patch.object(ara_agent_lite.asyncio, 'create_subprocess_exec', launch):
        outcome = await run_task(sidecar, config, host, token=TOKEN, operation_directory=operations)
    Path(result).write_text(json.dumps({'stop': outcome['stop'], 'decoded': host.decoded}), encoding='utf-8')


if __name__ == '__main__':
    asyncio.run(main(*sys.argv[1:8]))
