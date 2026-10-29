//! Fixed OMP 596f2da, packages/ai/src/dialect/harmony.ts (MIT).
//! Copyright (c) 2025 Mario Zechner; Copyright (c) 2025-2026 Can Bölük;
//! Copyright (c) 2026 Stencil Labs, Inc. See LICENSE.
//!
//! Restricted scalar-string facade over the one UTF-16 scanner kernel.

use super::harmony_units::HarmonyInbandUnitsScanner;
use super::types::{InbandScanEvent, InbandScanner};
use super::units::checked_string_events;
use crate::relaxed_json::FinalValueFactory;
use ara_rpc::WireString;
use std::convert::Infallible;
use std::sync::Arc;

pub const START: &str = "<|start|>";
pub const END: &str = "<|end|>";
pub const MESSAGE: &str = "<|message|>";
pub const CHANNEL: &str = "<|channel|>";
pub const CONSTRAIN: &str = "<|constrain|>";
pub const RETURN: &str = "<|return|>";
pub const CALL: &str = "<|call|>";

pub struct HarmonyInbandScanner<F: FinalValueFactory> {
    inner: HarmonyInbandUnitsScanner<F>,
}
impl<F: FinalValueFactory> HarmonyInbandScanner<F> {
    pub fn new(factory: Arc<F>) -> Self {
        Self { inner: HarmonyInbandUnitsScanner::new(factory) }
    }
    pub fn with_id_source(factory: Arc<F>, id_source: Arc<dyn Fn() -> String + Send + Sync>) -> Self {
        Self {
            inner: HarmonyInbandUnitsScanner::with_id_source(factory, Arc::new(move || WireString::from(id_source()))),
        }
    }
}
impl<F: FinalValueFactory<Error = Infallible>> InbandScanner for HarmonyInbandScanner<F> {
    type Arguments = F::Value;
    fn feed(&mut self, text: &str) -> Vec<InbandScanEvent<F::Value>> {
        let events = match self.inner.try_feed_units(&WireString::from(text)) {
            Ok(events) => events,
            Err(error) => match error {},
        };
        checked_string_events(events)
    }
    fn flush(&mut self) -> Vec<InbandScanEvent<F::Value>> {
        let events = match self.inner.try_flush_units() {
            Ok(events) => events,
            Err(error) => match error {},
        };
        checked_string_events(events)
    }
}

/// Argument serialization is supplied by Root's unique actual carrier bridge.
/// Native's full rendering path propagates serialization errors; it does not
/// flatten cycles/BigInt/opaque values into a serde map.
pub struct HarmonyRenderer<R> {
    pub arguments: R,
}

impl<R: super::types::ToolArgumentJson> super::types::DialectRenderer for HarmonyRenderer<R> {
    type Error = R::Error;
    fn dialect(&self) -> super::types::Dialect {
        super::types::Dialect::Harmony
    }
    fn prompt(&self) -> &'static str {
        include_str!("harmony.md")
    }

    fn render_tool_call(
        &self,
        call: &crate::ToolCall,
        _options: super::types::DialectRenderOptions<'_>,
    ) -> Result<String, Self::Error> {
        let arguments = self.arguments.stringify_tool_arguments(call)?.unwrap_or_else(|| "null".into());
        Ok(format!(
            "{START}assistant{CHANNEL}commentary to={}{MESSAGE}{arguments}{CALL}",
            super::rendering::harmony_recipient(&call.name)
        ))
    }

    fn render_assistant_tool_calls(
        &self,
        calls: &[&crate::ToolCall],
        options: super::types::DialectRenderOptions<'_>,
    ) -> Result<String, Self::Error> {
        let mut out = String::new();
        for call in calls {
            out.push_str(&self.render_tool_call(call, options)?);
        }
        Ok(out)
    }

    fn render_tool_results(
        &self,
        results: &[super::types::DialectToolResult],
        _options: super::types::DialectRenderOptions<'_>,
    ) -> String {
        let mut out = String::new();
        for result in results {
            out.push_str(&format!(
                "{START}{} to=assistant{CHANNEL}commentary{MESSAGE}{}{END}",
                super::rendering::harmony_recipient(&result.name),
                result.text
            ));
        }
        out
    }

    fn render_thinking(&self, text: &str) -> String {
        if text.is_empty() { String::new() } else { format!("{START}assistant{CHANNEL}analysis{MESSAGE}{text}{END}") }
    }

    fn render_transcript(
        &self,
        messages: &[crate::Message],
        options: super::types::DialectRenderOptions<'_>,
    ) -> Result<String, Self::Error> {
        let mut out = String::new();
        let mut index = 0;
        while let Some(message) = messages.get(index) {
            match message {
                crate::Message::Assistant(message) => {
                    let parts = super::rendering::assistant_transcript_parts(message);
                    let mut emitted = false;
                    if !parts.thinking.is_empty() {
                        out.push_str(&self.render_thinking(&parts.thinking));
                        emitted = true;
                    }
                    if !parts.text.is_empty() {
                        out.push_str(&format!("{START}assistant{CHANNEL}final{MESSAGE}{}{END}", parts.text));
                        emitted = true;
                    }
                    if !parts.tool_calls.is_empty() {
                        out.push_str(&self.render_assistant_tool_calls(&parts.tool_calls, options)?);
                        emitted = true;
                    }
                    if !emitted {
                        out.push_str(&format!("{START}assistant{CHANNEL}final{MESSAGE}{END}"));
                    }
                    index += 1;
                }
                crate::Message::ToolResult(_) => {
                    let (results, next) = super::rendering::collect_tool_result_run(messages, index);
                    out.push_str(&self.render_tool_results(&results, options));
                    index = next;
                }
                crate::Message::User(message) => {
                    out.push_str(&format!(
                        "{START}user{MESSAGE}{}{END}",
                        super::rendering::message_content_text(&message.content)
                    ));
                    index += 1;
                }
                crate::Message::Developer(message) => {
                    out.push_str(&format!(
                        "{START}developer{MESSAGE}{}{END}",
                        super::rendering::message_content_text(&message.content)
                    ));
                    index += 1;
                }
            }
        }
        Ok(out)
    }
}
