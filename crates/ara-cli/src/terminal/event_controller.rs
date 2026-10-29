//! Fixed event-controller EDIT timeline, early previews and held completion.
//! Copyright (c) 2025 Mario Zechner; Copyright (c) 2025-2026 Can Bölük;
//! Copyright (c) 2026 Stencil Labs, Inc. MIT; see THIRD_PARTY_NOTICES.md.
//! Graph field reads are scoped to the admitted event callback. Retained draw
//! and spinner producers only submit metadata; the Session evaluator owns Gets.
use super::graph_renderer::DisplayGraph as HostGraph;
use super::{
    edit_renderer::is_edit_renderer,
    render_utils::sanitize,
    theme::Theme,
    tool_execution::ToolExecutionComponent,
    transcript::{BlockKey, Transcript},
    types::*,
};
use ara_agent::{AgentEvent, RawContextError, SessionValues};
use ara_ai::{AssistantBlock, UserBlock};
use ara_rpc::value_heap::ValueRuntime;
use ara_rpc::{
    WireString,
    value_heap::{ValueError, ValueHandle, ValueKind, ValueSnapshot},
};
use std::{collections::HashMap, sync::Arc};
pub struct EventController {
    pub generation: u64,
    native: bool,
    session_transcript: SessionTranscriptState,
    pub live: bool,
    pub tools: HashMap<WireString, ToolExecutionComponent>,
    pub transcript: Transcript,
    archived: Vec<(BlockKey, ToolExecutionComponent)>,
    early: HashMap<WireString, (Vec<PerFilePreview>, bool)>,
    held: HashMap<WireString, (WireString, EditResult, bool)>,
    held_graph: HashMap<WireString, (WireString, ValueHandle, Option<bool>, bool)>,
    display_errors: Vec<(BlockKey, ValueError)>,
    spinner: usize,
    presentation_now: std::time::Instant,
    theme: Arc<Theme>,
    mode: ara_edit::EditMode,
    presentation: super::render_utils::TerminalPresentationSettings,
    last_assistant_usage: Option<ara_ai::Usage>,
    cache_markers: HashMap<u64, super::cache_marker::CacheInvalidation>,
    assistant_first_tool: Option<BlockKey>,
    text: Option<u64>,
    next_text: u64,
    text_streamed: bool,
    text_source: String,
    native_markdown_theme: (u64, u64),
    native_markdown_options: super::markdown::MarkdownOptions,
}
impl EventController {
    pub fn new(theme: Arc<Theme>, mode: ara_edit::EditMode) -> Self {
        let native_markdown_theme = (theme.identity, theme.revision);
        let native_markdown_options = theme.native_markdown_options();
        Self {
            generation: 0,
            native: false,
            session_transcript: SessionTranscriptState::default(),
            live: false,
            tools: HashMap::new(),
            transcript: Transcript::default(),
            archived: Vec::new(),
            early: HashMap::new(),
            held: HashMap::new(),
            held_graph: HashMap::new(),
            display_errors: Vec::new(),
            spinner: 0,
            presentation_now: std::time::Instant::now(),
            theme,
            mode,
            presentation: Default::default(),
            last_assistant_usage: None,
            cache_markers: HashMap::new(),
            assistant_first_tool: None,
            text: None,
            next_text: 1,
            text_streamed: false,
            text_source: String::new(),
            native_markdown_theme,
            native_markdown_options,
        }
    }
    pub fn set_presentation(&mut self, settings: super::render_utils::TerminalPresentationSettings) {
        self.presentation = settings;
        self.transcript.begin_history_replay();
    }
    pub(crate) fn marker_rows(&self, key: &BlockKey, width: usize) -> Option<Vec<String>> {
        if let BlockKey::CacheMarker(id) = key {
            self.cache_markers.get(id).map(|m| super::cache_marker::render(m, width, &self.theme))
        } else {
            None
        }
    }
    fn observe_usage(&mut self, usage: ara_ai::Usage) {
        if let Some(marker) = super::cache_marker::observe(&mut self.last_assistant_usage, usage, &self.presentation) {
            let id = self.next_text;
            self.next_text = self.next_text.wrapping_add(1);
            self.cache_markers.insert(id, marker);
            self.transcript.add_cache_marker(id, self.assistant_first_tool.as_ref());
        }
        self.assistant_first_tool = None;
    }
    fn upsert_write_json(&mut self, id: WireString, value: &serde_json::Value) {
        if !self.accepts_call_args(&id) {
            return;
        }
        self.finish_text();
        let key = self.call_key(&id);
        let tool = self.tools.entry(id.clone()).or_insert_with(|| {
            ToolExecutionComponent::new(id, "write".into(), EditRenderArgs::default(), self.mode, self.theme.clone())
        });
        tool.write.as_mut().unwrap().update_args(super::write_renderer::args_from_json(value));
        tool.revision = tool.revision.wrapping_add(1);
        self.add_assistant_tool(key.clone(), false);
    }
    fn write_result_json(
        &mut self,
        id: WireString,
        output: &ara_agent::ToolOutput,
        error: Option<bool>,
        partial: bool,
    ) {
        // A start event normally precedes updates; admission of an early
        // completion still creates the one retained identity, never a replay.
        let key = self.call_key(&id);
        if !self.tools.contains_key(&id) {
            self.upsert_write_json(id.clone(), &serde_json::json!({}));
        }
        if let Some(tool) = self.tools.get_mut(&id) {
            if tool.sealed || tool.disposed {
                return;
            }
            let mut result = super::write_renderer::result_from_output(output);
            if let Some(error) = error {
                result.error = error;
            }
            let error = result.error;
            let repaint = tool.write.as_mut().unwrap().update_result(result, tool.component.options.expanded);
            tool.update_result(EditResult { is_error: Some(error), ..Default::default() }, partial);
            if repaint {
                self.transcript.begin_history_replay();
            }
            if !partial {
                self.transcript.settle(&key);
            }
        }
    }
    fn upsert_write_graph(
        &mut self,
        g: &HostGraph<'_>,
        id: WireString,
        root: ValueHandle,
        block: Option<ValueHandle>,
    ) -> Result<(), ValueError> {
        if !self.accepts_call_args(&id) {
            return Ok(());
        }
        g.0.check_handle(&root)?;
        // Preserve the effect receipt of this failed input through cumulative
        // updates/final completion. Only a new authoritative args root permits
        // another construction; raw growth is not permission to replay a getter.
        if self.tools.get(&id).is_some_and(|tool| {
            tool.constructor_failed && tool.graph_args_root.as_ref().is_some_and(|old| old.same_identity(&root))
        }) {
            return Ok(());
        }
        // WRITE cannot use EDIT's args-identity shortcut: its raw Symbol can
        // grow while provider arguments still have the previous parse identity.
        self.finish_text();
        let key = self.call_key(&id);
        let tool = self.tools.entry(id.clone()).or_insert_with(|| {
            ToolExecutionComponent::new(
                id.clone(),
                "write".into(),
                EditRenderArgs::default(),
                self.mode,
                self.theme.clone(),
            )
        });
        tool.graph_args_root = Some(root);
        tool.write.as_mut().unwrap().raw_block = block;
        tool.revision = tool.revision.wrapping_add(1);
        self.add_assistant_tool(key.clone(), false);
        if let Some(tool) = self.tools.get_mut(&id) {
            self.display_errors.extend(
                tool.rebuild_write_live_in(g, self.presentation_now, self.presentation.smooth_streaming)
                    .into_iter()
                    .map(|error| (key.clone(), error)),
            );
        }
        if let Some((name, result, error, partial)) = self.held_graph.remove(&id) {
            self.result_graph(g, id, name, result, error, partial)?;
        }
        Ok(())
    }
    pub fn begin_run(&mut self, mode: ara_edit::EditMode) -> u64 {
        self.seal_run();
        let previous = self.generation;
        self.archived.extend(self.tools.drain().map(|(id, component)| (BlockKey::Tool(previous, id), component)));
        self.generation = self.generation.wrapping_add(1);
        self.live = true;
        self.mode = mode;
        self.early.clear();
        self.held.clear();
        self.held_graph.clear();
        self.text = None;
        self.assistant_first_tool = None;
        self.generation
    }
    pub fn seal_run(&mut self) {
        for tool in self.tools.values_mut() {
            if !tool.sealed {
                tool.seal();
            }
        }
        let generation = self.generation;
        for id in self.tools.keys() {
            self.transcript.settle(&BlockKey::Tool(generation, id.clone()));
        }
        self.finish_text();
        self.live = false;
        self.early.clear();
        self.held.clear();
        self.held_graph.clear();
    }
    fn finish_text(&mut self) {
        if let Some(id) = self.text.take() {
            if let Some(SessionRow::Markdown(component)) = self.session_transcript.rows.get_mut(&BlockKey::Text(id)) {
                component.set_transient_render_cache(false);
            }
            self.transcript.settle(&BlockKey::Text(id));
        }
        self.text_source.clear();
    }
    fn append_text(&mut self, text: WireString) {
        let text = sanitize(&text);
        if text.is_empty() { return; }
        if self.native_markdown_theme != (self.theme.identity, self.theme.revision) { self.invalidate_session_rows(); }
        let id = if let Some(id) = self.text.filter(|id| self.transcript.contains(&BlockKey::Text(*id))) {
            id
        } else {
            let id = self.next_text;
            self.next_text = self.next_text.wrapping_add(1);
            self.text_source.clear();
            let mut component = host_markdown(String::new().into(), self.theme.clone(), self.native_markdown_options.clone());
            component.set_transient_render_cache(true);
            self.session_transcript.rows.insert(BlockKey::Text(id), SessionRow::Markdown(component));
            // Text key retains its message/receipt identity and trim_edges; only
            // renderer admission changes. Generic tool text still add_text.
            self.transcript.add_tool(BlockKey::Text(id), false);
            self.text = Some(id);
            id
        };
        self.text_source.push_str(&text);
        if let Some(SessionRow::Markdown(component)) = self.session_transcript.rows.get_mut(&BlockKey::Text(id)) {
            component.set_text(self.text_source.clone().into());
        }
        self.text_streamed = true;
    }
    fn set_stream_text(&mut self, text: String) {
        if text.is_empty() { return; }
        if self.native_markdown_theme != (self.theme.identity, self.theme.revision) { self.invalidate_session_rows(); }
        if let Some(id) = self.text.filter(|id| self.transcript.contains(&BlockKey::Text(*id))) {
            self.text_source = text;
            if let Some(SessionRow::Markdown(component)) = self.session_transcript.rows.get_mut(&BlockKey::Text(id)) {
                component.set_text(self.text_source.clone().into());
            }
        } else { self.append_text(text.into()); }
        self.text_streamed = true;
    }
    fn call_key(&self, id: &WireString) -> BlockKey {
        BlockKey::Tool(self.generation, id.clone())
    }
    fn add_assistant_tool(&mut self, key: BlockKey, settled: bool) {
        if self.assistant_first_tool.is_none() {
            self.assistant_first_tool = Some(key.clone());
        }
        self.transcript.add_tool(key, settled);
    }
    fn upsert(&mut self, id: WireString, name: WireString, args: EditRenderArgs) {
        if !is_edit_renderer(&name) {
            return;
        }
        self.finish_text();
        let key = self.call_key(&id);
        if let Some(component) = self.tools.get_mut(&id) {
            if !component.sealed
                && component.component.result.as_ref().is_none_or(|_| component.component.options.is_partial)
            {
                component.update_args(args);
            }
            return;
        }
        let mut component = ToolExecutionComponent::new(id.clone(), name, args, self.mode, self.theme.clone());
        if let Some((files, streaming)) = self.early.remove(&id) {
            component.update_stream_preview(files, streaming);
        }
        if let Some((_, result, partial)) = self.held.remove(&id) {
            component.update_result(result, partial);
        }
        self.add_assistant_tool(
            key,
            component.component.result.as_ref().is_some_and(|_| !component.component.options.is_partial),
        );
        self.tools.insert(id, component);
    }
    fn preview(&mut self, id: WireString, name: WireString, files: Vec<PerFilePreview>, streaming: bool) {
        if !is_edit_renderer(&name) {
            return;
        }
        if let Some(component) = self.tools.get_mut(&id) {
            component.update_stream_preview(files, streaming);
        } else {
            self.early.insert(id, (files, streaming));
        }
    }
    fn result(&mut self, id: WireString, name: WireString, result: EditResult, partial: bool) {
        if !is_edit_renderer(&name) {
            self.finish_text();
            let label = if result.is_error == Some(true) {
                "failed"
            } else if partial {
                "running"
            } else {
                "done"
            };
            let content = result.content.iter().map(sanitize).collect::<Vec<_>>().join("\n");
            let line = format!(
                "{}: {label}{}",
                sanitize(&name),
                if content.is_empty() { String::new() } else { format!("\n{content}") }
            );
            let text_id = self.next_text;
            self.next_text = self.next_text.wrapping_add(1);
            self.transcript.add_text(text_id, line, !partial);
            self.session_transcript.result_keys.entry(id).or_default().push(BlockKey::Text(text_id));
            return;
        }
        let key = self.call_key(&id);
        if let Some(component) = self.tools.get_mut(&id) {
            component.update_result(result, partial);
            if !partial {
                self.transcript.settle(&key);
            }
        } else {
            self.held.insert(id, (name, result, partial));
        }
    }
    pub(crate) fn sync_presentation(&mut self, spinner: usize, now: std::time::Instant) {
        self.presentation_now = now;
        self.spinner = spinner;
        for tool in self.tools.values_mut() {
            tool.sync_spinner(spinner)
        }
        for (_, tool) in &mut self.archived {
            tool.sync_spinner(spinner)
        }
    }
    pub fn toggle_expanded(&mut self) {
        self.toggle_expanded_in(None);
    }
    pub(crate) fn toggle_expanded_in(&mut self, g: Option<&HostGraph<'_>>) {
        self.toggle_summary_expanded();
        // Constructor effects follow transcript/component order, never map order.
        for key in self.transcript.tool_keys() {
            let tool = match &key {
                BlockKey::Tool(generation, id) if *generation == self.generation => self.tools.get_mut(id),
                _ => self.archived.iter_mut().find(|(k, _)| k == &key).map(|(_, t)| t),
            };
            if let Some(tool) = tool {
                if tool.disposed {
                    continue;
                }
                tool.toggle_expanded();
                if let Some(g) = g {
                    self.display_errors.extend(tool.rebuild_in(g).into_iter().map(|error| (key.clone(), error)));
                }
            }
        }
        self.transcript.begin_history_replay();
    }
    pub(crate) fn invalidate_in(&mut self, g: Option<&HostGraph<'_>>) {
        self.invalidate_session_rows();
        for key in self.transcript.tool_keys() {
            let tool = match &key {
                BlockKey::Tool(generation, id) if *generation == self.generation => self.tools.get_mut(id),
                _ => self.archived.iter_mut().find(|(k, _)| k == &key).map(|(_, t)| t),
            };
            if let Some(tool) = tool {
                self.display_errors.extend(tool.invalidate_in(g).into_iter().map(|error| (key.clone(), error)));
            }
        }
        self.transcript.begin_history_replay();
    }
    pub(crate) fn set_native(&mut self, active: bool) {
        self.native = active;
        for tool in self.tools.values_mut().chain(self.archived.iter_mut().map(|(_,tool)|tool)) { tool.set_native(active); }
    }
    pub(crate) fn set_tool_expanded(&mut self, generation:u64, id:&WireString, expanded:bool) {
        let key=BlockKey::Tool(generation,id.clone());
        let tool=if generation==self.generation {self.tools.get_mut(id)} else {self.archived.iter_mut().find(|(k,_)|k==&key).map(|(_,t)|t)};
        if let Some(tool)=tool {if !tool.disposed && tool.component.options.expanded!=expanded {tool.toggle_expanded();}}
    }
    fn rebuild_tool(&mut self, g: &HostGraph<'_>, id: &WireString) {
        let key = self.call_key(id);
        if let Some(tool) = self.tools.get_mut(id) {
            tool.set_native(self.native);
            self.display_errors.extend(tool.rebuild_in(g).into_iter().map(|error| (key.clone(), error)));
        }
    }
    pub(crate) fn reveal_deadline(&self) -> Option<std::time::Instant> {
        self.tools.values().filter_map(ToolExecutionComponent::reveal_deadline).min()
    }
    pub fn consume(
        &mut self,
        generation: u64,
        event: &AgentEvent,
        values: Option<&SessionValues>,
    ) -> Result<(), RawContextError> {
        if generation != self.generation || !self.live {
            return Ok(());
        }
        let result = self.consume_admitted(event, values);
        result.map_err(|e| RawContextError(format!("terminal display admission failed: {e}")))
    }
    fn consume_admitted(&mut self, event: &AgentEvent, values: Option<&SessionValues>) -> Result<(), ValueError> {
        self.consume_in(event, values, values.map(SessionValues::runtime))
    }
    pub(crate) fn consume_in(
        &mut self,
        event: &AgentEvent,
        values: Option<&SessionValues>,
        runtime: Option<&dyn ValueRuntime>,
    ) -> Result<(), ValueError> {
        match event {
            AgentEvent::ToolExecutionUpdate { tool_call_id, tool_name, partial } if tool_name == "write" => {
                self.write_result_json(tool_call_id.as_str().into(), partial, None, true);
                return Ok(());
            }
            AgentEvent::ToolExecutionEnd { tool_call_id, tool_name, result, is_error } if tool_name == "write" => {
                self.write_result_json(tool_call_id.as_str().into(), result, Some(*is_error), false);
                return Ok(());
            }
            _ => {}
        }
        match event {
            AgentEvent::MessageStart { .. } => {
                self.text_streamed = false;
                self.assistant_first_tool = None;
                self.finish_text();
            }
            AgentEvent::MessageUpdate { event, .. } => {
                let partial = event.partial();
                let text = partial
                    .content
                    .iter()
                    .filter_map(|block| match block {
                        AssistantBlock::Text(text) => Some(text.text.as_str()),
                        _ => None,
                    })
                    .collect::<Vec<_>>()
                    .join("");
                self.set_stream_text(text);
                for block in &partial.content {
                    if let AssistantBlock::ToolCall(call) = block {
                        if call.name == "write" {
                            self.upsert_write_json(
                                call.id.as_str().into(),
                                &serde_json::Value::Object(call.arguments.clone()),
                            );
                            continue;
                        }
                        self.upsert(
                            call.id.as_str().into(),
                            call.name.as_str().into(),
                            args_from_json(&serde_json::Value::Object(call.arguments.clone())),
                        );
                    }
                }
            }
            AgentEvent::ActualMessageUpdate { event, .. } => {
                let values = values.ok_or(ValueError::LegacyBridge("terminal requires current Session realm"))?;
                let graph = HostGraph(values, runtime.unwrap_or(values.runtime()));
                let content = graph.get(event.partial(), "content")?;
                let mut text = Vec::new();
                if values.heap().kind(&content)? == ValueKind::Array {
                    for block in graph.members(&content)? {
                        if graph.is(&block, "type", "text")? {
                            text.push(graph.string(&graph.get(&block, "text")?)?);
                        } else if graph.is(&block, "type", "toolCall")? {
                            let name = graph.string(&graph.get(&block, "name")?)?;
                            if name.equals_ascii("write") {
                                let id = graph.string(&graph.get(&block, "id")?)?;
                                self.upsert_write_graph(
                                    &graph,
                                    id,
                                    graph.get(&block, "arguments")?,
                                    Some(block.clone()),
                                )?;
                                continue;
                            }
                            if !is_edit_renderer(&name) {
                                continue;
                            }
                            let id = graph.string(&graph.get(&block, "id")?)?;
                            if !self.accepts_call_args(&id) {
                                continue;
                            }
                            self.upsert_graph(&graph, id, name, graph.get(&block, "arguments")?)?;
                        }
                    }
                }
                self.set_stream_text(text.iter().map(sanitize).collect::<Vec<_>>().join(""));
            }
            AgentEvent::MessageEnd { message } | AgentEvent::ActualMessageEnd { message, .. } => {
                let had_streamed = self.text_streamed;
                match message.body_snapshot() {
                    ara_agent::AgentMessageBody::Graph(root) => {
                        let values =
                            values.ok_or(ValueError::LegacyBridge("terminal requires current Session realm"))?;
                        let graph = HostGraph(values, runtime.unwrap_or(values.runtime()));
                        if graph.is(&root, "role", "assistant")? {
                            // Flush raw WRITE before final args drop reveal ownership.
                            let mut flushed = HashMap::new();
                            for (id, tool) in &mut self.tools {
                                let had_raw = tool.write.as_ref().is_some_and(|write| write.raw_block.is_some());
                                let previous = tool.graph_args_root.clone();
                                tool.finish_write_in(&graph)?;
                                if had_raw && !tool.constructor_failed {
                                    if let (Some(previous), Some(decoded)) = (previous, tool.graph_args_root.clone()) {
                                        flushed.insert(id.clone(), (previous, decoded));
                                    }
                                }
                            }
                            let content = graph.get(&root, "content")?;
                            if values.heap().kind(&content)? == ValueKind::Array {
                                for block in graph.members(&content)? {
                                    if graph.is(&block, "type", "toolCall")? {
                                        let name = graph.string(&graph.get(&block, "name")?)?;
                                        let is_write = name.equals_ascii("write");
                                        if is_edit_renderer(&name) || is_write {
                                            let id = graph.string(&graph.get(&block, "id")?)?;
                                            if self.accepts_call_args(&id) {
                                                let mut args = graph.get(&block, "arguments")?;
                                                if is_write {
                                                    if let Some((previous, decoded)) = flushed.get(&id) {
                                                        if args.same_identity(previous) {
                                                            args = decoded.clone();
                                                        }
                                                    }
                                                }
                                                self.upsert_graph(&graph, id.clone(), name, args)?;
                                            }
                                            if let Some(component) = self.tools.get_mut(&id) {
                                                component.set_args_complete();
                                            }
                                            // WRITE upsert already constructs once, or retains
                                            // its failed same-root receipt. EDIT keeps its lifecycle.
                                            if !is_write {
                                                self.rebuild_tool(&graph, &id);
                                            }
                                        }
                                    } else if !had_streamed && graph.is(&block, "type", "text")? {
                                        self.append_text(graph.string(&graph.get(&block, "text")?)?);
                                    }
                                }
                            }
                            self.observe_usage(usage_from_graph(&graph, &graph.get(&root, "usage")?)?);
                        }
                    }
                    _ => {
                        if let Some(ara_ai::Message::Assistant(message)) = message
                            .checked_model_snapshot()
                            .map_err(|_| ValueError::LegacyBridge("invalid legacy assistant display"))?
                        {
                            let usage = message.usage.clone();
                            for block in message.content {
                                match block {
                                    AssistantBlock::ToolCall(call) => {
                                        if call.name == "write" {
                                            let id: WireString = call.id.into();
                                            self.upsert_write_json(
                                                id.clone(),
                                                &serde_json::Value::Object(call.arguments),
                                            );
                                            if let Some(tool) = self.tools.get_mut(&id) {
                                                tool.set_args_complete();
                                            }
                                            continue;
                                        }
                                        let id: WireString = call.id.into();
                                        self.upsert(
                                            id.clone(),
                                            call.name.into(),
                                            args_from_json(&serde_json::Value::Object(call.arguments)),
                                        );
                                        if let Some(component) = self.tools.get_mut(&id) {
                                            component.set_args_complete();
                                        }
                                    }
                                    AssistantBlock::Text(text) if !had_streamed => self.append_text(text.text.into()),
                                    _ => {}
                                }
                            }
                            self.observe_usage(usage);
                        }
                    }
                }
                self.finish_text();
            }
            AgentEvent::ToolExecutionStart { tool_call_id, tool_name, args, .. } => {
                let id: WireString = tool_call_id.as_str().into();
                if tool_name == "write" {
                    self.upsert_write_json(id.clone(), &serde_json::Value::Object(args.clone()));
                    if let Some(tool) = self.tools.get_mut(&id) {
                        tool.set_execution_started_at(self.presentation_now);
                    }
                    return Ok(());
                }
                self.upsert(
                    id.clone(),
                    tool_name.as_str().into(),
                    args_from_json(&serde_json::Value::Object(args.clone())),
                );
                if let Some(component) = self.tools.get_mut(&id) {
                    component.set_execution_started_at(self.presentation_now);
                }
            }
            AgentEvent::ActualToolExecutionStart { tool_call_id, tool_name, args, .. } => {
                if (!is_edit_renderer(tool_name) && !tool_name.equals_ascii("write"))
                    || !self.accepts_call_args(tool_call_id)
                {
                    return Ok(());
                }
                let values = values.ok_or(ValueError::LegacyBridge("terminal requires current Session realm"))?;
                let graph = HostGraph(values, runtime.unwrap_or(values.runtime()));
                self.upsert_graph(&graph, tool_call_id.clone(), tool_name.clone(), args.clone())?;
                if let Some(component) = self.tools.get_mut(tool_call_id) {
                    component.set_execution_started_at(self.presentation_now);
                }
                if !tool_name.equals_ascii("write") {
                    self.rebuild_tool(&graph, tool_call_id);
                }
            }
            AgentEvent::ToolStreamUpdate { tool_call_id, tool_name, update } => {
                if let Some(files) = previews_from_json(update) {
                    self.preview(
                        tool_call_id.as_str().into(),
                        tool_name.as_str().into(),
                        files,
                        update.get("streaming") != Some(&serde_json::Value::Bool(false)),
                    );
                    if let Some(values) = values {
                        self.rebuild_tool(
                            &HostGraph(values, runtime.unwrap_or(values.runtime())),
                            &WireString::from(tool_call_id.as_str()),
                        );
                    }
                }
            }
            AgentEvent::ToolExecutionUpdate { tool_call_id, tool_name, partial } => self.result(
                tool_call_id.as_str().into(),
                tool_name.as_str().into(),
                result_from_tool_output(partial),
                true,
            ),
            AgentEvent::ToolExecutionEnd { tool_call_id, tool_name, result, is_error } => {
                let mut result = result_from_tool_output(result);
                result.is_error = Some(*is_error);
                self.result(tool_call_id.as_str().into(), tool_name.as_str().into(), result, false);
            }
            AgentEvent::ActualToolExecutionUpdate { tool_call_id, tool_name, partial, .. } => {
                let values = values.ok_or(ValueError::LegacyBridge("terminal requires current Session realm"))?;
                self.result_graph(
                    &HostGraph(values, runtime.unwrap_or(values.runtime())),
                    tool_call_id.clone(),
                    tool_name.clone(),
                    partial.clone(),
                    None,
                    true,
                )?;
            }
            AgentEvent::ActualToolExecutionEnd { tool_call_id, tool_name, result, is_error } => {
                let values = values.ok_or(ValueError::LegacyBridge("terminal requires current Session realm"))?;
                self.result_graph(
                    &HostGraph(values, runtime.unwrap_or(values.runtime())),
                    tool_call_id.clone(),
                    tool_name.clone(),
                    result.clone(),
                    Some(*is_error),
                    false,
                )?;
            }
            AgentEvent::AgentEnd { .. } | AgentEvent::ActualAgentEnd { .. } => self.seal_run(),
            _ => {}
        }
        Ok(())
    }
    fn upsert_graph(
        &mut self,
        g: &HostGraph<'_>,
        id: WireString,
        name: WireString,
        root: ValueHandle,
    ) -> Result<(), ValueError> {
        if name.equals_ascii("write") {
            return self.upsert_write_graph(g, id, root, None);
        }
        if !self.accepts_call_args(&id) {
            return Ok(());
        }
        g.0.check_handle(&root)?;
        if self.tools.get(&id).and_then(|t| t.graph_args_root.as_ref()).is_some_and(|old| old.same_identity(&root)) {
            return Ok(());
        }
        self.finish_text();
        let key = self.call_key(&id);
        if let Some(tool) = self.tools.get_mut(&id) {
            tool.update_args(EditRenderArgs::default());
            tool.graph_args_root = Some(root);
        } else {
            let mut tool =
                ToolExecutionComponent::new(id.clone(), name, EditRenderArgs::default(), self.mode, self.theme.clone());
            tool.graph_args_root = Some(root);
            tool.sync_spinner(self.spinner);
            self.add_assistant_tool(key, false);
            self.tools.insert(id.clone(), tool);
        }
        self.rebuild_tool(g, &id);
        // Initial renderCall construction precedes the early preview's real
        // updateDisplay and any completion held before the call was admitted.
        if let Some((files, streaming)) = self.early.remove(&id) {
            if let Some(tool) = self.tools.get_mut(&id) {
                tool.update_stream_preview(files, streaming);
            }
            self.rebuild_tool(g, &id);
        }
        if let Some((name, result, error, partial)) = self.held_graph.remove(&id) {
            self.result_graph(g, id.clone(), name, result, error, partial)?;
        }
        if let Some((_, result, partial)) = self.held.remove(&id) {
            if let Some(tool) = self.tools.get_mut(&id) {
                tool.update_result(result, partial);
                tool.graph = None;
            }
            if !partial {
                self.transcript.settle(&self.call_key(&id));
            }
        }
        Ok(())
    }
    fn result_graph(
        &mut self,
        g: &HostGraph<'_>,
        id: WireString,
        name: WireString,
        root: ValueHandle,
        error: Option<bool>,
        partial: bool,
    ) -> Result<(), ValueError> {
        if name.equals_ascii("write") {
            g.0.check_handle(&root)?;
            let Some(tool) = self.tools.get_mut(&id) else {
                self.held_graph.insert(id, (name, root, error, partial));
                return Ok(());
            };
            if tool.sealed || tool.disposed {
                return Ok(());
            }
            let failed = error.unwrap_or(false);
            let repaint = tool.write.as_ref().unwrap().needs_first_result_repaint(tool.component.options.expanded);
            tool.result_root = Some(root);
            tool.update_result(EditResult { is_error: Some(failed), ..Default::default() }, partial);
            self.rebuild_tool(g, &id);
            if repaint {
                self.transcript.begin_history_replay();
            }
            if !partial {
                self.transcript.settle(&self.call_key(&id));
            }
            return Ok(());
        }
        if !is_edit_renderer(&name) {
            let result = result_from_graph(g, &root, &name, &EditRenderArgs::default(), error)?;
            self.result(id, name, result, partial);
            return Ok(());
        }
        g.0.check_handle(&root)?;
        let Some(tool) = self.tools.get(&id) else {
            self.held_graph.insert(id, (name, root, error, partial));
            return Ok(());
        };
        if tool.sealed || tool.disposed {
            return Ok(());
        }
        let error = match error {
            Some(error) => error,
            None => {
                let details = g.get(&root, "details")?;
                if g.nullish(&details)? {
                    false
                } else {
                    let a = g.get(&details, "async")?;
                    !g.nullish(&a)? && g.is(&a, "state", "failed")?
                }
            }
        };
        if let Some(tool) = self.tools.get_mut(&id) {
            tool.result_root = Some(root);
            tool.update_result(EditResult { is_error: Some(error), ..Default::default() }, partial);
        }
        self.rebuild_tool(g, &id);
        if !partial {
            self.transcript.settle(&self.call_key(&id));
        }
        Ok(())
    }
    pub(crate) fn tool_at(&self, key: &BlockKey) -> Option<&ToolExecutionComponent> {
        match key {
            BlockKey::Tool(g, id) if *g == self.generation => self.tools.get(id),
            _ => self.archived.iter().find(|(k, _)| k == key).map(|(_, t)| t),
        }
    }
    pub(crate) fn commit_tool(&mut self, key: &BlockKey, tool: ToolExecutionComponent) {
        if let BlockKey::Tool(g, id) = key {
            if *g == self.generation {
                self.tools.insert(id.clone(), tool);
                return;
            }
        }
        if let Some((_, old)) = self.archived.iter_mut().find(|(k, _)| k == key) {
            *old = tool;
        }
    }
    pub(crate) fn take_display_errors(&mut self) -> Vec<(BlockKey, ValueError)> {
        std::mem::take(&mut self.display_errors)
    }
    pub(crate) fn theme(&self) -> Arc<Theme> {
        self.theme.clone()
    }
    fn accepts_call_args(&self, id: &WireString) -> bool {
        self.tools.get(id).is_none_or(|component| {
            !component.sealed
                && !component.disposed
                && (component.component.result.is_none() || component.component.options.is_partial)
        })
    }
    fn graph_args(
        &self,
        graph: &HostGraph<'_>,
        id: &WireString,
        name: &WireString,
        root: &ValueHandle,
    ) -> Result<EditRenderArgs, ValueError> {
        let mode = if name.equals_ascii("apply_patch") { ara_edit::EditMode::ApplyPatch } else { self.mode };
        let previews = self.early.get(id).map(|(files, _)| files.as_slice());
        let existing = self.tools.get(id).map(|component| &component.component.context);
        let has_preview_diff = previews
            .is_some_and(|files| files.iter().any(|file| file.diff.as_ref().is_some_and(|diff| !diff.is_empty())))
            || existing.is_some_and(|context| {
                matches!(&context.edit_diff_preview, Some(EditPreview::Diff { diff, .. }) if !diff.is_empty())
                    || context.per_file_diff_preview.as_ref().is_some_and(|files| {
                        files.iter().any(|file| file.diff.as_ref().is_some_and(|diff| !diff.is_empty()))
                    })
            });
        let has_multi_preview = previews
            .or_else(|| existing.and_then(|context| context.per_file_diff_preview.as_deref()))
            .is_some_and(|files| {
                files.len() > 1
                    && files.iter().any(|file| {
                        file.diff.as_ref().is_some_and(|diff| !diff.is_empty())
                            || file.error.as_ref().is_some_and(|error| !error.is_empty())
                    })
            });
        args_from_graph(graph, root, mode, has_preview_diff, has_multi_preview)
    }
    fn graph_result(
        &self,
        graph: &HostGraph<'_>,
        id: &WireString,
        name: &WireString,
        root: &ValueHandle,
        authoritative_error: Option<bool>,
    ) -> Result<Option<EditResult>, ValueError> {
        let component = self.tools.get(id);
        if is_edit_renderer(name) && component.is_some_and(|component| component.sealed || component.disposed) {
            return Ok(None);
        }
        let args = component.map(|component| component.component.args.clone()).unwrap_or_default();
        result_from_graph(graph, root, name, &args, authoritative_error).map(Some)
    }
    pub fn render_blocks(&mut self, frame: &FrameContext, spinner: usize) {
        for (id, marker) in &self.cache_markers {
            self.transcript.update_rows(
                &BlockKey::CacheMarker(*id),
                super::cache_marker::render(marker, frame.columns, &self.theme),
            );
        }
        for (key, tool) in &mut self.archived {
            self.transcript.update_rows(key, tool.render(frame.columns, frame, spinner));
        }
        let generation = self.generation;
        for (id, tool) in &mut self.tools {
            let key = BlockKey::Tool(generation, id.clone());
            if self.transcript.contains(&key) {
                self.transcript.update_rows(&key, tool.render(frame.columns, frame, spinner));
                if tool.component.result.as_ref().is_some_and(|_| !tool.component.options.is_partial) || tool.sealed {
                    self.transcript.settle(&key);
                }
            }
        }
    }
}
pub fn args_from_json(value: &serde_json::Value) -> EditRenderArgs {
    let text = |key| value.get(key).and_then(serde_json::Value::as_str).map(WireString::from);
    EditRenderArgs {
        path: text("path"),
        file_path: text("file_path"),
        old_text: text("oldText"),
        new_text: text("newText"),
        new_string: text("new_string"),
        patch: text("patch"),
        input: text("input"),
        alternate_input: text("_input"),
        replace_all: value.get("replace_all").and_then(serde_json::Value::as_bool),
        op: value.get("op").and_then(serde_json::Value::as_str).and_then(Operation::parse),
        rename: text("rename"),
        diff: text("diff"),
        preview_diff: text("previewDiff"),
        partial_json: text("__partialJson"),
        edits: value.get("edits").and_then(serde_json::Value::as_array).map(|entries| {
            entries
                .iter()
                .map(|entry| {
                    let text = |key| entry.get(key).and_then(serde_json::Value::as_str).map(WireString::from);
                    EditEntry {
                        path: text("path"),
                        rename: text("rename"),
                        move_to: text("move"),
                        op: entry.get("op").and_then(serde_json::Value::as_str).and_then(Operation::parse),
                    }
                })
                .collect()
        }),
    }
}
fn details_from_json(value: &serde_json::Value) -> EditDetails {
    let text = |key| value.get(key).and_then(serde_json::Value::as_str).map(WireString::from);
    EditDetails {
        diff: WireText(text("diff").unwrap_or_else(|| "".into())),
        first_changed_line: value.get("firstChangedLine").and_then(serde_json::Value::as_u64).map(|n| n as u32),
        diagnostics: value.get("diagnostics").filter(|v| v.is_object()).map(|d| Diagnostics {
            errored: d.get("errored").and_then(serde_json::Value::as_bool).unwrap_or(false),
            summary: WireText(d.get("summary").and_then(serde_json::Value::as_str).unwrap_or("").into()),
            messages: d
                .get("messages")
                .and_then(serde_json::Value::as_array)
                .into_iter()
                .flatten()
                .filter_map(serde_json::Value::as_str)
                .map(WireString::from)
                .collect(),
        }),
        op: value.get("op").and_then(serde_json::Value::as_str).and_then(Operation::parse),
        move_to: text("move"),
        per_file_results: value
            .get("perFileResults")
            .and_then(serde_json::Value::as_array)
            .map(|rows| rows.iter().map(details_from_json).collect()),
        path: text("path"),
        old_text: text("oldText"),
        new_text: text("newText"),
        snapshots_pruned: value.get("snapshotsPruned").and_then(serde_json::Value::as_bool),
        source_path: text("sourcePath"),
        is_error: value.get("isError").and_then(serde_json::Value::as_bool),
        error_text: text("errorText"),
        display_error_text: text("displayErrorText"),
    }
}
fn result_from_tool_output(value: &ara_agent::ToolOutput) -> EditResult {
    EditResult {
        content: value
            .content
            .iter()
            .filter_map(|block| match block {
                UserBlock::Text(text) => Some(text.text.as_str().into()),
                _ => None,
            })
            .collect(),
        details: value.details.as_ref().map(details_from_json),
        is_error: Some(value.is_error),
    }
}
pub fn previews_from_json(value: &serde_json::Value) -> Option<Vec<PerFilePreview>> {
    Some(
        value
            .get("files")?
            .as_array()?
            .iter()
            .filter_map(|file| {
                let path = file.get("path")?.as_str()?;
                Some(PerFilePreview {
                    path: WireText(path.into()),
                    diff: file.get("diff").and_then(serde_json::Value::as_str).map(WireString::from),
                    first_changed_line: file
                        .get("firstChangedLine")
                        .and_then(serde_json::Value::as_u64)
                        .map(|n| n as u32),
                    error: file.get("error").and_then(serde_json::Value::as_str).map(WireString::from),
                })
            })
            .collect(),
    )
}
fn optional_string(values: &SessionValues, value: &ValueHandle) -> Result<Option<WireString>, ValueError> {
    values.check_handle(value)?;
    Ok(match values.heap().snapshot(value)? {
        ValueSnapshot::String(text) => Some(text),
        _ => None,
    })
}
fn graph_text(graph: &HostGraph<'_>, root: &ValueHandle, key: &str) -> Result<Option<WireString>, ValueError> {
    optional_string(graph.0, &graph.get(root, key)?)
}
fn graph_bool(graph: &HostGraph<'_>, root: &ValueHandle, key: &str) -> Result<Option<bool>, ValueError> {
    Ok(match graph.0.heap().snapshot(&graph.get(root, key)?)? {
        ValueSnapshot::Boolean(value) => Some(value),
        _ => None,
    })
}
fn graph_number(graph: &HostGraph<'_>, root: &ValueHandle, key: &str) -> Result<Option<u32>, ValueError> {
    Ok(match graph.0.heap().snapshot(&graph.get(root, key)?)? {
        ValueSnapshot::Number(number) if number.is_finite() && number >= 0.0 => Some(number as u32),
        _ => None,
    })
}
fn graph_present_text(graph: &HostGraph<'_>, root: &ValueHandle, key: &str) -> Result<Option<WireString>, ValueError> {
    if graph.0.heap().has(root, &key.into(), graph.runtime())? { graph_text(graph, root, key) } else { Ok(None) }
}
fn operation_value(graph: &HostGraph<'_>, value: &ValueHandle) -> Result<Option<Operation>, ValueError> {
    if !graph.0.heap().truthy(value)? {
        return Ok(None);
    }
    // Unknown truthy operations still select this branch in JavaScript. Their
    // display title/default behavior is Edit; they must not read a fallback op.
    Ok(Some(
        optional_string(graph.0, value)?
            .and_then(|text| text.to_utf8().ok())
            .as_deref()
            .and_then(Operation::parse)
            .unwrap_or(Operation::Update),
    ))
}
fn selected_input(graph: &HostGraph<'_>, root: &ValueHandle, args: &mut EditRenderArgs) -> Result<(), ValueError> {
    let input = graph.get(root, "input")?;
    if graph.nullish(&input)? {
        args.alternate_input = graph_text(graph, root, "_input")?;
    } else {
        args.input = optional_string(graph.0, &input)?;
    }
    Ok(())
}
pub(crate) fn args_from_graph(
    graph: &HostGraph<'_>,
    root: &ValueHandle,
    mode: ara_edit::EditMode,
    has_preview_diff: bool,
    has_multi_preview: bool,
) -> Result<EditRenderArgs, ValueError> {
    graph.0.check_handle(root)?;
    let mut args = EditRenderArgs::default();
    if graph.nullish(root)? {
        return Ok(args);
    }
    // ToolExecutionComponent builds this context before invoking renderCall.
    // Fixed renderer.ts:207-216 reads only replace-mode fallback fields.
    if mode == ara_edit::EditMode::Replace && !has_preview_diff {
        args.new_string = graph_text(graph, root, "new_string")?;
        if args.new_string.is_none() {
            args.new_text = graph_text(graph, root, "newText")?;
        }
    }
    // resolveEditCallFacts: input helpers, first edit, path, rename, op, count.
    // input ?? _input is evaluated by the fixed helpers before their mode gate.
    selected_input(graph, root, &mut args)?;
    let edits_value = graph.get(root, "edits")?;
    let edits = if graph.0.heap().kind(&edits_value)? == ValueKind::Array { Some(edits_value) } else { None };
    let edit_count =
        edits.as_ref().map(|edits| graph.0.heap().array_length(edits, graph.runtime())).transpose()?.unwrap_or(0);
    let first = if edit_count > 0 { Some(graph.get(edits.as_ref().unwrap(), "0")?) } else { None };
    let mut first_observation = EditEntry::default();
    let mut first_path_read = false;
    args.file_path = graph_text(graph, root, "file_path")?;
    if args.file_path.is_none() {
        args.path = graph_text(graph, root, "path")?;
        if args.path.is_none() {
            if let Some(first) = &first {
                first_observation.path = graph_text(graph, first, "path")?;
                first_path_read = true;
            }
            if first_observation.path.is_none() {
                args.partial_json = graph_text(graph, root, "__partialJson")?;
            }
        }
    }
    args.rename = graph_text(graph, root, "rename")?;
    if args.rename.is_none() {
        if let Some(first) = &first {
            first_observation.rename = graph_text(graph, first, "rename")?;
            if first_observation.rename.is_none() {
                first_observation.move_to = graph_text(graph, first, "move")?;
            }
        }
    }
    let op = graph.get(root, "op")?;
    args.op = operation_value(graph, &op)?;
    if !graph.0.heap().truthy(&op)? {
        if let Some(first) = &first {
            first_observation.op = operation_value(graph, &graph.get(first, "op")?)?;
        }
    }
    if let Some(edits) = &edits {
        let mut observations = Vec::with_capacity(edit_count as usize);
        for index in 0..edit_count {
            if index == 0 {
                if !first_path_read {
                    first_observation.path = graph_text(graph, first.as_ref().unwrap(), "path")?;
                }
                observations.push(first_observation.clone());
            } else {
                // countEditFiles reads only each remaining entry's path.
                let entry = graph.get(edits, &index.to_string())?;
                observations.push(EditEntry { path: graph_text(graph, &entry, "path")?, ..Default::default() });
            }
        }
        args.edits = Some(observations);
    }
    // hasEditCallPayload/getCallPreview share the same short-circuit priority.
    // A multi-file preview consumes no argument preview fields.
    if !has_multi_preview {
        args.preview_diff = graph_text(graph, root, "previewDiff")?;
        if args.preview_diff.as_ref().is_none_or(WireString::is_empty) {
            args.diff = graph_text(graph, root, "diff")?;
            if args.diff.as_ref().is_none_or(WireString::is_empty) {
                if args.new_text.is_none() {
                    args.new_text = graph_text(graph, root, "newText")?;
                }
                if args.new_text.as_ref().is_none_or(WireString::is_empty) {
                    args.patch = graph_text(graph, root, "patch")?;
                }
            }
        }
    }
    Ok(args)
}
pub(crate) fn diagnostics_from_graph(graph: &HostGraph<'_>, root: &ValueHandle) -> Result<Diagnostics, ValueError> {
    let messages_value = graph.get(root, "messages")?;
    let length = graph.0.heap().array_length(&messages_value, graph.runtime())?;
    let mut messages = Vec::with_capacity(length as usize);
    for index in 0..length {
        if let Some(text) = optional_string(graph.0, &graph.get(&messages_value, &index.to_string())?)? {
            messages.push(text);
        }
    }
    // formatDiagnostics returns before errored/summary for an empty list.
    if length == 0 {
        return Ok(Diagnostics::default());
    }
    Ok(Diagnostics {
        messages,
        errored: graph_bool(graph, root, "errored")?.unwrap_or(false),
        summary: WireText(graph_text(graph, root, "summary")?.unwrap_or_else(|| "".into())),
    })
}
fn single_details_from_graph(
    graph: &HostGraph<'_>,
    root: &ValueHandle,
    args: &EditRenderArgs,
    authoritative_error: Option<bool>,
    content_root: Option<&ValueHandle>,
) -> Result<(EditDetails, Vec<WireString>), ValueError> {
    let is_error = match authoritative_error {
        Some(value) => value,
        None if graph.0.heap().has(root, &"isError".into(), graph.runtime())? => {
            graph_bool(graph, root, "isError")?.unwrap_or(false)
        }
        None => false,
    };
    let mut details = EditDetails { is_error: Some(is_error), ..Default::default() };
    details.source_path = graph_present_text(graph, root, "sourcePath")?;
    details.path = graph_present_text(graph, root, "path")?;
    let first = args.edits.as_ref().and_then(|edits| edits.first());
    if args.op.or_else(|| first.and_then(|edit| edit.op)).is_none() {
        details.op = operation_value(graph, &graph.get(root, "op")?)?;
    }
    if args.rename.as_ref().or_else(|| first.and_then(|edit| edit.rename.as_ref().or(edit.move_to.as_ref()))).is_none()
    {
        details.move_to = graph_present_text(graph, root, "move")?;
    }
    if is_error {
        details.display_error_text = graph_present_text(graph, root, "displayErrorText")?;
        if details.display_error_text.as_ref().is_none_or(WireString::is_empty) {
            details.error_text = graph_present_text(graph, root, "errorText")?;
        }
    } else {
        details.diff = WireText(graph_text(graph, root, "diff")?.unwrap_or_else(|| "".into()));
    }
    let content = if is_error
        && details.display_error_text.as_ref().is_none_or(WireString::is_empty)
        && details.error_text.as_ref().is_none_or(WireString::is_empty)
    {
        content_root.map(|result| content_from_graph(graph, result, true)).transpose()?.unwrap_or_default()
    } else {
        Vec::new()
    };
    let op = args.op.or_else(|| first.and_then(|edit| edit.op)).or(details.op);
    let rename = args
        .rename
        .as_ref()
        .or_else(|| first.and_then(|edit| edit.rename.as_ref().or(edit.move_to.as_ref())))
        .or(details.move_to.as_ref());
    let mut preliminary_diag = None;
    if !is_error && details.diff.0.is_empty() {
        let diag = graph.get(root, "diagnostics")?;
        if !graph.0.heap().truthy(&diag)? && (op == Some(Operation::Delete) || rename.is_some()) {
            return Ok((details, content));
        }
        preliminary_diag = Some(diag);
    }
    if !is_error {
        details.first_changed_line = graph_number(graph, root, "firstChangedLine")?;
    }
    let diag = match preliminary_diag {
        Some(diag) => diag,
        None => graph.get(root, "diagnostics")?,
    };
    if graph.0.heap().truthy(&diag)? {
        details.diagnostics = Some(diagnostics_from_graph(graph, &diag)?);
    }
    Ok((details, content))
}
pub(crate) fn content_from_graph(
    graph: &HostGraph<'_>,
    root: &ValueHandle,
    first_only: bool,
) -> Result<Vec<WireString>, ValueError> {
    let content = graph.get(root, "content")?;
    if graph.nullish(&content)? {
        return Ok(Vec::new());
    }
    let length = graph.0.heap().array_length(&content, graph.runtime())?;
    let mut out = Vec::new();
    for index in 0..length {
        let block = graph.get(&content, &index.to_string())?;
        if graph.is(&block, "type", "text")? {
            if let Some(text) = graph_text(graph, &block, "text")? {
                out.push(text);
            }
            if first_only {
                break;
            }
        }
    }
    Ok(out)
}
pub(crate) fn result_from_graph(
    graph: &HostGraph<'_>,
    root: &ValueHandle,
    name: &WireString,
    args: &EditRenderArgs,
    authoritative_error: Option<bool>,
) -> Result<EditResult, ValueError> {
    result_from_graph_with_count(graph,root,name,args,authoritative_error,None)
}
pub(crate) fn result_from_graph_with_count(
    graph:&HostGraph<'_>,root:&ValueHandle,name:&WireString,args:&EditRenderArgs,authoritative_error:Option<bool>,file_count:Option<usize>
) -> Result<EditResult, ValueError> {
    graph.0.check_handle(root)?;
    // Generic summary has no EDIT-details consumer. Do not fetch details merely
    // to complete a DTO, and never re-read an End root.isError getter.
    if !is_edit_renderer(name) {
        return Ok(EditResult {
            content: content_from_graph(graph, root, false)?,
            is_error: Some(authoritative_error.unwrap_or(false)),
            ..Default::default()
        });
    }
    let details_root = graph.get(root, "details")?;
    let has_details = !graph.nullish(&details_root)?;
    let is_error = if let Some(error) = authoritative_error {
        error
    } else if has_details {
        // Fixed event-controller UPDATE consumes async.state and supplies the
        // renderer's isError; the partial's own root.isError is not authoritative.
        let async_root = graph.get(&details_root, "async")?;
        !graph.nullish(&async_root)? && graph.is(&async_root, "state", "failed")?
    } else {
        false
    };
    if !has_details {
        return Ok(EditResult {
            content: if is_error { content_from_graph(graph, root, true)? } else { Vec::new() },
            is_error: Some(is_error),
            ..Default::default()
        });
    }
    let per_file = graph.get(&details_root, "perFileResults")?;
    let total_files = file_count.unwrap_or_else(||args
        .edits
        .as_ref()
        .map(|edits| {
            edits
                .iter()
                .filter_map(|edit| edit.path.as_ref())
                .filter(|path| !path.is_empty())
                .collect::<std::collections::HashSet<_>>()
                .len()
        })
        .unwrap_or(0));
    if graph.0.heap().kind(&per_file)? == ValueKind::Array {
        let length = graph.0.heap().array_length(&per_file, graph.runtime())?;
        if length > 1 || total_files > 1 {
            let mut files = Vec::with_capacity(length as usize);
            for index in 0..length {
                let file = graph.get(&per_file, &index.to_string())?;
                // renderMultiFileResult delegates to single-file rendering; it
                // does not recursively project nested perFileResults.
                let error = graph_bool(graph, &file, "isError")?;
                files.push(single_details_from_graph(graph, &file, &EditRenderArgs::default(), error, None)?.0);
            }
            return Ok(EditResult {
                details: Some(EditDetails { per_file_results: Some(files), ..Default::default() }),
                is_error: Some(is_error),
                ..Default::default()
            });
        }
    }
    let (details, content) = single_details_from_graph(graph, &details_root, args, Some(is_error), Some(root))?;
    Ok(EditResult { content, details: Some(details), is_error: Some(is_error) })
}

#[cfg(test)]
include!("event_controller_observations.rs");

fn usage_from_graph(g: &HostGraph<'_>, root: &ValueHandle) -> Result<ara_ai::Usage, ValueError> {
    if g.nullish(root)? {
        return Ok(ara_ai::Usage::unknown());
    }
    let number = |key: &str| -> Result<Option<u64>, ValueError> {
        Ok(match g.0.heap().snapshot(&g.get(root, key)?)? {
            ValueSnapshot::Number(n) if n.is_finite() && n >= 0.0 && n.fract() == 0.0 && n < (u64::MAX as f64) => {
                Some(n as u64)
            }
            _ => None,
        })
    };
    Ok(ara_ai::Usage {
        input: number("input")?,
        cache_read: number("cacheRead")?,
        cache_write: number("cacheWrite")?,
        ..Default::default()
    })
}

include!("session_transcript.rs");

#[cfg(test)]
#[path = "native_markdown_host_tests.rs"]
mod native_markdown_host_tests;
