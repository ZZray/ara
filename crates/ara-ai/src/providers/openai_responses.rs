//! Stateless OpenAI-compatible Responses request encoding.
//!
//! Source: pinned OMP `packages/ai/src/providers/openai-responses.ts`
//! (`buildParams`, `convertTools`) and `openai-shared.ts`
//! (`buildResponsesInput`, `appendResponsesToolResultMessages`) at
//! 596f2da7101178214aa27a753529d15e6b7ad91d.
//!
//! This module is a request encoder only. Stream decoding, host selection and
//! native reasoning item replay are separate delivery steps.

use crate::error::ProviderError;
use crate::transform::{ToolCallOriginScope, responses_call_component, transform_messages};
use crate::types::{AssistantBlock, Context, Message, Model, ToolChoice, UserBlock, UserContent};
use serde_json::{Value, json};
use std::collections::{HashMap, HashSet, VecDeque};

pub const API: &str = "openai-responses";
const NON_VISION_IMAGE_PLACEHOLDER: &str = "[image omitted: model does not support vision]";

/// Request fields supported by the stateless Responses path.
#[derive(Clone, Debug, Default)]
pub struct RequestOptions {
    pub max_tokens: Option<u64>,
    pub temperature: Option<f64>,
    pub tool_choice: Option<ToolChoice>,
    /// Host-confirmed input capability; unknown defaults to text only.
    pub supports_images: bool,
}

fn content_parts(content: &UserContent, supports_images: bool) -> Vec<Value> {
    match content {
        UserContent::Text(text) if !text.trim().is_empty() => vec![json!({"type": "input_text", "text": text})],
        UserContent::Text(_) => Vec::new(),
        UserContent::Blocks(blocks) => {
            let mut parts = Vec::new();
            let mut omitted_images = false;
            for block in blocks {
                match block {
                    UserBlock::Text(text) if !text.text.trim().is_empty() => {
                        parts.push(json!({"type": "input_text", "text": text.text}));
                    }
                    UserBlock::Image(image) if supports_images => parts.push(json!({
                        "type": "input_image", "detail": "auto",
                        "image_url": format!("data:{};base64,{}", image.mime_type, image.data),
                    })),
                    UserBlock::Image(_) => omitted_images = true,
                    UserBlock::Text(_) => {}
                }
            }
            if omitted_images {
                parts.push(json!({"type": "input_text", "text": NON_VISION_IMAGE_PLACEHOLDER}));
            }
            parts
        }
    }
}

fn tool_output(blocks: &[UserBlock], supports_images: bool) -> Value {
    let has_images = blocks.iter().any(|block| matches!(block, UserBlock::Image(_)));
    if has_images && supports_images {
        Value::Array(
            blocks
                .iter()
                .map(|block| match block {
                    UserBlock::Text(text) => json!({"type": "input_text", "text": text.text}),
                    UserBlock::Image(image) => json!({
                        "type": "input_image",
                        "detail": "auto",
                        "image_url": format!("data:{};base64,{}", image.mime_type, image.data),
                    }),
                })
                .collect(),
        )
    } else {
        let mut text = blocks
            .iter()
            .filter_map(|block| match block {
                UserBlock::Text(text) => Some(text.text.as_str()),
                _ => None,
            })
            .collect::<Vec<_>>()
            .join("\n");
        if has_images {
            if !text.is_empty() {
                text.push('\n');
            }
            text.push_str(NON_VISION_IMAGE_PLACEHOLDER);
        }
        Value::String(text)
    }
}

fn call_id(id: &str, source_api: &str) -> String {
    let source = if matches!(source_api, "openai-responses" | "openai-codex-responses" | "azure-openai-responses") {
        responses_call_component(id)
    } else {
        id
    };
    source
        .chars()
        .map(|ch| if ch.is_ascii_alphanumeric() || matches!(ch, '_' | '-') { ch } else { '_' })
        .take(64)
        .collect()
}

