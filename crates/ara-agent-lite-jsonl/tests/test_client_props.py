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
from ara_agent_lite import ReconciliationRequired, UnknownEffect, parse_unknown_effects

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


if __name__ == '__main__':
    unittest.main()
