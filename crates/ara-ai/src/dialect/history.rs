//! Fixed OMP 596f2da, packages/ai/src/dialect/history.ts (MIT).
//! Copyright (c) 2025 Mario Zechner; Copyright (c) 2025-2026 Can Bölük;
//! Copyright (c) 2026 Stencil Labs, Inc. See LICENSE.

use super::types::{DialectRenderOptions, DialectRenderer, DialectToolResult};
use crate::{AssistantBlock, AssistantMessage, Message, ToolResultMessage, UserBlock, UserContent, UserMessage};

pub fn encode_inband_tool_history<D: DialectRenderer>(
    messages: &[Message],
    definition: &D,
    options: DialectRenderOptions<'_>,
) -> Result<Vec<Message>, D::Error> {
    let mut out = Vec::new();
    let mut index = 0;
    while let Some(message) = messages.get(index) {
        match message {
            Message::Assistant(message) => {
                out.push(Message::Assistant(encode_assistant_message(message, definition, options)?));
                index += 1;
            }
            Message::ToolResult(_) => {
                let mut run = Vec::new();
                while let Some(Message::ToolResult(result)) = messages.get(index) {
                    run.push(result);
                    index += 1;
                }
                out.push(encode_tool_results(&run, definition));
            }
            _ => {
                out.push(message.clone());
                index += 1;
            }
        }
    }
    Ok(out)
}

fn encode_assistant_message<D: DialectRenderer>(
    message: &AssistantMessage,
    definition: &D,
    options: DialectRenderOptions<'_>,
) -> Result<AssistantMessage, D::Error> {
    let calls = message.tool_calls().collect::<Vec<_>>();
    if calls.is_empty() {
        return Ok(message.clone());
    }
    let prose = message
        .content
        .iter()
        .filter_map(|block| match block {
            AssistantBlock::Text(block) => Some(block.text.as_str()),
            _ => None,
        })
        .collect::<Vec<_>>()
        .join("\n");
    let rendered = definition.render_assistant_tool_calls(&calls, options)?;
    let text = if ara_prompt::js::trim(&prose).is_empty() {
        rendered
    } else {
        format!("{}\n{rendered}", ara_prompt::js::trim_end(&prose))
    };
    let mut encoded = message.clone();
    encoded.content = vec![AssistantBlock::text(text)];
    Ok(encoded)
}

fn encode_tool_results<D: DialectRenderer>(results: &[&ToolResultMessage], definition: &D) -> Message {
    let mut dialect_results = Vec::new();
    let mut images = Vec::new();
    for (index, result) in results.iter().enumerate() {
        let mut text = String::new();
        for block in &result.content {
            match block {
                UserBlock::Text(block) => text.push_str(&block.text),
                UserBlock::Image(block) => images.push(UserBlock::Image(block.clone())),
            }
        }
        dialect_results.push(DialectToolResult {
            id: result.tool_call_id.clone(),
            name: result.tool_name.clone(),
            index,
            text,
            is_error: result.is_error,
        });
    }
    let mut content =
        vec![UserBlock::text(definition.render_tool_results(&dialect_results, DialectRenderOptions::default()))];
    content.extend(images);
    Message::User(UserMessage {
        content: UserContent::Blocks(content),
        synthetic: None,
        provider_payload: None,
        provider_payload_origin: None,
        timestamp: results.first().map_or_else(crate::now_ms, |result| result.timestamp),
    })
}
