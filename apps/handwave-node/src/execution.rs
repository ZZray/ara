//! Host-owned execution modes and reviewed, bounded goal progress.
use anyhow::{bail, ensure, Result};
use async_trait::async_trait;
use rpi_agent::{AgentError, AgentTool, AgentToolResult, ToolResultPartial};
use rpi_ai::{Schema, Tool};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::sync::Arc;
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum RunMode {
    #[default]
    Execute,
    Plan,
    Goal,
}
impl RunMode {
    pub fn parse(value: &str) -> Result<Self> {
        match value {
            "" | "execute" => Ok(Self::Execute),
            "plan" => Ok(Self::Plan),
            "goal" => Ok(Self::Goal),
            _ => bail!("unsupported execution_mode; expected execute, plan or goal"),
        }
    }
    pub fn allows(self, name: &str) -> bool {
        self != Self::Plan
            || matches!(
                name,
                "read_file"
                    | "search_files"
                    | "list_dir"
                    | "find_files"
                    | "system_info"
                    | "reference_read"
                    | "reference_search"
                    | "experience_search"
                    | "history_read"
                    | "history_search"
                    | "job_status"
                    | "job_read_log"
            )
    }
    pub fn validate_command(self, text: &str) -> Result<()> {
        let parts: Vec<_> = text.split_whitespace().collect();
        if self == Self::Plan && matches!(parts.first(), Some(&"/memory" | &"/reference" | &"/goal")) {
            ensure!(
                matches!(
                    parts.as_slice(),
                    ["/memory" | "/reference", "list"] | ["/goal", "status"] | ["/goal", "status", _]
                ),
                "plan mode permits only /memory list, /reference list and /goal status management commands"
            );
        }
        Ok(())
    }
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct GoalStep {
    pub name: String,
    pub status: String,
    pub evidence: String,
}
pub fn validate_steps(steps: &[GoalStep]) -> Result<()> {
    ensure!(!steps.is_empty() && steps.len() <= 8, "goal requires 1..8 steps");
    for step in steps {
        ensure!(!step.name.trim().is_empty() && step.name.len() <= 256, "goal step name must be 1..256 UTF-8 bytes");
        ensure!(
            matches!(step.status.as_str(), "pending" | "in_progress" | "done" | "blocked"),
            "invalid goal step status"
        );
        ensure!(step.evidence.len() <= 1024, "goal step evidence exceeds 1024 bytes");
    }
    Ok(())
}
pub fn validate_criteria(criteria: &[String]) -> Result<()> {
    ensure!(criteria.len() <= 8, "goal criteria exceed 8 entries");
    ensure!(
        criteria.iter().all(|s| !s.trim().is_empty() && s.len() <= 1024),
        "goal criteria must be 1..1024 UTF-8 bytes each"
    );
    Ok(())
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct GoalState {
    pub id: String,
    pub objective: String,
    pub criteria: Vec<String>,
    pub steps: Vec<GoalStep>,
    pub status: String,
    pub run: String,
    pub revision: u64,
    #[serde(default)]
    pub budget: Option<crate::goal_runtime::Budget>,
    #[serde(default)]
    pub last_settled_run: String,
    #[serde(default = "initial_revision")]
    pub criteria_revision: u64,
}
fn initial_revision() -> u64 {
    1
}
impl GoalState {
    pub fn new(id: &str, objective: &str, criteria: &[String]) -> Result<Self> {
        validate_criteria(criteria)?;
        ensure!(!criteria.is_empty(), "new goal requires explicit goal_criteria");
        ensure!(objective.len() <= 8192, "goal objective exceeds 8192 bytes");
        Ok(Self {
            id: id.into(),
            objective: objective.into(),
            criteria: criteria.to_vec(),
            steps: vec![],
            status: "running".into(),
            run: id.into(),
            revision: 1,
            budget: None,
            last_settled_run: String::new(),
            criteria_revision: 1,
        })
    }
    pub fn command_checked(
        &mut self,
        command: &str,
        run: &str,
        revision: Option<u64>,
        reviewed_run: &str,
    ) -> Result<()> {
        if command != "status" && self.budget.is_some() {
            ensure!(revision == Some(self.revision), "goal revision changed; refresh the goal card");
        }
        if command == "accept" && self.budget.is_some() {
            ensure!(
                !reviewed_run.is_empty() && reviewed_run == self.last_settled_run,
                "reviewed Run differs from the latest settled goal result"
            );
        }
        self.command(command, run)
    }
    pub fn command(&mut self, command: &str, run: &str) -> Result<()> {
        match command {
            "status" => return Ok(()),
            "accept" => {
                ensure!(self.status == "awaiting_review", "only an awaiting_review goal can be accepted");
                self.status = "accepted".into();
            }
            "pause" => {
                ensure!(self.status != "accepted", "accepted goal cannot be paused");
                self.status = "paused".into();
            }
            "resume" => {
                ensure!(self.status == "paused", "only a paused goal can be resumed");
                self.status = "running".into();
            }
            "stop" => {
                ensure!(self.status != "accepted", "accepted goal cannot be stopped");
                self.status = "canceled".into();
            }
            _ => bail!("use /goal status|accept|pause|resume"),
        }
        self.run = run.into();
        self.revision += 1;
        Ok(())
    }
}

pub type UpdateGoal = Arc<dyn Fn(Vec<GoalStep>, CancellationToken) -> Result<GoalState> + Send + Sync>;
pub fn goal_tool(update: UpdateGoal, events: mpsc::Sender<Value>) -> Arc<dyn AgentTool> {
    Arc::new(GoalTool { update, events, schema: Tool {
        name: "goal_update".into(),
        description: "Record 1..8 bounded progress steps for the current authorized goal. Evidence must describe actual observations; done is a proposal, never user acceptance. This tool cannot change the objective, criteria, permissions, budgets or execution mode.".into(),
        parameters: Schema(json!({"type":"object","additionalProperties":false,"required":["steps"],"properties":{"steps":{"type":"array","minItems":1,"maxItems":8,"items":{"type":"object","additionalProperties":false,"required":["name","status","evidence"],"properties":{"name":{"type":"string","maxLength":256},"status":{"type":"string","enum":["pending","in_progress","done","blocked"]},"evidence":{"type":"string","maxLength":1024}}}}}})),
        constrained_sampling: None,
    } })
}
struct GoalTool {
    update: UpdateGoal,
    events: mpsc::Sender<Value>,
    schema: Tool,
}
#[async_trait]
impl AgentTool for GoalTool {
    fn schema(&self) -> &Tool {
        &self.schema
    }
    fn label(&self) -> &str {
        "goal_update"
    }
    async fn execute(
        &self,
        _: &str,
        args: Value,
        signal: CancellationToken,
        _: Arc<dyn Fn(ToolResultPartial) + Send + Sync>,
    ) -> Result<AgentToolResult, AgentError> {
        if signal.is_cancelled() {
            return Err(AgentError::Abort);
        }
        let steps: Vec<GoalStep> = serde_json::from_value(args.get("steps").cloned().unwrap_or(Value::Null))
            .map_err(|e| AgentError::Tool(e.to_string()))?;
        validate_steps(&steps).map_err(|e| AgentError::Tool(e.to_string()))?;
        let goal = (self.update)(steps, signal.clone()).map_err(|e| AgentError::Tool(e.to_string()))?;
        tokio::select! {
            _ = signal.cancelled() => return Err(AgentError::Abort),
            sent = self.events.send(json!({"type":"goal_update","goal":goal})) => if sent.is_err() { signal.cancel(); return Err(AgentError::Tool("goal event transport closed after durable update".into())); },
        }
        Ok(AgentToolResult::text(serde_json::to_string(&goal).unwrap()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn plan_is_a_small_explicit_allowlist() {
        for name in [
            "exec",
            "run_script",
            "write_file",
            "edit_file",
            "download_file",
            "memory_propose",
            "experience_propose",
            "goal_update",
            "future_tool",
        ] {
            assert!(!RunMode::Plan.allows(name));
        }
        for name in [
            "read_file",
            "search_files",
            "list_dir",
            "find_files",
            "system_info",
            "reference_read",
            "reference_search",
            "experience_search",
        ] {
            assert!(RunMode::Plan.allows(name));
        }
        assert_eq!(RunMode::parse("").unwrap(), RunMode::Execute);
        assert!(RunMode::parse("auto").is_err());
        assert!(RunMode::Plan.validate_command("/memory accept mem-1").is_err());
        assert!(RunMode::Plan.validate_command("/memory list").is_ok());
    }
    #[test]
    fn goal_review_is_explicit_and_bounded() {
        let mut goal = GoalState::new("run-1", "fix", &["test passes".into()]).unwrap();
        assert!(goal.command("accept", "run-2").is_err());
        goal.status = "awaiting_review".into();
        goal.command("accept", "run-3").unwrap();
        assert_eq!(goal.status, "accepted");
        assert!(GoalState::new("x", "x", &[]).is_err());
        assert!(
            validate_steps(&[GoalStep { name: "x".into(), status: "accepted".into(), evidence: "".into() }]).is_err()
        );
    }
}
