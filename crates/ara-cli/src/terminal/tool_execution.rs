//! Retained fixed EDIT ToolExecutionComponent state and preview settlement.
//! Copyright (c) 2025 Mario Zechner; Copyright (c) 2025-2026 Can Bölük;
//! Copyright (c) 2026 Stencil Labs, Inc. MIT; see THIRD_PARTY_NOTICES.md.
use super::{
    edit_renderer::{EditComponent, streaming_fallback},
    theme::Theme,
    types::*,
};
use ara_rpc::WireString;
use std::sync::Arc;
use tokio::sync::Notify;
#[derive(Clone, PartialEq, Eq)]
struct DisplayKey {
    revision: u64,
    expanded: bool,
    partial: bool,
    args_complete: bool,
    started: bool,
    spinner: Option<usize>,
    theme: (u64, u64),
}
#[derive(Clone)]
pub struct ToolExecutionComponent {
    pub id: WireString,
    pub name: WireString,
    pub args_complete: bool,
    pub execution_started: bool,
    pub sealed: bool,
    pub disposed: bool,
    native: bool,
    pub revision: u64,
    pub preview_ready: bool,
    pub component: EditComponent,
    pub(crate) write: Option<super::write_renderer::WriteComponent>,
    xdev: Option<super::xdev_renderer::XdevComponent>,
    xdev_options: Option<ara_rpc::value_heap::ValueHandle>,
    xdev_display_key: Option<(DisplayKey, u64)>,
    xdev_constructor_failed: bool,
    previews: Option<Vec<PerFilePreview>>,
    notify: Arc<Notify>,
    pub(crate) graph: Option<super::graph_renderer::GraphEditRenderer>,
    pub(crate) graph_args_root: Option<ara_rpc::value_heap::ValueHandle>,
    pub(crate) warned: bool,
    pub(crate) result_root: Option<ara_rpc::value_heap::ValueHandle>,
    pub(crate) constructor_failed: bool,
    pub(crate) fallback_text: Option<String>,
    display_key: Option<DisplayKey>,
    execution_started_at: Option<std::time::Instant>,
    execution_ended_at: Option<std::time::Instant>,
    graph_shared: Arc<std::sync::Mutex<super::graph_renderer::DisplayRenderState>>,
}
impl ToolExecutionComponent {
    pub fn new(
        id: WireString,
        name: WireString,
        args: EditRenderArgs,
        mode: ara_edit::EditMode,
        theme: Arc<Theme>,
    ) -> Self {
        let context = EditRenderContext {
            edit_mode: Some(if name.equals_ascii("apply_patch") { ara_edit::EditMode::ApplyPatch } else { mode }),
            ..Default::default()
        };
        let graph_shared = Arc::new(std::sync::Mutex::new(super::graph_renderer::DisplayRenderState {
            options: RenderOptions { is_partial: true, ..Default::default() },
            context: context.clone(),
            frame: FrameContext::default(),
        },
        ));
        let write = name.equals_ascii("write").then(super::write_renderer::WriteComponent::default);
        Self {
            id,
            name,
            args_complete: false,
            execution_started: false,
            sealed: false,
            disposed: false,
            native: false,
            revision: 0,
            preview_ready: false,
            component: EditComponent::call(
                args,
                RenderOptions { is_partial: true, ..Default::default() },
                context,
                theme,
            ),
            write,
            xdev: None,
            xdev_options: None,
            xdev_display_key: None,
            xdev_constructor_failed: false,
            previews: None,
            notify: Arc::new(Notify::new()),
            graph: None,
            graph_args_root: None,
            warned: false,
            result_root: None,
            constructor_failed: false,
            fallback_text: None,
            display_key: None,
            execution_started_at: None,
            execution_ended_at: None,
            graph_shared,
        }
    }
    pub(crate) fn set_native(&mut self, active: bool) {
        if self.native != active {
            self.native = active;
            self.display_key = None;
            self.graph = None;
            self.component.invalidate();
        }
    }
    fn decorate_native(&self,node:super::tsp_backend::Node,generation:u64,graph:Option<&super::graph_renderer::DisplayGraph<'_>>)->Result<super::tsp_backend::Node,ara_rpc::value_heap::ValueError>{
        let mut node=super::native_view::decorate(node,self.id.clone(),generation,self.component.options,self.execution_started,self.sealed,self.component.result.as_ref());
        node.props["name"]=serde_json::json!(super::render_utils::sanitize(&self.name));node.props["role"]=serde_json::json!(format!("omp.tool.{}",super::render_utils::sanitize(&self.name)));node.props["key"]=serde_json::json!(super::render_utils::sanitize(&self.id));
        if let Some(started)=self.execution_started_at{let live=matches!(node.props["status"].as_str(),Some("pending"|"running"));let ended=self.execution_ended_at.unwrap_or_else(std::time::Instant::now);let elapsed=ended.saturating_duration_since(started).as_millis().min(u64::MAX as u128) as u64;node.props[if live{"age"}else{"took"}]=serde_json::json!(elapsed);}
        if let(Some(g),Some(args))=(graph,self.graph_args_root.as_ref()){let intent=g.get(args,"i")?;if let Some(intent)=g.optional(&intent)?.filter(|s|!s.is_empty()){node.props["intent"]=serde_json::json!(super::render_utils::sanitize(&intent));}}
        Ok(node)
    }
    pub(crate) fn describe_native(
        &mut self,
        graph: Option<&super::graph_renderer::DisplayGraph<'_>>,
        generation: u64,
    ) -> Result<Option<super::tsp_backend::Node>, ara_rpc::value_heap::ValueError> {
        if self.disposed {
            return Ok(None);
        }
        if let (Some(g),Some(args))=(graph,self.graph_args_root.as_ref()) {
            if let Some(node)=super::native_view::ast_graph(g,&self.name,args,self.result_root.as_ref(),self.execution_started)? {
                return self.decorate_native(node,generation,graph).map(Some);
            }
        }
        if !super::edit_renderer::is_edit_renderer(&self.name) {
            return Ok(None);
        }
        let mut facts=None;
        if let Some(g)=graph {
            if let Some(root)=&self.graph_args_root {
                let (args,resolved)=super::graph_renderer::GraphEditRenderer::native_call_args(g,root,self.component.options,&self.component.context)?;
                self.component.args=args;facts=Some(resolved);
            }
            if let Some(root)=&self.result_root {
                self.component.result=Some(super::event_controller::result_from_graph_with_count(g,root,&self.name,&self.component.args,self.component.result.as_ref().and_then(|r|r.is_error),facts.as_ref().map(|f|f.file_count))?);
            }
        }
        let node=super::native_view::edit_with_facts(&self.component.args,self.component.result.as_ref(),self.component.options,&self.component.context,facts);
        self.decorate_native(node,generation,graph).map(Some)
    }
    pub fn update_args(&mut self, args: EditRenderArgs) {
        if self.sealed || self.disposed {
            return;
        }
        self.component.replace_args(args);
        self.revision = self.revision.wrapping_add(1);
        self.refresh_context();
    }
    pub fn set_args_complete(&mut self) {
        if self.args_complete || self.disposed || self.sealed {
            return;
        }
        self.args_complete = true;
        if let Some(write) = &mut self.write {
            write.raw_block = None;
            write.stop_reveal();
        }
        self.revision = self.revision.wrapping_add(1);
        self.refresh_context();
    }
    pub fn set_execution_started(&mut self) {
        self.set_execution_started_at(std::time::Instant::now());
    }
    pub(crate) fn set_execution_started_at(&mut self, now: std::time::Instant) {
        if self.execution_started || self.disposed || self.sealed {
            return;
        }
        self.execution_started = true;
        self.execution_started_at = Some(now);
        self.args_complete = true;
        if let Some(write) = &mut self.write {
            write.raw_block = None;
            write.stop_reveal();
        }
        self.revision = self.revision.wrapping_add(1);
        self.refresh_context();
    }
    pub fn preview_settled(&self) -> bool {
        self.component
            .result
            .as_ref()
            .is_some_and(|r| {
            !self.component.options.is_partial && (r.is_error == Some(true) || r.details.is_some())
        })
    }
    fn ready(&mut self) {
        if !self.preview_ready {
            self.preview_ready = true;
            self.notify.notify_waiters();
        }
    }
    /// Empty final batches settle readiness but retain visible previous files.
    /// Files of the wrong shape are rejected by the event adapter before this
    /// method; generations belong to the controller's exact Run identity.
    pub fn update_stream_preview(&mut self, files: Vec<PerFilePreview>, streaming: bool) {
        if self.disposed || self.sealed || self.preview_settled() {
            return;
        }
        if !streaming {
            self.ready();
        }
        if files.is_empty() {
            return;
        }
        self.previews = Some(files);
        self.revision = self.revision.wrapping_add(1);
        self.refresh_context();
    }
    pub fn update_result(&mut self, result: EditResult, partial: bool) {
        if self.disposed || self.sealed {
            return;
        }
        self.component.result = Some(result);
        self.component.options.is_partial = partial;
        if !partial {
            self.execution_ended_at.get_or_insert_with(std::time::Instant::now);
            self.args_complete = true;
            if let Some(write) = &mut self.write {
                write.stop_reveal();
            }
            self.ready();
        }
        self.revision = self.revision.wrapping_add(1);
        self.refresh_context();
    }
    pub fn seal(&mut self) {
        self.ready();
        self.sealed = true;
        self.execution_ended_at.get_or_insert_with(std::time::Instant::now);
        if let Some(write) = &mut self.write {
            write.stop_reveal();
        }
        self.component.options.spinner_frame = None;
    }
    pub fn dispose(&mut self) {
        self.seal();
        self.disposed = true;
        self.previews = None;
        self.xdev = None;
        self.xdev_options = None;
        self.xdev_display_key = None;
    }
    pub fn toggle_expanded(&mut self) {
        if self.disposed {
            return;
        }
        self.component.options.expanded = !self.component.options.expanded;
        self.revision = self.revision.wrapping_add(1);
    }
    pub fn spinner_active(&self) -> bool {
        !self.sealed
            && !self.disposed
            && self.component.result.as_ref().is_none_or(|_| self.component.options.is_partial)
    }
    fn refresh_context(&mut self) {
        let context = &mut self.component.context;
        context.edit_diff_preview = None;
        context.per_file_diff_preview = None;
        self.component.args.preview_diff = None;
        if let Some(previews) = &self.previews {
            if let Some(first) = previews.first() {
                if let Some(error) = first.error.as_ref().filter(|e| !e.is_empty()) {
                    context.edit_diff_preview = Some(EditPreview::Error(error.clone()));
                } else if let Some(diff) = first.diff.as_ref().filter(|d| !d.is_empty()) {
                    context.edit_diff_preview =
                        Some(EditPreview::Diff { diff: diff.clone(), first_changed_line: first.first_changed_line,
                    });
                    self.component.args.preview_diff = Some(diff.clone());
                }
            }
            if previews.len() > 1 {
                context.per_file_diff_preview = Some(previews.clone());
            }
        }
        context.edit_streaming_fallback =
            if self.previews.as_ref().is_none_or(|p| {
            !p.iter().any(|p| p.diff.as_ref().is_some_and(|d| !d.is_empty()))
        })
            {
                context
                    .edit_mode
                    .map(|mode| streaming_fallback(mode, &self.component.args, &self.component.theme))
                    .filter(|f| !f.is_empty())
            } else {
                None
            };
    }
    pub fn render(&mut self, width: usize, frame: &FrameContext, spinner: usize) -> Vec<String> {
        if self.disposed {
            return Vec::new();
        }
        self.component.options.spinner_frame = if self.spinner_active() { Some(spinner) } else { None };
        if let Some(write) = &mut self.write {
            return write.render(width, frame, self.component.options, &self.component.theme);
        }
        self.component.render(width, frame)
    }
    pub(crate) fn sync_spinner(&mut self, spinner: usize) {
        self.component.options.spinner_frame = if self.spinner_active() { Some(spinner) } else { None };
    }
    pub(crate) fn rebuild_write_live_in(
        &mut self,
        g: &super::graph_renderer::DisplayGraph<'_>,
        now: std::time::Instant,
        smooth: bool,
    ) -> Vec<ara_rpc::value_heap::ValueError> {
        let Some(root) = self.graph_args_root.as_ref() else { return Vec::new();
        };
        let Some(write) = &mut self.write else { return Vec::new();
        };
        let result = write.rebuild_live(g, root, now, smooth).and_then(|()| self.rebuild_xdev_in(g));
        self.constructor_failed = result.is_err() || self.xdev_constructor_failed;
        if self.constructor_failed {
            self.write.as_mut().unwrap().stop_reveal();
        }
        match result {
            Ok(()) => Vec::new(),
            Err(error) => vec![error],
        }
    }
    pub(crate) fn reveal_deadline(&self) -> Option<std::time::Instant> {
        if self.sealed || self.disposed || self.args_complete || self.constructor_failed {
            None
        } else {
            self.write.as_ref().and_then(|write| write.reveal_deadline())
        }
    }
    /// Snap an unfinished raw buffer before completion clears its private
    /// producer handle. Store only a display root, never mutate tool arguments.
    pub(crate) fn finish_write_in(
        &mut self,
        g: &super::graph_renderer::DisplayGraph<'_>,
    ) -> Result<(), ara_rpc::value_heap::ValueError> {
        let Some(write) = self.write.as_ref() else { return Ok(());
        };
        // A constructor failure already has an effect receipt. Teardown must
        // not replay its producer/property getters in a later frame.
        let finish = if self.constructor_failed {
            Ok(None)
        } else {
            (|| {
                if let (Some(block), Some(full)) = (&write.raw_block, &self.graph_args_root) {
                    let key = ara_ai::graph_values::streaming_partial_json_key(g.0.heap())?;
                    let raw = g.0.heap().get(block, &key, g.runtime())?;
                    if let ara_rpc::value_heap::ValueSnapshot::String(raw) = g.0.heap().snapshot(&raw)? {
                        if !raw.is_empty() {
                            let decoded = super::write_renderer::decode_graph_args(g, full, &raw)?;
                            let args = super::write_renderer::args_from_graph(g, &decoded)?;
                            return Ok(Some((decoded, args)));
                        }
                    }
                }
                Ok(None)
            })()
        };
        let write = self.write.as_mut().unwrap();
        write.raw_block = None;
        write.stop_reveal();
        match finish {
            Ok(Some((decoded, args))) => {
                write.update_args(args);
                self.graph_args_root = Some(decoded);
                self.rebuild_xdev_in(g)
            }
            Ok(None) => Ok(()),
            Err(error) => {
                self.constructor_failed = true;
                Err(error)
            }
        }
    }
    /// Called at the real outer updateDisplay entrances, never by a timer frame.
    /// A failed constructor still owns this key, so same input is not replayed.
    pub(crate) fn rebuild_in(
        &mut self,
        g: &super::graph_renderer::DisplayGraph<'_>,
    ) -> Vec<ara_rpc::value_heap::ValueError> {
        if self.native {
            // Native describe runs only in the display transaction. Unsupported
            // components explicitly enter their existing rows fallback there.
            return Vec::new();
        }
        if self.write.is_some() {
            let Some(root) = self.graph_args_root.as_ref() else { return Vec::new();
            };
            let error = self.component.result.as_ref().is_some_and(|r| r.is_error == Some(true));
            let result = self.write.as_mut().unwrap().rebuild(
                g,
                root,
                self.result_root.as_ref(),
                error,
                self.component.options.is_partial,
            ).and_then(|()| self.rebuild_xdev_in(g));
            self.constructor_failed = result.is_err() || self.xdev_constructor_failed;
            return match result {
                Ok(()) => Vec::new(),
                Err(e) => vec![e],
            };
        }
        let key = DisplayKey {
            revision: self.revision,
            expanded: self.component.options.expanded,
            partial: self.component.options.is_partial,
            args_complete: self.args_complete,
            started: self.execution_started,
            spinner: self.component.options.spinner_frame,
            theme: (self.component.theme.identity, self.component.theme.revision),
        };
        if self.display_key.as_ref() == Some(&key) {
            return Vec::new();
        }
        self.display_key = Some(key);
        {
            let mut shared = self.graph_shared.lock().unwrap_or_else(|e| e.into_inner());
            shared.options = self.component.options;
            shared.context = self.component.context.clone();
        }
        let built = if let Some(root) = &self.result_root {
            super::graph_renderer::GraphEditRenderer::result_with_state(
                g,
                root,
                self.graph_args_root.as_ref(),
                &self.component.args,
                self.component.result.as_ref().is_some_and(|r| r.is_error == Some(true)),
                self.component.options,
                self.component.context.clone(),
                self.component.theme.clone(),
                self.graph_shared.clone(),
            )
        } else if let Some(root) = &self.graph_args_root {
            super::graph_renderer::GraphEditRenderer::call(
                g,
                root.clone(),
                self.component.options,
                self.component.context.clone(),
                self.component.theme.clone(),
            )
        } else {
            return Vec::new();
        };
        self.warned = false;
        self.constructor_failed = false;
        self.fallback_text = None;
        match built {
            Ok(renderer) => {
                if self.result_root.is_none() {
                    self.component.args = renderer.owned_args();
                    if let Some(context) = renderer.owned_context() {
                        self.component.context = context;
                    }
                } else {
                    let mut result = renderer.owned_result();
                    result.is_error = self.component.result.as_ref().and_then(|r| r.is_error);
                    self.component.result = Some(result);
                }
                self.graph = Some(renderer);
                Vec::new()
            }
            Err(error) => {
                self.graph = None;
                self.constructor_failed = true;
                self.warned = true;
                let mut errors = vec![error];
                if let Err(error) = self.capture_fallback(g) {
                    errors.push(error)
                }
                errors
            }
        }
    }
    pub(crate) fn invalidate_in(
        &mut self,
        g: Option<&super::graph_renderer::DisplayGraph<'_>>,
    ) -> Vec<ara_rpc::value_heap::ValueError> {
        if let Some(write) = &mut self.write {
            write.invalidate();
        }
        self.component.invalidate();
        if let (Some(renderer), Some(g)) = (&mut self.xdev, g) {
            if let Err(error) = renderer.invalidate(g) {
                return vec![error];
            }
        }
        if let Some(renderer) = &mut self.graph {
            if let Some(g) = g {
                if let Err(error) = renderer.invalidate_in(g) {
                    return vec![error];
                }
            } else {
                renderer.invalidate();
            }
        }
        if let Some(g) = g { self.rebuild_in(g) } else { Vec::new() }
    }
    pub(crate) fn rebuild_rows_fallback_in(&mut self,g:&super::graph_renderer::DisplayGraph<'_>)->Vec<ara_rpc::value_heap::ValueError>{
        let native=self.native;self.native=false;let errors=self.rebuild_in(g);self.native=native;errors
    }
    pub(crate) fn render_in(
        &mut self,
        graph: &super::graph_renderer::DisplayGraph<'_>,
        width: usize,
        frame: &FrameContext,
        spinner: usize,
        allocation: usize,
        now: std::time::Instant,
    ) -> Result<Vec<String>, ara_rpc::value_heap::ValueError> {
        if self.disposed {
            return Ok(Vec::new());
        }
        self.component.options.spinner_frame = if self.spinner_active() { Some(spinner) } else { None };
        {
            let mut shared = self.graph_shared.lock().unwrap_or_else(|e| e.into_inner());
            shared.options = self.component.options;
            shared.context = self.component.context.clone();
            shared.frame = *frame;
        }
        if self.write.is_some() {
            let prior_write_revision = self.write.as_ref().unwrap().revision;
            if !self.constructor_failed && !self.args_complete && !self.sealed {
                if let Some(root) = self.graph_args_root.as_ref() {
                    if let Err(error) = self.write.as_mut().unwrap().advance_reveal(graph, root, now) {
                        self.write.as_mut().unwrap().stop_reveal();
                        self.constructor_failed = true;
                        return Err(error);
                    }
                }
            } else if self.sealed && self.write.as_ref().is_some_and(|write| write.raw_block.is_some()) {
                self.finish_write_in(graph)?;
            }
            let state_changed = self.xdev_display_key.as_ref().is_some_and(|(key, _)| {
                key.revision != self.revision
                    || key.started != self.execution_started
                    || key.args_complete != self.args_complete
                    || key.expanded != self.component.options.expanded
                    || key.partial != self.component.options.is_partial
                    || key.theme != (self.component.theme.identity, self.component.theme.revision)
            });
            if !self.constructor_failed
                && (state_changed || self.write.as_ref().unwrap().revision != prior_write_revision)
            {
                if let Err(error) = self.rebuild_xdev_in(graph) {
                    self.constructor_failed = true;
                    self.write.as_mut().unwrap().stop_reveal();
                    return Err(error);
                }
            }
            let rows = if self.constructor_failed {
                self.fallback_rows(width)
            } else if let Some(xdev) = &mut self.xdev {
                xdev.update_options(graph,self.component.options,self.execution_started,self.args_complete,
                )?;
                xdev.render(graph, width, &self.component.theme)?
            } else {
                self.write.as_mut().expect("WRITE component").render(
                    width,
                    frame,
                    self.component.options,
                    &self.component.theme,
                )
            };
            return self.compact_in(graph, width, rows, allocation, now);
        }
        let rows = if self.constructor_failed {
            self.fallback_rows(width)
        } else {
            match &mut self.graph {
                Some(renderer) => renderer.render(graph, width, frame, self.component.options, &self.component.context,
                )?,
                None => self.component.render(width, frame),
            }
        };
        self.compact_in(graph, width, rows, allocation, now)
    }
    /// Retain the source callback output across render frames. Only a real
    /// argument/result/state change (including streamed Write reveal) rebuilds
    /// it; a throwing constructor owns the same key and is never retried by a
    /// timer. `args_complete` and `execution_started` remain distinct facts.
    fn rebuild_xdev_in(
        &mut self,
        g: &super::graph_renderer::DisplayGraph<'_>,
    ) -> Result<(), ara_rpc::value_heap::ValueError> {
        let Some(write) = &self.write else { return Ok(());
        };
        let key = (DisplayKey {
            revision: self.revision,
            expanded: self.component.options.expanded,
            partial: self.component.options.is_partial,
            args_complete: self.args_complete,
            started: self.execution_started,
            spinner: self.component.options.spinner_frame,
            theme: (self.component.theme.identity, self.component.theme.revision),
        }, write.revision,
        );
        if self.xdev_display_key.as_ref() == Some(&key) {
            return Ok(());
        }
        self.xdev_display_key = Some(key);
        self.xdev = None;
        self.xdev_constructor_failed = false;
        if self.xdev_options.is_none() {
            self.xdev_options=Some(super::xdev_renderer::options_value(g,self.component.options,self.execution_started,self.args_complete,
            )?);
        }
        let built = super::xdev_renderer::XdevComponent::build(
            g, &write.args, self.result_root.as_ref(), self.component.options,
            self.execution_started, self.args_complete, &self.component.theme,
            self.xdev_options.as_ref(),
        );
        match built {
            Ok(renderer) => { self.xdev = renderer; Ok(()) }
            Err(error) => { self.xdev_constructor_failed = true; self.constructor_failed = true; Err(error) }
        }
    }
    pub(crate) fn compact_in(
        &self,
        graph: &super::graph_renderer::DisplayGraph<'_>,
        width: usize,
        rows: Vec<String>,
        allocation: usize,
        now: std::time::Instant,
    ) -> Result<Vec<String>, ara_rpc::value_heap::ValueError> {
        if allocation >= 3 {
            return Ok(rows);
        }
        let start = rows.iter().position(|r| !r.trim().is_empty()).unwrap_or(rows.len());
        let end = rows.iter().rposition(|r| !r.trim().is_empty()).map(|i| i + 1).unwrap_or(start);
        let trimmed = rows[start..end].to_vec();
        if trimmed.len() <= allocation {
            return Ok(trimmed);
        }
        if self.write.is_some() {
            return Ok(trimmed
                .into_iter()
                .take(allocation)
                .map(|r| super::render_utils::truncate(&r, width))
                .collect());
        }
        let Some(root) = &self.graph_args_root else { return Ok(trimmed);
        };
        // The compact caller follows full render, then independently resolves
        // call facts. It does not run renderCall's payload/fallback helpers.
        let (label, detail) = super::graph_renderer::GraphEditRenderer::activity(
            graph,
            root,
            self.component.options,
            &self.component.context,
        )?;
        let detail = detail.filter(|d| !d.is_empty()).or_else(|| self.spinner_active().then(|| "running".to_owned()));
        let theme = &self.component.theme;
        let elapsed = if self.spinner_active() {
            self.execution_started_at
                .map(|start| {
                    theme.fg("dim", &format!(" {}s", now.saturating_duration_since(start).as_secs()),
                    )
                })
                .unwrap_or_default()
        } else {
            String::new()
        };
        let text = format!(
            "{}{}{}",
            theme.fg("toolTitle", &format!("\x1b[1m{label}\x1b[22m")),
            detail
                .map(|d| theme.fg("muted", &format!(" · {}", d.split_whitespace().collect::<Vec<_>>().join(" "))))
                .unwrap_or_default(),
            elapsed
        );
        let text = super::render_utils::truncate(&text, width.saturating_sub(4).max(1));
        if allocation == 1 {
            let glyph = self
                .component
                .options
                .spinner_frame
                .and_then(|s| theme.spinner_frames.get(s % theme.spinner_frames.len()))
                .map(String::as_str)
                .unwrap_or("•");
            Ok(vec![super::render_utils::truncate(
                &format!(
                    "{} {text}",
                    theme.fg(if self.component.options.spinner_frame.is_some() { "muted" } else { "dim" }, glyph)
                ),
                width,
            )])
        } else {
            Ok(vec![
                super::render_utils::truncate(&format!("{} {text}", theme.fg("dim", "╭─")), width),
                theme.fg("dim", "╰"),
            ])
        }
    }
    pub(crate) fn fallback_rows(&self, width: usize) -> Vec<String> {
        let text = self
            .fallback_text
            .clone()
            .unwrap_or_else(|| {
            self.component.theme.fg("toolTitle", &super::render_utils::sanitize(&self.name))
        });
        text.split('\n').flat_map(|line| super::render_utils::wrap(line, width)).collect()
    }
    pub(crate) fn capture_fallback(
        &mut self,
        g: &super::graph_renderer::DisplayGraph<'_>,
    ) -> Result<(), ara_rpc::value_heap::ValueError> {
        if let Some(root) = &self.result_root {
            let text = super::event_controller::content_from_graph(g, root, false)?
                .iter()
                .map(super::render_utils::sanitize)
                .collect::<Vec<_>>()
                .join("\n");
            self.fallback_text = Some(self.component.theme.fg("toolOutput", &super::render_utils::replace_tabs(&text)),
            );
        } else {
            self.fallback_text = Some(self.component.theme.fg("toolTitle", &super::render_utils::sanitize(&self.name)),
            );
        }
        Ok(())
    }
    pub async fn when_preview_settled(&self, timeout: Option<std::time::Duration>) {
        if self.preview_ready {
            return;
        }
        let notified = self.notify.notified();
        if self.preview_ready {
            return;
        }
        if let Some(timeout) = timeout {
            let _ = tokio::time::timeout(timeout, notified).await;
        } else {
            notified.await;
        }
    }
}
