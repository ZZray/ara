//! Requirement 26 byte preservation.
//!
//! - Property 35: `local_tools::tools` `write_file` then `read_file` returns
//!   the exact UTF-8 bytes (no inserted `EF BF BD`, no cut at `00`, no
//!   normalization). At least 100 generated cases plus the fixed sample.
//! - DR-P3-1 (conclusion: model patch): a loopback fake upstream replays the
//!   recorded model tool-call arguments of `closed-budget-snapshot-20261025`
//!   through the real agent loop and Lite `edit_file`; disk bytes must equal
//!   the patch target text, and a replayed patch whose target is the sample
//!   leaves exactly the 17 sample bytes on disk.
mod support;

use std::sync::Arc;

use ara_agent_lite_core::{
    local_tools,
    rpi_agent::{types::TextContentOrImage, AgentTool},
};
use proptest::{
    prelude::*,
    test_runner::{Config, TestCaseError, TestError, TestRunner},
};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use support::{call, run, text, Provider, TempDir};
use tokio_util::sync::CancellationToken;

const SAMPLE: &str = "header\u{0}中文😀";
const SAMPLE_BYTES: [u8; 17] =
    [0x68, 0x65, 0x61, 0x64, 0x65, 0x72, 0x00, 0xE4, 0xB8, 0xAD, 0xE6, 0x96, 0x87, 0xF0, 0x9F, 0x98, 0x80];
const REPLACEMENT: &[u8] = &[0xEF, 0xBF, 0xBD];
/// Exact `tool_call` frame `rpc-19` of the closed-budget snapshot live Run
/// (`acr-live-snapshot-v1/harness/jsonl-rpc-19-request.json`).
const RPC_19: &[u8] = include_bytes!("fixtures/dr-p3-1/jsonl-rpc-19-request.json");
const RPC_19_SHA256: &str = "dc0ad1f1de11ec906e788de49265698dcafc1d77f92dde9423327a195b2cd18c";

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|byte| format!("{byte:02X}")).collect::<Vec<_>>().join(" ")
}

fn count(haystack: &[u8], needle: &[u8]) -> usize {
    haystack.windows(needle.len()).filter(|window| *window == needle).count()
}

fn tool(tools: &[Arc<dyn AgentTool>], name: &str) -> Arc<dyn AgentTool> {
    tools.iter().find(|tool| tool.schema().name == name).cloned().unwrap()
}

async fn invoke(tool: &Arc<dyn AgentTool>, args: Value) -> String {
    let result = tool.execute("byte-roundtrip", args, CancellationToken::new(), Arc::new(|_| {})).await.unwrap();
    result
        .content
        .iter()
        .map(|part| match part {
            TextContentOrImage::Text(text) => text.text.clone(),
            TextContentOrImage::Image(_) => panic!("read_file returned an image"),
        })
        .collect()
}

/// Write `content` with `write_file`, read it back with `read_file`;
/// returns (disk bytes, read_file text bytes).
async fn roundtrip(root: &std::path::Path, content: &str) -> (Vec<u8>, Vec<u8>) {
    let tools = local_tools::tools(root.to_path_buf());
    invoke(&tool(&tools, "write_file"), json!({"path":"bytes.txt","content":content})).await;
    let disk = std::fs::read(root.join("bytes.txt")).unwrap();
    let read = invoke(&tool(&tools, "read_file"), json!({"path":"bytes.txt"})).await;
    (disk, read.into_bytes())
}

/// Property 35 generator: 4..=1024 scalar values with at least one U+0000,
/// one U+10000..=U+10FFFF and one base + U+0300..=U+036F pair; the rest are
/// any scalar except U+000A and U+000D.
fn generated() -> impl Strategy<Value = String> {
    let filler =
        proptest::collection::vec(any::<char>().prop_filter("no LF/CR", |c| !matches!(c, '\n' | '\r')), 0..=1020);
    let supplementary = (0x10000u32..=0x10FFFF).prop_map(|code| char::from_u32(code).unwrap());
    let base = proptest::char::range('a', 'z');
    let combining = (0x300u32..=0x36F).prop_map(|code| char::from_u32(code).unwrap());
    (filler, supplementary, base, combining, any::<[prop::sample::Index; 3]>()).prop_map(
        |(mut chars, supplementary, base, combining, at)| {
            let nul = at[0].index(chars.len() + 1);
            chars.insert(nul, '\u{0}');
            let wide = at[1].index(chars.len() + 1);
            chars.insert(wide, supplementary);
            let pair = at[2].index(chars.len() + 1);
            chars.splice(pair..pair, [base, combining]);
            chars.into_iter().collect()
        },
    )
}

#[test]
fn property_35_tool_io_bytes_roundtrip() {
    let root = TempDir::new();
    let runtime = tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap();
    // The fixed sample always runs first.
    let (disk, read) = runtime.block_on(roundtrip(root.path(), SAMPLE));
    assert_eq!(disk, SAMPLE_BYTES, "sample disk bytes: {}", hex(&disk));
    assert_eq!(read, SAMPLE_BYTES, "sample read_file bytes: {}", hex(&read));

    let mut runner = TestRunner::new(Config { cases: 256, failure_persistence: None, ..Config::default() });
    let result = runner.run(&generated(), |input| {
        let expected = input.as_bytes();
        let (disk, read) = runtime.block_on(roundtrip(root.path(), &input));
        for (stage, actual) in [("disk", &disk), ("read_file", &read)] {
            if actual.as_slice() != expected {
                return Err(TestCaseError::fail(format!(
                    "{stage} bytes differ (expected {} bytes, got {}; inserted EF BF BD: {})",
                    expected.len(),
                    actual.len(),
                    count(actual, REPLACEMENT) > count(expected, REPLACEMENT)
                )));
            }
        }
        Ok(())
    });
    if let Err(TestError::Fail(reason, minimal)) = &result {
        panic!("{reason}\nminimal counterexample hex: {}", hex(minimal.as_bytes()));
    }
    result.unwrap();
}

