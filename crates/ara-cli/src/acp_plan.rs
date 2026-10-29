//! Session-owned ACP plan proposal and prompt context. Fixed OMP
//! 596f2da7101178214aa27a753529d15e6b7ad91d: modes/acp/acp-agent.ts,
//! plan-mode/approved-plan.ts, session/agent-session.ts, tools/resolve.ts.
//! Includes only b91db66e plan-autosave.ts and the known ACP approval save delta.
//! The two included prompt files retain the exact original fixed upstream contents.
// MIT License
// Copyright (c) 2025 Mario Zechner
// Copyright (c) 2025-2026 Can Bölük
// Copyright (c) 2026 Stencil Labs, Inc.
// Permission is hereby granted, free of charge, to any person obtaining a copy
// of this software and associated documentation files (the "Software"), to deal
// in the Software without restriction, including without limitation the rights
// to use, copy, modify, merge, publish, distribute, sublicense, and/or sell
// copies of the Software, and to permit persons to whom the Software is
// furnished to do so, subject to the following conditions:
// The above copyright notice and this permission notice shall be included in all
// copies or substantial portions of the Software.
// THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF ANY KIND, EXPRESS OR
// IMPLIED, INCLUDING BUT NOT LIMITED TO THE WARRANTIES OF MERCHANTABILITY,
// FITNESS FOR A PARTICULAR PURPOSE AND NONINFRINGEMENT. IN NO EVENT SHALL THE
// AUTHORS OR COPYRIGHT HOLDERS BE LIABLE FOR ANY CLAIM, DAMAGES OR OTHER
// LIABILITY, WHETHER IN AN ACTION OF CONTRACT, TORT OR OTHERWISE, ARISING FROM,
// OUT OF OR IN CONNECTION WITH THE SOFTWARE OR THE USE OR OTHER DEALINGS IN THE
// SOFTWARE.

use crate::acp_transport::AcpConnection;
use ara_agent::{ToolError, ToolOutput};
use ara_tools::{ContentUriPort, ContentUriRoute, UriResource};
use async_trait::async_trait;
use serde_json::{Value, json};
use std::{
    future::Future,
    io,
    path::{Component, Path, PathBuf},
    pin::Pin,
    sync::{Arc, Mutex, MutexGuard, Weak},
    time::SystemTime,
};
use tokio::io::AsyncWriteExt;
use tokio_util::sync::CancellationToken;
use yaml_rust2::{Yaml, YamlLoader};

const DEFAULT_PLAN_FILE_URL: &str = "local://PLAN.md";
const PROPOSE: &str = "xd://propose";
const APPROVE_OPTION: &str = "Approve and execute";
const REFINE_OPTION: &str = "Refine plan";
const ACTIVE_TEMPLATE: &str = include_str!("plan-mode-active.md");
const REFERENCE_TEMPLATE: &str = include_str!("plan-mode-reference.md");

/// Narrow Host snapshot of fixed settings-schema.ts:4786 (default true) and
/// Settings.get:637-646. Global first-existing file, then native project YAML.
/// Nested leaf lookup and JS truthiness match the ACP mode-admission consumer;
/// a quoted dotted key is not the nested plan.enabled value.
pub fn plan_enabled(cwd: &Path, agent_dir: &Path) -> Result<bool, String> {
    let mut configured = None;
    for filename in ["config.yml", "config.yaml"] {
        if let Some(document) = read_plan_settings(&agent_dir.join(filename))? {
            apply_plan_settings(&document, &mut configured);
            break;
        }
    }
    if let Some(document) = read_plan_settings(&cwd.join(".ara").join("config.yml"))? {
        apply_plan_settings(&document, &mut configured);
    }
    Ok(match configured {
        None | Some(Yaml::BadValue) => true,
        Some(Yaml::Null) => false,
        Some(Yaml::Boolean(value)) => value,
        Some(Yaml::String(value)) => !value.is_empty(),
        Some(Yaml::Integer(value)) => value != 0,
        Some(value @ Yaml::Real(_)) => value.as_f64().is_some_and(|value| value != 0.0 && !value.is_nan()),
        Some(_) => true,
    })
}
fn read_plan_settings(path: &Path) -> Result<Option<Yaml>, String> {
    let source = match std::fs::read_to_string(path) {
        Ok(source) => source,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(format!("reading native settings {}: {error}", path.display())),
    };
    let mut documents = YamlLoader::load_from_str(&source)
        .map_err(|error| format!("parsing native settings {}: {error}", path.display()))?;
    if documents.len() > 1 {
        return Err(format!("native settings must contain one YAML document: {}", path.display()));
    }
    match documents.pop() {
        None | Some(Yaml::Null) => Ok(Some(Yaml::Hash(Default::default()))),
        Some(document @ Yaml::Hash(_)) => Ok(Some(document)),
        Some(_) => Err(format!("native settings must contain a mapping at the document root: {}", path.display())),
    }
}
fn apply_plan_settings(document: &Yaml, configured: &mut Option<Yaml>) {
    match &document["plan"] {
        Yaml::BadValue => {}
        group @ Yaml::Hash(_) => {
            if !matches!(&group["enabled"], Yaml::BadValue) {
                *configured = Some(group["enabled"].clone());
            }
        }
        // DeepMerge lets this layer replace a prior mapping; getByPath then
        // finds no enabled leaf and uses the fixed default.
        _ => *configured = None,
    }
}

// b91db66e plan-mode/settings.ts and plan-autosave.ts. Reuse this Host's
// native layered YAML source, but read it live at each known approval. Unlike
// plan.enabled's existing admission consumer, autosave requires strict true.
fn read_plan_autosave_settings(
    cwd: &Path,
    agent_dir: &Path,
) -> Result<(Option<Yaml>, Option<Yaml>), String> {
    let mut configured = (None, None);
    let apply = |document: &Yaml, configured: &mut (Option<Yaml>, Option<Yaml>)| match &document["plan"]
    {
        Yaml::BadValue => {}
        group @ Yaml::Hash(_) => {
            if !matches!(&group["autosave"], Yaml::BadValue) {
                configured.0 = Some(group["autosave"].clone());
            }
            if !matches!(&group["autosaveDir"], Yaml::BadValue) {
                configured.1 = Some(group["autosaveDir"].clone());
            }
        }
        _ => *configured = (None, None),
    };
    for filename in ["config.yml", "config.yaml"] {
        if let Some(document) = read_plan_settings(&agent_dir.join(filename))? {
            apply(&document, &mut configured);
            break;
        }
    }
    if let Some(document) = read_plan_settings(&cwd.join(".ara").join("config.yml"))? {
        apply(&document, &mut configured);
    }
    Ok(configured)
}

