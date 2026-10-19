"""Validate the legacy fixture adapter without concealing actual wire evidence."""
import copy
import json
import unittest

from verify_host import RUN_BUDGET_PREFIX, fixture_view, run_budget


class BudgetFixtureTests(unittest.TestCase):
    def carrier(self, **changes):
        value = dict(scope="current_run", reserved_calls=1, call_limit=12,
                     calls_remaining_including_next=11, milliseconds_remaining=50000, phase="working")
        value.update(changes)
        return dict(role="user", content=RUN_BUDGET_PREFIX + json.dumps(value))

    def test_tail_only_preserves_task_steering_tool_pairs_and_wire(self):
        messages = [dict(role="user", content="task"), dict(role="assistant", tool_calls=[dict(id="a")]),
                    dict(role="tool", tool_call_id="a", content="receipt"), dict(role="user", content="steer")]
        body = dict(messages=messages + [self.carrier()], tools=[dict(name="tool")])
        before = copy.deepcopy(body)
        view = fixture_view(body)
        self.assertEqual(view["messages"], messages)
        self.assertEqual(body, before)
        self.assertIsNot(view, body)
        self.assertIs(view["tools"], body["tools"])
        self.assertEqual(run_budget(body["messages"][-1])["reserved_calls"], 1)
        middle = dict(messages=[self.carrier(), dict(role="user", content="real latest user")])
        self.assertIs(fixture_view(middle), middle)

    def test_summary_plain_text_malformed_and_wrong_types_are_not_filtered(self):
        for message in [dict(role="user", content="budget guidance"),
                        dict(role="user", content=RUN_BUDGET_PREFIX + "broken JSON"),
                        self.carrier(reserved_calls=True), self.carrier(call_limit=-1),
                        self.carrier(calls_remaining_including_next=12),
                        self.carrier(scope="goal"), self.carrier(phase="accepted"),
                        self.carrier(extra="not host metadata"),
                        dict(role="assistant", content=self.carrier()["content"])]:
            body = dict(messages=[message])
            self.assertIs(fixture_view(body), body)
            self.assertIsNone(run_budget(message))
        summary = dict(messages=[dict(role="user", content="Summarize source data")])
        self.assertIs(fixture_view(summary), summary)


if __name__ == "__main__":
    unittest.main()
