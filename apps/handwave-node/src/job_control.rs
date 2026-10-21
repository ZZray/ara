//! Controller-only process console. Input is never a chat message or model
//! observation; the worker owns dispatch receipts and the original deadline.
use crate::{config::Resolved, goal_runtime::Runner, managed_jobs::Manager};
use anyhow::{ensure, Context, Result};
use serde_json::{json, Value};

fn string<'a>(sig: &'a Value, key: &str) -> &'a str {
    sig[key].as_str().unwrap_or_default()
}
fn identity(id: &str) -> bool {
    !id.is_empty() && id.len() <= 192 && id.bytes().all(|c| c.is_ascii_alphanumeric() || matches!(c, b'-' | b'_'))
}
fn validate(sig: &Value, fp: &str, owner: &str) -> Result<()> {
    ensure!(!owner.is_empty() && string(sig, "ctlfp") == owner, "process console controller mismatch");
    ensure!(string(sig, "fp") == fp, "process console device mismatch");
    ensure!(
        identity(string(sig, "command_id")) && identity(string(sig, "goal_id")),
        "invalid process command identity"
    );
    let key = string(sig, "session_key");
    ensure!(key.len() <= 193 && key.split('/').count() == 2 && key.split('/').all(identity), "invalid process Session");
    ensure!(
        matches!(
            string(sig, "job_action"),
            "list" | "status" | "log" | "input" | "input_status" | "cancel" | "resize" | "signal"
        ),
        "unsupported process console operation"
    );
    Ok(())
}

/// Strip private local-worker capabilities before any browser-facing receipt.
fn public(mut value: Value) -> Value {
    match &mut value {
        Value::Object(object) => {
            object.remove("worker_nonce");
            for item in object.values_mut() {
                *item = public(item.take());
            }
        }
        Value::Array(items) => {
            for item in items {
                *item = public(item.take());
            }
        }
        _ => {}
    }
    value
}

pub async fn handle(sig: &Value, settings: &Resolved, owner: &str, goals: &Runner) -> Result<Value> {
    validate(sig, &settings.fp, owner)?;
    let action = string(sig, "job_action");
    let key = string(sig, "session_key");
    let goal_id = string(sig, "goal_id");
    let input = matches!(action, "input" | "resize" | "signal");
    let lease = goals.job_contract(goal_id, owner, key, input)?;
    let state = settings.session_path_for(owner, key)?;
    if input {
        lease.validate(owner, &settings.fp, key, goal_id, crate::goal_runtime::now())?;
        let goal = crate::agent::session_goal(&state, &settings.cwd)?.context("process Goal unavailable")?;
        ensure!(
            goal.id == goal_id
                && goal.budget.as_ref().is_some_and(|b| b.lease == lease)
                && matches!(goal.status.as_str(), "running" | "yielded" | "waiting_job"),
            "native Goal is not accepting process input"
        );
    }
    let manager = Manager::new(
        &state,
        &settings.cwd,
        crate::managed_jobs::Policy {
            worker_executable: Some(std::env::current_exe()?),
            owner: crate::managed_jobs::Owner::new(&state, &settings.fp, key)?,
            goal_id: lease.goal_id.clone(),
            lease_expires_at: lease.expires_at,
            max_jobs: 8,
            max_log_bytes: 8 * 1024 * 1024,
            max_seconds: 3600,
        },
    )?;
    let job = string(sig, "job_id");
    let data = match action {
        "list" => json!({"jobs": manager.inventory()?}),
        "status" => serde_json::to_value(manager.status(job)?)?,
        "log" => {
            let cursor =
                sig.get("job_cursor").map(|v| v.as_u64().context("invalid log cursor")).transpose()?.unwrap_or(0);
            let limit: usize = sig
                .get("job_limit")
                .map(|v| v.as_u64().context("invalid log limit"))
                .transpose()?
                .unwrap_or(8192)
                .try_into()
                .context("log limit overflow")?;
            manager.read_log(job, string(sig, "job_stream"), cursor, limit)?
        }
        "input" => {
            let eof = sig
                .get("job_eof")
                .map(|v| v.as_bool().context("invalid process EOF flag"))
                .transpose()?
                .unwrap_or(false);
            let text = match sig.get("job_text") {
                Some(value) => value.as_str().context("process input text must be a string")?,
                None if eof => "",
                None => anyhow::bail!("process input text required"),
            };
            manager.write_input(job, string(sig, "job_input_id"), text, eof).await?
        }
        "input_status" => manager.input_status(job, string(sig, "job_input_id"))?,
        "resize" => {
            let dimension = |key: &str| -> Result<u16> {
                sig[key].as_u64().and_then(|v| u16::try_from(v).ok()).context("invalid terminal dimension")
            };
            manager.resize(job, string(sig, "job_input_id"), dimension("job_rows")?, dimension("job_cols")?).await?
        }
        "signal" => manager.signal(job, string(sig, "job_input_id"), string(sig, "job_signal")).await?,
        "cancel" => serde_json::to_value(manager.cancel(job)?)?,
        _ => unreachable!("validated console operation"),
    };
    Ok(public(data))
}

