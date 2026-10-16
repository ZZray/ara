//! Host-issued goal leases and monotonic root accounting. No model can extend a lease.
use crate::wire::Frame;
use anyhow::{ensure, Result};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
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

/// One Node owns this durable dispatch list under the existing OS node.lock.
/// These records carry no bearer token. Every dispatch still asks the Hub for a
/// new short Run grant; a stored lease is never sufficient to call a provider.
#[derive(Clone, Debug, Serialize, Deserialize)]
struct Entry {
    frame: Frame,
    lease: Lease,
    status: String,
    revision: u64,
    next_at: u64,
    #[serde(default)]
    last_run: String,
}
pub struct Runner {
    path: PathBuf,
    entries: Vec<Entry>,
    pub writer_epoch: String,
}
impl Runner {
    pub fn load(root: &Path) -> Result<Self> {
        use std::io::Read;
        let path = root.join("goal-runner.json");
        let entries = if path.exists() {
            let mut bytes = Vec::new();
            std::fs::File::open(&path)?.take(256 * 1024 + 1).read_to_end(&mut bytes)?;
            ensure!(bytes.len() <= 256 * 1024, "goal runner manifest exceeds 256 KiB");
            let entries: Vec<Entry> = serde_json::from_slice(&bytes)?;
            ensure!(entries.len() <= 8, "goal runner has more than 8 retained goals");
            entries
        } else {
            Vec::new()
        };
        Ok(Self { path, entries, writer_epoch: crate::wire::fresh_id() })
    }
    fn save(&self) -> Result<()> {
        let bytes = serde_json::to_vec(&self.entries)?;
        ensure!(bytes.len() <= 256 * 1024, "goal runner manifest exceeds 256 KiB; original dispatch manifest retained");
        crate::session_journal::atomic_write(&self.path, &bytes)
    }
    pub fn register(&mut self, frame: &Frame, lease: &Lease) -> Result<()> {
        if let Some(entry) = self.entries.iter().find(|e| e.lease.goal_id == lease.goal_id) {
            ensure!(
                entry.lease == *lease && entry.frame.session_key == frame.session_key,
                "goal dispatch contract changed; original goal retained"
            );
            return Ok(());
        }
        ensure!(
            !self.entries.iter().any(|e| e.lease.controller == lease.controller
                && e.lease.session == lease.session
                && !matches!(e.status.as_str(), "accepted" | "canceled")),
            "another long goal owns this Session; stop or accept it first"
        );
        self.entries.retain(|e| !matches!(e.status.as_str(), "accepted" | "canceled"));
        ensure!(self.entries.len() < 8, "8 retained long goals; inspect existing goals before adding another");
        // Store only dispatch identity. The native Session owns objective,
        // criteria and sourced context; never retain an upload or cloud body.
        let saved = Frame {
            k: "agent-loop".into(),
            id: frame.id.clone(),
            model: frame.model.clone(),
            protocol: frame.protocol.clone(),
            smart: frame.smart,
            session_key: frame.session_key.clone(),
            execution_mode: "goal".into(),
            goal_id: lease.goal_id.clone(),
            goal_background: true,
            ..Frame::default()
        };
        self.entries.push(Entry {
            frame: saved,
            lease: lease.clone(),
            status: "running".into(),
            revision: 0,
            next_at: 0,
            last_run: frame.id.clone(),
        });
        self.save()
    }
    pub fn observe(&mut self, session: &str, goal: &crate::execution::GoalState) -> Result<()> {
        if let Some(entry) = self.entries.iter_mut().find(|e| e.lease.goal_id == goal.id && e.lease.session == session)
        {
            if entry.status == "canceled" && goal.status != "canceled" {
                return Ok(());
            }
            // Replayed command receipts retain their original result. They must
            // not roll the live dispatch projection back to that old revision.
            if goal.revision < entry.revision {
                return Ok(());
            }
            entry.status =
                if goal.status == "running" && goal.budget.as_ref().is_some_and(|b| b.last_started_run != goal.run) {
                    "resume_ready".into()
                } else {
                    goal.status.clone()
                };
            entry.revision = goal.revision;
            entry.last_run = goal.run.clone();
            entry.next_at = now().saturating_add(2);
            self.save()?;
        }
        Ok(())
    }
    pub fn wait(&mut self, goal: &str, status: &str, seconds: u64) -> Result<()> {
        if let Some(entry) = self.entries.iter_mut().find(|e| e.lease.goal_id == goal) {
            entry.status = status.into();
            entry.next_at = now().saturating_add(seconds);
            self.save()?;
        }
        Ok(())
    }
    pub fn matching_contracts(&self, owner: &str, id: &str) -> Vec<Lease> {
        self.entries
            .iter()
            .filter(|e| {
                e.lease.controller == owner
                    && (id.is_empty() || e.lease.goal_id == id || e.frame.id == id || e.last_run == id)
            })
            .map(|e| e.lease.clone())
            .collect()
    }
    pub fn cancel_matching(&mut self, owner: &str, id: &str) -> Result<()> {
        for entry in &mut self.entries {
            if entry.lease.controller == owner
                && (id.is_empty() || entry.lease.goal_id == id || entry.frame.id == id || entry.last_run == id)
                && entry.status != "accepted"
            {
                entry.status = "canceled".into();
            }
        }
        self.save()
    }
    pub fn has_lease(&self, owner: &str) -> bool {
        self.entries.iter().any(|e| {
            e.lease.controller == owner
                && e.lease.expires_at > now()
                && !matches!(e.status.as_str(), "accepted" | "canceled" | "failed" | "budget_limited")
        })
    }
    pub fn requires_lease(&self, goal: &str) -> bool {
        self.entries.iter().any(|e| e.lease.goal_id == goal)
    }
    pub fn contract(&self, goal: &str, owner: &str, session: &str) -> Result<Lease> {
        self.entries
            .iter()
            .find(|e| e.lease.goal_id == goal && e.lease.controller == owner && e.lease.session == session)
            .map(|e| e.lease.clone())
            .ok_or_else(|| anyhow::anyhow!("no matching local goal contract"))
    }
    /// Reading a receipt does not renew execution authority. Input needs both
    /// this durable dispatch fence and the native Session's current Goal.
    pub fn job_contract(&self, goal: &str, owner: &str, session: &str, input: bool) -> Result<Lease> {
        let lease = self.contract(goal, owner, session)?;
        if input {
            let entry = self.entries.iter().find(|e| e.lease == lease).expect("contract exists");
            ensure!(
                matches!(entry.status.as_str(), "running" | "yielded" | "resume_ready" | "waiting_job"),
                "goal is not accepting process input"
            );
            ensure!(lease.allow_jobs && lease.expires_at > now(), "goal process-input authority expired or denied");
        }
        Ok(lease)
    }
    pub fn waiting_jobs(&self, owner: &str) -> Vec<Lease> {
        self.entries
            .iter()
            .filter(|e| e.lease.controller == owner && e.status == "waiting_job" && e.next_at <= now())
            .map(|e| e.lease.clone())
            .collect()
    }
    pub fn expired_dispatches(&self, owner: &str) -> Vec<Lease> {
        self.entries
            .iter()
            .filter(|e| {
                e.lease.controller == owner
                    && e.lease.expires_at <= now()
                    && matches!(
                        e.status.as_str(),
                        "running" | "yielded" | "resume_ready" | "waiting_job" | "waiting_provider" | "dispatching"
                    )
            })
            .map(|e| e.lease.clone())
            .collect()
    }
    /// Mark the selected dispatch before returning it. If admission crashes before
    /// a Session receipt, restart reports waiting_recovery instead of replaying.
    pub fn next(&mut self, owner: &str) -> Result<Option<Frame>> {
        let time = now();
        let Some(index) = self.entries.iter().position(|e| {
            e.lease.controller == owner
                && e.next_at <= time
                && e.lease.expires_at > time
                && matches!(e.status.as_str(), "yielded" | "resume_ready" | "waiting_provider")
        }) else {
            return Ok(None);
        };
        let entry = &mut self.entries[index];
        let mut frame = entry.frame.clone();
        frame.id = crate::wire::fresh_id();
        entry.last_run = frame.id.clone();
        frame.msg = "Continue the existing authorized goal from its durable receipts and unfinished steps. Verify actual effects; do not replay unknown operations. Propose completion only when criteria are evidenced.".into();
        frame.task = None;
        entry.status = "dispatching".into();
        self.save()?;
        Ok(Some(frame))
    }
    pub fn sessions(&self, owner: &str) -> Vec<String> {
        self.entries.iter().filter(|e| e.lease.controller == owner).map(|e| e.lease.session.clone()).collect()
    }
    pub fn recover(&mut self, session: &str, goal: Option<&crate::execution::GoalState>) -> Result<()> {
        for entry in self.entries.iter_mut().filter(|e| e.lease.session == session) {
            if entry.status == "canceled" {
                continue;
            }
            if let Some(goal) = goal.filter(|g| g.id == entry.lease.goal_id) {
                // Only an already settled durable yield is restart-safe.
                entry.status = if matches!(goal.status.as_str(), "yielded" | "waiting_job") {
                    goal.status.clone()
                } else if goal.status == "running" {
                    "waiting_recovery".into()
                } else {
                    goal.status.clone()
                };
                entry.revision = goal.revision;
            } else {
                entry.status = "waiting_recovery".into();
            }
        }
        self.save()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn lease() -> Lease {
        Lease {
            goal_id: "goal".into(),
            controller: "owner".into(),
            device: "device".into(),
            session: "session".into(),
            expires_at: 100,
            max_calls: 3,
            max_slices: 2,
            allow_jobs: true,
        }
    }
    #[test]
    fn root_reservations_survive_serialization_and_do_not_refund_unknown_usage() {
        let mut b = Budget::new(lease());
        b.admit_run("first", 1).unwrap();
        b.reserve_call(1).unwrap();
        b.reserve_call(1).unwrap();
        let mut b: Budget = serde_json::from_slice(&serde_json::to_vec(&b).unwrap()).unwrap();
        assert!(b.admit_run("first", 2).is_err());
        b.admit_run("second", 2).unwrap();
        b.reserve_call(2).unwrap();
        assert!(b.reserve_call(2).is_err());
        assert!(!b.can_continue(2));
        assert!(!b.usage_known);
    }
    #[test]
    fn expiry_and_owner_are_independent_of_token_ttl() {
        assert!(lease().validate("owner", "device", "session", "goal", 1).is_ok());
        assert!(lease().validate("other", "device", "session", "goal", 1).is_err());
        assert!(lease().validate("owner", "device", "session", "goal", 100).is_err());
        assert!(Budget::new(lease()).admit_run("first", 100).is_err());
    }
    #[test]
    fn canceled_dispatch_survives_stale_observation_and_restart() {
        let root = std::env::temp_dir().join(format!("goal-fence-{}", crate::wire::fresh_id()));
        std::fs::create_dir(&root).unwrap();
        let mut runner = Runner::load(&root).unwrap();
        let mut contract = lease();
        contract.expires_at = now() + 120;
        let frame = Frame {
            id: "first".into(),
            session_key: "session".into(),
            brief: "large original brief".repeat(100000),
            ..Frame::default()
        };
        runner.register(&frame, &contract).unwrap();
        assert!(std::fs::metadata(&runner.path).unwrap().len() < 4096);
        let mut goal = crate::execution::GoalState::new("goal", "verify", &["file verified".into()]).unwrap();
        goal.status = "waiting_job".into();
        goal.revision = 2;
        runner.observe("session", &goal).unwrap();
        runner.cancel_matching("owner", "first").unwrap();
        runner.observe("session", &goal).unwrap();
        let mut restarted = Runner::load(&root).unwrap();
        restarted.recover("session", Some(&goal)).unwrap();
        assert_eq!(restarted.entries[0].status, "canceled");
        assert!(restarted.job_contract("goal", "owner", "session", true).is_err());
        assert!(restarted.job_contract("goal", "owner", "session", false).is_ok());
        assert!(restarted.next("owner").unwrap().is_none());
        std::fs::remove_dir_all(root).unwrap();
    }
    #[test]
    fn process_input_requires_live_authority_and_active_dispatch() {
        let root = std::env::temp_dir().join(format!("goal-input-{}", crate::wire::fresh_id()));
        std::fs::create_dir(&root).unwrap();
        let mut runner = Runner::load(&root).unwrap();
        let mut contract = lease();
        contract.expires_at = now() + 120;
        runner
            .register(&Frame { id: "r".into(), session_key: "session".into(), ..Frame::default() }, &contract)
            .unwrap();
        assert!(runner.job_contract("goal", "owner", "session", true).is_ok());
        assert!(runner.job_contract("goal", "other", "session", true).is_err());
        assert!(runner.job_contract("goal", "owner", "other", true).is_err());
        runner.entries[0].status = "paused".into();
        assert!(runner.job_contract("goal", "owner", "session", true).is_err());
        runner.entries[0].status = "waiting_job".into();
        runner.entries[0].lease.expires_at = now();
        assert!(runner.job_contract("goal", "owner", "session", true).is_err());
        runner.entries[0].lease.expires_at = now() + 120;
        runner.entries[0].lease.allow_jobs = false;
        assert!(runner.job_contract("goal", "owner", "session", true).is_err());
        assert!(runner.job_contract("goal", "owner", "session", false).is_ok());
        std::fs::remove_dir_all(root).unwrap();
    }
}
