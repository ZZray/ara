//! Host-collected evidence for reviewed experience candidates, not proof of task success.
use anyhow::{bail, Result};
use rpi_agent::AgentMessage;
use rpi_ai::Content;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{collections::HashMap, fmt::Write, sync::Arc};

/// The host supplies only the current run's persisted native transcript.
pub type Snapshot = Arc<dyn Fn() -> Result<Vec<AgentMessage>> + Send + Sync>;

const MAX_RECEIPTS: usize = 8;
const EXCERPT_LIMIT: usize = 1024;
const SOURCE: &str = "native_tool_result";

#[derive(Serialize, Deserialize, Clone, Debug)]
#[serde(deny_unknown_fields)]
pub struct Evidence {
    pub run: String,
    pub conditions: String,
    pub receipts: Vec<Receipt>,
}

#[derive(Serialize, Deserialize, Clone, Debug)]
#[serde(deny_unknown_fields)]
pub struct Receipt {
    pub source: String,
    pub tool_call_id: String,
    pub tool_name: String,
    pub is_error: bool,
    /// SHA256 of all text blocks joined by one newline, before excerpt truncation.
    pub output_hash: String,
    pub excerpt: String,
    pub truncated: bool,
}

fn identifier(value: &str) -> bool {
    !value.is_empty() && value.len() <= 256 && value.trim() == value && !value.chars().any(char::is_control)
}

fn allowed_tool(name: &str) -> bool {
    matches!(
        name,
        "read_file"
            | "search_files"
            | "find_files"
            | "exec"
            | "run_script"
            | "edit_file"
            | "write_file"
            | "download_file"
            | "list_dir"
            | "system_info"
    )
}

fn hex_digest(hash: Sha256) -> String {
    let mut result = String::with_capacity(64);
    for byte in hash.finalize() {
        write!(result, "{byte:02x}").expect("writing into a String cannot fail");
    }
    result
}

impl Evidence {
    /// Validate stored shape and local consistency. Truncated excerpts cannot verify a full-output hash.
    /// A caller must retain the host provenance; valid JSON alone does not establish authenticity.
    pub fn validate(&self) -> Result<()> {
        if !identifier(&self.run) || self.conditions.trim().is_empty() || self.conditions.len() > 1024 {
            bail!("experience requires a bounded run identifier and nonempty conditions (up to 1024 bytes)");
        }
        if !(2..=MAX_RECEIPTS).contains(&self.receipts.len()) {
            bail!("experience needs 2 to 8 tool receipts including failure followed by success");
        }
        let mut ids = std::collections::HashSet::new();
        let mut failure = false;
        let mut success = false;
        for receipt in &self.receipts {
            if receipt.source != SOURCE
                || !identifier(&receipt.tool_call_id)
                || !ids.insert(receipt.tool_call_id.as_str())
                || !allowed_tool(&receipt.tool_name)
            {
                bail!("invalid, duplicate, or unsupported tool receipt");
            }
            if receipt.output_hash.len() != 64
                || !receipt.output_hash.bytes().all(|c| c.is_ascii_digit() || (b'a'..=b'f').contains(&c))
                || receipt.excerpt.len() > EXCERPT_LIMIT
                || (receipt.truncated && receipt.excerpt.len() < EXCERPT_LIMIT - 3)
            {
                bail!("invalid tool output hash or excerpt bounds");
            }
            if !receipt.truncated {
                let mut hash = Sha256::new();
                hash.update(receipt.excerpt.as_bytes());
                if hex_digest(hash) != receipt.output_hash {
                    bail!("untruncated tool excerpt does not match its hash");
                }
            }
            if receipt.is_error {
                if success {
                    bail!("failure receipts must precede successful receipts");
                }
                failure = true;
            } else {
                if !failure {
                    bail!("experience must include an earlier failed tool result");
                }
                success = true;
            }
        }
        if !failure || !success {
            bail!("experience requires failure followed by success; tool status alone does not prove task success");
        }
        Ok(())
    }
}

