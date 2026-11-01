use std::path::Path;

use ara_agent_lite_harness::{TaskKind, TaskOptions, TaskSpec};
use ara_lite_host::{journal::Journal, types::*};

fn input(event_id: &str) -> InputEvent {
    InputEvent {
        source: "local-operator".into(),
        event_id: event_id.into(),
        actor: "operator".into(),
        authorization_revision: "grant-v1".into(),
        explicit: true,
        body: EventBody::Followup {
            task_id: "task-a".into(),
            generation: 1,
            text: "Inspect the original receipt.".into(),
        },
    }
}

fn binding(root: &Path) -> ExecutionBinding {
    ExecutionBinding {
        spec: TaskSpec {
            task_id: "task-a".into(),
            criteria_revision: "criteria-v1".into(),
            authorization_revision: "grant-v1".into(),
            kind: TaskKind::Reasoning,
            evidence_dir: root.join("evidence"),
            max_runs: 3,
            max_calls: 12,
            deadline_unix_ms: now_ms() + 60_000,
        },
        options: TaskOptions::default(),
        session_path: root.join("session.json"),
        environment_path: root.join("environment"),
        workspace: root.into(),
        generation: 1,
        phase: ExecutionPhase::Running,
        attempt: 1,
        adapter_revision: "adapter-v1".into(),
        initial_run_id: "run-original".into(),
    }
}

#[test]
fn event_identity_and_content_survive_reopen_without_requeue() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("host.db");
    let first = input("event-1");
    let key;
    {
        let journal = Journal::open(&path).unwrap();
        let (record, inserted) = journal.ingest(&first).unwrap();
        key = record.key;
        assert!(inserted);
        assert!(!journal.ingest(&first).unwrap().1);
        let mut changed = first.clone();
        changed.actor = "another-operator".into();
        assert!(journal.ingest(&changed).is_err());
        journal.bind_event(&key, "task-a", 1).unwrap();
        assert!(journal.bind_event(&key, "task-b", 1).is_err());
        assert!(journal.bind_event(&key, "task-a", 2).is_err());
        journal.mark_event(&key, DeliveryState::Dispatching, "before steering submission").unwrap();
        assert!(Journal::open(&path).is_err(), "a second owner must not recover an active journal");
    }
    let journal = Journal::open(&path).unwrap();
    let (record, inserted) = journal.ingest(&first).unwrap();
    assert!(!inserted);
    assert_eq!(record.key, key);
    assert_eq!(record.state, DeliveryState::Unknown);
    assert_eq!(record.task_id.as_deref(), Some("task-a"));
    assert!(journal.events(DeliveryState::Pending).unwrap().is_empty());
    assert_eq!(journal.events(DeliveryState::Unknown).unwrap().len(), 1);
    assert!(journal.mark_event(&key, DeliveryState::Pending, "retry without evidence").is_err());
    journal.mark_event(&key, DeliveryState::Unapplied, "operator inspected the original source").unwrap();
    assert!(journal.mark_event(&key, DeliveryState::Dispatching, "retry terminal event").is_err());
}

#[test]
fn binding_terms_are_immutable_and_restart_marks_active_execution_uncertain() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("host.db");
    let original = binding(dir.path());
    {
        let journal = Journal::open(&path).unwrap();
        journal.put_binding(&original).unwrap();
        let mut changed = original.clone();
        changed.spec.max_calls += 1;
        assert!(journal.put_binding(&changed).is_err());
        changed = original.clone();
        changed.spec.deadline_unix_ms += 1;
        assert!(journal.put_binding(&changed).is_err());
        changed = original.clone();
        changed.spec.authorization_revision = "grant-v2".into();
        assert!(journal.put_binding(&changed).is_err());
        changed = original.clone();
        changed.session_path = dir.path().join("replacement.json");
        assert!(journal.put_binding(&changed).is_err());
        changed = original.clone();
        changed.workspace = dir.path().join("different-workspace");
        assert!(journal.put_binding(&changed).is_err());
        changed = original.clone();
        changed.generation = 2;
        assert!(journal.put_binding(&changed).is_err());
        changed = original.clone();
        changed.adapter_revision = "adapter-v2".into();
        assert!(journal.put_binding(&changed).is_err());
        changed = original.clone();
        changed.initial_run_id = "replacement-run".into();
        assert!(journal.put_binding(&changed).is_err());
        changed = original.clone();
        changed.spec.evidence_dir = dir.path().join("different-evidence-same-attempt");
        assert!(journal.put_binding(&changed).is_err());
        changed = original.clone();
        changed.spec.evidence_dir = dir.path().join("resume-evidence");
        changed.attempt = 2;
        journal.put_binding(&changed).unwrap();
    }
    let journal = Journal::open(&path).unwrap();
    let restored = journal.binding("task-a").unwrap().unwrap();
    assert_eq!(restored.phase, ExecutionPhase::NeedsReconciliation);
    assert_eq!(restored.attempt, 2);
    assert_eq!(restored.spec.max_calls, original.spec.max_calls);
    assert_eq!(restored.spec.deadline_unix_ms, original.spec.deadline_unix_ms);
    assert_eq!(restored.session_path, original.session_path);
    assert_eq!(journal.bindings().unwrap().len(), 1);
}

