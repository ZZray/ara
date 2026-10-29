//! Complete AST Edit proposal/apply family from fixed OMP 596f2da
//! tools/ast-edit.ts (MIT; THIRD_PARTY_NOTICES.md). Hosts own URI/backing-file
//! mappings. This tool reuses the Session's EditStore and resolution queue.

use crate::{
    ToolContext,
    ast_edit_engine::{self, AstResult},
    paths::{self, SearchScope},
    resolve::{PREVIEW_PENDING_NOTICE, PendingResolution},
};
use ara_agent::{AgentTool, Concurrency, ToolError, ToolOutput, UpdateFn};
use ara_ai::{JsonObject, Tool};
use async_trait::async_trait;
use serde_json::{Value, json};
use std::{
    collections::{BTreeMap, BTreeSet},
    path::{Path, PathBuf},
    sync::Arc,
};
use tokio_util::sync::CancellationToken;

pub const DESCRIPTION: &str = include_str!("ast-edit.md");
pub struct AstEditOsValues {
    pub ops: Vec<(String, String)>,
    pub paths: Vec<String>,
}
pub struct AstEditTool {
    pub ctx: ToolContext,
}
/// Native text leaves awaiting materialization on the original graph driver.
/// Typed compatibility cannot claim to represent an unpaired UTF-16 unit.
pub struct AstWireOutput {
    pub output: ToolOutput,
    pub model_units: Vec<u16>,
    pub display_units: Option<Vec<u16>>,
    pub compatibility_valid: bool,
    pub pending_id: Option<String>,
}

pub fn examples() -> Vec<ara_ai::ToolExample> {
    serde_json::from_value(json!([
        {"caption":"Rename a call site across TypeScript files","call":{"ops":[{"pat":"oldApi($$$ARGS)","out":"newApi($$$ARGS)"}],"paths":["src/**/*.ts"]}},
        {"caption":"Delete matching calls","call":{"ops":[{"pat":"console.log($$$ARGS)","out":""}],"paths":["src/**/*.ts"]}},
        {"caption":"Rewrite import source path","call":{"ops":[{"pat":"import { $$$IMPORTS } from \"old-package\"","out":"import { $$$IMPORTS } from \"new-package\""}],"paths":["src/**/*.ts"]}},
        {"caption":"Modernize to optional chaining (same metavariable enforces identity)","call":{"ops":[{"pat":"$A && $A()","out":"$A?.()"}],"paths":["src/**/*.ts"]}},
        {"caption":"Swap two arguments using captures","call":{"ops":[{"pat":"assertEqual($A, $B)","out":"assertEqual($B, $A)"}],"paths":["tests/**/*.ts"]}},
        {"caption":"Python — convert print calls to logging","call":{"ops":[{"pat":"print($$$ARGS)","out":"logger.info($$$ARGS)"}],"paths":["src/**/*.py"]}}
    ])).expect("fixed authored AST examples")
}

pub fn schema() -> Value {
    json!({"type":"object","properties":{
    "ops":{"type":"array","minItems":1,"items":{"type":"object","properties":{
        "pat":{"type":"string","description":"ast pattern"},"out":{"type":"string","description":"replacement template"}},
        "required":["pat","out"],"additionalProperties":false}},
    "paths":{"type":"array","minItems":1,"items":{"type":"string"}}},
    "required":["ops","paths"],"additionalProperties":false})
}

#[async_trait]
impl AgentTool for AstEditTool {
    fn definition(&self) -> Tool {
        Tool {
            name: "ast_edit".into(),
            description: DESCRIPTION.into(),
            parameters: schema(),
            strict: Some(true),
            examples: Some(examples()),
            ..Default::default()
        }
    }
    fn concurrency(&self, _: &JsonObject) -> Concurrency {
        Concurrency::Shared
    }
    async fn execute(
        &self,
        _: &str,
        args: JsonObject,
        cancel: CancellationToken,
        _: UpdateFn,
    ) -> Result<ToolOutput, ToolError> {
        let ops = args
            .get("ops")
            .and_then(Value::as_array)
            .ok_or_else(|| ToolError("`ops` must include at least one op entry".into()))?
            .iter()
            .enumerate()
            .map(|(i, entry)| {
                let pat = entry
                    .get("pat")
                    .and_then(Value::as_str)
                    .ok_or_else(|| ToolError(format!("`ops[{i}].pat` must be a string")))?;
                let out = entry
                    .get("out")
                    .and_then(Value::as_str)
                    .ok_or_else(|| ToolError(format!("`ops[{i}].out` must be a string")))?;
                Ok((pat.to_owned(), out.to_owned()))
            })
            .collect::<Result<Vec<_>, ToolError>>()?;
        let paths = args
            .get("paths")
            .and_then(Value::as_array)
            .ok_or_else(|| ToolError("`paths` must contain at least one path or glob".into()))?
            .iter()
            .map(|p| p.as_str().map(str::to_owned).ok_or_else(|| ToolError("Scope paths must be strings".into())))
            .collect::<Result<_, _>>()?;
        self.execute_os_values(AstEditOsValues { ops, paths }, cancel).await
    }
}

