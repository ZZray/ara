//! Compaction_Refresh (requirement 19): after a summary replaces history,
//! re-read the files the Run recently touched and the current plan state, so
//! the model continues from disk instead of from a possibly stale summary.
//! `candidates` and `plan` are pure; Core reads the files and stores the
//! messages as references.
use std::{
    collections::BTreeMap,
    io,
    path::{Path, PathBuf},
};

use rpi_agent::AgentMessage;
use rpi_ai::Content;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FileToolKind {
    Read,
    Write,
    Edit,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RefreshPolicy {
    pub max_files: usize,
    pub max_bytes: usize,
    /// Tool name → kind; the `path` argument names the file.
    pub file_tools: BTreeMap<String, FileToolKind>,
}

impl RefreshPolicy {
    /// Preset with the builtin `read_file`, `write_file` and `edit_file`.
    pub fn new(max_files: usize, max_bytes: usize) -> Self {
        let file_tools =
            [("read_file", FileToolKind::Read), ("write_file", FileToolKind::Write), ("edit_file", FileToolKind::Edit)]
                .into_iter()
                .map(|(name, kind)| (name.to_owned(), kind))
                .collect();
        Self { max_files, max_bytes, file_tools }
    }

    pub fn validate(&self) -> anyhow::Result<()> {
        anyhow::ensure!(self.max_files > 0 && self.max_bytes > 0, "compaction_refresh limits must be positive");
        // Each refresh message becomes one context reference (at most 64 KiB).
        anyhow::ensure!(self.max_bytes <= 64 * 1024, "compaction_refresh max_bytes must not exceed 65536");
        Ok(())
    }

    pub fn declaration(&self) -> Value {
        json!({"max_files": self.max_files, "max_bytes": self.max_bytes, "file_tools": self.file_tools})
    }
}

pub(crate) const HEADER: &str =
    "Compaction refresh (source=compaction_refresh; current disk state, supersedes the summary for this file).";
pub(crate) const PLAN_HEADER: &str =
    "Compaction refresh (source=compaction_refresh; current plan state, supersedes the summary for the plan).";

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Candidate {
    pub path: PathBuf,
    pub last_raw_index: usize,
}

/// Files of successful file-tool calls in `messages`, deduplicated by path,
/// most recent first, at most `max_files`.
pub(crate) fn candidates(
    messages: &[AgentMessage],
    raw_base: usize,
    cwd: &Path,
    policy: &RefreshPolicy,
) -> Vec<Candidate> {
    let mut calls = BTreeMap::new();
    let mut latest: BTreeMap<PathBuf, usize> = BTreeMap::new();
    for (offset, message) in messages.iter().enumerate() {
        match message {
            AgentMessage::Assistant(assistant) => {
                for content in &assistant.content {
                    if let Content::ToolCall(call) = content {
                        if let (true, Some(path)) = (
                            policy.file_tools.contains_key(&call.name),
                            call.arguments.get("path").and_then(Value::as_str),
                        ) {
                            calls.insert(call.id.clone(), cwd.join(path));
                        }
                    }
                }
            }
            AgentMessage::ToolResult(result) if !result.is_error => {
                if let Some(path) = calls.get(&result.tool_call_id) {
                    latest.insert(path.clone(), raw_base + offset);
                }
            }
            _ => {}
        }
    }
    let mut found: Vec<Candidate> =
        latest.into_iter().map(|(path, last_raw_index)| Candidate { path, last_raw_index }).collect();
    found.sort_by_key(|candidate| std::cmp::Reverse(candidate.last_raw_index));
    found.truncate(policy.max_files);
    found
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum ReadErrorKind {
    NotFound,
    PermissionDenied,
    Other,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(tag = "result", rename_all = "snake_case")]
pub(crate) enum FileOutcome {
    Full,
    Window { start: u64, end: u64 },
    ReferenceOnly,
    ReadError { kind: ReadErrorKind },
    SkippedByLimit,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct RefreshPlan {
    pub messages: Vec<String>,
    pub outcomes: Vec<(PathBuf, FileOutcome, Option<String>)>,
    pub bytes: usize,
    pub plan_state: bool,
}

impl RefreshPlan {
    /// `compaction_refresh` event (19.9).
    pub(crate) fn event(&self) -> Value {
        let files: Vec<Value> = self
            .outcomes
            .iter()
            .map(|(path, outcome, sha256)| json!({"path": path, "result": outcome, "sha256": sha256}))
            .collect();
        json!({"type": "compaction_refresh", "files": files, "bytes": self.bytes, "plan_state": self.plan_state})
    }
}

/// Largest prefix of `bytes` of at most `limit` bytes ending on a UTF-8 boundary.
fn utf8_prefix(text: &str, limit: usize) -> &str {
    let mut end = limit.min(text.len());
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    &text[..end]
}

/// Plan state first, then files in candidate order; the total message bytes
/// never exceed `max_bytes`. A file that does not fit is shortened to a
/// window, then to a reference, then skipped.
pub(crate) fn plan(
    plan_state: Option<String>,
    files: Vec<(PathBuf, io::Result<Vec<u8>>)>,
    max_bytes: usize,
) -> RefreshPlan {
    fn push(result: &mut RefreshPlan, text: String) {
        result.bytes += text.len();
        result.messages.push(text);
    }
    let mut result = RefreshPlan { messages: Vec::new(), outcomes: Vec::new(), bytes: 0, plan_state: false };
    if let Some(state) = plan_state {
        let text = format!("{PLAN_HEADER}\n{state}");
        if text.len() <= max_bytes {
            push(&mut result, text);
            result.plan_state = true;
        }
    }
    for (path, read) in files {
        let remaining = max_bytes - result.bytes;
        let shown = path.display().to_string();
        let bytes = match read {
            Err(error) => {
                let kind = match error.kind() {
                    io::ErrorKind::NotFound => ReadErrorKind::NotFound,
                    io::ErrorKind::PermissionDenied => ReadErrorKind::PermissionDenied,
                    _ => ReadErrorKind::Other,
                };
                let text = format!("{HEADER}\npath={shown}\nerror={}", serde_json::to_value(kind).unwrap_or_default());
                let outcome = if text.len() <= remaining {
                    push(&mut result, text);
                    FileOutcome::ReadError { kind }
                } else {
                    FileOutcome::SkippedByLimit
                };
                result.outcomes.push((path, outcome, None));
                continue;
            }
            Ok(bytes) => bytes,
        };
        let sha256 = format!("{:x}", Sha256::digest(&bytes));
        let reference = format!("{HEADER}\npath={shown}\nsha256={sha256}\nbytes={}", bytes.len());
        let outcome = match std::str::from_utf8(&bytes) {
            Ok(text) if reference.len() + 1 + text.len() <= remaining => {
                push(&mut result, format!("{reference}\n{text}"));
                FileOutcome::Full
            }
            Ok(text) => {
                let window_header = format!("{reference}\nwindow=0..");
                // Room for the header, the end offset digits and the newline.
                let room = remaining.saturating_sub(window_header.len() + 21);
                let window = utf8_prefix(text, room);
                if !window.is_empty() {
                    push(&mut result, format!("{window_header}{}\n{window}", window.len()));
                    FileOutcome::Window { start: 0, end: window.len() as u64 }
                } else if reference.len() <= remaining {
                    push(&mut result, reference);
                    FileOutcome::ReferenceOnly
                } else {
                    FileOutcome::SkippedByLimit
                }
            }
            Err(_) if reference.len() <= remaining => {
                push(&mut result, reference);
                FileOutcome::ReferenceOnly
            }
            Err(_) => FileOutcome::SkippedByLimit,
        };
        result.outcomes.push((path, outcome, Some(sha256)));
    }
    debug_assert!(result.bytes <= max_bytes);
    result
}

#[cfg(test)]
mod tests {
    use super::*;
    use rpi_ai::{ToolCall, ToolResultMessage};

    fn assistant(calls: &[(&str, &str, &str)]) -> AgentMessage {
        let content = calls
            .iter()
            .map(|(id, name, path)| {
                Content::ToolCall(ToolCall {
                    kind: Default::default(),
                    id: (*id).into(),
                    name: (*name).into(),
                    arguments: json!({"path": path}),
                    thought_signature: None,
                    namespace: None,
                })
            })
            .collect();
        let mut message = rpi_ai::AssistantMessage::empty(rpi_ai::Api::OpenaiCompletions, "fixture", "fixture", 0);
        message.stop_reason = rpi_ai::StopReason::ToolUse;
        message.content = content;
        AgentMessage::Assistant(Box::new(message))
    }

    fn result(id: &str, is_error: bool) -> AgentMessage {
        AgentMessage::ToolResult(Box::new(ToolResultMessage {
            role: Default::default(),
            tool_call_id: id.into(),
            tool_name: "read_file".into(),
            content: vec![Content::text("x")],
            details: None,
            usage: None,
            added_tool_names: vec![],
            is_error,
            timestamp: 0,
        }))
    }

    #[test]
    fn candidates_are_successful_file_calls_newest_first_and_deduplicated() {
        let cwd = Path::new("/w");
        let messages = vec![
            assistant(&[("1", "read_file", "a.cpp"), ("2", "write_file", "b.cpp"), ("3", "exec", "c")]),
            result("1", false),
            result("2", true),
            assistant(&[("4", "edit_file", "a.cpp"), ("5", "read_file", "d.cpp")]),
            result("4", false),
            result("5", false),
        ];
        let found = candidates(&messages, 100, cwd, &RefreshPolicy::new(5, 1000));
        assert_eq!(
            found,
            vec![
                Candidate { path: cwd.join("d.cpp"), last_raw_index: 105 },
                Candidate { path: cwd.join("a.cpp"), last_raw_index: 104 },
            ]
        );
        assert_eq!(candidates(&messages, 0, cwd, &RefreshPolicy::new(1, 1000)).len(), 1);
    }

    #[test]
    fn plan_fits_the_byte_budget_with_windows_references_and_skips() {
        let files = || {
            vec![
                (PathBuf::from("a.txt"), Ok("中文".repeat(100).into_bytes())),
                (PathBuf::from("b.bin"), Ok(vec![0xff, 0xfe])),
                (PathBuf::from("c.txt"), Err(io::Error::from(io::ErrorKind::NotFound))),
            ]
        };
        let roomy = plan(Some("goal: x".into()), files(), 100_000);
        assert!(roomy.plan_state);
        assert_eq!(
            roomy.outcomes.iter().map(|(_, outcome, _)| outcome.clone()).collect::<Vec<_>>(),
            vec![
                FileOutcome::Full,
                FileOutcome::ReferenceOnly,
                FileOutcome::ReadError { kind: ReadErrorKind::NotFound }
            ]
        );
        for budget in [0, 50, 200, 400, 700, 2000] {
            let tight = plan(Some("goal: x".into()), files(), budget);
            assert!(tight.bytes <= budget, "{budget}");
            assert_eq!(tight.bytes, tight.messages.iter().map(String::len).sum::<usize>());
            if let Some((_, FileOutcome::Window { end, .. }, _)) = tight.outcomes.first() {
                assert!("中文".repeat(100).is_char_boundary(*end as usize));
            }
        }
        assert_eq!(plan(None, files(), 0).outcomes[0].1, FileOutcome::SkippedByLimit);
    }
}
