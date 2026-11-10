"""Offline evidence tests; these fixtures never call a model or execute a process."""
from copy import deepcopy
import hashlib
import json
from pathlib import Path
import tempfile
import unittest

import verify_environment_live as live


def write_json(path, value):
    path.write_bytes(json.dumps(value, ensure_ascii=False, indent=2).encode("utf-8") + b"\n")


class AuditTests(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory()
        self.addCleanup(self.temp.cleanup)
        self.root = Path(self.temp.name)
        self.work = self.root / "work"
        self.work.mkdir()
        self.expected = live.write_fixtures(self.work)
        self.bundle = self.work / ".handwave" / "skills" / "log-summary"
        (self.bundle / "scripts").mkdir(parents=True)
        (self.bundle / "SKILL.md").write_bytes(b"# Log summary\nUse scripts/summary.py.\n")
        (self.bundle / "scripts" / "summary.py").write_bytes(b"# reusable fixture script\n")
        knowledge = self.work / "docs" / "knowledge"
        (knowledge / "notes").mkdir()
        note = knowledge / "notes" / "workflow.md"
        note.write_bytes(b"# Workflow\nStatus: observed\nUse the saved log-summary skill.\n")
        manifest = [{"path": "notes/workflow.md", "status": "observed",
                     "sha256": hashlib.sha256(note.read_bytes()).hexdigest()}]
        index = (knowledge / "INDEX.md").read_text(encoding="utf-8")
        (knowledge / "INDEX.md").write_bytes((index + "\n<!-- handwave-knowledge-v1:begin -->\n<!-- manifest:"
                                              + json.dumps(manifest) + " -->\n").encode("utf-8"))
        for trial, name in [("create", "first"), ("reuse", "second")]:
            write_json(self.work / (name + "-summary.json"), self.expected[name])
        self.calls, self.events = self.trial_calls("create")
        self.save_metadata()

    def trial_calls(self, trial, recognized=False):
        command = "python .handwave/skills/log-summary/scripts/summary.py first.jsonl first-summary.json" if recognized else "import subprocess; subprocess.run(saved_command, check=True)"
        calls = {}
        if trial == "create":
            calls["rule"] = {"name": "knowledge_read", "arguments": {"scope": "project", "path": "log-format.md"}}
        else:
            calls["entry"] = {"name": "skill_read", "arguments": {"scope": "project", "name": "log-summary"}}
            calls["note"] = {"name": "knowledge_read", "arguments": {"scope": "project", "path": "notes/workflow.md"}}
        calls["process"] = {"name": "exec" if recognized else "run_script",
                            "arguments": {"cmd" if recognized else "script": command, "cwd": str(self.work)}}
        if trial == "create":
            for name in ("skill_create", "skill_verify", "knowledge_write"):
                calls[name] = {"name": name, "arguments": {"scope": "project"}}
        events = [{"type": "tool_end", "step_id": call_id, "title": call["name"], "status": "completed",
                   "task_id": "environment-live-" + trial, "output": "actual fixture receipt " + call_id}
                  for call_id, call in calls.items()]
        return calls, events

    def save_metadata(self, **changes):
        event = next(event for event in self.events if event["step_id"] == "process")
        metadata = {"verified_hash": live.bundle_hash(self.bundle), "verified_run": "environment-live-create",
                    "evidence": {"source": "native_current_run", "run": "environment-live-create",
                                 "receipts": [{"source": "native_tool_result", "tool_call_id": "process",
                                               "tool_name": self.calls["process"]["name"], "is_error": False,
                                               "output_hash": hashlib.sha256(event["output"].encode()).hexdigest()}]}}
        metadata.update(changes)
        write_json(self.bundle / ".handwave-skill.json", metadata)
        return metadata

    def validate(self, trial="create", defer=True, calls=None, events=None, before=None, audit=None):
        return live.validate_trial(trial, self.work, self.root / "state",
                                   self.expected["first" if trial == "create" else "second"],
                                   self.calls if calls is None else calls, self.events if events is None else events,
                                   "ERROR invalid_records=2" if trial == "create" else "ERROR invalid_records=3",
                                   [] if before is None else before, defer_invocation_audit=defer,
                                   audit_directory=audit)

    def test_strict_unknown_fails_deferred_unknown_requires_review(self):
        with self.assertRaises(AssertionError):
            self.validate(defer=False)
        result = self.validate()
        self.assertEqual("REVIEW_REQUIRED", result["status"])
        self.assertEqual([], result["exercised_call_ids"])
        candidate = result["path_evidence"]["successful_execution_candidates"][0]
        self.assertEqual(self.calls["process"]["arguments"], candidate["arguments"])
        self.assertEqual("process", candidate["call_id"])
        self.assertEqual("environment-live-create", candidate["task_id"])

    def test_strict_recognized_invocation_still_passes(self):
        self.calls, self.events = self.trial_calls("create", recognized=True)
        self.save_metadata()
        self.assertEqual("PASS", self.validate(defer=False)["status"])

    def test_missing_failed_foreign_wrong_title_or_id_receipts_fail(self):
        for replacement in (None, {"status": "failed"}, {"title": "read_file"},
                            {"step_id": "foreign"}, {"task_id": "environment-live-reuse"},
                            {"is_error": True}):
            with self.subTest(replacement=replacement):
                events = deepcopy(self.events)
                events = [e for e in events if e["step_id"] != "process"] if replacement is None else events
                if replacement is not None:
                    next(e for e in events if e["step_id"] == "process").update(replacement)
                with self.assertRaises(AssertionError):
                    self.validate(events=events)

    def test_stale_or_fabricated_skill_evidence_fails(self):
        original = self.save_metadata()
        mutations = [lambda m: m.update(verified_hash="stale"),
                     lambda m: m.update(verified_run="old-run"),
                     lambda m: m["evidence"].update(source="model_claim"),
                     lambda m: m["evidence"].update(run="old-run")]
        for key, value in [("tool_call_id", "not-a-call"), ("tool_name", "exec"),
                           ("is_error", True), ("output_hash", "wrong"), ("output_hash", None),
                           ("source", "model_claim")]:
            mutations.append(lambda m, k=key, v=value: m["evidence"]["receipts"][0].update({k: v}))
        for mutate in mutations:
            metadata = deepcopy(original)
            mutate(metadata)
            write_json(self.bundle / ".handwave-skill.json", metadata)
            with self.assertRaises(AssertionError):
                self.validate()

    def test_defer_still_checks_native_creation_verification_and_knowledge_write(self):
        for name in ("skill_create", "skill_verify", "knowledge_write"):
            calls = {k: v for k, v in self.calls.items() if k != name}
            with self.subTest(name=name), self.assertRaises(AssertionError):
                self.validate(calls=calls)

    def test_defer_checks_rule_order_final_counts_and_observed_index(self):
        calls = {k: v for k, v in self.calls.items() if k != "rule"}
        with self.assertRaises(AssertionError):
            self.validate(calls=calls)
        write_json(self.work / "first-summary.json", {"valid_records": 999})
        with self.assertRaises(AssertionError):
            self.validate()
        write_json(self.work / "first-summary.json", self.expected["first"])
        note = self.work / "docs" / "knowledge" / "notes" / "workflow.md"
        note.write_bytes(b"Status: observed\nchanged after index\n")
        with self.assertRaises(AssertionError):
            self.validate()

    def test_human_rule_and_index_preserved(self):
        rule = self.work / "docs" / "knowledge" / "log-format.md"
        rule.write_bytes(b"changed\n")
        with self.assertRaises(AssertionError):
            self.validate()

        rule.write_bytes(live.RULE_TEXT.encode("utf-8"))
        index = self.work / "docs" / "knowledge" / "INDEX.md"
        index.write_bytes(index.read_bytes().replace(b"Human-maintained index must remain intact.", b""))
        with self.assertRaises(AssertionError):
            self.validate()

    def test_defer_does_not_relax_final_reply_counts(self):
        with self.assertRaises(AssertionError):
            live.validate_trial("create", self.work, self.root / "state", self.expected["first"],
                                self.calls, self.events, "done", [], defer_invocation_audit=True)

    def test_reuse_requires_reads_successful_process_and_unchanged_bundle(self):
        before = live.skill_snapshot(self.work)
        calls, events = self.trial_calls("reuse")
        self.assertEqual("REVIEW_REQUIRED", self.validate("reuse", calls=calls, events=events, before=before)["status"])
        for missing in ("entry", "note", "process"):
            with self.subTest(missing=missing), self.assertRaises(AssertionError):
                self.validate("reuse", calls={k: v for k, v in calls.items() if k != missing}, events=events, before=before)
        for forbidden in ("skill_create", "skill_install"):
            with self.subTest(forbidden=forbidden), self.assertRaises(AssertionError):
                self.validate("reuse", calls=calls | {forbidden: {"name": forbidden, "arguments": {}}}, events=events, before=before)
        (self.bundle / "scripts" / "summary.py").write_bytes(b"changed\n")
        with self.assertRaises(AssertionError):
            self.validate("reuse", calls=calls, events=events, before=before)

    def test_output_can_be_redirected_without_overwriting_historical_audit(self):
        historical = self.root / "create-path-evidence.json"
        historical.write_bytes(b"historical evidence\n")
        destination = self.root / "new-audit"
        self.validate(audit=destination)
        self.assertEqual(b"historical evidence\n", historical.read_bytes())
        self.assertTrue((destination / historical.name).is_file())

    def test_suite_hard_failures_override_review_and_exit_codes(self):
        trials = [{"name": "create", "status": "REVIEW_REQUIRED"}, {"name": "reuse", "status": "PASS"}]
        self.assertEqual("REVIEW_REQUIRED", live.suite_status(trials, []))
        self.assertEqual(2, live.status_exit_code(live.suite_status(trials, [])))
        self.assertEqual("FAIL", live.suite_status(trials, [{"reason": "denied"}]))
        for status in ("FAIL", "NOT_RUN", None):
            bad = deepcopy(trials)
            bad[1]["status"] = status
            self.assertEqual("FAIL", live.suite_status(bad, []))
        self.assertEqual("FAIL", live.suite_status(trials[:1], []))
        self.assertEqual("PASS", live.suite_status([{"name": "create", "status": "PASS"}, {"name": "reuse", "status": "PASS"}], []))
        self.assertEqual(0, live.status_exit_code("PASS"))
        self.assertEqual(1, live.status_exit_code("FAIL"))


if __name__ == "__main__":
    unittest.main()
