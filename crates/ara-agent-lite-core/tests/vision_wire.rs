//! Controlled HTTP evidence for generic image input and multimodal summaries.
//! The ignored opt-in trial exercises a real model; no test captures a desktop.
mod support;

use std::{path::Path, sync::Arc};

use anyhow::Result;
use ara_agent_lite_core::{
    execute,
    model::{CatalogModelHost, ModelHost, ModelSelection, Route},
    model_caps::ModelCaps,
    rpi_agent::{AgentError, AgentTool, AgentToolResult, TextContentOrImage, ToolResultPartial},
    rpi_ai::{ImageContent, Schema, Tool},
    session_journal::Journal,
    RunRequest, RunResult, ToolRegistration,
};
use async_trait::async_trait;
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use support::*;
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

// A 1x1 PNG. The provider fixture checks transport, not visual interpretation.
const PNG: &str = "iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAQAAAC1HAwCAAAAC0lEQVR42mP8/x8AAwMCAO+jD0sAAAAASUVORK5CYII=";
const MAX_OUTPUT: u32 = 6000;
const CONTEXT_WINDOW: u64 = 60_000 + MAX_OUTPUT as u64 + 4096;

struct InputHost(Value);

#[async_trait]
impl ModelHost for InputHost {
    async fn resolve(&self, route: Route, endpoint: &str, token: &str, run: &str) -> Result<ModelSelection> {
        let mut selection = CatalogModelHost.resolve(route, endpoint, token, run).await?;
        selection.capabilities.input = ModelCaps::parse(&self.0)?.input;
        Ok(selection)
    }
}

struct ScreenshotTool(Tool);

#[async_trait]
impl AgentTool for ScreenshotTool {
    fn schema(&self) -> &Tool {
        &self.0
    }

    fn label(&self) -> &str {
        &self.0.name
    }

    async fn execute(
        &self,
        _: &str,
        _: Value,
        _: CancellationToken,
        _: Arc<dyn Fn(ToolResultPartial) + Send + Sync>,
    ) -> std::result::Result<AgentToolResult, AgentError> {
        let mut result = AgentToolResult::text("Controlled target-window image.");
        result.content.push(TextContentOrImage::Image(ImageContent {
            kind: Default::default(),
            data: PNG.into(),
            mime_type: "image/png".into(),
        }));
        Ok(result)
    }
}

fn vision_request(root: &Path, provider: &Provider, input: Value, objective: &str) -> RunRequest {
    let mut request = request(root, provider, &[], 4, objective);
    request.model_host = Arc::new(InputHost(input));
    request.max_output = MAX_OUTPUT;
    request.extra_tools.push(ToolRegistration {
        tool: Arc::new(ScreenshotTool(Tool {
            name: "fixture_screenshot".into(),
            description: "Return a controlled image without capturing the desktop.".into(),
            parameters: Schema(json!({"type":"object","additionalProperties":false})),
            constrained_sampling: None,
        })),
        allow_in_plan: true,
    });
    request
}

fn image_urls(body: &Value) -> Vec<&str> {
    body["messages"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|message| message["content"].as_array())
        .flatten()
        .filter_map(|part| part["image_url"]["url"].as_str())
        .collect()
}

fn persisted_images(root: &Path) -> Vec<Value> {
    let journal = Journal::for_session(&root.join("session.json"));
    let count = journal.open().unwrap().raw_count;
    let messages = serde_json::to_value(journal.read(0, count, 4 * 1024 * 1024).unwrap()).unwrap();
    messages
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|message| message["content"].as_array())
        .flatten()
        .filter(|part| part["type"] == "image")
        .cloned()
        .collect()
}

// A refusal before Run admission may be Err; one after an image tool result
// is recorded may be a failed Run. Both must prevent the forbidden HTTP call.
async fn run_observed(request: RunRequest) -> (Result<RunResult>, Vec<Value>) {
    let (sender, mut receiver) = mpsc::channel(256);
    let collector = tokio::spawn(async move {
        let mut events = Vec::new();
        while let Some(event) = receiver.recv().await {
            events.push(event);
        }
        events
    });
    let result = execute(request, CancellationToken::new(), sender).await;
    (result, collector.await.unwrap())
}

