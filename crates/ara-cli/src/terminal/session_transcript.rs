// Fixed OMP 596f2da ui-helpers, chat-transcript-builder and rebuild consumer.
// Copyright (c) 2025 Mario Zechner; 2025-2026 Can Bölük; 2026 Stencil Labs. MIT.
use super::{
    markdown::MarkdownComponent, session_observation::SessionTranscriptObservation,
    summary_message::SummaryMessageComponent,
};
use ara_rpc::WireValue as SessionWire;

fn host_markdown(text: WireString, theme: Arc<Theme>, options: super::markdown::MarkdownOptions) -> MarkdownComponent {
    let mut component = MarkdownComponent::new(text, theme);
    component.set_options(options);
    component
}

struct CachedSessionEntry {
    body: Arc<SessionWire>,
    keys: Vec<BlockKey>,
}
pub(crate) enum SessionRow {
    Summary(SummaryMessageComponent),
    Markdown(MarkdownComponent),
}
impl SessionRow {
    pub(crate) fn frame_copy(&self) -> Self {
        match self {
            Self::Summary(c) => Self::Summary(c.frame_copy()),
            Self::Markdown(c) => Self::Markdown(c.frame_copy()),
        }
    }
    pub(crate) fn render_for_frame(&mut self, width: usize, frame: &FrameContext) -> Vec<String> {
        match self {
            Self::Summary(c) => {
                c.set_frame_capabilities(frame);
                c.render(width).as_ref().clone()
            }
            Self::Markdown(c) => {
                c.set_frame_capabilities(frame);
                c.render(width).as_ref().clone()
            }
        }
    }
}
#[derive(Default)]
struct SessionTranscriptState {
    id: Option<String>,
    revision: u64,
    collapsed: bool,
    expanded: bool,
    entries: HashMap<String, CachedSessionEntry>,
    rows: HashMap<BlockKey, SessionRow>,
    message_start: Vec<BlockKey>,
    result_keys: HashMap<WireString, Vec<BlockKey>>,
    theme: Option<(u64, u64)>,
}
impl EventController {
    pub(crate) fn start_entry_observation(&mut self) {
        self.session_transcript.message_start = self.transcript.block_keys();
    }
    pub(crate) fn observe_entry_receipt(&mut self, id: String, body: Arc<SessionWire>) -> Result<(), ValueError> {
        let before: std::collections::HashSet<_> = self.session_transcript.message_start.iter().collect();
        let mut keys: Vec<_> = self.transcript.block_keys().into_iter().filter(|k| !before.contains(k)).collect();
        if session_is(&body, "role", "toolResult") {
            // ExecutionEnd precedes the tool-result MessageStart. Its original
            // generic text rows therefore cannot be found by a start/end delta.
            keys = body
                .get("toolCallId")
                .and_then(SessionWire::as_string)
                .and_then(|id| self.session_transcript.result_keys.get(id))
                .cloned()
                .unwrap_or_default();
        }
        // A persisted summary has no streamed text/tool rows. Admit through the
        // same component and Session key as restored history, before caching
        // this receipt; otherwise the next equal snapshot would retain no rows.
        if keys.is_empty() {
            if let Some(mut summary) = SummaryMessageComponent::from_wire_with_options(&body, self.theme.clone(), self.native_markdown_options.clone())
                .map_err(|_| ValueError::LegacyBridge("invalid Session summary display DTO"))?
            {
                summary.set_expanded(self.session_transcript.expanded);
                let key = BlockKey::Session(id.clone(), 0);
                self.session_transcript.rows.insert(key.clone(), SessionRow::Summary(summary));
                self.transcript.add_tool(key.clone(), true);
                keys.push(key);
            }
        }
        // User input is persisted before its live receipt reaches this owner.
        // Existing streamed assistant/tool rows retain their original identity.
        if keys.is_empty() && session_is(&body, "role", "user") {
            let key = BlockKey::Session(id.clone(), 0);
            if let Some(text) = session_text(&body) {
                self.session_transcript
                    .rows
                    .insert(key.clone(), SessionRow::Markdown(host_markdown(text, self.theme.clone(), self.native_markdown_options.clone())));
                self.transcript.add_tool(key.clone(), true);
                keys.push(key);
            }
        }
        self.session_transcript.entries.insert(id, CachedSessionEntry { body, keys });
        Ok(())
    }
    pub(crate) fn session_rows(&mut self, key: &BlockKey, width: usize) -> Option<Vec<String>> {
        self.session_transcript.rows.get_mut(key).map(|row| match row {
            SessionRow::Summary(c) => c.render(width).as_ref().clone(),
            SessionRow::Markdown(c) => c.render(width).as_ref().clone(),
        })
    }
    pub(crate) fn session_frame_copy(&self, key: &BlockKey) -> Option<SessionRow> {
        self.session_transcript.rows.get(key).map(SessionRow::frame_copy)
    }
    pub(crate) fn commit_session_row(&mut self, key: &BlockKey, row: SessionRow) {
        if let Some(retained) = self.session_transcript.rows.get_mut(key) {
            *retained = row;
        }
    }
    fn toggle_summary_expanded(&mut self) {
        self.session_transcript.expanded = !self.session_transcript.expanded;
        for row in self.session_transcript.rows.values_mut() {
            if let SessionRow::Summary(c) = row {
                c.set_expanded(self.session_transcript.expanded);
            }
        }
    }
    fn invalidate_session_rows(&mut self) {
        let theme = (self.theme.identity, self.theme.revision);
        if self.native_markdown_theme != theme {
            self.native_markdown_options = self.theme.native_markdown_options();
            self.native_markdown_theme = theme;
        }
        for row in self.session_transcript.rows.values_mut() {
            match row {
                SessionRow::Summary(c) => {
                    c.invalidate(self.theme.clone());
                    c.set_markdown_options(self.native_markdown_options.clone());
                },
                SessionRow::Markdown(c) => {
                    c.invalidate(self.theme.clone());
                    c.set_options(self.native_markdown_options.clone());
                },
            }
        }
    }
    pub(crate) fn observe_session(
        &mut self,
        snapshot: SessionTranscriptObservation,
        g: Option<&HostGraph<'_>>,
    ) -> Result<(), ValueError> {
        if self.session_transcript.id.as_ref().is_some_and(|id| id != &snapshot.session_id) {
            return Err(ValueError::UnsupportedOperation("new logical Session needs a new display owner"));
        }
        if snapshot.revision <= self.session_transcript.revision {
            return Ok(());
        }
        let theme = (self.theme.identity, self.theme.revision);
        let changed_theme = self.native_markdown_theme != theme || self.session_transcript.theme.is_some_and(|old| old != theme);
        if changed_theme {
            self.invalidate_session_rows();
        }
        let previous: Vec<_> = self.transcript.block_keys();
        let active = self.transcript.active_keys();
        let mut keys = Vec::new();
        for message in &snapshot.view.messages {
            let same = !changed_theme
                && self
                    .session_transcript
                    .entries
                    .get(&message.entry_id)
                    .is_some_and(|old| old.body.deep_equal(&message.body));
            if same {
                keys.extend(self.session_transcript.entries[&message.entry_id].keys.clone());
                continue;
            }
            let body = Arc::new(ara_session::native_clone_wire(&message.body));
            let key = BlockKey::Session(message.entry_id.clone(), 0);
            let mut entry_keys = Vec::new();
            if let Some(mut summary) = SummaryMessageComponent::from_wire_with_options(&body, self.theme.clone(), self.native_markdown_options.clone())
                .map_err(|_| ValueError::LegacyBridge("invalid Session summary display DTO"))?
            {
                summary.set_expanded(self.session_transcript.expanded);
                self.session_transcript.rows.insert(key.clone(), SessionRow::Summary(summary));
                self.transcript.add_tool(key.clone(), true);
                entry_keys.push(key);
            } else if session_is(&body, "role", "assistant") {
                if let Some(content) = body.get("content").and_then(SessionWire::as_array) {
                    for (index, block) in content.iter().enumerate() {
                        if session_is(block, "type", "toolCall") {
                            if let Some(id) = block.get("id").and_then(SessionWire::as_string) {
                                let existing = self
                                    .transcript
                                    .tool_keys()
                                    .into_iter()
                                    .find(|key| matches!(key,BlockKey::Tool(_,old) if old==id));
                                let tool = if let Some(key) = existing {
                                    key
                                } else {
                                    self.replay_session_tool(
                                        &message.entry_id,
                                        id,
                                        snapshot.originals.get(&message.entry_id),
                                        g,
                                    )?;
                                    self.call_key(id)
                                };
                                if self.transcript.contains(&tool) {
                                    entry_keys.push(tool);
                                }
                            }
                        } else if session_is(block, "type", "text") {
                            if let Some(text) = block.get("text").and_then(SessionWire::as_string) {
                                let key = BlockKey::Session(message.entry_id.clone(), index + 1);
                                self.session_transcript.rows.insert(
                                    key.clone(),
                                    SessionRow::Markdown(host_markdown(text.clone(), self.theme.clone(), self.native_markdown_options.clone())),
                                );
                                self.transcript.add_tool(key.clone(), true);
                                entry_keys.push(key);
                            }
                        }
                    }
                }
            } else if session_is(&body, "role", "toolResult") {
                if let Some(result_keys) = body
                    .get("toolCallId")
                    .and_then(SessionWire::as_string)
                    .and_then(|id| self.session_transcript.result_keys.get(id))
                {
                    entry_keys.extend(result_keys.iter().filter(|key| self.transcript.contains(key)).cloned());
                }
                if entry_keys.is_empty() {
                    let before: std::collections::HashSet<_> = self.transcript.block_keys().into_iter().collect();
                    if let Some(original) = snapshot.originals.get(&message.entry_id) {
                        self.replay_session_result(original, g)?;
                    }
                    entry_keys.extend(self.transcript.block_keys().into_iter().filter(|key| !before.contains(key)));
                }
                // Existing renderer owns the corresponding call block; never add a second result block.
            } else if let Some(text) = session_text(&body) {
                // Non-display custom messages are not visible history entries.
                if !session_is(&body, "role", "custom") || matches!(body.get("display"), Some(SessionWire::Bool(true)))
                {
                    self.session_transcript
                        .rows
                        .insert(key.clone(), SessionRow::Markdown(host_markdown(text, self.theme.clone(), self.native_markdown_options.clone())));
                    self.transcript.add_tool(key.clone(), true);
                    entry_keys.push(key);
                }
            }
            keys.extend(entry_keys.clone());
            self.session_transcript
                .entries
                .insert(message.entry_id.clone(), CachedSessionEntry { body, keys: entry_keys });
        }
        // Preserve uncommitted live components in their original order. A terminal
        // result has settled its call already; async running retains its live key.
        for key in active {
            if !keys.contains(&key) {
                keys.push(key);
            }
        }
        if !self.live {
            for key in &keys {
                if let BlockKey::Tool(generation, id) = key {
                    if *generation == self.generation {
                        if let Some(tool) = self.tools.get_mut(id) {
                            if tool.component.result.is_none() {
                                tool.seal();
                                self.transcript.settle(key);
                            }
                        }
                    }
                }
            }
        }
        let changed = keys != previous;
        let clear = self.session_transcript.id.is_some()
            && snapshot.collapsed
            && changed
            && previous.iter().any(|key| !keys.contains(key));
        self.transcript.select_session(keys, clear);
        self.session_transcript.id = Some(snapshot.session_id);
        self.session_transcript.revision = snapshot.revision;
        self.session_transcript.collapsed = snapshot.collapsed;
        self.session_transcript.theme = Some(theme);
        let retained: std::collections::HashSet<_> =
            snapshot.view.messages.iter().map(|entry| entry.entry_id.as_str()).collect();
        self.session_transcript.entries.retain(|id, _| retained.contains(id.as_str()));
        self.prune_session_rows();
        Ok(())
    }
    pub(crate) fn acknowledge_session_history(&mut self, id: u64) -> bool {
        let acknowledged = self.transcript.acknowledge_history(id);
        if acknowledged {
            self.prune_session_rows();
        }
        acknowledged
    }
    fn prune_session_rows(&mut self) {
        if self.session_transcript.id.is_none() {
            return;
        }
        let keys = self.transcript.block_keys();
        let keys: std::collections::HashSet<_> = keys.into_iter().collect();
        self.session_transcript.rows.retain(|key, _| keys.contains(key));
        self.session_transcript.result_keys.retain(|_, result_keys| {
            result_keys.retain(|key| keys.contains(key));
            !result_keys.is_empty()
        });
        self.archived.retain(|(key, _)| keys.contains(key));
    }
    fn replay_session_tool(
        &mut self,
        _entry: &str,
        id: &WireString,
        original: Option<&ara_agent::AgentMessage>,
        g: Option<&HostGraph<'_>>,
    ) -> Result<(), ValueError> {
        let Some(original) = original else {
            return Err(ValueError::LegacyBridge("Session display tool lacks original owner"));
        };
        match original.body_snapshot() {
            ara_agent::AgentMessageBody::Graph(root) => {
                let g = g.ok_or(ValueError::LegacyBridge("Session display tool requires original Native owner"))?;
                let content = g.get(&root, "content")?;
                for block in g.members(&content)? {
                    if g.is(&block, "type", "toolCall")? && g.string(&g.get(&block, "id")?)? == *id {
                        let name = g.string(&g.get(&block, "name")?)?;
                        let args = g.get(&block, "arguments")?;
                        self.upsert_graph(g, id.clone(), name, args)?;
                        if let Some(tool) = self.tools.get_mut(id) {
                            tool.set_args_complete();
                        }
                        return Ok(());
                    }
                }
            }
            _ => {
                let body =
                    original.json().map_err(|_| ValueError::LegacyBridge("invalid original Session display tool"))?;
                if let Some(content) = body["content"].as_array() {
                    for block in content {
                        if block["type"] == "toolCall"
                            && block["id"].as_str().is_some_and(|value| id.equals_ascii(value))
                        {
                            let name: WireString = block["name"].as_str().unwrap_or("").into();
                            if name.equals_ascii("write") {
                                self.upsert_write_json(id.clone(), &block["arguments"]);
                            } else {
                                self.upsert(id.clone(), name, args_from_json(&block["arguments"]));
                            }
                            return Ok(());
                        }
                    }
                }
            }
        }
        Err(ValueError::LegacyBridge("selected Session tool call missing from original body"))
    }
    fn replay_session_result(
        &mut self,
        original: &ara_agent::AgentMessage,
        g: Option<&HostGraph<'_>>,
    ) -> Result<(), ValueError> {
        match original.body_snapshot() {
            ara_agent::AgentMessageBody::Graph(root) => {
                let g = g.ok_or(ValueError::LegacyBridge("Session result needs original Native owner"))?;
                let id = g.string(&g.get(&root, "toolCallId")?)?;
                let name = g.string(&g.get(&root, "toolName")?)?;
                let error = match g.0.heap().snapshot(&g.get(&root, "isError")?)? {
                    ValueSnapshot::Boolean(v) => Some(v),
                    _ => None,
                };
                let details = g.get(&root, "details")?;
                let asynchronous = if g.nullish(&details)? {
                    false
                } else {
                    let a = g.get(&details, "async")?;
                    !g.nullish(&a)? && g.is(&a, "state", "running")?
                };
                self.result_graph(g, id, name, root, error, asynchronous)?;
            }
            _ => {
                let value = original.json().map_err(|_| ValueError::LegacyBridge("invalid original Session result"))?;
                let id: WireString = value["toolCallId"].as_str().unwrap_or("").into();
                let name: WireString = value["toolName"].as_str().unwrap_or("").into();
                let partial = value["details"]["async"]["state"] == "running";
                let mut output = ara_agent::ToolOutput::text("");
                output.content = serde_json::from_value(value["content"].clone())
                    .map_err(|_| ValueError::LegacyBridge("invalid Session tool result"))?;
                output.details = value.get("details").cloned();
                output.is_error = value["isError"].as_bool().unwrap_or(false);
                if name.equals_ascii("write") {
                    self.write_result_json(id, &output, value["isError"].as_bool(), partial);
                } else {
                    self.result(id, name, result_from_tool_output(&output), partial);
                }
            }
        }
        Ok(())
    }
}
fn session_is(body: &SessionWire, key: &str, value: &str) -> bool {
    body.get(key).and_then(SessionWire::as_string).is_some_and(|s| s.equals_ascii(value))
}
fn session_text(body: &SessionWire) -> Option<WireString> {
    let content = body.get("content").or_else(|| body.get("output"))?;
    if let Some(text) = content.as_string() {
        return Some(text.clone());
    }
    let parts: Vec<_> = content
        .as_array()?
        .iter()
        .filter(|v| session_is(v, "type", "text"))
        .filter_map(|v| v.get("text").and_then(SessionWire::as_string))
        .collect();
    let mut units = Vec::new();
    for (i, text) in parts.iter().enumerate() {
        if i > 0 {
            units.push(10)
        }
        units.extend_from_slice(text.units());
    }
    Some(WireString::from_units(units))
}

#[cfg(test)]
#[path = "session_consumer_observations.rs"]
mod consumer_observations;
