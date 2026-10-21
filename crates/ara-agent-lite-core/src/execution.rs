//! Host-owned execution modes and reviewed, bounded goal progress.
use anyhow::{bail, ensure, Result};
use async_trait::async_trait;
use rpi_agent::{AgentError, AgentTool, AgentToolResult, ToolResultPartial};
use rpi_ai::{Schema, Tool};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::{collections::BTreeMap, sync::Arc};
use tokio::sync::{mpsc, Mutex};
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
                    | "web_search"
                    | "fetch_url"
                    | "environment_catalog"
                    | "library_evidence"
                    | "knowledge_read"
                    | "knowledge_search"
                    | "skill_read"
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
                    | "job_input_status"
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

const CHECKLIST_ITEMS: usize = 12;
const CHECKLIST_BYTES: usize = 32 * 1024;

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct ChecklistItem {
    id: String,
    title: String,
    status: String,
    #[serde(default)]
    evidence: String,
}
impl ChecklistItem {
    fn parse(args: Value, token: &str) -> Result<Self> {
        let mut item: Self = serde_json::from_value(args).map_err(|_| {
            anyhow::anyhow!(
                "task_update requires string id/title/status and optional string evidence, with no extra fields"
            )
        })?;
        ensure!(
            !item.id.is_empty()
                && item.id.len() <= 64
                && item.id.bytes().all(|c| c.is_ascii_alphanumeric() || matches!(c, b'_' | b'-')),
            "checklist id must be 1..64 ASCII letters, digits, underscores or hyphens"
        );
        ensure!(token.is_empty() || !item.id.contains(token), "checklist id must not contain a grant credential");
        ensure!(!item.title.trim().is_empty() && item.title.len() <= 240, "checklist title must be 1..240 UTF-8 bytes");
        ensure!(matches!(item.status.as_str(), "pending" | "running" | "done" | "blocked"), "invalid checklist status");
        ensure!(item.evidence.len() <= 2048, "checklist evidence exceeds 2048 UTF-8 bytes");
        ensure!(item.status != "done" || !item.evidence.trim().is_empty(), "done requires nonempty observed evidence");
        if !token.is_empty() {
            item.title = item.title.replace(token, "[redacted grant]");
            item.evidence = item.evidence.replace(token, "[redacted grant]");
        }
        ensure!(item.title.len() <= 240 && item.evidence.len() <= 2048, "redacted checklist fields exceed byte limits");
        Ok(item)
    }
}

/// One instance belongs to one Execute Run. This is displayable progress, not
/// a Project/Task store, Goal state, approval or acceptance of external effects.
pub fn checklist_tool(events: mpsc::Sender<Value>, token: String) -> Arc<dyn AgentTool> {
    Arc::new(ChecklistTool {
        items: Mutex::new(BTreeMap::new()),
        events,
        token,
        schema: Tool {
            name: "task_update".into(),
            description: "Update one step in this Execute Run's local checklist, at most 12 steps. Reuse its id to update it. id is 1..64 ASCII letters/digits/_/-, title is 1..240 UTF-8 bytes, evidence is at most 2048 UTF-8 bytes. Status is pending/running/done/blocked; done requires nonempty evidence from actual observations. Returns the complete checklist within 32 KiB. This is progress metadata, not a cross-project Task, permission, approval or task acceptance; Goal mode uses goal_update.".into(),
            parameters: Schema(json!({"type":"object","additionalProperties":false,"required":["id","title","status"],"properties":{
                "id":{"type":"string","minLength":1,"maxLength":64},
                "title":{"type":"string","minLength":1,"maxLength":240},
                "status":{"type":"string","enum":["pending","running","done","blocked"]},
                "evidence":{"type":"string","maxLength":2048}
            }})),
            constrained_sampling: None,
        },
    })
}

