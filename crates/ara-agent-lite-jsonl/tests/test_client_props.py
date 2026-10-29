"""Property 32 (23.2, 23.7): Python client parsing of `unknown_effects` records.

Pure function; no sidecar, provider or model.
"""
from __future__ import annotations

import copy
from pathlib import Path
import sys
import unittest

from hypothesis import given, settings, strategies as st

HERE = Path(__file__).resolve().parent
sys.path.insert(0, str(HERE.parent / 'clients/python'))
import asyncio
import json
import time

from ara_agent_lite import FRAME_BYTES, ReconciliationRequired, UnknownEffect, parse_unknown_effects, reconcile_reply

FIELDS = ('run_id', 'native_call_id', 'tool', 'receipt_ref')
names = st.text(min_size=1, max_size=24)
digests = st.text(alphabet='0123456789abcdef', min_size=64, max_size=64)
journal = st.fixed_dictionaries({'kind': st.just('journal'), 'session_path': names,
                                 'raw_index': st.integers(0, 10_000), 'message_sha256': digests})
event = st.fixed_dictionaries({'kind': st.just('event'), 'events_path': names, 'seq': st.integers(0, 10_000)})
host_receipt = st.fixed_dictionaries({'path': names, 'sha256': digests})
receipt_refs = st.builds(lambda evidence, host: {'evidence': evidence, **({'host_receipt': host} if host else {})},
                         st.one_of(journal, event), st.one_of(st.none(), host_receipt))
records = st.fixed_dictionaries({'run_id': names, 'native_call_id': names, 'tool': names, 'receipt_ref': receipt_refs})
wrong_values = st.one_of(st.none(), st.integers(), st.lists(st.integers(), max_size=2), st.just(''), st.just({}))


class UnknownEffectParsingProperties(unittest.TestCase):
    @settings(max_examples=256, deadline=None)
    @given(st.lists(records, max_size=6))
    def test_valid_records_keep_order_and_exact_values(self, items):
        outcome = {'task_id': 't', 'unknown_effects': copy.deepcopy(items)}
        parsed = parse_unknown_effects(outcome)
        self.assertEqual(len(parsed), len(items))
        for effect, item in zip(parsed, items):
            self.assertIsInstance(effect, UnknownEffect)
            self.assertEqual((effect.run_id, effect.native_call_id, effect.tool, effect.receipt_ref),
                             tuple(item[field] for field in FIELDS))
        self.assertEqual(outcome['unknown_effects'], items, 'parsing never changes the original outcome')

    @settings(max_examples=256, deadline=None)
    @given(st.lists(records, min_size=1, max_size=4), st.data())
    def test_invalid_record_names_its_field_and_builds_nothing(self, items, data):
        index = data.draw(st.integers(0, len(items) - 1))
        field = data.draw(st.sampled_from(FIELDS))
        broken = copy.deepcopy(items)
        if data.draw(st.booleans()):
            del broken[index][field]
        else:
            value = data.draw(wrong_values)
            if field == 'receipt_ref' and isinstance(value, dict):
                value = {'evidence': {}}
            broken[index][field] = value
        with self.assertRaises(ReconciliationRequired) as raised:
            parse_unknown_effects({'unknown_effects': broken})
        self.assertEqual(str(raised.exception), f'invalid unknown effect record: {field}')

    def test_absent_list_is_empty_and_non_list_is_refused(self):
        self.assertEqual(parse_unknown_effects({'task_id': 't'}), [])
        with self.assertRaises(ReconciliationRequired):
            parse_unknown_effects({'unknown_effects': {}})
        with self.assertRaises(ReconciliationRequired) as raised:
            parse_unknown_effects({'unknown_effects': [{**{f: 'x' for f in FIELDS[:3]},
                                                        'receipt_ref': {'evidence': {'kind': 'event'}}, 'extra': 1}]})
        self.assertEqual(str(raised.exception), 'invalid unknown effect record: extra')


settlements = st.one_of(st.just({'kind': 'not_executed'}),
                        st.fixed_dictionaries({'kind': st.sampled_from(['completed', 'failed']), 'output': st.text(max_size=40)}))
receipts = st.fixed_dictionaries({
    'committed': st.booleans(), 'settlement': st.sampled_from(['Settled', 'Unknown']),
    'tool_calls': st.lists(st.fixed_dictionaries({'call_id': names, 'settlement': settlements}), max_size=4),
    'evidence': st.lists(st.text(max_size=40), max_size=4)})
FRAME = {'kind': 'reconcile_request', 'version': 1, 'rpc_id': 'rpc-1', 'task_id': 'task-1', 'run_id': 'run-1',
         'review': {}}