fn plan_autosave_dir(cwd: &Path, configured: Option<&Yaml>) -> Result<PathBuf, String> {
    let raw = configured
        .and_then(Yaml::as_str)
        .unwrap_or("")
        .trim_matches(plan_whitespace);
    if raw.is_empty() {
        return Ok(cwd.join(".ara").join("plans"));
    }
    let expanded = if let Some(tail) = raw.strip_prefix('~') {
        let home = crate::user_home();
        if tail.is_empty() {
            home
        } else if tail.starts_with(['/', '\\']) {
            // Upstream concatenates these forms; on POSIX a backslash remains
            // literal, while Windows recognizes it as a path separator.
            let mut value = home.into_os_string();
            value.push(tail);
            PathBuf::from(value)
        } else {
            home.join(tail)
        }
    } else {
        PathBuf::from(raw)
    };
    absolute_lexical(&if expanded.is_absolute() {
        expanded
    } else {
        cwd.join(expanded)
    })
}

// This private consumer receives only resolve_plan_title's normalized ASCII
// title. Byte length equals JS UTF-16 length here; full standalone Unicode
// NFC/category/uppercase helper parity remains outside this bounded ACP delta.
fn plan_autosave_file_name(title: &str) -> String {
    let mut stem = String::new();
    for value in title.bytes() {
        if value.is_ascii_alphanumeric() {
            stem.push(char::from(value.to_ascii_uppercase()));
        } else if !stem.is_empty() && !stem.ends_with('_') {
            stem.push('_');
        }
    }
    let mut stem = stem.trim_end_matches('_').to_owned();
    if stem.len() > 32 {
        let cut = stem[..=32].rfind('_').filter(|cut| *cut > 0).unwrap_or(32);
        stem.truncate(cut);
    }
    if stem.is_empty() || stem == "PLAN" {
        return "PLAN.md".into();
    }
    if !stem.ends_with("_PLAN") {
        stem.push_str("_PLAN");
    }
    format!("{stem}.md")
}

async fn write_plan_autosave_exclusive(path: &Path, content: &str) -> io::Result<()> {
    let mut file = tokio::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(path)
        .await?;
    file.write_all(content.as_bytes()).await?;
    // Tokio writes may still be in its blocking worker after write_all. Wait
    // before dropping the handle; upstream writeFile also settles before return.
    file.flush().await
}

async fn autosave_approved_plan(cwd: &Path, title: &str, content: &str) -> Result<(), String> {
    let settings_cwd = cwd.to_owned();
    let agent_dir = crate::ara_home().join("agent");
    let settings =
        tokio::task::spawn_blocking(move || read_plan_autosave_settings(&settings_cwd, &agent_dir))
            .await;
    // The source catches settings.get failures and disables autosave.
    let Ok(Ok((Some(Yaml::Boolean(true)), directory))) = settings else {
        return Ok(());
    };
    if content.trim_matches(plan_whitespace).is_empty() {
        return Ok(());
    }
    let dir = plan_autosave_dir(cwd, directory.as_ref())?;
    tokio::fs::create_dir_all(&dir)
        .await
        .map_err(|error| error.to_string())?;
    let filename = plan_autosave_file_name(title);
    let stem = filename.strip_suffix(".md").unwrap();
    for index in 0..1000 {
        let name = if index == 0 {
            filename.clone()
        } else {
            format!("{stem}-{index}.md")
        };
        match write_plan_autosave_exclusive(&dir.join(name), content).await {
            Ok(()) => return Ok(()),
            Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {}
            Err(error) => return Err(error.to_string()),
        }
    }
    let timestamp = SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .map_err(|error| error.to_string())?
        .as_millis();
    write_plan_autosave_exclusive(&dir.join(format!("{timestamp}-{filename}")), content)
        .await
        .map_err(|error| error.to_string())
}

pub type PlanRefresh = Arc<dyn Fn() -> Pin<Box<dyn Future<Output = Result<(), String>> + Send>> + Send + Sync>;
/// Host supplies its complete current mode/model/thinking configuration list.
pub type PlanConfigOptions = Arc<dyn Fn(&str) -> Value + Send + Sync>;

#[derive(Clone)]
struct ActivePlan {
    plan_file_path: String,
    workflow: &'static str,
    reentry: bool,
}
#[derive(Clone)]
struct State {
    active: Option<ActivePlan>,
    /// Tool preparation reads this staged target. Path authorization keeps
    /// the public active plan until a default snapshot has actually published.
    transition_mode: Option<&'static str>,
    reference_path: String,
    reference_sent: bool,
    reference_epoch: u64,
}