fn refusal(result: Result<RunResult>) -> String {
    let error = match result {
        Ok(result) => {
            assert_ne!(result.code, 0, "unsupported image input must not succeed");
            result.error
        }
        Err(error) => error.to_string(),
    };
    assert!(error.to_lowercase().contains("image"), "{error}");
    assert!(!error.contains(PNG), "refusal must not expose image data");
    error
}

#[tokio::test]
async fn image_tool_result_reaches_provider_and_survives_a_new_run() {
    let root = TempDir::new();
    let provider =
        Provider::new(vec![call("capture", "fixture_screenshot", json!({})), text("observed"), text("continued")]);
    let input = json!({"input":["text","image"]});
    let (first, events) =
        run(vision_request(root.path(), &provider, input.clone(), "Inspect the fixture image.")).await;
    assert_eq!(first.code, 0, "{}", first.error);
    assert_eq!(provider.request_count(), 2);
    let expected = format!("data:image/png;base64,{PNG}");
    assert_eq!(image_urls(&provider.bodies()[1]), vec![expected.as_str()]);
    let capabilities = of_type(&events, "model_capabilities");
    assert_eq!(capabilities.len(), 1);
    assert_eq!(capabilities[0]["host_input"], json!(["text", "image"]));
    let before = persisted_images(root.path());
    assert_eq!(before.len(), 1);
    assert_eq!(before[0]["data"], PNG);

    let (second, _) = run(vision_request(root.path(), &provider, input, "Continue from the original image.")).await;
    assert_eq!(second.code, 0, "{}", second.error);
    assert_eq!(provider.request_count(), 3);
    assert_eq!(image_urls(&provider.bodies()[2]), vec![expected.as_str()]);
    assert_eq!(persisted_images(root.path()), before);
}

#[tokio::test]
async fn undeclared_or_conflicting_image_capability_refuses_the_next_request() {
    for input in [json!({"input":["text"]}), json!({}), json!({"input":["text","image"],"omp":{"input":["text"]}})] {
        let root = TempDir::new();
        let provider =
            Provider::new(vec![call("capture", "fixture_screenshot", json!({})), text("must not be requested")]);
        let (result, events) =
            run_observed(vision_request(root.path(), &provider, input, "Inspect the fixture.")).await;
        refusal(result);
        assert_eq!(provider.request_count(), 1);
        assert_eq!(of_type(&events, "model_call").len(), 1, "rejected image must not reserve a model call");
        let images = persisted_images(root.path());
        assert_eq!(images.len(), 1, "the already-produced image remains in the original journal");
        assert_eq!(images[0]["data"], PNG);
    }
}