fn unique_call_id(base: String, used: &mut HashSet<String>) -> String {
    if used.insert(base.clone()) {
        return base;
    }
    let mut suffix_number = 1usize;
    loop {
        let suffix = format!("_dup{suffix_number}");
        let prefix = base.chars().take(64 - suffix.len()).collect::<String>();
        let candidate = format!("{prefix}{suffix}");
        if used.insert(candidate.clone()) {
            return candidate;
        }
        suffix_number += 1;
    }
}

/// Build the outbound `/responses` body. No request or model state is retained.
pub fn build_request(model: &Model, context: &Context, options: &RequestOptions) -> Result<Value, ProviderError> {
    if model.api != API {
        return Err(ProviderError::Config(format!("Responses encoder requires {API} model API")));
    }
    if options.temperature.is_some_and(|value| !value.is_finite()) {
        return Err(ProviderError::Config("temperature must be finite".into()));
    }

    let mut input = Vec::new();
    let transformed = transform_messages(&context.messages);
    let scope = ToolCallOriginScope::collect(&transformed);
    let mut remapped: HashMap<String, VecDeque<String>> = HashMap::new();
    let mut used_call_ids = HashSet::new();
    for message in &transformed {
        match message {
            Message::User(user) => {
                let content = content_parts(&user.content, options.supports_images);
                if !content.is_empty() {
                    input.push(json!({"role": "user", "content": content}));
                }
            }
            Message::Developer(developer) => {
                let content = content_parts(&developer.content, options.supports_images);
                if !content.is_empty() {
                    input.push(json!({"role": "user", "content": content}));
                }
            }
            Message::Assistant(assistant) => {
                for block in &assistant.content {
                    match block {
                        AssistantBlock::Text(text) if !text.text.trim().is_empty() => input.push(json!({
                            "type": "message", "role": "assistant", "status": "completed",
                            "content": [{"type": "output_text", "text": text.text, "annotations": []}],
                        })),
                        AssistantBlock::ToolCall(call) => {
                            let base = call_id(&call.id, &assistant.api);
                            if base.is_empty() {
                                return Err(ProviderError::Config("tool call has no usable Responses call ID".into()));
                            }
                            let wire_id = unique_call_id(base, &mut used_call_ids);
                            remapped
                                .entry(scope.pairing_key(&call.id).to_owned())
                                .or_default()
                                .push_back(wire_id.clone());
                            input.push(json!({
                                "type": "function_call", "call_id": wire_id,
                                "name": call.name,
                                "arguments": serde_json::to_string(&call.arguments).map_err(|e| ProviderError::Config(e.to_string()))?,
                            }));
                        }
                        _ => {}
                    }
                }
            }
            Message::ToolResult(result) => {
                let wire_id = remapped
                    .get_mut(scope.pairing_key(&result.tool_call_id))
                    .and_then(VecDeque::pop_front)
                    .ok_or_else(|| {
                    ProviderError::Config("tool result has no matching Responses function call".into())
                })?;
                input.push(json!({
                    "type": "function_call_output", "call_id": wire_id,
                    "output": tool_output(&result.content, options.supports_images),
                }));
            }
        }
    }

    let mut body = json!({"model": model.id, "input": input, "stream": true, "store": false});
    let instructions =
        context.system_prompt.iter().filter(|prompt| !prompt.trim().is_empty()).cloned().collect::<Vec<_>>();
    if !instructions.is_empty() {
        body["instructions"] = json!(instructions.join("\n\n"));
    }
    if let Some(max) = options.max_tokens.or(model.max_tokens) {
        body["max_output_tokens"] = json!(max);
    }
    if let Some(temperature) = options.temperature {
        body["temperature"] = json!(temperature);
    }

    let tools = context.tools.as_ref().map(|tools| {
        tools
            .iter()
            .map(|tool| {
                json!({
                    "type": "function", "name": tool.name, "description": tool.description,
                    "parameters": tool.parameters,
                })
            })
            .collect::<Vec<_>>()
    });
    if let Some(tools) = tools.as_ref().filter(|tools| !tools.is_empty()) {
        body["tools"] = json!(tools);
    }
    if let Some(choice) = &options.tool_choice
        && tools.as_ref().is_some_and(|tools| !tools.is_empty())
    {
        let wire = match choice {
            ToolChoice::Auto => Some(json!("auto")),
            ToolChoice::None => Some(json!("none")),
            ToolChoice::Required => Some(json!("required")),
            ToolChoice::Tool(name)
                if context.tools.as_ref().is_some_and(|tools| tools.iter().any(|tool| tool.name == *name)) =>
            {
                Some(json!({"type": "function", "name": name}))
            }
            ToolChoice::Tool(_) => None,
        };
        if let Some(wire) = wire {
            body["tool_choice"] = wire;
        }
    }
    Ok(body)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::{AssistantMessage, Tool, ToolCall, ToolResultMessage, UserMessage, now_ms};

    fn model() -> Model {
        Model {
            id: "example-model".into(),
            api: API.into(),
            provider: "example".into(),
            base_url: "https://example.invalid/v1".into(),
            reasoning: false,
            max_tokens: None,
            tokenizer: None,
        }
    }

    fn call(id: &str) -> AssistantBlock {
        AssistantBlock::ToolCall(ToolCall {
            id: id.into(),
            name: "read".into(),
            arguments: serde_json::from_value(json!({"path": "a.txt"})).unwrap(),
            thought_signature: None,
        })
    }

    fn result(id: &str) -> Message {
        Message::ToolResult(ToolResultMessage {
            tool_call_id: id.into(),
            tool_name: "read".into(),
            content: vec![UserBlock::text("file contents")],
            details: None,
            is_error: false,
            timestamp: now_ms(),
        })
    }

    #[test]
    fn canonical_stateless_request_preserves_roles_and_function_schema() {
        let context = Context {
            system_prompt: vec!["one".into(), "two".into()],
            messages: vec![Message::User(UserMessage::text("hello"))],
            tools: Some(vec![Tool {
                name: "read".into(),
                description: "Read a file".into(),
                parameters: json!({"type": "object", "properties": {"path": {"type": "string"}}}),
            }]),
        };
        let body = build_request(
            &model(),
            &context,
            &RequestOptions {
                max_tokens: Some(100),
                temperature: Some(0.25),
                tool_choice: Some(ToolChoice::Tool("read".into())),
                supports_images: false,
            },
        )
        .unwrap();
        assert_eq!(body["store"], false);
        assert_eq!(body["stream"], true);
        assert_eq!(body["instructions"], "one\n\ntwo");
        assert_eq!(body["max_output_tokens"], 100);
        assert_eq!(body["temperature"], 0.25);
        assert_eq!(body["input"], json!([{"role": "user", "content": [{"type": "input_text", "text": "hello"}]}]));
        assert_eq!(body["tools"][0]["name"], "read");
        assert_eq!(body["tools"][0]["parameters"]["properties"]["path"]["type"], "string");
        assert_eq!(body["tool_choice"], json!({"type": "function", "name": "read"}));
    }

    #[test]
    fn responses_composite_call_and_result_replay_use_the_same_call_id() {
        let mut assistant = AssistantMessage::empty(API, "example", "example-model");
        assistant.content = vec![AssistantBlock::text("checking"), call("call_A|fc_X")];
        let context =
            Context { messages: vec![Message::Assistant(assistant), result("call_A|fc_X")], ..Context::default() };
        let input =
            build_request(&model(), &context, &RequestOptions::default()).unwrap()["input"].as_array().unwrap().clone();
        assert_eq!(input[0]["content"][0]["text"], "checking");
        assert_eq!(
            input[1],
            json!({"type": "function_call", "call_id": "call_A", "name": "read", "arguments": "{\"path\":\"a.txt\"}"})
        );
        assert_eq!(input[2], json!({"type": "function_call_output", "call_id": "call_A", "output": "file contents"}));
    }

    #[test]
    fn foreign_opaque_ids_and_colliding_wire_ids_keep_results_paired() {
        let mut assistant = AssistantMessage::empty("openai-completions", "other", "example-model");
        assistant.content = vec![call("a|b"), call("a_b")];
        let context = Context {
            messages: vec![Message::Assistant(assistant), result("a|b"), result("a_b")],
            ..Context::default()
        };
        let input =
            build_request(&model(), &context, &RequestOptions::default()).unwrap()["input"].as_array().unwrap().clone();
        assert_eq!(input[0]["call_id"], "a_b");
        assert_eq!(input[1]["call_id"], "a_b_dup1");
        assert_eq!(input[2]["call_id"], "a_b");
        assert_eq!(input[3]["call_id"], "a_b_dup1");
    }

    #[test]
    fn unavailable_tool_choice_is_omitted_like_pinned_omp() {
        for choice in [ToolChoice::Auto, ToolChoice::None, ToolChoice::Required, ToolChoice::Tool("missing".into())] {
            let body = build_request(
                &model(),
                &Context::default(),
                &RequestOptions { tool_choice: Some(choice), ..RequestOptions::default() },
            )
            .unwrap();
            assert!(body.get("tool_choice").is_none());
        }
    }

    #[test]
    fn stored_developer_turn_uses_generic_responses_user_role() {
        let context = Context {
            messages: vec![Message::Developer(crate::types::DeveloperMessage {
                content: UserContent::Text("developer note".into()),
                timestamp: now_ms(),
            })],
            ..Context::default()
        };
        let body = build_request(&model(), &context, &RequestOptions::default()).unwrap();
        assert_eq!(
            body["input"][0],
            json!({"role": "user", "content": [{"type": "input_text", "text": "developer note"}]})
        );
    }

    #[test]
    fn images_are_native_only_with_confirmed_model_capability() {
        let image = UserBlock::Image(crate::types::ImageContent { data: "AQI=".into(), mime_type: "image/png".into() });
        let context = Context {
            messages: vec![Message::User(UserMessage {
                content: UserContent::Blocks(vec![UserBlock::text("look"), image.clone()]),
                synthetic: None,
                timestamp: now_ms(),
            })],
            ..Context::default()
        };
        let text_only = build_request(&model(), &context, &RequestOptions::default()).unwrap();
        assert_eq!(
            text_only["input"][0]["content"][1],
            json!({"type": "input_text", "text": NON_VISION_IMAGE_PLACEHOLDER})
        );
        let vision =
            build_request(&model(), &context, &RequestOptions { supports_images: true, ..RequestOptions::default() })
                .unwrap();
        assert_eq!(
            vision["input"][0]["content"][1],
            json!({
                "type": "input_image", "detail": "auto", "image_url": "data:image/png;base64,AQI="
            })
        );
        let mut tool_result = result("call_A");
        let Message::ToolResult(ref mut value) = tool_result else { unreachable!() };
        value.content.push(image);
        let mut assistant = AssistantMessage::empty(API, "example", "example-model");
        assistant.content.push(call("call_A|fc_X"));
        let tool_context = Context { messages: vec![Message::Assistant(assistant), tool_result], ..Context::default() };
        let text_only = build_request(&model(), &tool_context, &RequestOptions::default()).unwrap();
        assert_eq!(text_only["input"][1]["output"], format!("file contents\n{NON_VISION_IMAGE_PLACEHOLDER}"));
        let vision = build_request(
            &model(),
            &tool_context,
            &RequestOptions { supports_images: true, ..RequestOptions::default() },
        )
        .unwrap();
        assert_eq!(
            vision["input"][1]["output"][1],
            json!({
                "type": "input_image", "detail": "auto", "image_url": "data:image/png;base64,AQI="
            })
        );
    }
}