/// Resolve model-selected IDs against actual host messages. Never accept model-authored receipts.
pub fn collect(run: &str, conditions: &str, tool_call_ids: &[String], messages: &[AgentMessage]) -> Result<Evidence> {
    if !identifier(run) || conditions.trim().is_empty() || conditions.len() > 1024 {
        bail!("experience requires a bounded run identifier and nonempty conditions (up to 1024 bytes)");
    }
    if !(1..=MAX_RECEIPTS).contains(&tool_call_ids.len()) {
        bail!("select 1 to 8 unique tool call IDs");
    }
    let mut selected = HashMap::new();
    for (index, id) in tool_call_ids.iter().enumerate() {
        if !identifier(id) || selected.insert(id.as_str(), index).is_some() {
            bail!("invalid or duplicate selected tool call ID");
        }
    }
    let mut calls = HashMap::<&str, &str>::new();
    let mut receipts = Vec::with_capacity(tool_call_ids.len());
    for message in messages {
        match message {
            AgentMessage::Assistant(assistant) => {
                for content in &assistant.content {
                    if let Content::ToolCall(call) = content {
                        if selected.contains_key(call.id.as_str())
                            && (!allowed_tool(&call.name) || calls.insert(&call.id, &call.name).is_some())
                        {
                            bail!("selected call is duplicated or uses an unsupported tool");
                        }
                    }
                }
            }
            AgentMessage::ToolResult(result) => {
                let Some(&index) = selected.get(result.tool_call_id.as_str()) else { continue };
                if calls.get(result.tool_call_id.as_str()).copied() != Some(result.tool_name.as_str()) {
                    bail!("selected tool result has no earlier matching native call");
                }
                if index != receipts.len() {
                    bail!("selected tool IDs must follow native result order without duplicates");
                }
                let mut hash = Sha256::new();
                let mut excerpt = String::with_capacity(EXCERPT_LIMIT);
                let mut total = 0usize;
                for (block_index, block) in result.content.iter().enumerate() {
                    let Content::Text(text) = block else { bail!("experience receipts require text-only tool output") };
                    let separator = if block_index == 0 { "" } else { "\n" };
                    for part in [separator, text.text.as_str()] {
                        hash.update(part.as_bytes());
                        let remaining = EXCERPT_LIMIT.saturating_sub(total);
                        if remaining > 0 {
                            let mut end = part.len().min(remaining);
                            while !part.is_char_boundary(end) {
                                end -= 1;
                            }
                            excerpt.push_str(&part[..end]);
                        }
                        total = total.saturating_add(part.len());
                    }
                }
                receipts.push(Receipt {
                    source: SOURCE.into(),
                    tool_call_id: result.tool_call_id.clone(),
                    tool_name: result.tool_name.clone(),
                    is_error: result.is_error,
                    output_hash: hex_digest(hash),
                    truncated: total > excerpt.len(),
                    excerpt,
                });
            }
            _ => {}
        }
    }
    if receipts.len() != selected.len() {
        bail!("selected native tool call/result pair is missing or incomplete");
    }
    let evidence = Evidence { run: run.into(), conditions: conditions.into(), receipts };
    evidence.validate()?;
    Ok(evidence)
}

/// Provider-only ID directory from the host's current-run native transcript.
/// It exposes no output bodies and does not establish that a task or causal lesson was accepted.
pub fn directory(messages: &[AgentMessage]) -> Option<serde_json::Value> {
    let mut calls = HashMap::<&str, &str>::new();
    let mut results = std::collections::HashSet::new();
    let mut failure = None;
    let mut success = None;
    for message in messages {
        match message {
            AgentMessage::Assistant(assistant) => {
                for content in &assistant.content {
                    if let Content::ToolCall(call) = content {
                        if !identifier(&call.id) || calls.insert(&call.id, &call.name).is_some() {
                            return None;
                        }
                    }
                }
            }
            AgentMessage::ToolResult(result) => {
                if !results.insert(result.tool_call_id.as_str())
                    || calls.get(result.tool_call_id.as_str()).copied() != Some(result.tool_name.as_str())
                {
                    return None;
                }
                if !allowed_tool(&result.tool_name) {
                    continue;
                }
                // collect() requires text-only native evidence; never advertise unusable IDs.
                if result.content.iter().any(|block| !matches!(block, Content::Text(_))) {
                    return None;
                }
                if result.is_error {
                    failure = Some(result);
                    success = None;
                } else if failure.is_some() {
                    success = Some(result);
                }
            }
            _ => {}
        }
    }
    let (failure, success) = (failure?, success?);
    let value = serde_json::json!({
        "source":"native_current_run",
        "receipts":[
            {"tool_call_id":failure.tool_call_id,"tool_name":failure.tool_name,"is_error":true},
            {"tool_call_id":success.tool_call_id,"tool_name":success.tool_name,"is_error":false}
        ],
        "boundary":"IDs only, not Task acceptance"
    });
    (value.to_string().len() <= 2048).then_some(value)
}