#[test]
fn queued_and_finished_bindings_keep_their_phase_on_reopen() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("host.db");
    {
        let journal = Journal::open(&path).unwrap();
        let mut item = binding(dir.path());
        item.phase = ExecutionPhase::Queued;
        journal.put_binding(&item).unwrap();
        item.spec.task_id = "finished-task".into();
        item.phase = ExecutionPhase::Finished;
        journal.put_binding(&item).unwrap();
    }
    let journal = Journal::open(&path).unwrap();
    assert_eq!(journal.binding("task-a").unwrap().unwrap().phase, ExecutionPhase::Queued);
    assert_eq!(journal.binding("finished-task").unwrap().unwrap().phase, ExecutionPhase::Finished);
}

#[test]
fn questions_answers_and_notices_are_idempotent_and_crash_does_not_resend() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("host.db");
    let attention = Attention {
        id: "question-1".into(),
        task_id: "task-a".into(),
        generation: 1,
        question: "Which artifact should be checked?".into(),
        answer: None,
    };
    let notice = Notice {
        id: "notice-1".into(),
        task_id: "task-a".into(),
        generation: 1,
        kind: "attention".into(),
        body: "One decision is needed.".into(),
        state: NoticeState::Pending,
    };
    {
        let journal = Journal::open(&path).unwrap();
        journal.put_attention(&attention).unwrap();
        journal.put_attention(&attention).unwrap();
        journal.answer("question-1", "task-a", 1, "artifact-a").unwrap();
        journal.answer("question-1", "task-a", 1, "artifact-a").unwrap();
        assert!(journal.answer("question-1", "task-a", 1, "artifact-b").is_err());
        assert!(journal.answer("question-1", "task-a", 2, "artifact-a").is_err());
        assert!(journal.answer("question-1", "task-b", 1, "artifact-a").is_err());
        journal.put_attention(&attention).unwrap();
        assert_eq!(journal.attention("question-1").unwrap().unwrap().answer.as_deref(), Some("artifact-a"));
        journal.put_notice(&notice).unwrap();
        journal.put_notice(&notice).unwrap();
        let mut changed = notice.clone();
        changed.body = "Another body under the same id".into();
        assert!(journal.put_notice(&changed).is_err());
        journal.mark_notice("notice-1", NoticeState::Dispatching).unwrap();
    }
    let journal = Journal::open(&path).unwrap();
    assert_eq!(journal.attention("question-1").unwrap().unwrap().answer.as_deref(), Some("artifact-a"));
    assert!(journal.notices(NoticeState::Pending).unwrap().is_empty());
    assert_eq!(journal.notices(NoticeState::Unknown).unwrap().len(), 1);
    assert!(journal.mark_notice("notice-1", NoticeState::Pending).is_err());
    journal.put_notice(&notice).unwrap();
    assert_eq!(journal.notices(NoticeState::Unknown).unwrap().len(), 1, "duplicate input must not reset delivery");
    assert!(journal.mark_notice("notice-1", NoticeState::Sent).is_err());
    assert!(journal.settle_notice("notice-1", " ").is_err());
    journal.settle_notice("notice-1", "operator checked destination receipt delivery-001").unwrap();
    journal.settle_notice("notice-1", "operator checked destination receipt delivery-001").unwrap();
    assert!(journal.settle_notice("notice-1", "another receipt").is_err());
    assert_eq!(journal.notices(NoticeState::Sent).unwrap().len(), 1);
    assert_eq!(journal.notice("notice-1").unwrap().unwrap().state, NoticeState::Sent);
}