/// DR-P3-1 Lite tool I/O leg: entry and exit hex of the sample.
#[test]
fn dr_p3_1_tool_io_leg_preserves_sample() {
    let root = TempDir::new();
    let runtime = tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap();
    let args = json!({"path":"bytes.txt","content":SAMPLE});
    let (disk, read) = runtime.block_on(roundtrip(root.path(), SAMPLE));
    println!("entry write_file content: {}", hex(SAMPLE.as_bytes()));
    println!("entry write_file args JSON: {}", serde_json::to_string(&args).unwrap());
    println!("disk: {}", hex(&disk));
    println!("exit read_file: {}", hex(&read));
    assert_eq!(disk, SAMPLE_BYTES);
    assert_eq!(read, SAMPLE_BYTES);
    assert_eq!(count(&read, REPLACEMENT), 0);
}

/// The recorded rpc-19 `source_replace` arguments, checked against the frame's
/// own `args_sha256`.
fn recorded_patch() -> (String, String) {
    assert_eq!(format!("{:x}", Sha256::digest(RPC_19)), RPC_19_SHA256, "fixture frame bytes changed");
    let frame: Value = serde_json::from_slice(RPC_19).unwrap();
    let args = &frame["arguments"];
    let canonical = serde_json::to_vec(args).unwrap();
    assert_eq!(format!("{:x}", Sha256::digest(&canonical)), frame["args_sha256"].as_str().unwrap());
    (args["old"].as_str().unwrap().to_owned(), args["new"].as_str().unwrap().to_owned())
}

/// Replays the given model tool calls through the agent loop with Lite local
/// tools enabled; returns the disk bytes of `name` afterwards.
fn replay(root: &std::path::Path, calls: Vec<Value>, name: &str) -> Vec<u8> {
    let mut responses = calls;
    responses.push(text("Replayed."));
    let provider = Provider::new(responses);
    let mut request = support::request(root, &provider, &[], 8, "Replay the recorded patch.");
    request.enable_local_tools = true;
    let (result, events) = support::runtime().block_on(run(request));
    assert_eq!(result.code, 0, "replay Run failed: {}", result.error);
    let ends = support::of_type(&events, "tool_end");
    assert!(!ends.is_empty(), "no tool was replayed");
    for end in ends {
        assert_eq!(end["status"], "completed", "replayed tool failed: {end}");
    }
    std::fs::read(root.join(name)).unwrap()
}

/// 26.4: the recorded patch text reaches disk byte for byte. The NUL → U+FFFD
/// rewrite is source code inside the patch (`text.replace("\x00", "\ufffd")`),
/// carried as ASCII escapes; it only changes text when that code runs.
#[test]
fn dr_p3_1_recorded_model_patch_replays_byte_for_byte() {
    let (old, new) = recorded_patch();
    let marker = br#"return text.replace("\x00", "\ufffd") if "\x00" in text else text"#;
    assert_eq!(count(new.as_bytes(), marker), 1, "recorded patch carries the NUL rewrite as source text");
    let root = TempDir::new();
    let original = format!("import os\n\n\n{old}\n    return [text]\n");
    std::fs::write(root.path().join("git_service.py"), &original).unwrap();
    let expected = original.replacen(&old, &new, 1).into_bytes();
    let disk = replay(
        root.path(),
        vec![call("call_replay_19", "edit_file", json!({"path":"git_service.py","old_text":old,"new_text":new}))],
        "git_service.py",
    );
    assert_eq!(
        disk,
        expected,
        "disk differs from patch target; disk hex tail: {}",
        hex(&disk[disk.len().saturating_sub(64)..])
    );
    assert_eq!(count(&disk, REPLACEMENT), count(&expected, REPLACEMENT));
    assert_eq!(count(&disk, b"\x00"), 0);
}

/// 26.4: `EF BF BD` already inside a patch is kept, not added or removed.
#[test]
fn dr_p3_1_patch_with_existing_replacement_char_is_kept() {
    let (old, new) = recorded_patch();
    let new = new.replacen("# NUL 会让部分", "# \u{FFFD} 与 \u{0} 原样保留；NUL 会让部分", 1);
    assert_eq!(count(new.as_bytes(), REPLACEMENT), 1);
    let root = TempDir::new();
    let original = format!("{old}\n");
    std::fs::write(root.path().join("git_service.py"), &original).unwrap();
    let expected = original.replacen(&old, &new, 1).into_bytes();
    let disk = replay(
        root.path(),
        vec![call("call_replay_fffd", "edit_file", json!({"path":"git_service.py","old_text":old,"new_text":new}))],
        "git_service.py",
    );
    assert_eq!(disk, expected);
    assert_eq!(count(&disk, REPLACEMENT), 1);
    assert_eq!(count(&disk, b"\x00"), 1);
}

/// 26.4: a replayed patch whose target text is the sample leaves 17 bytes.
#[test]
fn dr_p3_1_replayed_sample_patch_writes_seventeen_bytes() {
    let root = TempDir::new();
    let disk = replay(
        root.path(),
        vec![call("call_replay_sample", "write_file", json!({"path":"sample.txt","content":SAMPLE}))],
        "sample.txt",
    );
    assert_eq!(disk, SAMPLE_BYTES, "disk hex: {}", hex(&disk));
}