#[cfg(test)]
mod tests {
    use super::*;
    use rpi_ai::{Api, AssistantMessage, StopReason, ToolCall, ToolCallType, ToolResultMessage};
    use serde_json::json;

    fn pair(id: &str, name: &str, is_error: bool, output: &str) -> Vec<AgentMessage> {
        let mut assistant = AssistantMessage::empty(Api::OpenaiCompletions, "fixture", "fixture", 0);
        assistant.stop_reason = StopReason::ToolUse;
        assistant.content.push(Content::ToolCall(ToolCall {
            kind: ToolCallType,
            id: id.into(),
            name: name.into(),
            arguments: json!({}),
            thought_signature: None,
            namespace: None,
        }));
        vec![
            AgentMessage::Assistant(Box::new(assistant)),
            AgentMessage::ToolResult(Box::new(ToolResultMessage {
                role: Default::default(),
                tool_call_id: id.into(),
                tool_name: name.into(),
                content: vec![Content::text(output)],
                details: None,
                usage: None,
                added_tool_names: vec![],
                is_error,
                timestamp: 0,
            })),
        ]
    }

    fn messages() -> Vec<AgentMessage> {
        let mut messages = pair("failed", "run_script", true, "exit 7");
        messages.extend(pair("fixed", "run_script", false, "verified artifact"));
        messages
    }

    fn ids() -> Vec<String> {
        vec!["failed".into(), "fixed".into()]
    }

    #[test]
    fn valid_failure_then_success_and_json_roundtrip() {
        let evidence = collect("run1", "Python script correction", &ids(), &messages()).unwrap();
        assert_eq!(evidence.receipts[0].excerpt, "exit 7");
        assert!(evidence.receipts[0].is_error);
        assert!(!evidence.receipts[1].is_error);
        let restored: Evidence = serde_json::from_value(serde_json::to_value(&evidence).unwrap()).unwrap();
        restored.validate().unwrap();
    }

    #[test]
    fn rejects_missing_duplicate_incomplete_and_wrong_order() {
        for selected in
            [vec!["invented".into()], vec!["failed".into(), "failed".into()], vec!["fixed".into(), "failed".into()]]
        {
            assert!(collect("run", "condition", &selected, &messages()).is_err());
        }
        let mut incomplete = messages();
        incomplete.pop();
        assert!(collect("run", "condition", &ids(), &incomplete).is_err());
        let mut duplicate = messages();
        duplicate.push(duplicate[0].clone());
        assert!(collect("run", "condition", &ids(), &duplicate).is_err());
        let mut duplicate_result = messages();
        duplicate_result.push(duplicate_result[1].clone());
        assert!(collect("run", "condition", &ids(), &duplicate_result).is_err());
    }

    #[test]
    fn rejects_mismatch_result_before_call_and_disallowed_tools() {
        let mut wrong = messages();
        if let AgentMessage::ToolResult(result) = &mut wrong[1] {
            result.tool_name = "exec".into();
        }
        assert!(collect("run", "condition", &ids(), &wrong).is_err());
        let mut wrong_order = messages();
        wrong_order.swap(0, 1);
        assert!(collect("run", "condition", &ids(), &wrong_order).is_err());
        for name in ["memory_write", "reference", "experience_propose"] {
            let mut forbidden = pair("failed", name, true, "not operation evidence");
            forbidden.extend(pair("fixed", "exec", false, "ok"));
            assert!(collect("run", "condition", &ids(), &forbidden).is_err());
        }
    }

    #[test]
    fn rejects_only_success_only_failure_and_later_failure() {
        for statuses in [[false, false], [true, true], [false, true]] {
            let mut messages = pair("failed", "exec", statuses[0], "output");
            messages.extend(pair("fixed", "exec", statuses[1], "output"));
            assert!(collect("run", "condition", &ids(), &messages).is_err());
        }
    }