#[tokio::test]
async fn image_history_summary_sends_image_blocks_and_preserves_original_evidence() {
    let root = TempDir::new();
    let provider = Provider::with(ProviderConfig {
        context_window: CONTEXT_WINDOW,
        max_tokens: 8192,
        ..ProviderConfig::new(vec![
            call("capture", "fixture_screenshot", json!({})),
            text(&"a".repeat(16 * 1024)),
            text("done"),
        ])
    });
    let input = json!({"input":["text","image"]});
    let (first, _) = run(vision_request(root.path(), &provider, input.clone(), "Inspect image and report.")).await;
    assert_eq!(first.code, 0, "{}", first.error);
    let original = persisted_images(root.path());
    assert_eq!(original.len(), 1);
    let journal = Journal::for_session(&root.path().join("session.json"));
    let count = journal.open().unwrap().raw_count;
    let native = serde_json::to_value(journal.read(0, count, 4 * 1024 * 1024).unwrap()).unwrap();
    let (message_index, content_index) = native
        .as_array()
        .unwrap()
        .iter()
        .enumerate()
        .find_map(|(index, message)| {
            message["content"].as_array()?.iter().position(|part| part["type"] == "image").map(|part| (index, part))
        })
        .unwrap();
    let objective = format!("Continue. {}", "q".repeat(16 * 1024));
    let (second, events) = run(vision_request(root.path(), &provider, input, &objective)).await;
    assert_eq!(second.code, 0, "{}", second.error);
    let phases: Vec<_> = of_type(&events, "model_call").iter().map(|event| event["phase"].as_str().unwrap()).collect();
    assert_eq!(phases, ["summary", "agent"]);
    let bodies = provider.bodies();
    assert_eq!(bodies.len(), 4);
    assert!(is_summary(&bodies[2]));
    assert_eq!(image_urls(&bodies[2]), vec![format!("data:image/png;base64,{PNG}")]);
    let text_parts: Vec<_> = bodies[2]["messages"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|message| message["content"].as_array())
        .flatten()
        .filter_map(|part| part["text"].as_str())
        .collect();
    assert!(!text_parts.is_empty());
    assert!(text_parts.iter().all(|part| !part.contains(PNG)), "base64 must not be sent as summary text");
    let marker = json!({"summary_image":{
        "message_index":message_index,
        "content_index":content_index,
        "mime_type":"image/png",
        "base64_sha256":format!("{:x}", Sha256::digest(PNG.as_bytes()))
    }});
    let transcript: Value = serde_json::from_str(text_parts[0]).unwrap();
    let marked_image = &transcript["closed_old_messages"][message_index]["content"][content_index];
    assert_eq!(marked_image["type"], "text");
    assert_eq!(serde_json::from_str::<Value>(marked_image["text"].as_str().unwrap()).unwrap(), marker);
    assert!(
        text_parts.iter().skip(1).any(|part| serde_json::from_str::<Value>(part).ok().as_ref() == Some(&marker)),
        "the visual attachment must carry its matching source/hash label"
    );
    assert_eq!(persisted_images(root.path()), original, "summary must not rewrite raw image evidence");
}

#[tokio::test]
async fn restored_image_history_cannot_be_sent_to_a_text_or_unknown_model() {
    for input in [json!({"input":["text"]}), json!({})] {
        for compact in [false, true] {
            let root = TempDir::new();
            let provider = Provider::with(ProviderConfig {
                context_window: if compact { CONTEXT_WINDOW } else { 1_000_000 },
                max_tokens: 8192,
                ..ProviderConfig::new(vec![
                    call("capture", "fixture_screenshot", json!({})),
                    text(&"a".repeat(if compact { 16 * 1024 } else { 8 })),
                ])
            });
            let (first, _) =
                run(vision_request(root.path(), &provider, json!({"input":["text","image"]}), "Inspect image.")).await;
            assert_eq!(first.code, 0, "{}", first.error);
            let before = persisted_images(root.path());
            assert_eq!(provider.request_count(), 2);
            let objective = format!("Continue. {}", "q".repeat(if compact { 16 * 1024 } else { 8 }));
            let (second, events) =
                run_observed(vision_request(root.path(), &provider, input.clone(), &objective)).await;
            refusal(second);
            assert_eq!(provider.request_count(), 2, "neither summary nor agent may receive unsupported images");
            assert!(of_type(&events, "model_call").is_empty());
            assert_eq!(persisted_images(root.path()), before);
        }
    }
}