/// JS parseInt(raw,10), then positive-or-default. ARA keeps the project's
/// intentional PI_* -> ARA_* rename and NAPI get_value_uint32/.max(1) boundary.
pub fn max_files_from_env(raw: Option<&str>) -> usize {
    let Some(raw) = raw else {
        return 1000;
    };
    let raw = raw.trim_start();
    let (negative, raw) = if let Some(rest) = raw.strip_prefix('-') {
        (true, rest)
    } else {
        (false, raw.strip_prefix('+').unwrap_or(raw))
    };
    let digits: String = raw.chars().take_while(char::is_ascii_digit).collect();
    if negative || digits.is_empty() {
        return 1000;
    }
    let value = digits.parse::<f64>().unwrap_or(f64::INFINITY);
    if value <= 0.0 {
        return 1000;
    }
    let native = if value.is_finite() { value.trunc().rem_euclid(4294967296.0) as u32 } else { 0 };
    native.max(1) as usize
}
pub fn validate_ops(ops: &[(String, String)]) -> Result<(), ToolError> {
    for (i, (pat, _)) in ops.iter().enumerate() {
        if pat.is_empty() {
            return Err(ToolError(format!("`ops[{i}].pat` must be a non-empty pattern")));
        }
    }
    if ops.is_empty() {
        return Err(ToolError("`ops` must include at least one op entry".into()));
    }
    let mut seen = BTreeSet::new();
    for (pat, _) in ops {
        if !seen.insert(pat) {
            return Err(ToolError(format!("Duplicate rewrite pattern: {pat}")));
        }
    }
    Ok(())
}

async fn resolve_scope(
    ctx: &ToolContext,
    inputs: &[String],
    cancel: CancellationToken,
) -> Result<SearchScope, ToolError> {
    if inputs.is_empty() {
        return Err(ToolError("`paths` must contain at least one path or glob".into()));
    }
    let normalized: Vec<_> = inputs.iter().map(|p| paths::normalize_path_like_input(p)).collect();
    if normalized.iter().any(String::is_empty) {
        return Err(ToolError("Search scope entries must be non-empty paths or globs".into()));
    }
    let expanded = paths::expand_delimited_entries(ctx, &normalized, paths::Splitter::Search).map_err(ToolError)?;
    let mut resolved = Vec::new();
    for entry in expanded {
        resolved.push(resolve_ast_scope_entry(ctx, &entry, cancel.clone()).await?);
    }
    resolve_prepared_ast_scope(ctx, &resolved)
}

pub fn is_internal_scope_path(entry: &str) -> bool {
    let normalized = entry.strip_prefix('@').unwrap_or(entry);
    ["agent://", "artifact://", "skill://", "rule://", "security://", "local://", "mcp://", "ssh://", "vault://"]
        .iter()
        .any(|prefix| normalized.starts_with(prefix))
        || normalized.starts_with("local:/")
}
pub fn scope_entry_needs_await(ctx: &ToolContext, entry: &str) -> bool {
    regex::Regex::new(r"(?i)^(?:(?:https?|ftp|ws|wss)://|https?:/|www\.|ssh://)")
        .expect("fixed URL regex")
        .is_match(entry)
        || is_internal_scope_path(entry)
        || ctx.content_uri_port().is_some_and(|port| port.route(entry) != crate::ContentUriRoute::Unregistered)
}
fn path_policy(ctx: &ToolContext) -> ara_edit::PathPolicy {
    let state=ctx.edit_host().map(|host|host.path_policy()).unwrap_or_default();
    let mut policy=state.file_policy(&ctx.cwd);
    policy.home_dir=state.home_dir.unwrap_or_else(||std::env::var_os("HOME").map(PathBuf::from).unwrap_or_default());
    policy
}
pub fn resolve_prepared_ast_scope(ctx: &ToolContext, inputs: &[String]) -> Result<SearchScope, ToolError> {
    let resolver = path_policy(ctx);
    let inputs = inputs
        .iter()
        .map(|entry| {
            if entry.get(..7).is_some_and(|s| s.eq_ignore_ascii_case("file://")) {
                resolver
                    .resolve(entry, &std::collections::HashMap::new())
                    .map(|p| crate::internal_urls::search_path_string(&p.absolute))
                    .map_err(|e| ToolError(e.to_string()))
            } else {
                Ok(entry.clone())
            }
        })
        .collect::<Result<Vec<_>, _>>()?;
    paths::resolve_prepared_search_scope(ctx, &inputs, false, false, "").map_err(ToolError)
}

