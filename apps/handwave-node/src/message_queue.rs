//! Connection-owned projection and management of the transport's single FIFO.
//! No durable queue, fresh authority, or independent dispatcher is introduced.
use crate::wire::{self, Frame};
use anyhow::{ensure, Result};
use serde_json::{json, Value};
use std::collections::VecDeque;
use std::sync::{Arc, Mutex};

const CAPACITY: usize = 16;
pub const MAX_MESSAGE_BYTES: usize = 8 * 1024;
const COMMAND_RECEIPTS: usize = 128;

pub use ara_agent_lite_core::steering::SteeringBuffer;

fn valid_session(key: &str) -> bool {
    let parts = key.split('/').collect::<Vec<_>>();
    parts.len() == 2
        && parts.iter().all(|p| {
            !p.is_empty() && p.len() <= 96 && p.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-')
        })
}
fn validate_text(text: &str) -> Result<()> {
    ensure!(!text.trim().is_empty(), "queue message cannot be empty");
    ensure!(text.len() <= MAX_MESSAGE_BYTES, "queue message exceeds 8192 UTF-8 bytes");
    Ok(())
}
fn management_text(text: &str) -> bool {
    matches!(text.split_whitespace().next(), Some("/memory" | "/reference" | "/goal"))
}

pub struct Queue {
    entries: VecDeque<Frame>,
    epoch: String,
    revision: u64,
    active_id: String,
    session: String,
    outcomes: Arc<Mutex<Vec<Value>>>,
    commands: VecDeque<(String, String)>,
}
impl Queue {
    pub fn new() -> Self {
        Self {
            entries: VecDeque::new(),
            epoch: wire::fresh_id(),
            revision: 1,
            active_id: String::new(),
            session: String::new(),
            outcomes: Arc::default(),
            commands: VecDeque::new(),
        }
    }
    fn changed(&mut self) {
        self.revision = self.revision.checked_add(1).expect("connection queue revision exhausted");
    }
    pub fn len(&self) -> usize {
        self.entries.len()
    }
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }
    pub fn iter(&self) -> impl Iterator<Item = &Frame> {
        self.entries.iter()
    }
    pub fn push_back(&mut self, mut frame: Frame) -> Result<()> {
        // agent-loop consumes only task.objective. Keep one effective text in
        // pending messages so a successful msg edit cannot execute stale task
        // text instead. This does not touch the separate agent-task command.
        if let Some(objective) = frame.task.as_ref().and_then(|task| task["objective"].as_str()) {
            frame.msg = objective.to_string();
        }
        frame.task = None;
        validate_text(&frame.msg)?;
        ensure!(self.entries.len() < CAPACITY, "follow-up queue full");
        ensure!(!self.entries.iter().any(|e| e.id == frame.id), "duplicate queue entry");
        if !self.active_id.is_empty() || !self.entries.is_empty() {
            ensure!(frame.session_key == self.session, "queue Session differs from the active Run");
        }
        self.session = frame.session_key.clone();
        self.entries.push_back(frame);
        self.changed();
        Ok(())
    }
    pub fn pop_front(&mut self) -> Option<Frame> {
        let frame = self.entries.pop_front();
        if frame.is_some() {
            self.changed();
        }
        frame
    }
    pub fn take_dispatch(&mut self, id: &str) {
        if let Some(index) = self.entries.iter().position(|f| f.id == id) {
            self.entries.remove(index);
            self.changed();
        }
    }
    pub fn clear(&mut self) {
        if !self.entries.is_empty() {
            self.entries.clear();
            self.changed();
        }
    }
    pub fn remove_session(&mut self, key: &str) {
        let old = self.entries.len();
        self.entries.retain(|f| f.session_key != key);
        if self.entries.len() != old {
            self.changed();
        }
    }
    pub fn reset(&mut self) {
        *self = Self::new();
    }
    pub fn renew_epoch(&mut self) {
        self.clear();
        self.epoch = wire::fresh_id();
        self.revision = 1;
        self.commands.clear();
    }
    pub fn sync_active(&mut self, frame: Option<&Frame>) {
        let frame = frame.filter(|f| f.k == "agent-loop");
        let next = frame.map(|f| f.id.as_str()).unwrap_or_default();
        if self.active_id != next {
            self.active_id = next.to_string();
            if let Some(frame) = frame {
                self.session = frame.session_key.clone();
            }
            self.changed();
        }
    }
    pub fn buffer(&self) -> SteeringBuffer {
        SteeringBuffer::with_outcomes(self.outcomes.clone())
    }
    pub fn drain_outcomes(&self) -> Vec<Value> {
        std::mem::take(&mut *self.outcomes.lock().unwrap())
    }
    pub fn session(&self) -> &str {
        &self.session
    }
    pub fn snapshot(&self, session: &str, id: &str, error: Option<&str>) -> Value {
        let visible = session == self.session;
        let items = if visible {
            self.entries.iter().map(|f| json!({"id":f.id,"msg":f.msg})).collect::<Vec<_>>()
        } else {
            vec![]
        };
        let mut value = json!({"k":"agent-queue-state","id":id,"queue_epoch":self.epoch,"revision":self.revision,"session_key":session,
            "active_run_id":if visible{self.active_id.as_str()}else{""},"items":items,"ok":error.is_none()});
        if let Some(error) = error {
            value["error"] = json!(error);
        }
        value
    }
    fn signature(frame: &Frame) -> String {
        let value = json!({"op":frame.queue_op,"queue_id":frame.queue_id,"target_run_id":frame.target_run_id,"session_key":frame.session_key,
            "queue_epoch":frame.queue_epoch,"revision":frame.expected_revision,"msg":frame.msg});
        crate::context_memory::hash(&serde_json::to_vec(&value).expect("queue command serializes"))
    }
    pub fn command(
        &mut self,
        frame: &Frame,
        active: Option<&Frame>,
        live: bool,
        buffer: Option<&Arc<Mutex<SteeringBuffer>>>,
    ) -> (Value, Option<Value>) {
        let result = self.apply(frame, active, live, buffer);
        match result {
            Ok(pair) => pair,
            Err(error) => (self.snapshot(&frame.session_key, &frame.id, Some(&error.to_string())), None),
        }
    }
    fn apply(
        &mut self,
        frame: &Frame,
        active: Option<&Frame>,
        live: bool,
        buffer: Option<&Arc<Mutex<SteeringBuffer>>>,
    ) -> Result<(Value, Option<Value>)> {
        ensure!(valid_session(&frame.session_key), "queue requires an explicit valid Session");
        ensure!(frame.id.len() <= 256 && !frame.id.is_empty(), "queue command identity is required");
        if !self.active_id.is_empty() || !self.entries.is_empty() {
            ensure!(frame.session_key == self.session, "queue belongs to another Session");
        }
        if frame.queue_op == "list" {
            ensure!(
                frame.queue_epoch.is_empty() || frame.queue_epoch == self.epoch,
                "queue connection changed; refresh before acting"
            );
            return Ok((self.snapshot(&frame.session_key, &frame.id, None), None));
        }
        ensure!(frame.queue_epoch == self.epoch, "queue connection changed; refresh before acting");
        let signature = Self::signature(frame);
        if let Some((_, old)) = self.commands.iter().find(|(id, _)| id == &frame.id) {
            ensure!(signature == *old, "queue command identity reused with different content");
            return Ok((self.snapshot(&frame.session_key, &frame.id, None), None));
        }
        ensure!(frame.expected_revision == Some(self.revision), "queue revision changed; refresh before acting");
        ensure!(frame.target_run_id == self.active_id, "active Run changed; refresh before acting");
        ensure!(
            matches!(frame.queue_op.as_str(), "remove" | "edit" | "front" | "steer"),
            "unsupported queue operation"
        );
        let index = self
            .entries
            .iter()
            .position(|e| e.id == frame.queue_id)
            .ok_or_else(|| anyhow::anyhow!("queued item is no longer pending; inspect its Run instead"))?;
        let mut submitted = None;
        match frame.queue_op.as_str() {
            "edit" => {
                validate_text(&frame.msg)?;
                self.entries[index].msg = frame.msg.clone();
            }
            "remove" => {
                self.entries.remove(index);
            }
            "front" => {
                let item = self.entries.remove(index).unwrap();
                self.entries.push_front(item);
            }
            "steer" => {
                let active = active
                    .filter(|f| {
                        f.k == "agent-loop" && f.id == frame.target_run_id && f.session_key == frame.session_key
                    })
                    .ok_or_else(|| anyhow::anyhow!("no matching active agent Run"))?;
                ensure!(live, "active Run is ending; guidance was not submitted");
                let text = &self.entries[index].msg;
                validate_text(text)?;
                ensure!(!management_text(text), "memory/reference/goal management cannot be submitted as steering");
                let mut buffer = buffer.ok_or_else(|| anyhow::anyhow!("steering buffer unavailable"))?.lock().unwrap();
                ensure!(buffer.len() < CAPACITY, "steering buffer is full");
                let receipt = json!({"k":"agent-queue-steer-state","queue_id":frame.queue_id,"target_run_id":active.id,"session_key":frame.session_key,"queue_epoch":self.epoch,"status":"submitted"});
                let entry = self.entries.remove(index).unwrap();
                buffer.submit(entry.msg, receipt.clone())?;
                submitted = Some(receipt);
            }
            _ => unreachable!(),
        }
        self.changed();
        let reply = self.snapshot(&frame.session_key, &frame.id, None);
        if self.commands.len() == COMMAND_RECEIPTS {
            self.commands.pop_front();
        }
        self.commands.push_back((frame.id.clone(), signature));
        Ok((reply, submitted))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn active() -> Frame {
        Frame {
            k: "agent-loop".into(),
            id: "run-a".into(),
            session_key: "w/s".into(),
            execution_mode: "execute".into(),
            ..Frame::default()
        }
    }
    fn entry(id: &str) -> Frame {
        Frame { id: id.into(), msg: format!("message {id}"), ..active() }
    }
    fn setup() -> (Queue, Frame, Arc<Mutex<SteeringBuffer>>) {
        let mut queue = Queue::new();
        let active = active();
        queue.sync_active(Some(&active));
        let buffer = Arc::new(Mutex::new(queue.buffer()));
        queue.push_back(entry("next-a")).unwrap();
        queue.push_back(entry("next-b")).unwrap();
        (queue, active, buffer)
    }
    fn command(queue: &Queue, op: &str, id: &str) -> Frame {
        Frame {
            k: "agent-queue".into(),
            id: format!("cmd-{op}-{id}"),
            queue_op: op.into(),
            queue_id: id.into(),
            session_key: queue.session.clone(),
            queue_epoch: queue.epoch.clone(),
            expected_revision: Some(queue.revision),
            target_run_id: queue.active_id.clone(),
            ..Frame::default()
        }
    }
    #[test]
    fn scoped_snapshots_and_bootstrap_list_never_leak_other_session() {
        let (mut q, a, b) = setup();
        let list = command(&q, "list", "");
        let (reply, _) = q.command(&list, Some(&a), true, Some(&b));
        assert_eq!(reply["items"][0]["id"], "next-a");
        assert_eq!(reply["active_run_id"], "run-a");
        let mut wrong = list.clone();
        wrong.session_key = "w/other".into();
        let (reply, _) = q.command(&wrong, Some(&a), true, Some(&b));
        assert_eq!(reply["ok"], false);
        assert_eq!(reply["items"], json!([]));
        assert_eq!(reply["active_run_id"], "");
        assert_eq!(reply["session_key"], "w/other");
        wrong.session_key = "../x".into();
        assert_eq!(q.command(&wrong, Some(&a), true, Some(&b)).0["ok"], false);
        let mut idle = Queue::new();
        wrong.session_key = "w/other".into();
        wrong.queue_epoch.clear();
        assert_eq!(idle.command(&wrong, None, false, None).0["ok"], true);
    }
    #[test]
    fn mutation_fences_epoch_revision_run_and_item_without_changes() {
        for kind in ["epoch", "revision", "run", "session", "started"] {
            let (mut q, a, b) = setup();
            let before = q.snapshot("w/s", "", None);
            let mut request = command(&q, "remove", "next-a");
            match kind {
                "epoch" => request.queue_epoch = "old".into(),
                "revision" => request.expected_revision = Some(0),
                "run" => request.target_run_id = "wrong".into(),
                "session" => request.session_key = "w/wrong".into(),
                _ => request.queue_id = "run-a".into(),
            }
            assert_eq!(q.command(&request, Some(&a), true, Some(&b)).0["ok"], false);
            assert_eq!(q.snapshot("w/s", "", None), before);
        }
    }
    #[test]
    fn edit_only_changes_text_and_front_remove_preserve_authority() {
        let (mut q, a, b) = setup();
        q.entries[0].goal_id = "original-goal".into();
        q.entries[0].goal_policy = Some(json!({"original":true}));
        let mut edit = command(&q, "edit", "next-a");
        edit.msg = "new text".into();
        edit.model = "attacker".into();
        edit.goal_id = "attacker".into();
        edit.goal_policy = Some(json!({"original":false}));
        assert_eq!(q.command(&edit, Some(&a), true, Some(&b)).0["ok"], true);
        assert_eq!(q.entries[0].msg, "new text");
        assert_eq!(q.entries[0].goal_id, "original-goal");
        assert_ne!(q.entries[0].model, "attacker");
        assert_eq!(q.entries[0].goal_policy, Some(json!({"original":true})));
        let front = command(&q, "front", "next-b");
        assert_eq!(q.command(&front, Some(&a), true, Some(&b)).0["ok"], true);
        assert_eq!(q.entries[0].id, "next-b");
        let remove = command(&q, "remove", "next-a");
        assert_eq!(q.command(&remove, Some(&a), true, Some(&b)).0["ok"], true);
        assert_eq!(q.len(), 1);
    }
    #[test]
    fn duplicate_commands_are_idempotent_and_return_current_projection() {
        let (mut q, a, b) = setup();
        let remove = command(&q, "remove", "next-a");
        assert_eq!(q.command(&remove, Some(&a), true, Some(&b)).0["ok"], true);
        let edit = Frame { msg: "changed later".into(), ..command(&q, "edit", "next-b") };
        q.command(&edit, Some(&a), true, Some(&b));
        let revision = q.revision;
        let reply = q.command(&remove, Some(&a), true, Some(&b)).0;
        assert_eq!(reply["ok"], true);
        assert_eq!(reply["revision"], revision);
        assert_eq!(reply["items"][0]["msg"], "changed later");
        let changed = Frame { queue_id: "next-b".into(), ..remove };
        assert_eq!(q.command(&changed, Some(&a), true, Some(&b)).0["ok"], false);
        assert_eq!(q.len(), 1);
    }
    #[test]
    fn bounded_utf8_queue_and_compact_command_cache() {
        let mut q = Queue::new();
        for i in 0..16 {
            q.push_back(entry(&format!("next-{i}"))).unwrap();
        }
        assert!(q.push_back(entry("overflow")).is_err());
        let mut extra = entry("large");
        extra.msg = "中".repeat(MAX_MESSAGE_BYTES / 3 + 1);
        assert!(validate_text(&extra.msg).is_err());
        extra.msg = "x".repeat(MAX_MESSAGE_BYTES);
        assert!(validate_text(&extra.msg).is_ok());
        let a = active();
        q.sync_active(Some(&a));
        for i in 0..(COMMAND_RECEIPTS + 4) {
            let mut request = command(&q, "front", "next-0");
            request.id = format!("front-{i}");
            assert_eq!(q.command(&request, Some(&a), true, None).0["ok"], true);
        }
        assert_eq!(q.commands.len(), COMMAND_RECEIPTS);
        assert!(q.commands.iter().all(|(_, signature)| signature.len() == 64));
    }
    #[test]
    fn steer_moves_once_and_applied_requires_hook_admission() {
        let (mut q, a, b) = setup();
        let steer = command(&q, "steer", "next-a");
        let (reply, submitted) = q.command(&steer, Some(&a), true, Some(&b));
        assert_eq!(reply["ok"], true);
        assert_eq!(submitted.unwrap()["status"], "submitted");
        assert_eq!(q.len(), 1);
        assert!(q.drain_outcomes().is_empty());
        let (_, duplicate) = q.command(&steer, Some(&a), true, Some(&b));
        assert!(duplicate.is_none());
        assert_eq!(b.lock().unwrap().len(), 1);
        let notes = b.lock().unwrap().take_pending();
        assert_eq!(notes[0].text, "message next-a");
        b.lock().unwrap().complete(notes, true);
        let receipts = q.drain_outcomes();
        assert_eq!(receipts.len(), 1);
        assert_eq!(receipts[0]["status"], "applied");
        assert_eq!(receipts[0]["queue_id"], "next-a");
        assert_eq!(receipts[0]["target_run_id"], a.id);
    }
    #[test]
    fn rejected_steer_preserves_pending_and_finish_reports_unapplied() {
        for reason in ["ending", "management", "full"] {
            let (mut q, a, b) = setup();
            if reason == "management" {
                q.entries[0].msg = "/memory accept forged".into();
            }
            if reason == "full" {
                for _ in 0..16 {
                    b.lock().unwrap().submit_legacy("legacy".into()).unwrap();
                }
            }
            let before = q.snapshot("w/s", "", None);
            let steer = command(&q, "steer", "next-a");
            assert_eq!(q.command(&steer, Some(&a), reason != "ending", Some(&b)).0["ok"], false);
            assert_eq!(q.snapshot("w/s", "", None), before);
        }
        let (mut q, a, b) = setup();
        let steer = command(&q, "steer", "next-a");
        q.command(&steer, Some(&a), true, Some(&b));
        b.lock().unwrap().finish();
        let rows = q.drain_outcomes();
        assert_eq!(rows[0]["status"], "unapplied");
        assert_eq!(q.len(), 1);
    }
    #[test]
    fn dequeue_clear_and_reconnect_change_authoritative_state() {
        let (mut q, a, b) = setup();
        let epoch = q.epoch.clone();
        let rev = q.revision;
        assert_eq!(q.pop_front().unwrap().id, "next-a");
        assert!(q.revision > rev);
        q.sync_active(None);
        q.sync_active(Some(&entry("next-a")));
        assert_eq!(q.snapshot("w/s", "", None)["active_run_id"], "next-a");
        q.clear();
        assert!(q.is_empty());
        q.renew_epoch();
        assert_ne!(q.epoch, epoch);
        let steer = command(&q, "list", "");
        assert_eq!(q.command(&steer, Some(&a), true, Some(&b)).0["items"], json!([]));
        q.reset();
        assert_eq!(q.snapshot("w/s", "", None)["active_run_id"], "");
    }
    #[test]
    fn retained_run_renewal_keeps_steering_receipt_sink() {
        let (mut q, a, b) = setup();
        q.renew_epoch();
        q.push_back(entry("next-new")).unwrap();
        let steer = command(&q, "steer", "next-new");
        assert_eq!(q.command(&steer, Some(&a), true, Some(&b)).0["ok"], true);
        let notes = b.lock().unwrap().take_pending();
        b.lock().unwrap().complete(notes, true);
        assert_eq!(q.drain_outcomes()[0]["status"], "applied");
    }
    #[test]
    fn pending_structured_objective_has_one_editable_effective_text() {
        let (mut q, a, b) = setup();
        let mut item = entry("structured");
        item.msg = "misleading display".into();
        item.task = Some(json!({"objective":"actual original text","steps":["unused agent-loop task field"]}));
        q.push_back(item).unwrap();
        let view = q.snapshot("w/s", "", None);
        assert_eq!(view["items"][2]["msg"], "actual original text");
        let edit = Frame { msg: "actual edited text".into(), ..command(&q, "edit", "structured") };
        assert_eq!(q.command(&edit, Some(&a), true, Some(&b)).0["ok"], true);
        let saved = q.entries.iter().find(|frame| frame.id == "structured").unwrap();
        assert_eq!(saved.msg, "actual edited text");
        assert!(saved.task.is_none());
    }
}