TOKEN = 'model-grant-never-in-errors'


class CallbackHost:
    def __init__(self, result=None, error=None):
        self.result, self.error, self.calls = result, error, 0

    async def reconcile_resume(self, review, cancelled):
        self.calls += 1
        if self.error is not None:
            raise self.error
        return self.result


def settle(host):
    deadline = int(time.time() * 1000) + 60_000
    return asyncio.run(reconcile_reply(host, FRAME, {}, asyncio.Event(), deadline, 1.0))


def broken(receipt, data):
    """One field-set or type violation, anywhere in the receipt."""
    value = copy.deepcopy(receipt)
    targets = [value] + list(value['tool_calls']) + [call['settlement'] for call in value['tool_calls']]
    target = data.draw(st.sampled_from(targets))
    keys = sorted(target)
    action = data.draw(st.sampled_from(['missing', 'extra', 'type']))
    if action == 'missing':
        del target[data.draw(st.sampled_from(keys))]
    elif action == 'extra':
        target[data.draw(st.text(min_size=1, max_size=8).filter(lambda key: key not in target))] = True
    else:
        key = data.draw(st.sampled_from(keys))
        target[key] = data.draw(st.one_of(st.integers(), st.none(), st.just(['x']), st.just({}))
                                .filter(lambda wrong: type(wrong) is not type(target[key])))
    return value


class ReconcileReceiptProperties(unittest.TestCase):
    # Feature: ara-lite-core-capability, Property 20: 客户端回执校验
    @settings(max_examples=256, deadline=None)
    @given(receipts)
    def test_valid_receipt_is_written_with_the_request_id(self, receipt):
        host = CallbackHost(copy.deepcopy(receipt))
        reply = settle(host)
        self.assertEqual(host.calls, 1)
        self.assertEqual({key: reply[key] for key in ('kind', 'version', 'rpc_id', 'task_id', 'run_id')},
                         {**{key: FRAME[key] for key in ('version', 'rpc_id', 'task_id', 'run_id')}, 'kind': 'reconcile_reply'})
        self.assertEqual(reply['receipt'], receipt)

    # Feature: ara-lite-core-capability, Property 20: 客户端回执校验
    @settings(max_examples=256, deadline=None)
    @given(receipts, st.data())
    def test_invalid_receipt_is_never_written(self, receipt, data):
        value = broken(receipt, data)
        with self.assertRaises(ReconciliationRequired) as raised:
            settle(CallbackHost(value))
        self.assertTrue(str(raised.exception).startswith('reconcile_receipt_invalid:'), (value, str(raised.exception)))

    # Feature: ara-lite-core-capability, Property 20: 客户端回执校验
    @settings(max_examples=100, deadline=None)
    @given(receipts, st.integers(1, 4096))
    def test_oversized_reply_frame_is_never_written(self, receipt, extra):
        receipt['evidence'] = ['x' * (FRAME_BYTES + extra)]
        with self.assertRaises(ReconciliationRequired) as raised:
            settle(CallbackHost(receipt))
        self.assertTrue(str(raised.exception).startswith('reconcile_frame_too_large:'))

    # Feature: ara-lite-core-capability, Property 20: 客户端回执校验
    @settings(max_examples=256, deadline=None)
    @given(st.text(max_size=40), st.text(max_size=40), st.sampled_from([RuntimeError, ValueError, KeyError, OSError]))
    def test_exception_text_and_token_never_reach_the_error(self, before, after, kind):
        secret = f'{before}{TOKEN}{after}'
        with self.assertRaises(ReconciliationRequired) as raised:
            settle(CallbackHost(error=kind(secret)))
        message = str(raised.exception)
        self.assertTrue(message.startswith('reconcile_callback_exception:'))
        self.assertNotIn(TOKEN, message)
        self.assertIsNone(raised.exception.__cause__)

    def test_reply_at_the_byte_bound_is_written(self):
        receipt = {'committed': True, 'settlement': 'Settled', 'tool_calls': [], 'evidence': ['']}
        base = len(json.dumps({'kind': 'reconcile_reply', **{key: FRAME[key] for key in ('version', 'rpc_id', 'task_id', 'run_id')},
                               'receipt': receipt}, separators=(',', ':')).encode()) + 1
        receipt['evidence'] = ['x' * (FRAME_BYTES - base)]
        self.assertEqual(settle(CallbackHost(receipt))['receipt'], receipt)
        receipt['evidence'] = ['x' * (FRAME_BYTES - base + 1)]
        with self.assertRaises(ReconciliationRequired):
            settle(CallbackHost(receipt))


if __name__ == '__main__':
    unittest.main()