#[tokio::test]
async fn failed_image_summary_keeps_original_evidence_and_does_not_install_a_summary() {
    let root = TempDir::new();
    let provider = Provider::with(ProviderConfig {
        context_window: CONTEXT_WINDOW,
        max_tokens: 8192,
        summary_responses: vec![http_error(400)],
        ..ProviderConfig::new(vec![
            call("capture", "fixture_screenshot", json!({})),
            text(&"a".repeat(16 * 1024)),
            text("must not be requested"),
        ])
    });
    let input = json!({"input":["text","image"]});
    let (first, _) = run(vision_request(root.path(), &provider, input.clone(), "Inspect image.")).await;
    assert_eq!(first.code, 0, "{}", first.error);
    let original = persisted_images(root.path());
    let context_path = root.path().join("session.context.json");
    let before: Value = serde_json::from_slice(&std::fs::read(&context_path).unwrap()).unwrap();
    assert!(before["summary"].is_null());
    let objective = format!("Continue. {}", "q".repeat(16 * 1024));
    let (second, events) = run_observed(vision_request(root.path(), &provider, input, &objective)).await;
    match second {
        Ok(result) => assert_ne!(result.code, 0, "failed summary must not produce success"),
        Err(error) => assert!(!error.to_string().is_empty()),
    }
    let phases: Vec<_> = of_type(&events, "model_call").iter().map(|event| event["phase"].as_str().unwrap()).collect();
    assert_eq!(phases, ["summary"]);
    assert_eq!(provider.request_count(), 3, "failed summary must neither retry nor continue the agent");
    assert!(is_summary(&provider.bodies()[2]));
    assert_eq!(image_urls(&provider.bodies()[2]), vec![format!("data:image/png;base64,{PNG}")]);
    let after: Value = serde_json::from_slice(&std::fs::read(&context_path).unwrap()).unwrap();
    assert_eq!(after["summary"], before["summary"]);
    assert_eq!(persisted_images(root.path()), original);
}

// Generated with Python's standard-library struct/zlib/binascii: 64x32 RGB,
// each row is filter byte 0 followed by 32 red and 32 blue pixels. No text,
// customer data, or color labels are sent with the image.
const LIVE_PNG: &str = "iVBORw0KGgoAAAANSUhEUgAAAEAAAAAgCAIAAAAt/+nTAAAAOElEQVR4nO3PsQkAAAzDsPz/dHpFliLwbFCaTBvvu94DAAAAAAAAAAAAAAAAAAAAAAAAAAAAPAQcOkv4asq/aw0AAAAASUVORK5CYII=";

struct LiveImageTool {
    schema: Tool,
    calls: std::sync::atomic::AtomicUsize,
}

#[async_trait]
impl AgentTool for LiveImageTool {
    fn schema(&self) -> &Tool {
        &self.schema
    }

    fn label(&self) -> &str {
        &self.schema.name
    }

    async fn execute(
        &self,
        _: &str,
        _: Value,
        _: CancellationToken,
        _: Arc<dyn Fn(ToolResultPartial) + Send + Sync>,
    ) -> std::result::Result<AgentToolResult, AgentError> {
        self.calls.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        let mut result = AgentToolResult::text("Inspect this generated fixture image.");
        result.content.push(TextContentOrImage::Image(ImageContent {
            kind: Default::default(),
            data: LIVE_PNG.into(),
            mime_type: "image/png".into(),
        }));
        Ok(result)
    }
}