/// One genuine asynchronous backing-path resolution. Native callers resume
/// their original realm iterator only after this exact Host/OS await settles.
pub async fn resolve_ast_scope_entry(
    ctx: &ToolContext,
    entry: &str,
    cancel: CancellationToken,
) -> Result<String, ToolError> {
    let resolver = path_policy(ctx);
    let external = regex::Regex::new(r"(?i)^(?:https?|ftp|ws|wss)://").expect("fixed URL regex");
    let fuzzy = regex::Regex::new(r"(?i)^(?:https?:/|www\.)").expect("URL parser prefix");
    if cancel.is_cancelled() {
        return Err(ToolError("Aborted: Cancelled".into()));
    }
    if entry.is_empty() {
        return Err(ToolError("Search scope entries must be non-empty paths or globs".into()));
    }
    let missing = if fuzzy.is_match(entry) && !paths::has_glob_path_chars(entry) {
        tokio::fs::metadata(ctx.resolve(entry))
            .await
            .err()
            .is_some_and(|e| matches!(e.kind(), std::io::ErrorKind::NotFound | std::io::ErrorKind::NotADirectory))
    } else {
        false
    };
    if external.is_match(&entry) || (fuzzy.is_match(&entry) && !paths::has_glob_path_chars(&entry) && missing) {
        return Err(ToolError(if fuzzy.is_match(entry) {
            format!(
                "Cannot rewrite external URL: {entry}. Use `read` or `search` to inspect fetched web content; ast_edit only applies to local files."
            )
        } else {
            format!(
                "Cannot rewrite external URL: {entry}. Use `read` to fetch web content, then search the returned text."
            )
        }));
    }
    // The same URI owner as Read gets first refusal, including Removed.
    if let Some(port) = ctx.content_uri_port() {
        match port.route(entry) {
            crate::ContentUriRoute::Removed => {
                let scheme = crate::internal_urls::hierarchical_scheme(entry).unwrap_or("");
                return Err(ToolError(format!("Unknown protocol: {}://", scheme.to_ascii_lowercase())));
            }
            crate::ContentUriRoute::Registered { .. } => {
                if entry.get(..6).is_some_and(|p| p.eq_ignore_ascii_case("ssh://")) {
                    return Err(ToolError(format!(
                        "Cannot rewrite a remote ssh:// path (no local file): {entry}. Use `read {entry}` to view it, or use `grep` on a specific remote file."
                    )));
                }
                if paths::has_glob_path_chars(entry) {
                    return Err(ToolError(format!("Glob patterns are not supported for internal URLs: {entry}")));
                }
                let path = port
                    .search_path_only(entry, cancel.clone())
                    .await?
                    .ok_or_else(|| ToolError(format!("Cannot rewrite internal URL without a backing file: {entry}")))?;
                return Ok(crate::internal_urls::search_path_string(&path));
            }
            crate::ContentUriRoute::Unregistered => {}
        }
    }
    if entry.get(..6).is_some_and(|p| p.eq_ignore_ascii_case("ssh://")) {
        return Err(ToolError(format!(
            "Cannot rewrite a remote ssh:// path (no local file): {entry}. Use `read {entry}` to view it, or use `grep` on a specific remote file."
        )));
    }
    if crate::internal_urls::is_internal_url(&entry) {
        if paths::has_glob_path_chars(&entry) {
            return Err(ToolError(format!("Glob patterns are not supported for internal URLs: {entry}")));
        }
        let path = ctx.resolve_internal_url_path_only(&entry).map_err(ToolError)?;
        return Ok(crate::internal_urls::search_path_string(&path));
    } else if entry.starts_with("local://") || entry.starts_with("vault://") || entry.starts_with("file://") {
        if !entry.starts_with("file://") && paths::has_glob_path_chars(&entry) {
            return Err(ToolError(format!("Glob patterns are not supported for internal URLs: {entry}")));
        }
        let path = resolver.resolve(&entry, &crate::edit_urls::native_file_answers(&ctx.edit_host().map(|host|host.path_policy()).unwrap_or_default(),entry)).map_err(|e| ToolError(e.to_string()))?.absolute;
        return Ok(crate::internal_urls::search_path_string(&path));
    } else {
        return Ok(entry.to_owned());
    }
}
fn display_path(cwd: &Path, path: &Path) -> String {
    let path = PathBuf::from(crate::internal_urls::search_path_string(path));
    let cwd = PathBuf::from(crate::internal_urls::search_path_string(cwd));
    paths::format_path_relative_to_cwd(&path, &cwd, false)
}
fn counts(cwd: &Path, result: &AstResult) -> Vec<(String, usize)> {
    result.file_changes.iter().map(|(p, n)| (display_path(cwd, p), *n)).collect()
}
fn details(cwd: &Path, scope: &SearchScope, result: &AstResult, errors: &[String], total: usize, apply: bool) -> Value {
    let files = counts(cwd, result);
    let mut data = json!({"totalReplacements":result.total_replacements,"filesTouched":result.files_touched,
        "filesSearched":result.files_searched,"applied":result.applied,"limitReached":result.limit_reached,"scopePath":scope.scope_path,
        "files":files.iter().map(|(p,_)|p).collect::<Vec<_>>(),"fileReplacements":files.iter().map(|(p,n)|json!({"path":p,"count":n})).collect::<Vec<_>>()});
    if !result.missing_grammars.is_empty() { data["missingGrammars"] = json!(result.missing_grammars); }
    if !apply {
        data["searchPath"] = json!(scope.search_path);
        data["cwd"] = json!(cwd);
    }
    if !errors.is_empty() {
        data["parseErrors"] = json!(errors);
        data["parseErrorsTotal"] = json!(total);
    }
    data
}
fn error_lines(errors: &[String], total: usize) -> Vec<String> {
    if errors.is_empty() {
        return Vec::new();
    }
    let header = if total > errors.len() {
        format!("Parse issues ({} / {total}):", errors.len())
    } else {
        "Parse issues:".into()
    };
    std::iter::once(header).chain(errors.iter().map(|e| format!("- {e}"))).collect()
}
fn tags(cwd: &Path, use_hashlines: bool, store: &ara_edit::EditStore, result: &AstResult) -> BTreeMap<String, String> {
    let mut tags = BTreeMap::new();
    if !use_hashlines {
        return tags;
    }
    for (absolute, _) in &result.file_changes {
        if let Ok(text) = std::fs::read_to_string(absolute) {
            let key = ara_edit::path_policy::canonical_key(absolute);
            let tag = store.record(&key, &ara_edit::text::normalize_to_lf(&text), None);
            tags.insert(display_path(cwd, absolute), tag);
        }
    }
    tags
}
fn units(text: &str) -> Vec<u16> {
    text.encode_utf16().collect()
}
fn first_line(text: &str) -> Vec<u16> {
    text.split('\n').next().unwrap_or("").encode_utf16().take(120).collect()
}
fn join_units(lines: &[Vec<u16>]) -> Vec<u16> {
    let mut result = Vec::new();
    for (i, line) in lines.iter().enumerate() {
        if i > 0 {
            result.push(b'\n' as u16);
        }
        result.extend_from_slice(line);
    }
    result
}