pub struct AcpPlan {
    connection: Arc<AcpConnection>,
    session_id: String,
    cwd: PathBuf,
    local_root: PathBuf,
    supports_form: bool,
    transition: tokio::sync::Mutex<()>,
    state: Mutex<State>,
    refresh: Mutex<Option<PlanRefresh>>,
    config_options: Mutex<Option<PlanConfigOptions>>,
    session_close: Mutex<Option<CancellationToken>>,
    uri_port: Mutex<Option<Arc<dyn ContentUriPort>>>,
    live_uri_port: Mutex<Option<Weak<dyn ContentUriPort>>>,
}
impl AcpPlan {
    pub fn new(
        connection: Arc<AcpConnection>,
        session_id: String,
        cwd: PathBuf,
        local_root: PathBuf,
        supports_form: bool,
    ) -> Arc<Self> {
        Arc::new(Self {
            connection,
            session_id,
            cwd,
            local_root,
            supports_form,
            transition: tokio::sync::Mutex::new(()),
            state: Mutex::new(State {
                active: None,
                transition_mode: None,
                reference_path: DEFAULT_PLAN_FILE_URL.into(),
                reference_sent: false,
                reference_epoch: 0,
            }),
            refresh: Mutex::new(None),
            config_options: Mutex::new(None),
            session_close: Mutex::new(None),
            uri_port: Mutex::new(None),
            live_uri_port: Mutex::new(None),
        })
    }
    fn state(&self) -> MutexGuard<'_, State> {
        self.state.lock().unwrap_or_else(std::sync::PoisonError::into_inner)
    }
    pub fn bind_refresh(&self, refresh: PlanRefresh) {
        *self.refresh.lock().unwrap_or_else(std::sync::PoisonError::into_inner) = Some(refresh);
    }
    pub fn bind_config_options(&self, supplier: PlanConfigOptions) {
        *self.config_options.lock().unwrap_or_else(std::sync::PoisonError::into_inner) = Some(supplier);
    }
    pub fn bind_uri_port(&self, port: Option<Arc<dyn ContentUriPort>>) {
        *self.uri_port.lock().unwrap_or_else(std::sync::PoisonError::into_inner) = port;
    }
    pub fn bind_live_uri_port(&self, port: Weak<dyn ContentUriPort>) {
        *self.live_uri_port.lock().unwrap_or_else(std::sync::PoisonError::into_inner) = Some(port);
    }
    fn uri_port_snapshot(&self) -> Option<Arc<dyn ContentUriPort>> {
        // Drop the weak binding lock before upgrade/routing; retain one issuing
        // live owner through this synchronous call, never a permanent Arc cycle.
        let live=self.live_uri_port.lock().unwrap_or_else(std::sync::PoisonError::into_inner).clone();
        live.and_then(|port|port.upgrade()).or_else(||
            self.uri_port.lock().unwrap_or_else(std::sync::PoisonError::into_inner).clone())
    }
    fn normalize_plan_uri(&self, path: &str) -> String {
        let port=self.uri_port_snapshot();
        ara_tools::internal_urls::normalize_plan_uri(port.as_deref(), path)
    }
    pub fn bind_session_close(&self, close: CancellationToken) {
        *self.session_close.lock().unwrap_or_else(std::sync::PoisonError::into_inner) = Some(close);
    }
    fn is_closed(&self) -> bool {
        self.connection.is_closed()
            || self
                .session_close
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .as_ref()
                .is_some_and(CancellationToken::is_cancelled)
    }
    async fn refresh(&self) -> Result<(), String> {
        let callback = self.refresh.lock().unwrap_or_else(std::sync::PoisonError::into_inner).clone();
        if let Some(callback) = callback {
            callback().await?;
        }
        Ok(())
    }
    pub fn current_mode(&self) -> &'static str {
        if self.state().active.is_some() { "plan" } else { "default" }
    }
    pub fn snapshot_mode(&self) -> &'static str {
        let state = self.state();
        state.transition_mode.unwrap_or(if state.active.is_some() { "plan" } else { "default" })
    }
    /// plan.enabled and supported-mode admission belong to the outer ACP Host.
    pub async fn set_mode(&self, mode: &str) -> Result<(), String> {
        let _transition = self.transition.lock().await;
        let previous = self.state().clone();
        {
            let mut state = self.state();
            match mode {
                "plan" => {
                    let previous = state.active.take();
                    let active = ActivePlan {
                        plan_file_path: previous
                            .as_ref()
                            .map_or_else(|| DEFAULT_PLAN_FILE_URL.into(), |plan| plan.plan_file_path.clone()),
                        workflow: previous.as_ref().map_or("parallel", |plan| plan.workflow),
                        reentry: previous.is_some(),
                    };
                    state.reference_path = active.plan_file_path.clone();
                    state.reference_sent = false;
                    state.reference_epoch = state.reference_epoch.wrapping_add(1);
                    state.active = Some(active);
                    state.transition_mode = Some("plan");
                }
                "default" => state.transition_mode = Some("default"),
                _ => return Err(format!("Unsupported ACP mode: {mode}")),
            }
        }
        if let Err(error) = self.refresh().await {
            *self.state() = previous;
            return Err(format!("ACP mode transition failed; previous mode restored: {error}"));
        }
        {
            let mut state = self.state();
            if mode == "default" {
                state.active = None;
            }
            state.transition_mode = None;
        }
        if self.is_closed() {
            return Ok(());
        }
        self.notify_mode_update().await
    }
    pub fn reference_epoch(&self) -> u64 {
        self.state().reference_epoch
    }
    pub fn mark_reference_sent_at(&self, epoch: u64) {
        let mut state = self.state();
        if state.reference_epoch == epoch && state.active.is_none() {
            state.reference_sent = true;
        }
    }
    /// Fixed AgentSession resets this lifetime after compaction/history rewrite.
    pub fn reset_reference_after_history_rewrite(&self) {
        let mut state = self.state();
        state.reference_sent = false;
        state.reference_epoch = state.reference_epoch.wrapping_add(1);
    }

    /// Read the current state/file at each model preparation. The caller marks
    /// a returned non-plan reference sent only after admission to Agent history.
    pub fn prompt_context(&self, tool_names: &[String], edit_mode: &str) -> Result<Option<String>, String> {
        let (active, reference_path, reference_sent) = {
            let state = self.state();
            (state.active.clone(), state.reference_path.clone(), state.reference_sent)
        };
        if let Some(active) = active {
            let resolved = self.resolve_plan_path(&active.plan_file_path)?;
            let session_plan = self.resolve_plan_path(DEFAULT_PLAN_FILE_URL)?;
            let display_path = if active.plan_file_path.starts_with("local:") || resolved != session_plan {
                active.plan_file_path
            } else {
                DEFAULT_PLAN_FILE_URL.into()
            };
            let capable = |name: &str| tool_names.iter().any(|tool| tool == name);
            let context = json!({
                "planFilePath": display_path, "planExists": resolved.exists(),
                "askToolName": "ask", "writeToolName": "write", "editToolName": "edit",
                "askAvailable": capable("ask"), "taskAvailable": capable("task"),
                "isHashlineEditMode": edit_mode == "hashline", "reentry": active.reentry,
                "iterative": active.workflow == "iterative", "scoutAvailable": capable("task"),
            });
            return ara_prompt::render(ACTIVE_TEMPLATE, &context).map(Some).map_err(|error| error.to_string());
        }
        if reference_sent {
            return Ok(None);
        }
        let Some(content) = self.read_plan_sync(&reference_path)? else {
            return Ok(None);
        };
        if content.trim_matches(plan_whitespace).is_empty() {
            return Ok(None);
        }
        ara_prompt::render(REFERENCE_TEMPLATE, &json!({"planFilePath": reference_path, "planContent": content}))
            .map(Some)
            .map_err(|error| error.to_string())
    }
    async fn notify_mode_update(&self) -> Result<(), String> {
        if self.is_closed() {
            return Ok(());
        }
        self.connection
            .notify(
                "session/update",
                json!({"sessionId": self.session_id,
            "update": {"sessionUpdate": "current_mode_update", "currentModeId": self.current_mode()}}),
            )
            .await
            .map_err(|error| error.to_string())?;
        if self.is_closed() {
            return Ok(());
        }
        let current_mode = self.current_mode();
        let supplier = self.config_options.lock().unwrap_or_else(std::sync::PoisonError::into_inner).clone();
        let config_options = match supplier {
            Some(supplier) => supplier(current_mode),
            None => json!([{"id":"mode","name":"Mode","category":"mode","type":"select",
            "currentValue":self.current_mode(),"options":[
                {"value":"default","name":"Default","description":"Standard ACP headless mode"},
                {"value":"plan","name":"Plan","description":"Read-only planning mode that drafts a plan to a markdown file before any code changes"}
            ]}]),
        };
        self.connection
            .notify(
                "session/update",
                json!({"sessionId":self.session_id,
            "update":{"sessionUpdate":"config_option_update","configOptions":config_options}}),
            )
            .await
            .map_err(|error| error.to_string())
    }
    async fn propose(&self, supplied_title: &str, cancel: CancellationToken) -> Result<ToolOutput, ToolError> {
        let _transition = self.transition.lock().await;
        if cancel.is_cancelled() {
            return Err(ToolError("Cancelled before ACP plan proposal admission".into()));
        }
        let previous = self.state().clone();
        let active = previous.active.clone().ok_or_else(|| ToolError("Plan mode is not active.".into()))?;
        let title = supplied_title.trim_matches(plan_whitespace);
        let resolved = self.resolve_approved_plan(title, &active.plan_file_path).await?;
        if cancel.is_cancelled() {
            return Err(ToolError("Cancelled before ACP plan approval request admission".into()));
        }
        let approved = self.request_approval(&resolved.title, &resolved.content).await;
        {
            let mut state = self.state();
            if approved {
                state.transition_mode = Some("default");
            } else {
                state.active = Some(ActivePlan { plan_file_path: resolved.path.clone(), ..active.clone() });
                state.transition_mode = Some("plan");
            }
        }
        if let Err(error) = self.refresh().await {
            // A received Approve is evidence, but failed refresh does not grant
            // public default/full-write authority or commit approved history.
            let mut restored = previous;
            restored.active = Some(ActivePlan { plan_file_path: resolved.path.clone(), ..active });
            *self.state() = restored;
            let text = if approved {
                format!(
                    "Plan approval was received, but the mode transition failed; plan mode remains active at {}. Tool refresh failed: {error}",
                    resolved.path
                )
            } else {
                format!(
                    "Plan refinement requested; plan mode remains active at {}. Tool refresh failed: {error}",
                    resolved.path
                )
            };
            return Ok(proposal_details(ToolOutput::error(text), &resolved));
        }
        {
            let mut state = self.state();
            if approved {
                state.active = None;
            }
            state.transition_mode = None;
            state.reference_path = resolved.path.clone();
            state.reference_sent = false;
            state.reference_epoch = state.reference_epoch.wrapping_add(1);
        }
        let autosave_failed = if approved {
            match autosave_approved_plan(&self.cwd, &resolved.title, &resolved.content).await {
                Ok(()) => false,
                Err(error) => {
                    eprintln!("ara: Failed to autosave approved plan for Session {}: {error}", self.session_id);
                    true
                }
            }
        } else {
            false
        };
        if approved && !self.is_closed() {
            // Fixed ACP catches notification failure after its approval commit;
            // the known approval is not replayed because the client disconnected.
            let _ = self.notify_mode_update().await;
        }
        let text = if approved {
            let warning = if autosave_failed { " (Plan autosave failed; continuing.)" } else { "" };
            format!("Plan approved at {}. Plan mode exited; proceed with the implementation.{warning}", resolved.path)
        } else {
            format!(
                "Plan refinement requested. Update the plan file, then write {} to xd://propose again when ready.",
                resolved.title
            )
        };
        Ok(proposal_details(ToolOutput::text(text), &resolved))
    }
    async fn request_approval(&self, title: &str, content: &str) -> bool {
        if !self.supports_form {
            return true;
        }
        let lines = content.split('\n').collect::<Vec<_>>();
        let preview = lines.iter().take(12).copied().collect::<Vec<_>>().join("\n");
        let ellipsis = if lines.len() > 12 { "\n…" } else { "" };
        let message = format!("Approve plan \"{title}\" and start implementation?\n\n{preview}{ellipsis}");
        // No abort/timeout/retry race after admission. Reader owns the matching
        // response independently; transport failure/refusal keeps plan active.
        let response = self
            .connection
            .request(
                "elicitation/create",
                json!({
                    "mode":"form","sessionId":self.session_id,"message":message,
                    "requestedSchema":{"type":"object","properties":{"value":{"type":"string",
                        "enum":[APPROVE_OPTION,REFINE_OPTION]}},"required":["value"]}
                }),
            )
            .await;
        matches!(response, Ok(value) if value.get("action").and_then(Value::as_str)==Some("accept")
            && value.get("content").and_then(|value|value.get("value")).and_then(Value::as_str)==Some(APPROVE_OPTION))
    }
    async fn resolve_approved_plan(&self, title: &str, state_path: &str) -> Result<ResolvedPlan, ToolError> {
        let mut ordered = Vec::<String>::new();
        let consider = |ordered: &mut Vec<String>, path: &str| {
            if !path.is_empty() && !ordered.iter().any(|existing| existing == path) {
                ordered.push(path.into());
            }
        };
        if let Ok(normalized) = normalize_plan_title(title) {
            let slug = strip_ascii_suffix(&normalized, "-plan");
            consider(&mut ordered, &format!("local://{}-plan.md", if slug.is_empty() { &normalized } else { slug }));
        }
        let listed = self.list_local_plans().await;
        if !state_path.is_empty()
            && !listed.iter().any(|path| self.normalize_plan_uri(path) == self.normalize_plan_uri(state_path))
        {
            consider(&mut ordered, state_path);
        }
        for path in listed {
            consider(&mut ordered, &path);
        }
        consider(&mut ordered, state_path);
        for path in &ordered {
            if let Some(content) = self.read_plan(path).await.map_err(ToolError)? {
                let title = resolve_plan_title(title, &content, path);
                return Ok(ResolvedPlan { path: path.clone(), content, title });
            }
        }
        let target = ordered.first().map(String::as_str).unwrap_or(state_path);
        Err(ToolError(format!(
            "Plan file not found at {target}. Write the finalized plan to {target} before requesting approval."
        )))
    }
    async fn list_local_plans(&self) -> Vec<String> {
        let Ok(mut directory) = tokio::fs::read_dir(&self.local_root).await else {
            return Vec::new();
        };
        let mut plans = Vec::new();
        loop {
            let entry = match directory.next_entry().await {
                Ok(Some(entry)) => entry,
                Ok(None) => break,
                Err(_) => return Vec::new(),
            };
            let Ok(kind) = entry.file_type().await else {
                continue;
            };
            let Some(name) = entry.file_name().to_str().map(str::to_owned) else {
                continue;
            };
            if kind.is_file() && name.to_ascii_lowercase().ends_with("plan.md") {
                let modified =
                    entry.metadata().await.ok().and_then(|stat| stat.modified().ok()).unwrap_or(SystemTime::UNIX_EPOCH);
                plans.push((format!("local://{name}"), modified));
            }
        }
        plans.sort_by(|left, right| right.1.cmp(&left.1));
        plans.into_iter().map(|(path, _)| path).collect()
    }
    async fn read_plan(&self, path: &str) -> Result<Option<String>, String> {
        match tokio::fs::read(self.resolve_plan_path(path)?).await {
            Ok(bytes) => Ok(Some(String::from_utf8_lossy(&bytes).into_owned())),
            Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(None),
            Err(error) => Err(error.to_string()),
        }
    }
    fn read_plan_sync(&self, path: &str) -> Result<Option<String>, String> {
        match std::fs::read(self.resolve_plan_path(path)?) {
            Ok(bytes) => Ok(Some(String::from_utf8_lossy(&bytes).into_owned())),
            Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(None),
            Err(error) => Err(error.to_string()),
        }
    }
    fn resolve_plan_path(&self, path: &str) -> Result<PathBuf, String> {
        let port=self.uri_port_snapshot();
        let normalized=ara_tools::internal_urls::normalize_plan_uri(port.as_deref(), path);
        let route=port.as_ref().map_or(ContentUriRoute::Unregistered, |port|port.route(&normalized));
        if matches!(route, ContentUriRoute::Registered { .. }) {
            return port.unwrap().locate_sync(&ara_tools::internal_urls::split_content_url_selector(&normalized).0)
                .map_err(|error|error.0)?.ok_or_else(||format!("No local file backs plan path {path}"));
        }
        if route!=ContentUriRoute::Removed && ara_tools::internal_urls::hierarchical_scheme(&normalized)
            .is_some_and(|scheme|scheme.eq_ignore_ascii_case("local")) {
            return self.resolve_native_local_plan(&normalized).map_err(|_|format!("No local file backs plan path {path}"));
        }
        absolute_lexical(&self.cwd.join(path))
    }
    fn resolve_native_local_plan(&self, path: &str) -> Result<PathBuf, String> {
        let (_, tail)=path.split_once("://").ok_or_else(||format!("Invalid URL: {path}"))?;
        let tail = &tail[..tail.find(['?', '#']).unwrap_or(tail.len())];
        let (host, pathname) = tail.split_once('/').map_or((tail, ""), |(host, path)| (host, path));
        let host = decode_local(host).unwrap_or_else(|_| host.to_owned());
        let combined = if host.is_empty() {
            pathname.to_owned()
        } else if pathname.is_empty() {
            host
        } else {
            format!("{host}/{pathname}")
        };
        let relative = decode_local(&combined.replace('\\', "/"))
            .map_err(|_| format!("Invalid URL encoding in local:// path: {path}"))?;
        ara_tools::internal_urls::validate_relative_path(&relative)
            .map_err(|error| error.replace("skill://", "local://"))?;
        let root = absolute_lexical(&self.local_root)?;
        let target = absolute_lexical(&root.join(relative))?;
        if !target.starts_with(&root) {
            return Err("local:// URL escapes local root".into());
        }
        Ok(target)
    }
}
impl ara_cli::host_edit_session::EditPlanPolicy for AcpPlan {
    fn current_plan_path(&self) -> Option<String> {
        self.state().active.as_ref().map(|plan| plan.plan_file_path.clone())
    }
    fn read_reference_path(&self) -> Option<PathBuf> {
        let reference = self.state().reference_path.clone();
        ara_cli::host_edit_session::resolve_read_plan_reference(&self.local_root, &reference).ok()
    }
    fn read_reference_snapshot(&self) -> Result<ara_tools::read::ReadPlanReferenceSnapshot, ToolError> {
        let authored=self.state().reference_path.clone();
        let native=ara_tools::internal_urls::normalize_native_local_alias(&authored);
        let native=if let Some((scheme,tail))=native.split_once("://") {
            if scheme.eq_ignore_ascii_case("local") { format!("local://{tail}") } else { native }
        } else { native };
        let native_local_fallback=ara_cli::host_edit_session::resolve_read_plan_reference(&self.local_root,&native).ok();
        Ok(ara_tools::read::ReadPlanReferenceSnapshot::Uri { authored, native_local_fallback })
    }
}
#[async_trait]
impl ContentUriPort for AcpPlan {
    fn edit_specs(&self)->Vec<ara_tools::ContentUriEditSpec> {
        if self.state().active.is_none(){return Vec::new();}
        vec![ara_tools::ContentUriEditSpec {scheme:"xd".into(),single_slash_alias:false,line_selectors:false,port_authority:false,immutable:false,policy:ara_tools::ContentUriEditPolicy::Handler}]
    }
    fn route(&self, url: &str) -> ContentUriRoute {
        if url == PROPOSE && self.state().active.is_some() {
            ContentUriRoute::Registered { writable: true }
        } else {
            ContentUriRoute::Unregistered
        }
    }
    async fn read(&self, _url: &str, _cancel: CancellationToken) -> Result<UriResource, ToolError> {
        Err(ToolError("ACP plan proposal is a write-only device.".into()))
    }
    async fn write(&self, url: &str, content: &str, cancel: CancellationToken) -> Result<(), ToolError> {
        self.write_result(url, content, cancel).await.map(|_| ())
    }
    async fn write_result(
        &self,
        url: &str,
        content: &str,
        cancel: CancellationToken,
    ) -> Result<Option<ToolOutput>, ToolError> {
        if url != PROPOSE {
            return Err(ToolError(format!("Unregistered ACP plan URI: {url}")));
        }
        let mut output = self.propose(content, cancel).await?;
        let inner = output.details.take();
        output.details = Some(json!({"xdev":{"tool":"propose","mode":"execute",
            "args":{"title":content.trim_matches(plan_whitespace)},"inner":inner}}));
        Ok(Some(output))
    }
}

