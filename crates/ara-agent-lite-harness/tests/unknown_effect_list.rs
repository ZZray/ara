//! Property 30: the unknown-effect record list of a Task (requirements 22.4,
//! 22.7), over the real Core `execute`. Every judged call has exactly one
//! record, in Run then in-Run order, unique by `(run_id, native_call_id)`,
//! and each reference locates the persisted unknown result.
mod common;

use std::sync::Arc;

use ara_agent_lite_harness::{
    core::{
        rpi_agent::{AgentError, AgentMessage, AgentTool, AgentToolResult, ToolResultPartial},
        rpi_ai::{Schema, Tool},
        session_journal::Journal,
    },
    *,
};
use async_trait::async_trait;
use common::*;
use proptest::prelude::*;
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use tokio_util::sync::CancellationToken;

#[derive(Clone, Copy, Debug, PartialEq)]
enum Kind {
    Known,
    /// Only an `unknown_effect_record` key: no event marker, the Run continues.
    Record,
    /// Structured marker: the `tool_end` event also reports it and the Run stops.
    Marker,
}

struct Probe(Tool);

#[async_trait]
impl AgentTool for Probe {
    fn schema(&self) -> &Tool {
        &self.0
    }
    fn label(&self) -> &str {
        "fixture_probe"
    }
    async fn execute(
        &self,
        _: &str,
        args: Value,
        _: CancellationToken,
        _: Arc<dyn Fn(ToolResultPartial) + Send + Sync>,
    ) -> Result<AgentToolResult, AgentError> {
        let mut result = AgentToolResult::text("probe finished");
        result.details = match args["kind"].as_str() {
            Some("record") => json!({"unknown_effect_record":{"op":"fixture","state":"unconfirmed"}}),
            Some("marker") => json!({"executed":"unknown"}),
            _ => json!({"executed":"yes"}),
        };
        Ok(result)
    }
}

fn probe() -> Arc<Probe> {
    Arc::new(Probe(Tool {
        name: "fixture_probe".into(),
        description: "Run one fixture probe of the given kind.".into(),
        parameters: Schema(
            json!({"type":"object","properties":{"kind":{"type":"string"}},"required":["kind"],"additionalProperties":false}),
        ),
        constrained_sampling: None,
    }))
}

fn round(kinds: &[Kind]) -> Value {
    let mut calls = vec![json!({"index":0,"id":"write-round","type":"function",
        "function":{"name":"write_answer","arguments":"{\"content\":\"correct\"}"}})];
    for (index, kind) in kinds.iter().enumerate() {
        let kind = match kind {
            Kind::Known => "known",
            Kind::Record => "record",
            Kind::Marker => "marker",
        };
        calls.push(json!({"index":index + 1,"id":format!("probe-{index}"),"type":"function",
            "function":{"name":"fixture_probe","arguments":json!({"kind":kind}).to_string()}}));
    }
    json!({"delta":{"tool_calls":calls},"finish":"tool_calls"})
}

fn located(record: &UnknownEffectRecord) -> bool {
    match &record.receipt_ref.evidence {
        EffectEvidence::Journal { session_path, raw_index, message_sha256 } => {
            let messages = Journal::for_session(session_path).read(*raw_index, raw_index + 1, 8 * 1024 * 1024).unwrap();
            let message = &messages[0];
            let AgentMessage::ToolResult(result) = message else { return false };
            format!("{:x}", Sha256::digest(serde_json::to_vec(message).unwrap())) == *message_sha256
                && result.tool_call_id == record.native_call_id
                && serde_json::to_string(result).unwrap().contains("unknown")
        }
        EffectEvidence::Event { events_path, seq } => events(events_path).iter().any(|row| {
            row["seq"] == *seq
                && row["event"]["type"] == "tool_end"
                && row["event"]["step_id"] == record.native_call_id.as_str()
                && row["event"]["effects_unknown"] == true
        }),
    }
}

fn kinds() -> impl Strategy<Value = Vec<Kind>> {
    (proptest::collection::vec(prop_oneof![Just(Kind::Known), Just(Kind::Record)], 0..4), any::<bool>()).prop_map(
        |(mut kinds, marker)| {
            if marker {
                kinds.push(Kind::Marker);
            }
            kinds
        },
    )
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(32))]

    #[test]
    fn records_are_unique_ordered_and_locatable(earlier_run in any::<bool>(), kinds in kinds()) {
        let runtime = tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap();
        let directory = tempfile::tempdir().unwrap();
        let outcome = runtime.block_on(async {
            let root = directory.path().canonicalize().unwrap();
            let mut script = Vec::new();
            if earlier_run {
                // Run 1 is known-settled; the Host asks for changes.
                script.extend([write_answer("wrong"), text("Submitted.")]);
            }
            script.extend([round(&kinds), text("Submitted.")]);
            let provider = Provider::new(script);
            let mut host = Host::new(&root, &provider, Fault::None);
            host.read_tool = Some(probe());
            let host = Arc::new(host);
            let handle = start(host.spec("list"), host.request("first"), host.clone()).unwrap();
            tokio::time::timeout(std::time::Duration::from_secs(30), handle.wait()).await.unwrap()
        });
        let run_id = if earlier_run { "repair-2" } else { "first" };
        let expected: Vec<String> = kinds
            .iter()
            .enumerate()
            .filter(|(_, kind)| **kind != Kind::Known)
            .map(|(index, _)| format!("probe-{index}"))
            .collect();
        prop_assert!(!outcome.task_accepted);
        if expected.is_empty() {
            prop_assert_eq!(outcome.stop, TaskStop::AwaitingHostAcceptance, "{:?}", outcome);
            prop_assert!(outcome.unknown_effects.is_empty());
        } else {
            prop_assert_eq!(outcome.stop, TaskStop::ReconciliationRequired, "{:?}", outcome);
            let ids: Vec<&str> = outcome.unknown_effects.iter().map(|record| record.native_call_id.as_str()).collect();
            prop_assert_eq!(ids, expected.iter().map(String::as_str).collect::<Vec<_>>(), "{:?}", outcome);
            for record in &outcome.unknown_effects {
                prop_assert_eq!(record.run_id.as_str(), run_id);
                prop_assert_eq!(record.tool.as_str(), "fixture_probe");
                prop_assert!(located(record), "{:?}", record);
            }
        }
    }
}