impl AstEditTool {
    pub async fn execute_os_values(
        &self,
        mut input: AstEditOsValues,
        cancel: CancellationToken,
    ) -> Result<ToolOutput, ToolError> {
        if cancel.is_cancelled() {
            return Err(ToolError("Aborted: Cancelled".into()));
        }
        validate_ops(&input.ops)?;
        input.ops.sort_by(|a, b| a.0.cmp(&b.0));
        let max_files = max_files_from_env(std::env::var("ARA_MAX_AST_FILES").ok().as_deref());
        let scope = resolve_scope(&self.ctx, &input.paths, cancel.clone()).await?;
        Ok(self.execute_scope_wire(input.ops,scope,max_files,cancel,true,Some(input.paths)).await?.output)
    }
    pub async fn execute_resolved_scope(
        &self,
        ops: Vec<(String, String)>,
        scope: SearchScope,
        max_files: usize,
        cancel: CancellationToken,
    ) -> Result<ToolOutput, ToolError> {
        let result = self.execute_scope_wire(ops, scope, max_files, cancel, true,None).await?;
        Ok(result.output)
    }
    pub async fn execute_resolved_scope_wire(
        &self,
        ops: Vec<(String, String)>,
        scope: SearchScope,
        max_files: usize,
        cancel: CancellationToken,
    ) -> Result<AstWireOutput, ToolError> {
        self.execute_scope_wire(ops, scope, max_files, cancel, false,None).await
    }
    async fn execute_scope_wire(
        &self,
        ops: Vec<(String, String)>,
        scope: SearchScope,
        max_files: usize,
        cancel: CancellationToken,
        require_typed_compatibility: bool,
        scope_inputs:Option<Vec<String>>,
    ) -> Result<AstWireOutput, ToolError> {
        let run_once = |scope: SearchScope, rewrites: Vec<(String, String)>, token: CancellationToken| async move {
            tokio::task::spawn_blocking(move || ast_edit_engine::run(&scope, &rewrites, true, max_files, token))
                .await.map_err(|error| ToolError(error.to_string()))?
        };
        let mut result = run_once(scope.clone(), ops.clone(), cancel.clone()).await?;
        if crate::grammars::ensure_missing(&self.ctx, &result.missing_grammars, cancel.clone()).await? {
            // Search/preview only, at most one rerun. Apply never enters this
            // path and never replays writes after an uncertain external effect.
            result = run_once(scope.clone(), ops.clone(), cancel).await?;
        }
        self.finish_scope_wire(Some(ops), scope, max_files, result, require_typed_compatibility,scope_inputs)
    }
    pub fn finish_native_preview(
        &self,
        scope: SearchScope,
        max_files: usize,
        result: AstResult,
    ) -> Result<AstWireOutput, ToolError> {
        self.finish_scope_wire(None, scope, max_files, result, false,None)
    }
    pub fn finish_native_preview_with_scope_inputs(&self,scope:SearchScope,max_files:usize,result:AstResult,scope_inputs:Vec<String>)->Result<AstWireOutput,ToolError> {
        self.finish_scope_wire(None,scope,max_files,result,false,Some(scope_inputs))
    }
    fn finish_scope_wire(
        &self,
        ops: Option<Vec<(String, String)>>,
        scope: SearchScope,
        max_files: usize,
        result: AstResult,
        require_typed_compatibility: bool,
        scope_inputs:Option<Vec<String>>,
    ) -> Result<AstWireOutput, ToolError> {
        let (errors, total) = ast_edit_engine::cap_errors(result.parse_errors.clone());
        let mut data = details(&self.ctx.cwd, &scope, &result, &errors, total, false);
        if result.total_replacements == 0 {
            data["fileReplacements"] = json!([]);
            let mut text = vec!["No replacements made".into()];
            text.extend(error_lines(&errors, total));
            if !result.missing_grammars.is_empty() { text.push(crate::grammars::missing_grammars_note(&result.missing_grammars)); }
            let text = text.join("\n");
            return Ok(AstWireOutput {
                model_units: units(&text),
                display_units: None,
                compatibility_valid: true,
                pending_id: None,
                output: ToolOutput::text(text).with_details(data),
            });
        }
        let tags = tags(&self.ctx.cwd, self.ctx.hashlines(), &self.ctx.edit_store, &result);
        let files = counts(&self.ctx.cwd, &result);
        let render = |path: &str, display: bool| {
            let changes: Vec<_> =
                result.changes.iter().filter(|c| display_path(&self.ctx.cwd, &c.path) == path).collect();
            let width = changes.iter().map(|c| c.start_line.to_string().len()).max().unwrap_or(0);
            let mut lines = Vec::new();
            for change in changes {
                for (marker, text) in [("-", &change.before), ("+", &change.after)] {
                    let text = first_line(text);
                    let prefix = if display {
                        format!("{:>width$}│", format!("{marker}{}", change.start_line), width = width + 1)
                    } else if tags.contains_key(path) {
                        format!("{marker}{}:", change.start_line)
                    } else {
                        format!("{marker}{}:{} ", change.start_line, change.start_column)
                    };
                    let mut line = units(&prefix);
                    line.extend(text);
                    lines.push(line);
                }
            }
            lines
        };
        let suffix = |p: &str| {
            format!(
                "{} ({} replacement{})",
                tags.get(p).map(|t| format!("#{t}")).unwrap_or_default(),
                files.iter().find(|(path, _)| path == p).map_or(0, |(_, n)| *n),
                if files.iter().find(|(path, _)| path == p).is_some_and(|(_, n)| *n == 1) { "" } else { "s" }
            )
        };
        let names: Vec<_> = files.iter().map(|(p, _)| p.clone()).collect();
        let (mut model, display) = if scope.is_directory {
            (
                paths::format_grouped_file_values(&names, |p| (render(p, false), suffix(p)), units),
                paths::format_grouped_file_values(&names, |p| (render(p, true), suffix(p)), units),
            )
        } else {
            let mut model = Vec::new();
            let mut display = Vec::new();
            for p in &names {
                if !model.is_empty() {
                    model.push(Vec::new());
                    display.push(Vec::new());
                }
                if let Some(tag) = tags.get(p) {
                    model.push(units(&ara_edit::modes::hashline::format::format_hashline_header(p, tag)));
                }
                model.extend(render(p, false));
                display.extend(render(p, true));
            }
            (model, display)
        };
        if result.limit_reached {
            model.extend([Vec::new(), units("Limit reached; narrow paths.")]);
        }
        if !errors.is_empty() {
            model.push(Vec::new());
            model.extend(error_lines(&errors, total).iter().map(|line| units(line)));
        }
        if !result.missing_grammars.is_empty() {
            model.push(Vec::new()); model.push(units(&crate::grammars::missing_grammars_note(&result.missing_grammars)));
        }
        let display_units = join_units(&display);
        let display_compat = String::from_utf16(&display_units);
        data["displayContent"] = json!(display_compat.as_ref().map(String::as_str).unwrap_or(""));
        let label = format!(
            "AST Edit: {} replacement{} in {} file{}",
            result.total_replacements,
            if result.total_replacements == 1 { "" } else { "s" },
            result.files_touched,
            if result.files_touched == 1 { "" } else { "s" }
        );
        model.splice(0..0, [units(PREVIEW_PENDING_NOTICE), Vec::new()]);
        let model_units = join_units(&model);
        let model_compat = String::from_utf16(&model_units);
        let compatibility_valid = model_compat.is_ok() && display_compat.is_ok();
        // A failed typed result must not leave a new applyable proposal. The
        // Native realm still stages these exact UTF16 units through its port.
        if require_typed_compatibility && !compatibility_valid {
            return Err(ToolError(
                "AST preview contains an unpaired UTF-16 code unit; use the original Native realm output port".into(),
            ));
        }
        let pending_id = self.ctx.pending_resolution.register(
            "ast_edit",
            label,
            Arc::new(StagedAst {
                native: NativeAstStage {
                    context: self.ctx.clone(),
                    scope_inputs,
                    scope,
                    max_files,
                    preview: result,
                },
                rewrites: ops,
            }),
        );
        Ok(AstWireOutput {
            output: ToolOutput::text(model_compat.unwrap_or_default()).with_details(data),
            model_units,
            display_units: Some(display_units),
            compatibility_valid,
            pending_id: Some(pending_id),
        })
    }
}
#[derive(Clone)]
pub struct NativeAstStage {
    context: ToolContext,
    scope_inputs:Option<Vec<String>>,
    pub scope: SearchScope,
    pub max_files: usize,
    preview: AstResult,
}
struct StagedAst {
    native: NativeAstStage,
    rewrites: Option<Vec<(String, String)>>,
}
#[async_trait]
impl PendingResolution for StagedAst {
    fn native_ast_stage(&self) -> Option<NativeAstStage> {
        self.rewrites.is_none().then(|| self.native.clone())
    }
    async fn apply(&self, reason:&str)->Result<ToolOutput,crate::resolve::PendingApplyFault> {
        self.apply_in_context(reason,&self.native.context).await
    }
    async fn apply_in_context(&self,_reason:&str,ctx:&ToolContext)->Result<ToolOutput,crate::resolve::PendingApplyFault> {
        let scope=self.native.fresh_apply_scope(ctx).await.map_err(|e|crate::resolve::PendingApplyFault::Native(e.0))?;
        let rewrites=self.rewrites.clone().ok_or_else(||crate::resolve::PendingApplyFault::Native(
            "Original AST apply requires the current Native write driver".into()))?;
        let max_files=self.native.max_files;
        // Independent apply lifetime: never reuse the preview's cancellation.
        // Frozen scope identifies the reviewed target; current context owns
        // display policy, snapshots and the resolving Write's Session Store.
        let result=tokio::task::spawn_blocking(move ||ast_edit_engine::run(
            &scope,&rewrites,false,max_files,CancellationToken::new()
        )).await.map_err(|e|crate::resolve::PendingApplyFault::Native(e.to_string()))?
         .map_err(|e|crate::resolve::PendingApplyFault::Native(e.0))?;
        Ok(self.native.finish_apply_with_context(result,ctx))
    }
}