/// Explicit opt-in only. The caller chooses an authorized endpoint/model and a
/// fresh output directory. Native history stays in a temporary directory;
/// result.json contains final text and selected receipts, never thinking or
/// request/authentication frames. No provider failure is automatically retried.
#[tokio::test]
#[ignore = "requires authorized LITE_VISION_ENDPOINT/MODEL/TOKEN/OUT and a real vision model"]
async fn live_image_tool_result_is_understood_by_the_selected_model() {
    use base64::Engine;

    let endpoint = std::env::var("LITE_VISION_ENDPOINT").expect("set LITE_VISION_ENDPOINT");
    let model = std::env::var("LITE_VISION_MODEL").expect("set LITE_VISION_MODEL");
    let token = std::env::var("LITE_VISION_TOKEN").expect("set LITE_VISION_TOKEN");
    assert!(!token.is_empty(), "LITE_VISION_TOKEN must not be empty");
    let output = std::path::PathBuf::from(std::env::var_os("LITE_VISION_OUT").expect("set LITE_VISION_OUT"));
    assert!(!output.exists(), "LITE_VISION_OUT must be a fresh evidence directory");
    std::fs::create_dir_all(&output).expect("create evidence directory");
    let png = base64::engine::general_purpose::STANDARD.decode(LIVE_PNG).expect("valid fixture PNG");
    let png_hash = format!("{:x}", Sha256::digest(&png));
    assert_eq!(png_hash, "fc75966897d50143d883d0f9cc09b6b508989a4a6dcb03650983dba508377741");
    std::fs::write(output.join("fixture.png"), png).expect("write generated fixture");
    let root = TempDir::new();
    let tool = Arc::new(LiveImageTool {
        schema: Tool {
            name: "inspect_fixture".into(),
            description: "Return a generated image for visual inspection.".into(),
            parameters: Schema(json!({"type":"object","additionalProperties":false})),
            constrained_sampling: None,
        },
        calls: std::sync::atomic::AtomicUsize::new(0),
    });
    let mut request = RunRequest::new(
        format!("live-vision-{}", ara_agent_lite_core::fresh_id()),
        "First call inspect_fixture exactly once. Determine the dominant color in the left half and right half of the returned image by looking at that image. Reply only with a JSON object containing keys left and right, each a lower-case English color name. Do not guess before receiving the image.",
        ara_agent_lite_core::RuntimePaths::new(root.path(), root.path().join("session.json"), root.path().join("environment")),
    );
    request.endpoint = endpoint;
    request.token = token.clone();
    request.model = model.clone();
    request.route.model = model.clone();
    request.route.protocol = "openai-completions".into();
    request.mode = ara_agent_lite_core::RunMode::Execute;
    request.max_calls = 3;
    request.max_seconds = 60;
    request.max_output = 512;
    request.extra_tools.push(ToolRegistration { tool: tool.clone(), allow_in_plan: false });
    let run_id = request.id.clone();
    let started_at = chrono::Utc::now().to_rfc3339();
    let started = std::time::Instant::now();
    let (result, events) = run_observed(request).await;
    let (code, final_text, error) = match result {
        Ok(result) => (result.code, result.text, result.error),
        Err(error) => (-1, String::new(), error.to_string()),
    };
    let final_text = final_text.replace(&token, "[REDACTED]");
    let error = error.replace(&token, "[REDACTED]");
    let tool_calls = tool.calls.load(std::sync::atomic::Ordering::SeqCst);
    let final_value = serde_json::from_str::<Value>(final_text.trim()).ok();
    let accepted = code == 0 && tool_calls == 1 && final_value == Some(json!({"left":"red","right":"blue"}));
    let receipts: Vec<Value> = events.iter().filter_map(|event| match event["type"].as_str() {
        Some("model_capabilities") => Some(json!({"type":"model_capabilities","source":event["source"],"model":event["model"],"host_input":event["host_input"]})),
        Some("model_call") => Some(json!({"type":"model_call","phase":event["phase"],"call":event["call"],"status":event["status"],"output_limit":event["output_limit"]})),
        Some("model_result") => Some(json!({"type":"model_result","phase":event["phase"],"call":event["call"],"stop_reason":event["stop_reason"],"usage":event["usage"],"usage_metrics":event["usage_metrics"],"elapsed_ms":event["elapsed_ms"]})),
        _ => None,
    }).collect();
    let evidence = json!({
        "kind":"bounded_live_vision_trial","started_at":started_at,"elapsed_ms":started.elapsed().as_millis(),
        "run_id":run_id,"model":model,"protocol":"openai-completions",
        "budget":{"max_calls":3,"max_seconds":60,"max_output":512},
        "fixture":{"file":"fixture.png","width":64,"height":32,"sha256":png_hash},
        "tool_calls":tool_calls,"code":code,"final_text":final_text,"error":error,
        "expected":{"left":"red","right":"blue"},"accepted":accepted,"receipts":receipts,
        "scope":"Core image tool-result interpretation only; no MCP, desktop, customer data, or hardware"
    });
    let bytes = serde_json::to_vec_pretty(&evidence).expect("serialize sanitized result");
    let sanitized = String::from_utf8(bytes).expect("JSON is UTF-8").replace(&token, "[REDACTED]");
    std::fs::write(output.join("result.json"), sanitized).expect("write sanitized result");
    assert!(accepted, "live vision validation failed; inspect the sanitized result.json");
}