/// A normal user follow-up may write to the existing process only when the
/// durable scheduler and the native Goal agree on the exact original lease.
pub fn resume_lease(
    settings: &Resolved,
    owner: &str,
    key: &str,
    goals: &Runner,
) -> Result<Option<crate::goal_runtime::Lease>> {
    let state = settings.session_path_for(owner, key)?;
    let Some(goal) = crate::agent::session_goal(&state, &settings.cwd)? else {
        return Ok(None);
    };
    checked_resume(&goal, &settings.fp, owner, key, goals)
}

fn checked_resume(
    goal: &crate::execution::GoalState,
    fp: &str,
    owner: &str,
    key: &str,
    goals: &Runner,
) -> Result<Option<crate::goal_runtime::Lease>> {
    if !matches!(goal.status.as_str(), "running" | "yielded" | "waiting_job") {
        return Ok(None);
    }
    let lease = goals.job_contract(&goal.id, owner, key, true)?;
    lease.validate(owner, fp, key, &goal.id, crate::goal_runtime::now())?;
    ensure!(goal.budget.as_ref().is_some_and(|b| b.lease == lease), "process resume lease differs from native Goal");
    Ok(Some(lease))
}

pub fn reply(sig: &Value, fp: &str, result: Result<Value>) -> Value {
    // Explicit construction is intentional: never reflect input text into the
    // signal, log, native journal or an error response.
    let mut reply = json!({"k":"job-command-result", "fp":fp, "ctlfp":string(sig,"ctlfp"),
        "command_id":string(sig,"command_id"), "goal_id":string(sig,"goal_id"),
        "session_key":string(sig,"session_key"), "job_action":string(sig,"job_action"),
        "job_id":string(sig,"job_id"), "job_input_id":string(sig,"job_input_id")});
    match result {
        Ok(data) => {
            reply["job_ok"] = json!(true);
            reply["job_data"] = data;
        }
        Err(error) => {
            reply["job_ok"] = json!(false);
            reply["err"] = json!(format!("{error:#}"));
        }
    }
    reply
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn console_scope_and_action_are_explicit() {
        let mut sig = json!({"ctlfp":"controller", "fp":"device", "goal_id":"goal", "command_id":"c-1", "session_key":"w/s", "job_action":"list"});
        assert!(validate(&sig, "device", "controller").is_ok());
        assert!(validate(&sig, "device", "other").is_err());
        assert!(validate(&sig, "device", "").is_err());
        assert!(validate(&sig, "other", "controller").is_err());
        sig["job_action"] = json!("start");
        assert!(validate(&sig, "device", "controller").is_err());
        sig["job_action"] = json!("input");
        sig["session_key"] = json!("w/../s");
        assert!(validate(&sig, "device", "controller").is_err());
    }
    #[test]
    fn public_result_does_not_expose_worker_capability_or_input() {
        let data = public(json!({"jobs":[{"job_id":"j","worker_nonce":"private-worker-token","state":"running"}]}));
        assert!(data["jobs"][0].get("worker_nonce").is_none());
        let sig = json!({"job_text":"private-input-body", "job_action":"input", "job_input_id":"i"});
        let response = reply(&sig, "device", Ok(json!({"state":"written","bytes":4})));
        assert!(!response.to_string().contains("private-input-body"));
        assert_eq!(response["job_input_id"], "i");
    }
    #[test]
    fn stale_native_goal_cannot_reopen_canceled_dispatch_input_after_restart() {
        let root = std::env::temp_dir().join(format!("input-recovery-{}", crate::wire::fresh_id()));
        std::fs::create_dir(&root).unwrap();
        let mut runner = Runner::load(&root).unwrap();
        let lease = crate::goal_runtime::Lease {
            goal_id: "g".into(),
            controller: "c".into(),
            device: "d".into(),
            session: "w/s".into(),
            expires_at: crate::goal_runtime::now() + 120,
            max_calls: 3,
            max_slices: 3,
            allow_jobs: true,
        };
        runner
            .register(&crate::wire::Frame { id: "r".into(), session_key: "w/s".into(), ..Default::default() }, &lease)
            .unwrap();
        let mut stale = crate::execution::GoalState::new("g", "test", &["verified".into()]).unwrap();
        stale.status = "waiting_job".into();
        stale.budget = Some(crate::goal_runtime::Budget::new(lease));
        assert!(checked_resume(&stale, "d", "c", "w/s", &runner).unwrap().is_some());
        runner.cancel_matching("c", "g").unwrap();
        // Model the crash before cancel markers/native suspend: the stale
        // native Goal still claims waiting_job after Runner recovery.
        let mut restarted = Runner::load(&root).unwrap();
        restarted.recover("w/s", Some(&stale)).unwrap();
        assert!(checked_resume(&stale, "d", "c", "w/s", &restarted).is_err());
        std::fs::remove_dir_all(root).unwrap();
    }
}