struct ResolvedPlan {
    path: String,
    content: String,
    title: String,
}
#[async_trait]
impl ara_cli::native_factory::catalog::NativeProposalHost for AcpPlan {
    fn is_active(&self) -> bool {
        self.state().active.is_some()
    }
    async fn propose(&self, title: &str, cancel: CancellationToken) -> Result<ToolOutput, ToolError> {
        AcpPlan::propose(self, title, cancel).await
    }
}
fn proposal_details(output: ToolOutput, resolved: &ResolvedPlan) -> ToolOutput {
    output.with_details(json!({"planFilePath":resolved.path,"title":resolved.title,"planExists":true}))
}
fn plan_whitespace(value: char) -> bool {
    matches!(
        value,
        '\t' | '\u{000b}'
            | '\u{000c}'
            | ' '
            | '\u{00a0}'
            | '\u{feff}'
            | '\n'
            | '\r'
            | '\u{2028}'
            | '\u{2029}'
            | '\u{1680}'
            | '\u{2000}'..='\u{200a}' | '\u{202f}' | '\u{205f}' | '\u{3000}'
    )
}
fn strip_ascii_suffix<'a>(value: &'a str, suffix: &str) -> &'a str {
    if value.len() >= suffix.len()
        && value.get(value.len() - suffix.len()..).is_some_and(|tail| tail.eq_ignore_ascii_case(suffix))
    {
        &value[..value.len() - suffix.len()]
    } else {
        value
    }
}
fn normalize_plan_title(title: &str) -> Result<String, ToolError> {
    let trimmed = title.trim_matches(plan_whitespace);
    if trimmed.is_empty() {
        return Err(ToolError("Plan title is required and must not be empty.".into()));
    }
    if trimmed.contains(['/', '\\']) || trimmed.contains("..") {
        return Err(ToolError("Plan title must not contain path separators or '..'.".into()));
    }
    let without_extension = strip_ascii_suffix(trimmed, ".md");
    let mut normalized = String::new();
    let mut whitespace = false;
    for ch in without_extension.chars() {
        if plan_whitespace(ch) {
            if !whitespace {
                normalized.push('-');
            }
            whitespace = true;
        } else {
            whitespace = false;
            if ch.is_ascii_alphanumeric() || ch == '_' || ch == '-' {
                normalized.push(ch);
            }
        }
    }
    let mut collapsed = String::new();
    for ch in normalized.chars() {
        if ch != '-' || !collapsed.ends_with('-') {
            collapsed.push(ch);
        }
    }
    let collapsed = collapsed.trim_matches('-').to_owned();
    if collapsed.is_empty() {
        return Err(ToolError(
            "Plan title must contain at least one letter, number, underscore, or hyphen after sanitization.".into(),
        ));
    }
    Ok(collapsed)
}
fn resolve_plan_title(supplied: &str, content: &str, path: &str) -> String {
    let heading = content.split('\n').find_map(|line| {
        let line = line.trim_start_matches([' ', '\t']);
        let tail = line.strip_prefix('#')?;
        if !tail.starts_with([' ', '\t']) {
            return None;
        }
        let title = tail.trim_matches(plan_whitespace);
        (!title.is_empty()).then_some(title)
    });
    let scheme=regex::Regex::new(r"(?i)^[a-z][a-z0-9+.-]*:/+").expect("fixed plan scheme");
    let without_scheme=scheme.replace(path, "");
    let stem = without_scheme.rsplit(['/', '\\']).next().unwrap_or("");
    let stem = strip_ascii_suffix(stem, ".md");
    [Some(supplied), heading, Some(stem), Some("plan")]
        .into_iter()
        .flatten()
        .find_map(|candidate| normalize_plan_title(candidate).ok())
        .unwrap_or_else(|| "plan".into())
}
fn decode_local(input: &str) -> Result<String, ()> {
    let mut bytes = Vec::with_capacity(input.len());
    let source = input.as_bytes();
    let mut offset = 0;
    while offset < source.len() {
        if source[offset] == b'%' {
            let hex = |value: Option<&u8>| value.and_then(|value| char::from(*value).to_digit(16));
            let (Some(high), Some(low)) = (hex(source.get(offset + 1)), hex(source.get(offset + 2))) else {
                return Err(());
            };
            bytes.push((high * 16 + low) as u8);
            offset += 3;
        } else {
            bytes.push(source[offset]);
            offset += 1;
        }
    }
    String::from_utf8(bytes).map_err(|_| ())
}
fn absolute_lexical(path: &Path) -> Result<PathBuf, String> {
    let absolute = std::path::absolute(path).map_err(|error| error.to_string())?;
    let mut resolved = PathBuf::new();
    for component in absolute.components() {
        match component {
            Component::CurDir => {}
            Component::ParentDir => {
                resolved.pop();
            }
            other => resolved.push(other.as_os_str()),
        }
    }
    Ok(resolved)
}