struct ChecklistTool {
    items: Mutex<BTreeMap<String, ChecklistItem>>,
    events: mpsc::Sender<Value>,
    token: String,
    schema: Tool,
}
#[async_trait]
impl AgentTool for ChecklistTool {
    fn schema(&self) -> &Tool {
        &self.schema
    }
    fn label(&self) -> &str {
        "task_update"
    }
    fn execution_mode(&self) -> rpi_agent::ToolExecutionMode {
        rpi_agent::ToolExecutionMode::Sequential
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
        let item = ChecklistItem::parse(args, &self.token).map_err(|error| AgentError::Tool(error.to_string()))?;
        let mut items = tokio::select! {
            biased;
            _ = signal.cancelled() => return Err(AgentError::Abort),
            items = self.items.lock() => items,
        };
        if signal.is_cancelled() {
            return Err(AgentError::Abort);
        }
        if !items.contains_key(&item.id) && items.len() >= CHECKLIST_ITEMS {
            return Err(AgentError::Tool("Run checklist already contains 12 items; update an existing id".into()));
        }
        let mut next = items.clone();
        next.insert(item.id.clone(), item.clone());
        let result = serde_json::to_string(&json!({"scope":"current_run","items":next.values().collect::<Vec<_>>()}))
            .map_err(|_| AgentError::Tool("could not encode Run checklist".into()))?;
        if result.len() > CHECKLIST_BYTES {
            return Err(AgentError::Tool("complete Run checklist exceeds 32 KiB; shorten its evidence".into()));
        }
        *items = next;
        let mut event = json!({"type":"task_update","step_id":item.id,"output":item.title,"detail":item.status,"status":item.status});
        if !item.evidence.is_empty() {
            event["evidence"] = json!(item.evidence);
        }
        // Keep the lock through emission so concurrent callers cannot publish
        // snapshots out of order. A failed emission does not undo local metadata.
        tokio::select! {
            biased;
            _ = signal.cancelled() => return Err(AgentError::Abort),
            sent = self.events.send(event) => if sent.is_err() {
                signal.cancel();
                return Err(AgentError::Tool("Run checklist updated locally, but its UI event was not delivered".into()));
            },
        }
        Ok(AgentToolResult::text(result))
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
            "knowledge_write",
            "knowledge_revoke",
            "skill_create",
            "skill_install",
            "skill_verify",
            "skill_remove",
        ] {
            assert!(!RunMode::Plan.allows(name));
        }
        for name in [
            "read_file",
            "web_search",
            "fetch_url",
            "environment_catalog",
            "library_evidence",
            "knowledge_read",
            "knowledge_search",
            "skill_read",
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

    async fn update(
        tool: &Arc<dyn AgentTool>,
        args: Value,
        signal: CancellationToken,
    ) -> Result<AgentToolResult, AgentError> {
        tool.execute("fixture", args, signal, Arc::new(|_| {})).await
    }
    fn result_text(result: &AgentToolResult) -> &str {
        match result.content.first().unwrap() {
            rpi_agent::TextContentOrImage::Text(text) => &text.text,
            _ => panic!("checklist returned nontext content"),
        }
    }
    fn pending(id: &str) -> Value {
        json!({"id":id,"title":"Inspect the exact log","status":"pending"})
    }

    #[test]
    fn checklist_arguments_are_strict_and_byte_bounded() {
        for args in [
            Value::Null,
            json!({"id":7,"title":"x","status":"pending"}),
            json!({"id":"x","title":"x","status":"pending","extra":true}),
            json!({"id":"x","title":"x","status":"pending","evidence":null}),
            json!({"id":"../x","title":"x","status":"pending"}),
            json!({"id":"x".repeat(65),"title":"x","status":"pending"}),
            json!({"id":"x","title":" ","status":"pending"}),
            json!({"id":"x","title":"中".repeat(81),"status":"pending"}),
            json!({"id":"x","title":"x","status":"accepted"}),
            json!({"id":"x","title":"x","status":"done"}),
            json!({"id":"x","title":"x","status":"done","evidence":" \n"}),
            json!({"id":"x","title":"x","status":"blocked","evidence":"中".repeat(683)}),
        ] {
            assert!(ChecklistItem::parse(args, "").is_err());
        }
        assert!(ChecklistItem::parse(
            json!({"id":"x","title":"中".repeat(80),"status":"done","evidence":"中".repeat(682)}),
            ""
        )
        .is_ok());
    }

    #[tokio::test]
    async fn checklist_cap_keeps_full_originals_and_updates_existing_id() {
        let (tx, mut rx) = mpsc::channel(16);
        let tool = checklist_tool(tx, String::new());
        for n in 0..CHECKLIST_ITEMS {
            let args = json!({"id":format!("step-{n:02}"),"title":"文".repeat(80),"status":"done","evidence":"证".repeat(682)});
            let result = update(&tool, args, CancellationToken::new()).await.unwrap();
            assert!(result_text(&result).len() <= CHECKLIST_BYTES);
            let value: Value = serde_json::from_str(result_text(&result)).unwrap();
            assert_eq!(value["items"].as_array().unwrap().len(), n + 1);
            assert_eq!(value["items"][n]["evidence"], "证".repeat(682));
            rx.recv().await.unwrap();
        }
        assert!(update(&tool, pending("overflow"), CancellationToken::new()).await.is_err());
        assert!(rx.try_recv().is_err());
        let result = update(&tool, pending("step-00"), CancellationToken::new()).await.unwrap();
        assert_eq!(serde_json::from_str::<Value>(result_text(&result)).unwrap()["items"].as_array().unwrap().len(), 12);
    }

    #[tokio::test]
    async fn checklist_event_fields_match_the_existing_ui_contract() {
        let (tx, mut rx) = mpsc::channel(1);
        let tool = checklist_tool(tx, String::new());
        let result = update(&tool, json!({"id":"find-log","title":"Found the error","status":"done","evidence":"fixture.log:12 actual receipt"}), CancellationToken::new()).await.unwrap();
        assert_eq!(
            rx.recv().await.unwrap(),
            json!({"type":"task_update","step_id":"find-log","output":"Found the error","detail":"done","status":"done","evidence":"fixture.log:12 actual receipt"})
        );
        assert_eq!(serde_json::from_str::<Value>(result_text(&result)).unwrap()["scope"], "current_run");
    }

    #[tokio::test]
    async fn checklist_exact_token_is_redacted_in_event_and_complete_result() {
        let (tx, mut rx) = mpsc::channel(2);
        let tool = checklist_tool(tx, "secret-grant-731".into());
        let result = update(&tool, json!({"id":"safe","title":"secret-grant-731 title","status":"done","evidence":"read secret-grant-731 receipt"}), CancellationToken::new()).await.unwrap();
        let event = rx.recv().await.unwrap();
        assert!(!event.to_string().contains("secret-grant-731"));
        assert!(!result_text(&result).contains("secret-grant-731"));
        assert_eq!(event["output"], "[redacted grant] title");
        assert!(update(&tool, pending("secret-grant-731"), CancellationToken::new()).await.is_err());
        assert!(rx.try_recv().is_err());
    }

    #[tokio::test]
    async fn checklist_pre_cancellation_emits_nothing_and_keeps_no_item() {
        let (tx, mut rx) = mpsc::channel(1);
        let tool = checklist_tool(tx, String::new());
        let signal = CancellationToken::new();
        signal.cancel();
        assert!(matches!(update(&tool, pending("canceled"), signal).await, Err(AgentError::Abort)));
        assert!(rx.try_recv().is_err());
        let result = update(&tool, pending("new"), CancellationToken::new()).await.unwrap();
        assert_eq!(serde_json::from_str::<Value>(result_text(&result)).unwrap()["items"].as_array().unwrap().len(), 1);
    }

    #[tokio::test]
    async fn checklist_closed_event_transport_reports_local_update_and_cancels() {
        let (tx, rx) = mpsc::channel(1);
        drop(rx);
        let tool = checklist_tool(tx, String::new());
        let signal = CancellationToken::new();
        let error = update(&tool, pending("local-only"), signal.clone()).await.unwrap_err();
        assert!(
            matches!(error, AgentError::Tool(ref text) if text.contains("updated locally") && text.contains("not delivered"))
        );
        assert!(signal.is_cancelled());
    }

    #[tokio::test]
    async fn checklist_cancellation_interrupts_a_backpressured_event() {
        let (tx, mut rx) = mpsc::channel(1);
        tx.send(json!({"occupied":true})).await.unwrap();
        let tool = checklist_tool(tx, String::new());
        let signal = CancellationToken::new();
        let call = update(&tool, pending("cancel-wait"), signal.clone());
        let (result, _) = tokio::join!(call, async {
            tokio::task::yield_now().await;
            signal.cancel();
        });
        assert!(matches!(result, Err(AgentError::Abort)));
        assert_eq!(rx.recv().await.unwrap(), json!({"occupied":true}));
        assert!(rx.try_recv().is_err());
    }

    #[tokio::test]
    async fn checklist_instances_do_not_share_run_items() {
        let (tx_a, mut rx_a) = mpsc::channel(1);
        let (tx_b, mut rx_b) = mpsc::channel(1);
        let a = checklist_tool(tx_a, String::new());
        let b = checklist_tool(tx_b, String::new());
        update(&a, pending("run-a"), CancellationToken::new()).await.unwrap();
        rx_a.recv().await.unwrap();
        let result = update(&b, pending("run-b"), CancellationToken::new()).await.unwrap();
        rx_b.recv().await.unwrap();
        let value: Value = serde_json::from_str(result_text(&result)).unwrap();
        assert_eq!(value["items"].as_array().unwrap().len(), 1);
        assert_eq!(value["items"][0]["id"], "run-b");
        assert!(!RunMode::Plan.allows("task_update"));
    }
}
