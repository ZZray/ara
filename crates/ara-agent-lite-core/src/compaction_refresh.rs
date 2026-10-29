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

/// Property 25 (requirements 19.1–19.3, 19.8, 19.9): candidate selection and
/// the byte budget of the refresh messages, against reference models.
#[cfg(test)]
mod property_tests {
    use super::*;
    use proptest::prelude::*;
    use rpi_ai::{ToolCall, ToolResultMessage};

    const TOOLS: [&str; 4] = ["read_file", "write_file", "edit_file", "exec"];
    const PATHS: [&str; 4] = ["a.rs", "b.rs", "dir/c.rs", "d.txt"];

    fn assistant(calls: &[(String, usize, usize)]) -> AgentMessage {
        let mut message = rpi_ai::AssistantMessage::empty(rpi_ai::Api::OpenaiCompletions, "fixture", "fixture", 0);
        message.stop_reason = rpi_ai::StopReason::ToolUse;
        message.content = calls
            .iter()
            .map(|(id, tool, path)| {
                Content::ToolCall(ToolCall {
                    kind: Default::default(),
                    id: id.clone(),
                    name: TOOLS[*tool].into(),
                    arguments: json!({"path": PATHS[*path]}),
                    thought_signature: None,
                    namespace: None,
                })
            })
            .collect();
        AgentMessage::Assistant(Box::new(message))
    }

    fn result(id: &str, tool: usize, is_error: bool) -> AgentMessage {
        AgentMessage::ToolResult(Box::new(ToolResultMessage {
            role: Default::default(),
            tool_call_id: id.into(),
            tool_name: TOOLS[tool].into(),
            content: vec![Content::text("x")],
            details: None,
            usage: None,
            added_tool_names: vec![],
            is_error,
            timestamp: 0,
        }))
    }

    /// (tool, path, succeeded, raw index) of each answered call.
    type Answered = Vec<(usize, usize, bool, usize)>;

    /// Rounds of (tool, path, succeeded) calls, each answered in order.
    fn history(rounds: &[Vec<(usize, usize, bool)>]) -> (Vec<AgentMessage>, Answered) {
        let (mut messages, mut answered, mut next) = (Vec::new(), Vec::new(), 0);
        for round in rounds {
            let calls: Vec<_> = round
                .iter()
                .map(|(tool, path, _)| {
                    next += 1;
                    (format!("c{next}"), *tool, *path)
                })
                .collect();
            messages.push(assistant(&calls));
            for ((id, tool, path), (_, _, ok)) in calls.iter().zip(round) {
                answered.push((*tool, *path, *ok, messages.len()));
                messages.push(result(id, *tool, !ok));
            }
        }
        (messages, answered)
    }

    #[derive(Clone, Debug)]
    enum Disk {
        Text(String),
        Binary(Vec<u8>),
        Missing,
        Denied,
        Broken,
    }

    fn disk() -> impl Strategy<Value = Disk> {
        prop_oneof![
            4 => "[a-z\n中文😀]{0,600}".prop_map(Disk::Text),
            1 => prop::collection::vec(0x80u8..=0xff, 1..64).prop_map(Disk::Binary),
            1 => Just(Disk::Missing),
            1 => Just(Disk::Denied),
            1 => Just(Disk::Broken),
        ]
    }

    fn read(disk: &Disk) -> io::Result<Vec<u8>> {
        match disk {
            Disk::Text(text) => Ok(text.clone().into_bytes()),
            Disk::Binary(bytes) => Ok(bytes.clone()),
            Disk::Missing => Err(io::Error::from(io::ErrorKind::NotFound)),
            Disk::Denied => Err(io::Error::from(io::ErrorKind::PermissionDenied)),
            Disk::Broken => Err(io::Error::other("device failed with secret old contents")),
        }
    }