#[cfg(test)]
mod plan_uri_upgrade_tests {
    use super::*;
    struct Capability { backing:Option<PathBuf>, error:bool, removed:bool, alias:bool }
    #[async_trait]
    impl ContentUriPort for Capability {
        fn route(&self,url:&str)->ContentUriRoute {
            if url.starts_with("fixture://")||url.starts_with("local://") {
                if self.removed {ContentUriRoute::Removed} else {ContentUriRoute::Registered{writable:false}}
            } else {ContentUriRoute::Unregistered}
        }
        fn normalize_url(&self,input:&str)->String {
            if self.alias&&!self.removed {ara_tools::internal_urls::normalize_native_local_alias(input)} else {input.into()}
        }
        fn locate_sync(&self,_:&str)->Result<Option<PathBuf>,ToolError> {
            if self.error {Err(ToolError("sync callback error".into()))} else {Ok(self.backing.clone())}
        }
        async fn read(&self,_:&str,_:CancellationToken)->Result<UriResource,ToolError>{panic!("plan resolver must not materialize")}
        async fn write(&self,_:&str,_:&str,_:CancellationToken)->Result<(),ToolError>{panic!("plan resolver must not write")}
    }
    #[tokio::test]
    async fn plan_uri_registered_sync_stem_and_order_group() {
        tokio::time::timeout(std::time::Duration::from_secs(5),async {
            let work=tempfile::tempdir().unwrap();let local=tempfile::tempdir().unwrap();
            let (peer,stream)=tokio::io::duplex(4096);let (reader,writer)=tokio::io::split(stream);
            let transport=crate::acp_transport::start(reader,writer).await;
            let plan=AcpPlan::new(transport.connection.clone(),"uri-fixture".into(),work.path().into(),local.path().into(),false);
            let registered=local.path().join("current.md");
            std::fs::write(&registered,"current content without heading\n").unwrap();
            plan.bind_uri_port(Some(Arc::new(Capability{backing:Some(registered.clone()),error:false,removed:false,alias:true})));
            assert_eq!(plan.resolve_plan_path("fixture://reference.md").unwrap(),registered);
            assert_eq!(plan.read_plan("fixture://reference.md").await.unwrap().as_deref(),Some("current content without heading\n"));
            assert_eq!(plan.resolve_plan_path("local:/reference.md").unwrap(),registered);
            eprintln!("ARA-PLAN-URI PASS sync-registered-file");
            plan.bind_uri_port(Some(Arc::new(Capability{backing:None,error:false,removed:false,alias:false})));
            assert_eq!(plan.resolve_plan_path("fixture://virtual").unwrap_err(),"No local file backs plan path fixture://virtual");
            plan.bind_uri_port(Some(Arc::new(Capability{backing:None,error:true,removed:false,alias:false})));
            assert_eq!(plan.resolve_plan_path("fixture://error").unwrap_err(),"sync callback error");
            plan.bind_uri_port(Some(Arc::new(Capability{backing:Some(registered.clone()),error:false,removed:true,alias:false})));
            assert_eq!(plan.resolve_plan_path("local://reference.md").unwrap(),absolute_lexical(&work.path().join("local://reference.md")).unwrap());
            assert_eq!(plan.normalize_plan_uri("local:/reference.md"),"local:/reference.md");
            plan.bind_uri_port(None);
            assert_eq!(plan.resolve_plan_path("unsupported://reference.md").unwrap(),absolute_lexical(&work.path().join("unsupported://reference.md")).unwrap());
            assert_eq!(plan.resolve_plan_path("docs/current.md").unwrap(),absolute_lexical(&work.path().join("docs/current.md")).unwrap());
            assert_eq!(plan.resolve_plan_path("local:/nested/reference.md").unwrap(),local.path().join("nested/reference.md"));
            assert_eq!(plan.resolve_plan_path("LOCAL:/reference.md").unwrap(),local.path().join("reference.md"));
            assert_eq!(plan.resolve_plan_path("local://../escape.md").unwrap_err(),"No local file backs plan path local://../escape.md");
            eprintln!("ARA-PLAN-URI PASS sync-none-error-unsupported-removed-native");
            for (input,expected) in [("X+1.-:/feature.MD","feature"),("X+1.-://folder/feature.md","feature"),
                ("X+1.-:///feature.md","feature"),("X+1.-://folder\\feature.md","feature"),
                ("fixture://.md","plan"),("fixture://","plan"),("bare.md","bare")] {
                assert_eq!(resolve_plan_title("","no heading",input),expected,"{input}");
            }
            eprintln!("ARA-PLAN-URI PASS generic-scheme-stems");
            let older=local.path().join("old-plan.md");std::fs::write(&older,"old").unwrap();
            std::fs::File::options().write(true).open(&older).unwrap().set_modified(SystemTime::UNIX_EPOCH+std::time::Duration::from_secs(1_900_000_000)).unwrap();
            let newest=local.path().join("new-plan.md");std::fs::write(&newest,"new").unwrap();
            std::fs::File::options().write(true).open(&newest).unwrap().set_modified(SystemTime::UNIX_EPOCH+std::time::Duration::from_secs(1_900_000_010)).unwrap();
            let chosen=plan.resolve_approved_plan("","local:/old-plan.md").await.unwrap();
            assert_eq!(chosen.path,"local://new-plan.md");assert_eq!(chosen.content,"new");
            // A cwd state target outside local scan precedes old/new local candidates.
            std::fs::write(work.path().join("CURRENT.md"),"cwd current").unwrap();
            assert_eq!(plan.resolve_approved_plan("","CURRENT.md").await.unwrap().content,"cwd current");
            assert_eq!(plan.normalize_plan_uri("/repo/local://old-plan.md"),"local://old-plan.md");
            assert_eq!(plan.normalize_plan_uri("LOCAL:/old-plan.md"),"LOCAL://old-plan.md");
            let snapshot=ara_cli::host_edit_session::EditPlanPolicy::read_reference_snapshot(plan.as_ref()).unwrap();
            assert!(matches!(snapshot,ara_tools::read::ReadPlanReferenceSnapshot::Uri{authored,..} if authored=="local://PLAN.md"));
            // Same-process real Host owners: final chain retains plan, plan holds
            // only Weak final router, and initial external is expiry fallback.
            plan.bind_uri_port(Some(Arc::new(Capability{backing:Some(registered.clone()),error:false,removed:false,alias:false})));
            let uri_bridge=crate::rpc_host_uris::UriBridge::new(Arc::new(|_|panic!("sync plan resolution must not emit URI read")));
            uri_bridge.set_schemes(&ara_rpc::WireValue::parse(&json!([{"scheme":"dynamic"},{"scheme":"local"}]).to_string()).unwrap()).unwrap();
            let journal=ara_session::SessionJournal::create(work.path(),work.path()).unwrap();
            let store=ara_cli::session_artifacts::SessionArtifacts::for_journal(&journal);
            let containing_plan=ara_cli::host_vault::compose_ports(plan.clone(),Some(uri_bridge.clone()));
            let final_router=ara_cli::session_artifacts::ArtifactUriRouter::new(store,Some(containing_plan));
            let trait_router:Arc<dyn ContentUriPort>=final_router.clone();
            let final_weak=Arc::downgrade(&trait_router);
            plan.bind_live_uri_port(final_weak.clone());
            let live_snapshot=plan.uri_port_snapshot().unwrap();
            assert!(Arc::ptr_eq(&live_snapshot,&trait_router));drop(live_snapshot);
            assert_eq!(plan.resolve_plan_path("artifact://0").unwrap_err(),"No local file backs plan path artifact://0");
            assert_eq!(final_router.route("dynamic://plan.md"),ContentUriRoute::Registered{writable:false});
            assert_eq!(plan.resolve_plan_path("dynamic://plan.md").unwrap_err(),"No local file backs plan path dynamic://plan.md");
            // Registered local virtual override cannot inherit native backing
            // or the initial external file-backed capability.
            assert_eq!(plan.resolve_plan_path("local://live.md").unwrap_err(),"No local file backs plan path local://live.md");
            assert_eq!(plan.normalize_plan_uri("local:/live.md"),"local:/live.md");
            uri_bridge.set_schemes(&ara_rpc::WireValue::parse("[]").unwrap()).unwrap();
            assert_eq!(final_router.route("local://live.md"),ContentUriRoute::Removed);
            assert_eq!(plan.normalize_plan_uri("local:/live.md"),"local:/live.md");
            assert_eq!(plan.resolve_plan_path("local://live.md").unwrap(),absolute_lexical(&work.path().join("local://live.md")).unwrap());
            assert_eq!(final_router.route("dynamic://plan.md"),ContentUriRoute::Removed);
            uri_bridge.set_schemes(&ara_rpc::WireValue::parse(&json!([{"scheme":"local","writable":true},{"scheme":"dynamic","immutable":true}]).to_string()).unwrap()).unwrap();
            assert_eq!(final_router.route("local://live.md"),ContentUriRoute::Registered{writable:true});
            assert_eq!(plan.resolve_plan_path("local://live.md").unwrap_err(),"No local file backs plan path local://live.md");
            assert_eq!(plan.resolve_plan_path("dynamic://plan.md").unwrap_err(),"No local file backs plan path dynamic://plan.md");
            drop(trait_router);drop(final_router);
            assert!(final_weak.upgrade().is_none(),"final router must not be retained by a strong plan cycle");
            assert_eq!(plan.resolve_plan_path("fixture://expired.md").unwrap(),registered);

            eprintln!("ARA-PLAN-URI PASS canonical-order-and-owner-snapshot");
            // b91db66e isPlanAutosaveEnabled catches settings.get exceptions.
            // The Host matrix cannot reach this consumer by corrupting global
            // YAML: its earlier Native refresh first rejects that same file.
            // Exercise the real private consumer with a temp project fault.
            // The executor isolates ARA_HOME; this test never mutates env or
            // writes global settings, and creates no public production hook.
            let getter_work = tempfile::tempdir().unwrap();
            let project_settings = getter_work.path().join(".ara").join("config.yml");
            std::fs::create_dir_all(project_settings.parent().unwrap()).unwrap();
            let malformed = "plan: [\n";
            std::fs::write(&project_settings, malformed).unwrap();
            let getter_error = read_plan_autosave_settings(getter_work.path(), &crate::ara_home().join("agent")).unwrap_err();
            assert!(getter_error.starts_with(&format!("parsing native settings {}:", project_settings.display())), "{getter_error}");
            let plan_body = "# Getter fault\nPreserve exact approved bytes.\n";
            autosave_approved_plan(getter_work.path(), "getter fault", plan_body).await.unwrap();
            assert!(!getter_work.path().join(".ara/plans").exists(), "getter exception disables autosave before directory creation");
            assert_eq!(std::fs::read_to_string(&project_settings).unwrap(), malformed);
            // A same-owner valid-settings control proves the real helper can
            // save these bytes; the failure observation is not a no-op facade.
            std::fs::write(&project_settings, "plan:\n  autosave: true\n").unwrap();
            autosave_approved_plan(getter_work.path(), "getter fault", plan_body).await.unwrap();
            let saved_dir = getter_work.path().join(".ara/plans");
            assert_eq!(std::fs::read_to_string(saved_dir.join("GETTER_FAULT_PLAN.md")).unwrap(), plan_body);
            assert_eq!(std::fs::read_dir(saved_dir).unwrap().count(), 1);
            eprintln!("ARA-ACP AUTOSAVE b91db66e PASS getter-error-owner");
            drop(plan);drop(peer);
            transport.join().await.unwrap();
        }).await.expect("bounded plan owner group");
    }
}