    #[test]
    fn utf8_excerpt_hashes_complete_output_and_multiple_blocks() {
        let output = "好".repeat(400);
        let mut messages = pair("failed", "exec", true, &output);
        if let AgentMessage::ToolResult(result) = &mut messages[1] {
            result.content.push(Content::text("tail"));
        }
        messages.extend(pair("fixed", "exec", false, "ok"));
        let evidence = collect("run", "condition", &ids(), &messages).unwrap();
        let receipt = &evidence.receipts[0];
        assert_eq!(receipt.excerpt.len(), 1023);
        assert!(receipt.truncated);
        let mut hash = Sha256::new();
        hash.update(format!("{output}\ntail"));
        assert_eq!(receipt.output_hash, hex_digest(hash));
        evidence.validate().unwrap();
    }

    #[test]
    fn validates_deserialized_shape_bounds_hash_and_status() {
        let evidence = collect("run", "condition", &ids(), &messages()).unwrap();
        let mut value = serde_json::to_value(&evidence).unwrap();
        value["injected"] = json!(true);
        assert!(serde_json::from_value::<Evidence>(value).is_err());
        let mut value = serde_json::to_value(&evidence).unwrap();
        value["receipts"][0]["injected"] = json!(true);
        assert!(serde_json::from_value::<Evidence>(value).is_err());
        for mutate in 0..8 {
            let mut bad = evidence.clone();
            match mutate {
                0 => bad.conditions = " ".into(),
                1 => bad.conditions = "x".repeat(1025),
                2 => bad.receipts[0].output_hash = "G".repeat(64),
                3 => bad.receipts[0].excerpt.push('x'),
                4 => bad.receipts[1].tool_call_id = bad.receipts[0].tool_call_id.clone(),
                5 => bad.receipts[0].is_error = false,
                6 => bad.receipts[0].source = "model".into(),
                _ => bad.run = "x".repeat(257),
            }
            assert!(bad.validate().is_err());
        }
    }

    #[test]
    fn directory_advertises_only_native_recovery_ids_usable_by_collect() {
        let mut messages = messages();
        messages.extend(pair("last-success", "read_file", false, "OUTPUT_NEVER_IN_DIRECTORY"));
        let value = directory(&messages).unwrap();
        assert_eq!(value["source"], "native_current_run");
        assert_eq!(value["receipts"].as_array().unwrap().len(), 2);
        assert_eq!(value["receipts"][0]["tool_call_id"], "failed");
        assert_eq!(value["receipts"][1]["tool_call_id"], "last-success");
        let selected: Vec<String> = value["receipts"]
            .as_array()
            .unwrap()
            .iter()
            .map(|receipt| receipt["tool_call_id"].as_str().unwrap().into())
            .collect();
        collect("run", "condition", &selected, &messages).unwrap();
        assert!(value.to_string().len() <= 2048);
        assert!(!value.to_string().contains("OUTPUT_NEVER_IN_DIRECTORY"));
        assert!(!value.to_string().contains("output_hash"));
    }

    #[test]
    fn directory_requires_recovery_after_the_latest_allowed_failure() {
        assert!(directory(&[]).is_none());
        assert!(directory(&pair("success", "exec", false, "ok")).is_none());
        assert!(directory(&pair("failure", "exec", true, "error")).is_none());
        let mut messages = messages();
        messages.extend(pair("new-unrecovered-failure", "exec", true, "error"));
        assert!(directory(&messages).is_none());
        let mut only_other_tools = pair("failure", "memory_propose", true, "error");
        only_other_tools.extend(pair("success", "memory_propose", false, "candidate"));
        assert!(directory(&only_other_tools).is_none());
    }

    #[test]
    fn directory_rejects_unknown_mismatched_duplicate_and_invalid_native_ids() {
        let mut unknown = messages();
        unknown.remove(0);
        assert!(directory(&unknown).is_none());
        let mut mismatch = messages();
        if let AgentMessage::ToolResult(result) = &mut mismatch[1] {
            result.tool_name = "exec".into();
        }
        assert!(directory(&mismatch).is_none());
        let mut duplicate_call = messages();
        duplicate_call.push(duplicate_call[0].clone());
        assert!(directory(&duplicate_call).is_none());
        let mut duplicate_result = messages();
        duplicate_result.push(duplicate_result[1].clone());
        assert!(directory(&duplicate_result).is_none());
        let mut invalid = pair(&"x".repeat(257), "exec", true, "error");
        invalid.extend(pair("success", "exec", false, "ok"));
        assert!(directory(&invalid).is_none());
    }
}