impl NativeAstStage {
    /// Preserve the reviewed scope origin while resolving Host URI handlers
    /// from the current Write context, after the preview call has ended.
    pub async fn fresh_apply_scope(&self,ctx:&ToolContext)->Result<SearchScope,ToolError> {
        if let Some(inputs)=&self.scope_inputs {
            let mut fresh=ctx.clone();fresh.cwd=self.context.cwd.clone();
            resolve_scope(&fresh,inputs,CancellationToken::new()).await
        } else {Ok(self.scope.clone())}
    }
    pub fn needs_fresh_apply_scope(&self)->bool {self.scope_inputs.is_some()}
    pub fn finish_apply(&self, result: AstResult) -> ToolOutput {
        self.finish_apply_with_context(result,&self.context)
    }
    pub fn finish_apply_with_context(&self,result:AstResult,ctx:&ToolContext)->ToolOutput {
        let (errors, total) = ast_edit_engine::cap_errors(result.parse_errors.clone());
        let data = details(&ctx.cwd, &self.scope, &result, &errors, total, true);
        let previous = counts(&ctx.cwd, &self.preview);
        let current = counts(&ctx.cwd, &result);
        let stale = result.total_replacements != self.preview.total_replacements
            || result.files_touched != self.preview.files_touched
            || previous.iter().any(|(p, n)| !current.iter().any(|(q, m)| p == q && n == m))
            || current.iter().any(|(p, n)| !previous.iter().any(|(q, m)| p == q && n == m));
        let n = self.preview.total_replacements;
        let f = self.preview.files_touched;
        let mut text = if stale {
            if result.total_replacements == 0 {
                format!(
                    "Preview is stale / no longer matches; no replacements were applied. Preview expected {n} replacement{} in {f} file{}.",
                    if n == 1 { "" } else { "s" },
                    if f == 1 { "" } else { "s" }
                )
            } else if result.total_replacements < n {
                format!(
                    "Preview is stale / no longer matches; only {} of {n} replacements were applied in {} of {f} files.",
                    result.total_replacements, result.files_touched
                )
            } else {
                format!(
                    "Preview is stale / no longer matches; applied {} replacements but preview expected {n}.",
                    result.total_replacements
                )
            }
        } else {
            format!(
                "Applied {} replacement{} in {} file{}.",
                result.total_replacements,
                if result.total_replacements == 1 { "" } else { "s" },
                result.files_touched,
                if result.files_touched == 1 { "" } else { "s" }
            )
        };
        let fresh = tags(&ctx.cwd, ctx.hashlines(), &ctx.edit_store, &result);
        let mut lines = vec![text];
        if !result.missing_grammars.is_empty() { lines.push(crate::grammars::missing_grammars_note(&result.missing_grammars)); }
        lines.extend(current.iter().filter_map(|(p, _)| {
            fresh.get(p).map(|t| ara_edit::modes::hashline::format::format_hashline_header(p, t))
        }));
        let mut output = ToolOutput::text(lines.join("\n")).with_details(data);
        output.is_error = stale;
        output
    }
}

#[cfg(test)]
#[path = "ast_edit_tests.rs"]
mod tests;

#[cfg(test)]
#[path = "grammar_upgrade_tests.rs"]
mod grammar_upgrade_tests;