    proptest! {
        #![proptest_config(ProptestConfig::with_cases(256))]

        #[test]
        fn candidates_are_recent_successful_file_paths_deduplicated(
            rounds in prop::collection::vec(prop::collection::vec((0..4usize, 0..4usize, any::<bool>()), 1..4), 0..8),
            max_files in 1usize..5,
            raw_base in 0usize..50,
        ) {
            let cwd = Path::new("/workspace");
            let (messages, answered) = history(&rounds);
            let mut expected: Vec<Candidate> = Vec::new();
            for (tool, path, ok, index) in answered.iter().rev() {
                let path = cwd.join(PATHS[*path]);
                if *ok && *tool < 3 && !expected.iter().any(|candidate| candidate.path == path) {
                    expected.push(Candidate { path, last_raw_index: raw_base + index });
                }
            }
            expected.truncate(max_files);
            let policy = RefreshPolicy::new(max_files, 1024);
            prop_assert_eq!(candidates(&messages, raw_base, cwd, &policy), expected);
        }

        #[test]
        fn plan_respects_the_byte_budget_and_reports_each_outcome(
            state in prop::option::of("[a-z ]{0,300}"),
            files in prop::collection::vec(disk(), 0..5),
            max_bytes in prop_oneof![1usize..1200, 1200usize..4000, Just(64 * 1024)],
        ) {
            let paths: Vec<PathBuf> = (0..files.len()).map(|index| PathBuf::from(format!("f{index}.txt"))).collect();
            let inputs = paths.iter().cloned().zip(files.iter().map(read)).collect();
            let plan = plan(state.clone(), inputs, max_bytes);
            prop_assert_eq!(plan.bytes, plan.messages.iter().map(String::len).sum::<usize>());
            prop_assert!(plan.bytes <= max_bytes);
            prop_assert_eq!(plan.outcomes.len(), files.len());

            let mut messages = plan.messages.iter();
            let mut used = 0;
            // The plan state is counted first and only when it fits whole.
            let plan_text = state.as_ref().map(|state| format!("{PLAN_HEADER}\n{state}"));
            let plan_fits = plan_text.as_ref().is_some_and(|text| text.len() <= max_bytes);
            prop_assert_eq!(plan.plan_state, plan_fits);
            if plan_fits {
                prop_assert_eq!(messages.next(), plan_text.as_ref());
                used += plan_text.unwrap().len();
            }
            for ((path, disk), (outcome_path, outcome, sha256)) in paths.iter().zip(&files).zip(&plan.outcomes) {
                prop_assert_eq!(outcome_path, path);
                let remaining = max_bytes - used;
                let shown = path.display().to_string();
                let bytes = match read(disk) {
                    Err(error) => {
                        let kind = match error.kind() {
                            io::ErrorKind::NotFound => "not_found",
                            io::ErrorKind::PermissionDenied => "permission_denied",
                            _ => "other",
                        };
                        let text = format!("{HEADER}\npath={shown}\nerror=\"{kind}\"");
                        prop_assert!(sha256.is_none());
                        if text.len() <= remaining {
                            let read_error = matches!(outcome, FileOutcome::ReadError { .. });
                            prop_assert!(read_error);
                            let wire = serde_json::to_value(outcome).unwrap();
                            prop_assert_eq!(wire["kind"].as_str(), Some(kind));
                            prop_assert_eq!(messages.next(), Some(&text));
                            used += text.len();
                        } else {
                            prop_assert_eq!(outcome, &FileOutcome::SkippedByLimit);
                        }
                        continue;
                    }
                    Ok(bytes) => bytes,
                };
                let digest = format!("{:x}", Sha256::digest(&bytes));
                prop_assert_eq!(sha256.as_deref(), Some(digest.as_str()));
                let reference = format!("{HEADER}\npath={shown}\nsha256={digest}\nbytes={}", bytes.len());
                let message = match outcome {
                    FileOutcome::Full => {
                        let text = std::str::from_utf8(&bytes).unwrap();
                        prop_assert!(reference.len() + 1 + text.len() <= remaining);
                        format!("{reference}\n{text}")
                    }
                    FileOutcome::Window { start, end } => {
                        let text = std::str::from_utf8(&bytes).unwrap();
                        let end = *end as usize;
                        prop_assert_eq!(*start, 0);
                        prop_assert!(end > 0 && end < text.len() && text.is_char_boundary(end));
                        // A window is used only when the whole file does not fit.
                        prop_assert!(reference.len() + 1 + text.len() > remaining);
                        format!("{reference}\nwindow=0..{end}\n{}", &text[..end])
                    }
                    FileOutcome::ReferenceOnly => {
                        prop_assert!(
                            std::str::from_utf8(&bytes).is_err() || reference.len() + 1 + bytes.len() > remaining
                        );
                        reference.clone()
                    }
                    FileOutcome::SkippedByLimit => {
                        prop_assert!(reference.len() > remaining);
                        continue;
                    }
                    FileOutcome::ReadError { .. } => {
                        prop_assert!(false, "readable file reported a read error");
                        unreachable!()
                    }
                };
                prop_assert_eq!(messages.next(), Some(&message));
                used += message.len();
            }
            prop_assert!(messages.next().is_none());
            prop_assert_eq!(used, plan.bytes);
            prop_assert!(plan.messages.iter().all(|message| !message.contains("secret old contents")));

            let event = plan.event();
            prop_assert_eq!(event["bytes"].as_u64(), Some(plan.bytes as u64));
            prop_assert_eq!(event["plan_state"].as_bool(), Some(plan.plan_state));
            let files_event = event["files"].as_array().unwrap();
            prop_assert_eq!(files_event.len(), plan.outcomes.len());
            for (row, (path, outcome, sha256)) in files_event.iter().zip(&plan.outcomes) {
                prop_assert_eq!(row["path"].as_str(), path.to_str());
                let wire = serde_json::to_value(outcome).unwrap();
                prop_assert_eq!(&row["result"], &wire);
                prop_assert_eq!(row["sha256"].as_str(), sha256.as_deref());
            }
        }
    }
}
