//! Keeps the documentation and the prompts in step with the code: the README carries the shipped
//! prompts verbatim, every tool name in a prompt is a real MCP tool, and the README lists them all.

mod common;

use ara_lite::help::{HELP_TEXT, MASTER_PROMPT, WORKER_PROMPT};
use ara_lite::profile::save_private;
use ara_lite::types::Connection;
use common::McpProcess;
use serde_json::json;
use std::collections::BTreeSet;
use std::path::PathBuf;

fn crate_file(relative: &str) -> String {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join(relative);
    std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("{}: {e}", path.display())).replace("\r\n", "\n")
}

/// Tool names reported by the real `ara-lite mcp` process (`tools/list` needs no running service).
fn tool_names() -> BTreeSet<String> {
    let dir = tempfile::tempdir().unwrap();
    let profile = dir.path().join("profile.json");
    let connection = Connection {
        url: "http://127.0.0.1:9".into(),
        token: "docs-test-token".into(),
        id: "c1".into(),
        role: "worker".into(),
    };
    save_private(&profile, &connection, false).unwrap();
    let mut mcp = McpProcess::start(&profile);
    let reply = mcp.request("tools/list", json!({}));
    reply["result"]["tools"].as_array().unwrap().iter().map(|t| t["name"].as_str().unwrap().to_string()).collect()
}

/// `snake_case` words that look like tool names (they end the way tool names end).
fn tool_like_words(text: &str) -> BTreeSet<String> {
    const ENDINGS: [&str; 7] = ["_task", "_tasks", "_clients", "_messages", "_message", "_progress", "_stop"];
    text.split(|c: char| !(c.is_ascii_lowercase() || c == '_'))
        .filter(|w| ENDINGS.iter().any(|e| w.ends_with(e)) && w.contains('_'))
        .map(str::to_string)
        .collect()
}

#[test]
fn readme_carries_the_shipped_prompts_verbatim() {
    let readme = crate_file("README.md");
    assert!(readme.contains(MASTER_PROMPT.trim_end()), "README master prompt differs from src/help.rs");
    assert!(readme.contains(WORKER_PROMPT.trim_end()), "README worker prompt differs from src/help.rs");
}

#[test]
fn prompts_use_only_real_tools_and_cover_each_role() {
    let tools = tool_names();
    assert_eq!(tools.len(), 14, "{tools:?}");
    for (role, prompt, needed) in [
        (
            "master",
            MASTER_PROMPT,
            &[
                "poll_messages",
                "ack_messages",
                "list_tasks",
                "list_clients",
                "create_task",
                "snapshot_task",
                "review_task",
                "confirm_stop",
            ][..],
        ),
        (
            "worker",
            WORKER_PROMPT,
            &[
                "poll_messages",
                "ack_messages",
                "list_tasks",
                "claim_task",
                "update_progress",
                "heartbeat",
                "submit_task",
            ][..],
        ),
    ] {
        for name in tool_like_words(prompt) {
            assert!(tools.contains(&name), "{role} prompt names an unknown tool {name:?}");
        }
        for name in needed {
            assert!(prompt.contains(name), "{role} prompt never mentions {name}");
            assert!(tools.contains(*name), "{name} is not an MCP tool");
        }
        assert!(prompt.contains("Never claim to be listening"), "{role} prompt must not promise background listening");
        assert!(prompt.lines().count() <= 25, "{role} prompt grew past 25 lines");
    }
}

#[test]
fn readme_and_help_name_every_tool_and_the_docs_exist() {
    let readme = crate_file("README.md");
    for name in tool_names() {
        assert!(readme.contains(&format!("`{name}`")), "README does not list the MCP tool {name}");
    }
    assert!(HELP_TEXT.contains("mcp --profile FILE"));
    for doc in ["docs/port-map.md", "docs/simplification-proposals.md"] {
        assert!(!crate_file(doc).trim().is_empty(), "{doc} is empty");
    }
    // The prompts must not depend on one shell or platform (a defect of the Go prompts).
    for text in [MASTER_PROMPT, WORKER_PROMPT] {
        assert!(!text.contains("PowerShell") && !text.contains(".exe"), "prompt is shell or platform specific");
    }
}