#[test]
fn evidence_source_changes_require_new_version_and_revoke_persistently() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("host.db");
    let original = EvidenceSource {
        id: "source-a".into(),
        task_id: "task-a".into(),
        generation: 1,
        version: 1,
        path: dir.path().join("evidence.txt"),
        sha256: "a".repeat(64),
        allowed_tasks: vec!["task-b".into()],
        active: true,
    };
    {
        let journal = Journal::open(&path).unwrap();
        journal.put_source(&original).unwrap();
        journal.put_source(&original).unwrap();
        let mut changed = original.clone();
        changed.path = dir.path().join("replacement.txt");
        assert!(journal.put_source(&changed).is_err());
        changed = original.clone();
        changed.active = false;
        assert!(journal.put_source(&changed).is_err());
        changed.version = 2;
        journal.put_source(&changed).unwrap();
        assert!(journal.put_source(&original).is_err(), "an old active version cannot restore a revoked source");
        changed.version = 3;
        changed.task_id = "task-c".into();
        assert!(journal.put_source(&changed).is_err());
    }
    let journal = Journal::open(&path).unwrap();
    let restored = journal.source("source-a").unwrap().unwrap();
    assert!(!restored.active);
    assert_eq!(restored.version, 2);
    assert_eq!(journal.sources().unwrap().len(), 1);
}

#[test]
fn invalid_events_are_rejected_before_retaining_identity() {
    let dir = tempfile::tempdir().unwrap();
    let journal = Journal::open(&dir.path().join("host.db")).unwrap();
    let mut event = input("bad-event");
    event.source.clear();
    assert!(journal.ingest(&event).is_err());
    event = input("bad-event");
    event.body = EventBody::Followup { task_id: "task-a".into(), generation: 1, text: "x".repeat(65_537) };
    assert!(journal.ingest(&event).is_err());
    event = input("bad-event");
    assert!(journal.ingest(&event).unwrap().1);
    assert_eq!(journal.events(DeliveryState::Pending).unwrap().len(), 1);
}

#[test]
fn completion_and_result_outbox_commit_atomically_and_repeat_idempotently() {
    let dir = tempfile::tempdir().unwrap();
    let journal = Journal::open(&dir.path().join("host.db")).unwrap();
    let mut item = binding(dir.path());
    journal.put_binding(&item).unwrap();
    let mut notice = Notice {
        id: "result-task-a-1".into(),
        task_id: "task-a".into(),
        generation: 1,
        kind: "result".into(),
        body: "Original settlement.".into(),
        state: NoticeState::Pending,
    };
    journal.put_notice(&notice).unwrap();
    item.phase = ExecutionPhase::Finished;
    notice.body = "A conflicting settlement under the same ID.".into();
    assert!(journal.complete(&item, &notice).is_err());
    assert_eq!(
        journal.binding("task-a").unwrap().unwrap().phase,
        ExecutionPhase::Running,
        "outbox conflict must roll back terminal binding publication"
    );
    notice.body = "Original settlement.".into();
    journal.complete(&item, &notice).unwrap();
    journal.complete(&item, &notice).unwrap();
    assert_eq!(journal.binding("task-a").unwrap().unwrap().phase, ExecutionPhase::Finished);
    assert_eq!(journal.notices(NoticeState::Pending).unwrap().len(), 1);
    journal.mark_notice(&notice.id, NoticeState::Dispatching).unwrap();
    journal.mark_notice(&notice.id, NoticeState::Sent).unwrap();
    journal.complete(&item, &notice).unwrap();
    assert_eq!(
        journal.notice(&notice.id).unwrap().unwrap().state,
        NoticeState::Sent,
        "completion replay cannot resend an acknowledged result"
    );
}
