//! Experimental Host-side management of a bounded batch of background Tasks.
//!
//! The Host selects an explicit Task ID and authorizes every message before
//! submission. This module neither interprets messages nor grants authority.
//! Completed IDs and message digests remain until the set is dropped: capacity
//! bounds the whole batch, not just currently running Tasks. A durable scheduler
//! should keep its own identities, inbox and recovery records outside this set.

use std::{
    collections::BTreeMap,
    sync::{Arc, Mutex},
};

use anyhow::{ensure, Context, Result};
use futures::{future::BoxFuture, stream::FuturesUnordered, StreamExt};
use serde::{Deserialize, Serialize};
use serde_json::json;
use sha2::{Digest, Sha256};

use crate::{
    core::{self, steering::SteeringBuffer, RunRequest},
    HarnessHost, TaskHandle, TaskOptions, TaskOutcome, TaskSpec,
};

/// Fixed per-message limits, independent of the batch's configured capacities.
pub const MAX_GUIDANCE_BYTES: usize = 16 * 1024;
pub const MAX_MESSAGE_ID_BYTES: usize = 256;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Submission {
    /// Accepted into the original steering buffer; consumption is not promised.
    Queued,
    /// The identical message was already queued in this batch. No second input.
    Duplicate,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum GuidanceStatus {
    /// Saved and admitted to execution context, not necessarily read by the model.
    AppliedToContext,
    Unapplied,
    /// A receipt is missing or unrecognized. The Host must not assume delivery.
    Unknown,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct GuidanceReceipt {
    pub message_id: String,
    pub status: GuidanceStatus,
}

#[derive(Clone, Debug)]
pub struct Completion {
    /// The original, unmodified Harness outcome, including its acceptance boundary.
    pub outcome: Arc<TaskOutcome>,
    /// Batch-owned guidance only; unrelated Host receipts remain untouched.
    pub guidance: Vec<GuidanceReceipt>,
}

struct Entry {
    handle: TaskHandle,
    steering: Arc<Mutex<SteeringBuffer>>,
    messages: BTreeMap<String, [u8; 32]>,
}

/// Nonblocking dispatch and directed follow-ups over the actual Harness.
///
/// Every Task needs its own steering buffer. The Host delegates submission and
/// terminal `finish` of that buffer to this set, and reuses the exact same Arc in
/// `prepare_continuation`. External writes bypass this module's deduplication.
/// Dropping the set or a completion waiter does NOT cancel Tasks. Use `cancel`
/// (or `cancel_all`), then collect completions to join original effects. Neither
/// queued messages nor batch deduplication survive process exit.
pub struct TaskSet {
    id: String,
    max_tasks: usize,
    max_messages_per_task: usize,
    tasks: BTreeMap<String, Entry>,
    completions: FuturesUnordered<BoxFuture<'static, Arc<TaskOutcome>>>,
}

impl TaskSet {
    pub fn new(max_tasks: usize, max_messages_per_task: usize) -> Result<Self> {
        ensure!(max_tasks > 0 && max_messages_per_task > 0, "positive TaskSet capacities required");
        Ok(Self {
            id: core::fresh_id(),
            max_tasks,
            max_messages_per_task,
            tasks: BTreeMap::new(),
            completions: FuturesUnordered::new(),
        })
    }

    /// Starts immediately or refuses admission; there is no hidden launch queue.
    /// The existing Harness still checks Session ownership, budgets and authority.
    pub fn start(
        &mut self,
        spec: TaskSpec,
        initial: RunRequest,
        host: Arc<dyn HarnessHost>,
        options: TaskOptions,
    ) -> Result<TaskHandle> {
        ensure!(!self.tasks.contains_key(&spec.task_id), "Task ID already belongs to this batch");
        ensure!(self.tasks.len() < self.max_tasks, "TaskSet batch capacity reached");
        ensure!(
            !self.tasks.values().any(|entry| Arc::ptr_eq(&entry.steering, &initial.steering)),
            "each TaskSet Task requires its own steering buffer"
        );
        let task_id = spec.task_id.clone();
        let steering = initial.steering.clone();
        let handle = crate::start_with_options(spec, initial, host, options)?;
        let waiter = handle.clone();
        self.completions.push(Box::pin(async move { waiter.wait().await }));
        self.tasks.insert(task_id, Entry { handle: handle.clone(), steering, messages: BTreeMap::new() });
        Ok(handle)
    }

    /// Queue already-authorized, in-scope guidance for an explicit original Task.
    /// A duplicate returns its previous submission identity even after completion.
    /// A terminal race can return Queued followed by an Unapplied completion
    /// receipt. It never starts a Run, replenishes a budget or wakes an ended Task.
    pub fn guide(&mut self, task_id: &str, message_id: &str, text: &str) -> Result<Submission> {
        ensure!(!message_id.trim().is_empty() && message_id.len() <= MAX_MESSAGE_ID_BYTES, "invalid message ID");
        ensure!(!text.trim().is_empty() && text.len() <= MAX_GUIDANCE_BYTES, "invalid guidance size");
        let entry = self.tasks.get_mut(task_id).context("unknown Task ID")?;
        let digest: [u8; 32] = Sha256::digest(text.as_bytes()).into();
        if let Some(previous) = entry.messages.get(message_id) {
            ensure!(*previous == digest, "message ID reused with different content");
            return Ok(Submission::Duplicate);
        }
        ensure!(entry.handle.accepts_guidance(), "Task is finished or cancellation was requested");
        ensure!(entry.messages.len() < self.max_messages_per_task, "Task guidance capacity reached");
        entry
            .steering
            .lock()
            .unwrap()
            .submit(text.into(), json!({"task_set_id":self.id,"task_id":task_id,"message_id":message_id}))?;
        entry.messages.insert(message_id.into(), digest);
        Ok(Submission::Queued)
    }

    pub fn cancel(&self, task_id: &str) -> Result<()> {
        self.tasks.get(task_id).context("unknown Task ID")?.handle.cancel();
        Ok(())
    }

    pub fn cancel_all(&self) {
        for entry in self.tasks.values() {
            entry.handle.cancel();
        }
    }

    /// Receive one settled Task without blocking the Host's other work.
    /// This wait is cancellation-safe: dropping its future loses no completion.
    /// Returns None when every started Task has been collected (or none started).
    pub async fn next_completed(&mut self) -> Option<Completion> {
        let outcome = self.completions.next().await?;
        let entry = &self.tasks[&outcome.task_id];
        // Only after Core and Host callbacks joined may pending inputs be closed.
        // A message submitted during Host review can legitimately remain unapplied.
        let mut steering = entry.steering.lock().unwrap();
        steering.finish();
        let receipts = steering.outcomes_snapshot();
        let guidance = entry
            .messages
            .keys()
            .map(|message_id| {
                let status = receipts
                    .iter()
                    .find(|receipt| {
                        receipt["task_set_id"] == self.id
                            && receipt["task_id"] == outcome.task_id
                            && receipt["message_id"] == *message_id
                    })
                    .map(|receipt| match receipt["status"].as_str() {
                        Some("applied") => GuidanceStatus::AppliedToContext,
                        Some("unapplied") => GuidanceStatus::Unapplied,
                        _ => GuidanceStatus::Unknown,
                    })
                    .unwrap_or(GuidanceStatus::Unknown);
                GuidanceReceipt { message_id: message_id.clone(), status }
            })
            .collect();
        Some(Completion { outcome, guidance })
    }
}
