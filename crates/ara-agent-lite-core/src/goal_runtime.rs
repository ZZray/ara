//! Host-issued goal leases and monotonic root accounting. No model can extend a lease.
use anyhow::{ensure, Result};
use serde::{Deserialize, Serialize};
use std::time::{SystemTime, UNIX_EPOCH};

pub fn now() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).unwrap_or_default().as_secs()
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct Lease {
    pub goal_id: String,
    pub controller: String,
    pub device: String,
    pub session: String,
    pub expires_at: u64,
    pub max_calls: u32,
    pub max_slices: u32,
    pub allow_jobs: bool,
}
impl Lease {
    pub fn validate(&self, controller: &str, device: &str, session: &str, goal: &str, time: u64) -> Result<()> {
        ensure!(!goal.is_empty() && self.goal_id == goal, "goal lease identity mismatch");
        ensure!(!controller.is_empty() && self.controller == controller, "goal lease controller mismatch");
        ensure!(!device.is_empty() && self.device == device, "goal lease device mismatch");
        ensure!(!session.is_empty() && self.session == session, "goal lease Session mismatch");
        ensure!(
            self.expires_at > time && self.expires_at <= time.saturating_add(86400),
            "goal lease expired or exceeds 24 hours"
        );
        ensure!((1..=256).contains(&self.max_calls), "goal root call limit must be 1..256");
        ensure!((1..=32).contains(&self.max_slices), "goal slice limit must be 1..32");
        Ok(())
    }
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct Budget {
    pub lease: Lease,
    pub calls_reserved: u32,
    pub slices_started: u32,
    pub last_started_run: String,
    /// Unknown usage is deliberately never refunded after transport failure.
    pub usage_known: bool,
}
impl Budget {
    pub fn new(lease: Lease) -> Self {
        Self { lease, calls_reserved: 0, slices_started: 0, last_started_run: String::new(), usage_known: false }
    }
    pub fn admit_run(&mut self, run: &str, time: u64) -> Result<()> {
        ensure!(!run.is_empty(), "goal slice requires Run identity");
        ensure!(self.lease.expires_at > time, "goal_authorization_expired");
        ensure!(self.calls_reserved < self.lease.max_calls, "goal_root_budget_exhausted");
        ensure!(self.last_started_run != run, "goal slice already admitted; inspect its receipt");
        ensure!(self.slices_started < self.lease.max_slices, "goal_slice_budget_exhausted");
        self.slices_started += 1;
        self.last_started_run = run.into();
        Ok(())
    }
    /// Caller must synchronously commit the surrounding Session before HTTP.
    pub fn reserve_call(&mut self, time: u64) -> Result<()> {
        ensure!(self.lease.expires_at > time, "goal_authorization_expired");
        ensure!(self.calls_reserved < self.lease.max_calls, "goal_root_budget_exhausted");
        self.calls_reserved += 1;
        Ok(())
    }
    pub fn can_continue(&self, time: u64) -> bool {
        self.lease.expires_at > time
            && self.calls_reserved < self.lease.max_calls
            && self.slices_started < self.lease.max_slices
    }
}
