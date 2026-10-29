//! Structural search (OMP `packages/coding-agent/src/tools/ast-grep.ts` and
//! `crates/pi-natives/src/ast.rs` at 596f2da7101178214aa27a753529d15e6b7ad91d).
//!
//! This is an opt-in, read-only tool. Its supported scope is local files,
//! directories, globs, delimited path lists, loaded `skill://` paths and
//! registered Host URI backing paths. The TUI renderer is a separate owner.
//! ARA rejects `skip > 10_000` to bound retained search memory; fixed OMP
//! accepts any finite non-negative skip.

use crate::engine;
use crate::internal_urls;
use crate::paths::{self, SearchScope};
use crate::{DEFAULT_MAX_BYTES, ToolContext};
use ara_agent::{AgentTool, ToolError, ToolOutput, UpdateFn};
use ara_ai::{JsonObject, Tool};
use ara_ast::{SupportLang, ops};
use ara_walk::{self, Budget, EntryKind, Visit, WalkError, WalkOptions};
use ast_grep_core::{MatchStrictness, matcher::Pattern, tree_sitter::LanguageExt};
use async_trait::async_trait;
use serde_json::{Value, json};
use std::cmp::Ordering;
use std::collections::{BinaryHeap, HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};
use tokio_util::sync::CancellationToken;

pub const LIMIT: usize = 50;
const PARSE_ERRORS_LIMIT: usize = 20;
const TIMEOUT: Duration = Duration::from_secs(30);
// A model can request an arbitrarily large page. Bound retained match memory.
const MAX_SKIP: usize = 10_000;

pub const DESCRIPTION: &str = r"Structural code search: use when syntax shape matters more than text.

<instruction>
- Narrow each call to one language. `pat` is one AST pattern; use separate calls for unrelated patterns.
- `$NAME` captures one node; `$_` matches one node without binding; `$$$NAME` captures zero or more nodes. Use `$$$NAME`, not `$$NAME`.
- Repeated metavariables must match the same code. Patterns must parse as one AST node; wrap fragments in their language's enclosing construct when needed.
- C++ expression-statement calls need a trailing semicolon, for example `ns::doThing($ARG);`.
- `path` accepts a file, directory, glob, loaded `skill://` URL, or semicolon-delimited list. Results have 50 matches per page; use `skip` for the next page (maximum 10,000).
</instruction>

<critical>
- Narrow `path` before searching. Avoid a repository-root scan when a subsystem is known.
- Parse issues mean the query may be wrong or mis-scoped; fix the pattern or path before concluding absence.
</critical>";

/// Native action inputs; pattern is the result of the original trim Call.
pub struct AstGrepOsValues {
    pub pattern: String,
    pub language: Option<String>,
    pub path: Option<String>,
    pub skip: f64,
}

/// Actual Native reads remain on the original Graph driver at call sites.
/// Only language scalars cross this port; typed callers supply a frozen value.
#[async_trait]
pub trait AstGrepLanguagePort:Send+Sync {
    async fn language(&self,explicit_acquisition:bool)->Result<Option<String>,ToolError>;
}
pub struct AstGrepTool {
    pub ctx: ToolContext,
}

#[derive(Clone, Debug, Eq, PartialEq, Ord, PartialOrd)]
struct MatchKey {
    path: String,
    start_line: usize,
    start_column: usize,
    end_line: usize,
    end_column: usize,
    byte_start: usize,
    byte_end: usize,
    sequence: u64,
}

#[derive(Debug)]
struct FoundMatch {
    key: MatchKey,
    absolute: PathBuf,
    text: String,
    meta: Vec<(String, String)>,
}

impl PartialEq for FoundMatch {
    fn eq(&self, other: &Self) -> bool {
        self.key == other.key
    }
}
impl Eq for FoundMatch {}
impl PartialOrd for FoundMatch {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}
impl Ord for FoundMatch {
    fn cmp(&self, other: &Self) -> Ordering {
        self.key.cmp(&other.key)
    }
}

#[derive(Default)]
struct SearchResult {
    retained: BinaryHeap<FoundMatch>,
    total_matches: u64,
    files_with_matches: HashSet<PathBuf>,
    files_searched: u64,
    parse_errors: Vec<String>,
    parse_errors_total: u64,
    seen_errors: HashSet<String>,
    sequence: u64,
    seen_files: HashSet<PathBuf>,
    compiled: HashMap<SupportLang, Result<Pattern, String>>,
    missing_grammars: Vec<String>,
    language: Option<String>,
}

impl SearchResult {
    fn error(&mut self, message: String) {
        if !self.seen_errors.insert(message.clone()) {
            return;
        }
        self.parse_errors_total = self.parse_errors_total.saturating_add(1);
        if self.parse_errors.len() < PARSE_ERRORS_LIMIT {
            self.parse_errors.push(message);
        }
    }

    fn file(
        &mut self,
        absolute: &Path,
        pattern: &str,
        ctx: &ToolContext,
        capacity: usize,
        budget: &Budget,
    ) -> Result<(), ToolError> {
        budget.check().map_err(walk_error)?;
        let absolute = crate::normalize(absolute);
        if !self.seen_files.insert(absolute.clone()) || !ops::is_supported_file(&absolute, self.language.as_deref()) {
            return Ok(());
        }
        // The walker canonicalizes roots. Windows may add a `\\?\` prefix,
        // which is a filesystem spelling rather than a useful model path.
        let display_path = PathBuf::from(internal_urls::search_path_string(&absolute));
        let display_cwd = PathBuf::from(internal_urls::search_path_string(&ctx.cwd));
        let display = paths::format_path_relative_to_cwd(&display_path, &display_cwd, false);
        let language = match ops::resolve_language(self.language.as_deref(), &absolute) {
            Ok(language) => language,
            Err(error) => {
                self.files_searched=self.files_searched.saturating_add(1);
                self.error(format!("{display}: {error}"));
                return Ok(());
            }
        };
        if language.wasm_grammar().is_some_and(|grammar| !grammar.is_installed()) {
            let name=language.canonical_name().to_owned();
            if !self.missing_grammars.contains(&name) {self.missing_grammars.push(name);}
            return Ok(());
        }
        self.files_searched = self.files_searched.saturating_add(1);
        let compiled = self
            .compiled
            .entry(language)
            .or_insert_with(|| {
                ops::compile_pattern(pattern, None, &MatchStrictness::Smart, language)
                    .map_err(|error| error.to_string())
            })
            .clone();
        let compiled = match compiled {
            Ok(compiled) => compiled,
            Err(error) => {
                self.error(format!("{display}: {error}"));
                return Ok(());
            }
        };
        let source = match std::fs::read_to_string(&absolute) {
            Ok(source) => source,
            Err(error) => {
                self.error(format!("{display}: {error}"));
                return Ok(());
            }
        };
        budget.check().map_err(walk_error)?;
        let ast = language.ast_grep(source);
        if ast.root().dfs().any(|node| node.is_error()) {
            self.error(format!("{display}: parse error (syntax tree contains error nodes)"));
        }
        let mut found_in_file = false;
        for matched in ast.root().find_all(compiled) {
            budget.check().map_err(walk_error)?;
            self.total_matches = self.total_matches.saturating_add(1);
            if !found_in_file {
                self.files_with_matches.insert(absolute.clone());
                found_in_file = true;
            }
            let range = matched.range();
            let start = matched.start_pos();
            let end = matched.end_pos();
            let key = MatchKey {
                path: display.clone(),
                start_line: start.line() + 1,
                start_column: start.column(matched.get_node()) + 1,
                end_line: end.line() + 1,
                end_column: end.column(matched.get_node()) + 1,
                byte_start: range.start,
                byte_end: range.end,
                sequence: self.sequence,
            };
            self.sequence = self.sequence.saturating_add(1);
            if self.retained.len() >= capacity && self.retained.peek().is_some_and(|worst| key >= worst.key) {
                continue;
            }
            let mut meta: Vec<(String, String)> =
                HashMap::<String, String>::from(matched.get_env().clone()).into_iter().collect();
            meta.sort_by(|left, right| left.0.cmp(&right.0));
            self.retained.push(FoundMatch { key, absolute: absolute.clone(), text: matched.text().into_owned(), meta });
            if self.retained.len() > capacity {
                self.retained.pop();
            }
        }
        Ok(())
    }
}

fn walk_error(error: WalkError) -> ToolError {
    match error {
        WalkError::Timeout => ToolError("AST grep timed out after 30s; narrow `path`".into()),
        WalkError::Aborted => ToolError("AST grep was aborted".into()),
    }
}

fn search_target(
    target: &Path,
    glob: Option<&str>,
    pattern: &str,
    ctx: &ToolContext,
    capacity: usize,
    budget: &Budget,
    result: &mut SearchResult,
) -> Result<(), ToolError> {
    let metadata = std::fs::metadata(target)
        .map_err(|error| ToolError(format!("Path not found: {} ({error})", target.display())))?;
    if metadata.is_file() {
        return result.file(target, pattern, ctx, capacity, budget);
    }
    if !metadata.is_dir() {
        return Err(ToolError(format!("Search path must be a file or directory: {}", target.display())));
    }
    let matcher = glob
        .map(|g| engine::compile_glob(&engine::build_glob_pattern(g, false)))
        .transpose()
        .map_err(ToolError::from)?;
    let options = WalkOptions {
        include_hidden: true,
        use_gitignore: true,
        skip_node_modules: !glob.is_some_and(|g| g.contains("node_modules")),
        max_depth: usize::MAX,
    };
    let mut failure = None;
    ara_walk::walk(target, options, budget, |entry| {
        if entry.kind == EntryKind::File
            && matcher.as_ref().is_none_or(|g| g.is_match(&entry.relative))
            && let Err(error) = result.file(&entry.path, pattern, ctx, capacity, budget)
        {
            failure = Some(error);
            return Visit::Stop;
        }
        Visit::Continue
    })
    .map_err(walk_error)?;
    if let Some(error) = failure {
        return Err(error);
    }
    Ok(())
}

fn resolve_scope(ctx: &ToolContext, raw_path: Option<&str>) -> Result<(SearchScope, Vec<PathBuf>), ToolError> {
    let mut entries = paths::to_path_list(raw_path);
    if entries.is_empty() {
        entries.push(".".into());
    }
    let entries = paths::expand_delimited_entries(ctx, &entries, paths::Splitter::Search).map_err(ToolError)?;
    #[cfg(windows)]
    let search_ctx = entries.iter().any(|entry| internal_urls::is_internal_url(entry)).then(|| {
        let mut normalized = ctx.clone();
        normalized.cwd = PathBuf::from(internal_urls::search_path_string(&ctx.cwd));
        normalized
    });
    #[cfg(not(windows))]
    let search_ctx: Option<ToolContext> = None;
    let search_ctx = search_ctx.as_ref().unwrap_or(ctx);
    let mut immutable = Vec::new();
    let mut searchable = Vec::new();
    for entry in entries {
        if internal_urls::is_internal_url(&entry) {
            let (url, selector) = internal_urls::split_skill_url_selector(&entry);
            if selector.is_some() {
                return Err(ToolError(format!("Line selectors are not supported for ast_grep: {entry}")));
            }
            let absolute = search_ctx.resolve_internal_url_path_only(&url).map_err(ToolError)?;
            immutable.push(std::fs::canonicalize(&absolute).unwrap_or(absolute.clone()));
            searchable.push(internal_urls::search_path_string(&absolute));
        } else if entry.contains("://") {
            return Err(ToolError(format!("Unsupported search URL: {entry}")));
        } else {
            searchable.push(entry);
        }
    }
    let scope = paths::resolve_search_scope(search_ctx, &searchable).map_err(ToolError)?;
    Ok((scope, immutable))
}

fn parse_skip(args: &JsonObject) -> Result<usize, ToolError> {
    match args.get("skip") {
        None | Some(Value::Null) => Ok(0),
        Some(value) => match value.as_f64() {
            Some(number) if number.is_finite() && number >= 0.0 && number < MAX_SKIP as f64 + 1.0 => {
                Ok(number.floor() as usize)
            }
            _ => Err(ToolError(format!("skip must be a non-negative number up to {MAX_SKIP}"))),
        },
    }
}

fn render_parse_errors(errors: &[String], total: u64) -> String {
    if errors.is_empty() {
        return String::new();
    }
    let label = if total > errors.len() as u64 {
        format!("Parse issues ({} / {total}):", errors.len())
    } else {
        "Parse issues:".to_string()
    };
    format!("{label}\n{}", errors.iter().map(|error| format!("- {error}")).collect::<Vec<_>>().join("\n"))
}

impl AstGrepTool {
    async fn resolve_host_paths(
        &self,
        raw_path: Option<&str>,
        cancel: CancellationToken,
    ) -> Result<(Option<String>, Vec<PathBuf>), ToolError> {
        let Some(raw_path) = raw_path else {
            return Ok((None, Vec::new()));
        };
        let Some(port) = self.ctx.content_uri_port() else {
            return Ok((Some(raw_path.into()), Vec::new()));
        };
        let entries = paths::to_path_list(Some(raw_path));
        let entries =
            paths::expand_delimited_entries(&self.ctx, &entries, paths::Splitter::Search).map_err(ToolError)?;
        let mut resolved = Vec::new();
        let mut changed = false;
        let mut immutable = Vec::new();
        for entry in entries {
            if cancel.is_cancelled() {
                return Err(ToolError("AST grep was aborted".into()));
            }
            match port.route(&entry) {
                crate::ContentUriRoute::Unregistered => resolved.push(entry),
                crate::ContentUriRoute::Removed => {
                    let scheme = internal_urls::hierarchical_scheme(&entry).unwrap_or("");
                    return Err(ToolError(format!("Unknown protocol: {}://", scheme.to_ascii_lowercase())));
                }
                crate::ContentUriRoute::Registered { writable } => {
                    let (url, selector) = internal_urls::split_content_url_selector(&entry);
                    if selector.is_some() {
                        return Err(ToolError(format!("Line selectors are not supported for ast_grep: {entry}")));
                    }
                    if paths::has_glob_path_chars(&url) {
                        return Err(ToolError(format!("Glob patterns are not supported for internal URLs: {entry}")));
                    }
                    let path = port.search_path_only(&url, cancel.clone()).await?.ok_or_else(|| {
                        ToolError(format!("Cannot search internal URL without a backing file: {entry}"))
                    })?;
                    if !writable {
                        immutable.push(tokio::fs::canonicalize(&path).await.unwrap_or_else(|_| path.clone()));
                    }
                    resolved.push(internal_urls::search_path_string(&path));
                    changed = true;
                }
            }
        }
        Ok((
            Some(if changed {
                serde_json::to_string(&resolved).map_err(|error| ToolError(error.to_string()))?
            } else {
                raw_path.into()
            }),
            immutable,
        ))
    }
    #[cfg(test)]
    fn execute_blocking(&self, args: &JsonObject, cancel: &CancellationToken) -> Result<ToolOutput, ToolError> {
        let pattern = args.get("pat").and_then(Value::as_str).unwrap_or_default().trim();
        if pattern.is_empty() {
            return Err(ToolError("`pat` must be a non-empty pattern".into()));
        }
        let skip = parse_skip(args)?;
        self.execute_prepared_os_values(pattern, args.get("path").and_then(Value::as_str), skip, &[], cancel)
    }

    /// The Host observes pat.trim/skip/path before entering this action owner.
    pub async fn execute_os_values(
        &self,
        input: AstGrepOsValues,
        cancel: CancellationToken,
    ) -> Result<ToolOutput, ToolError> {
        self.execute_os_values_with_language(input,cancel,None).await
    }
    pub async fn execute_os_values_with_language(
        &self,mut input:AstGrepOsValues,cancel:CancellationToken,language_port:Option<std::sync::Arc<dyn AstGrepLanguagePort>>,
    )->Result<ToolOutput,ToolError> {
        if input.pattern.is_empty() {
            return Err(ToolError("`pat` must be a non-empty pattern".into()));
        }
        if !input.skip.is_finite() || input.skip < 0.0 || input.skip >= MAX_SKIP as f64 + 1.0 {
            return Err(ToolError(format!("skip must be a non-negative number up to {MAX_SKIP}")));
        }
        let (path, immutable) = self.resolve_host_paths(input.path.as_deref(), cancel.clone()).await?;
        input.path = path;
        let tool=Self {ctx:self.ctx.clone()};let raw_path=input.path.clone();
        let (scope,mut resolved_immutable)=tokio::task::spawn_blocking(move ||resolve_scope(&tool.ctx,raw_path.as_deref()))
            .await.map_err(|error|ToolError(format!("AST grep failed: {error}")))??;
        resolved_immutable.extend(immutable);
        // The full scope await settles before the original `params.lang` Gets.
        let explicit=match &language_port {Some(port)=>port.language(true).await?,None=>input.language.clone()};
        // Explicit language acquisition precedes searching; a search still
        // aggregates every missing language and retries only once afterward.
        if let Some(language)=&explicit {
            crate::grammars::ensure_missing(&self.ctx,std::slice::from_ref(language),cancel.clone()).await?;
        }
        let mut retries=0;
        loop {
            let tool=Self {ctx:self.ctx.clone()};
            let pattern=input.pattern.clone();let scope=scope.clone();
            let language=match &language_port {Some(port)=>port.language(false).await?,None=>input.language.clone()};
            let immutable=resolved_immutable.clone();
            let skip=input.skip.floor() as usize;let work=cancel.child_token();
            let _stop_on_drop=work.clone().drop_guard();
            let searched=tokio::task::spawn_blocking(move ||
                tool.search_resolved_scope(scope,&pattern,skip,immutable,language.as_deref(),&work)
            ).await.map_err(|error|ToolError(format!("AST grep failed: {error}")))??;
            if cancel.is_cancelled() {return Err(ToolError("AST grep was aborted".into()));}
            if retries==0 && crate::grammars::ensure_missing(&self.ctx,&searched.2.missing_grammars,cancel.clone()).await? {
                retries+=1;continue;
            }
            // Render/seen recording has no further await or detached worker:
            // it follows the final cancellation gate on this action owner.
            return self.render_search_result(searched.0,searched.1,searched.2,skip);
        }
    }

    fn execute_prepared_os_values(
        &self,
        pattern: &str,
        raw_path: Option<&str>,
        skip: usize,
        host_immutable: &[PathBuf],
        cancel: &CancellationToken,
    ) -> Result<ToolOutput, ToolError> {
        let (scope,immutable,result)=self.search_prepared_os_values(pattern,raw_path,skip,host_immutable,None,cancel)?;
        self.render_search_result(scope,immutable,result,skip)
    }
    fn search_prepared_os_values(
        &self,pattern:&str,raw_path:Option<&str>,skip:usize,host_immutable:&[PathBuf],language:Option<&str>,cancel:&CancellationToken,
    )->Result<(SearchScope,Vec<PathBuf>,SearchResult),ToolError> {
        let (scope, mut immutable) = resolve_scope(&self.ctx, raw_path)?;
        immutable.extend_from_slice(host_immutable);
        self.search_resolved_scope(scope,pattern,skip,immutable,language,cancel)
    }
    fn search_resolved_scope(
        &self,scope:SearchScope,pattern:&str,skip:usize,immutable:Vec<PathBuf>,language:Option<&str>,cancel:&CancellationToken,
    )->Result<(SearchScope,Vec<PathBuf>,SearchResult),ToolError> {
        let capacity = skip + LIMIT + 1;
        let budget = Budget { cancel: cancel.clone(), deadline: Instant::now() + TIMEOUT };
        let language=language.map(str::trim).filter(|value|!value.is_empty());
        let mut result = SearchResult {language:language.map(str::to_owned),..Default::default()};
        if let Some(files) = &scope.exact_file_paths {
            for file in files {
                result.file(file, pattern, &self.ctx, capacity, &budget)?;
            }
            result.missing_grammars.sort();
        } else if let Some(targets) = &scope.multi_targets {
            for target in targets {
                let missing_start=result.missing_grammars.len();
                search_target(
                    &target.base,
                    target.glob.as_deref(),
                    pattern,
                    &self.ctx,
                    capacity,
                    &budget,
                    &mut result,
                )?;
                // Native target output is sorted; TS keeps a stable union
                // across targets rather than globally sorting all targets.
                result.missing_grammars[missing_start..].sort();
            }
        } else {
            search_target(
                &scope.search_path,
                scope.glob.as_deref(),
                pattern,
                &self.ctx,
                capacity,
                &budget,
                &mut result,
            )?;
            result.missing_grammars.sort();
        }
        Ok((scope,immutable,result))
    }
    fn render_search_result(&self,scope:SearchScope,immutable:Vec<PathBuf>,result:SearchResult,skip:usize)->Result<ToolOutput,ToolError> {
        let grammar_note=(!result.missing_grammars.is_empty()).then(||crate::grammars::missing_grammars_note(&result.missing_grammars));
        let mut retained = result.retained.into_sorted_vec();
        let visible = if skip < retained.len() { retained.split_off(skip) } else { Vec::new() };
        let limit_reached = visible.len() > LIMIT;
        let visible: Vec<_> = visible.into_iter().take(LIMIT).collect();
        let mut files = Vec::<String>::new();
        let mut by_file: HashMap<String, Vec<FoundMatch>> = HashMap::new();
        for found in visible {
            let name = found.key.path.clone();
            if !by_file.contains_key(&name) {
                files.push(name.clone());
            }
            by_file.entry(name).or_default().push(found);
        }
        let mut details = json!({
            "matchCount": result.total_matches,
            "fileCount": result.files_with_matches.len(),
            "filesSearched": result.files_searched,
            "limitReached": limit_reached,
            "scopePath": scope.scope_path,
            "files": files,
            "fileMatches": files.iter().map(|path| json!({"path": path, "count": by_file[path].len()})).collect::<Vec<_>>(),
        });
        if !result.missing_grammars.is_empty() {details["missingGrammars"]=json!(result.missing_grammars);}
        if result.parse_errors_total > 0 {
            details["parseErrors"] = json!(result.parse_errors);
            details["parseErrorsTotal"] = json!(result.parse_errors_total);
        }
        let parse_text = render_parse_errors(&result.parse_errors, result.parse_errors_total);
        if files.is_empty() {
            let lead = if result.parse_errors_total > 0 {
                "No matches found. Parse issues mean the query may be mis-scoped; narrow `path` before concluding absence."
            } else {
                "No matches found"
            };
            let text = if parse_text.is_empty() { lead.to_string() } else { format!("{lead}\n{parse_text}") };
            let text=if let Some(note)=grammar_note {format!("{text}\n{note}")} else {text};
            return Ok(ToolOutput::text(text).with_details(details));
        }
        let mut bodies: HashMap<String, Vec<String>> = HashMap::new();
        let mut tags: HashMap<String, (PathBuf, String)> = HashMap::new();
        for file in &files {
            let matches = &by_file[file];
            let absolute = &matches[0].absolute;
            let canonical = std::fs::canonicalize(absolute).unwrap_or_else(|_| absolute.clone());
            let immutable = immutable.iter().any(|root| canonical.starts_with(root));
            let tag =
                if self.ctx.hashlines() && !immutable { self.ctx.edit_store.record_file(absolute, None) } else { None };
            let mut body = Vec::new();
            for found in matches {
                for (line_index, line) in found.text.split('\n').enumerate() {
                    body.push(crate::grep::format_match_line(
                        (found.key.start_line + line_index) as u64,
                        line,
                        line_index == 0,
                        tag.is_some(),
                    ));
                }
                if !found.meta.is_empty() {
                    body.push(format!(
                        "  meta: {}",
                        found.meta.iter().map(|(name, value)| format!("{name}={value}")).collect::<Vec<_>>().join(", ")
                    ));
                }
            }
            if let Some(tag) = tag {
                tags.insert(file.clone(), (absolute.clone(), tag));
            }
            bodies.insert(file.clone(), body);
        }
        let grouped = scope.is_directory || scope.exact_file_paths.is_some() || scope.multi_targets.is_some();
        let mut output = if grouped {
            paths::format_grouped_files(&files, |file| {
                (bodies[file].clone(), tags.get(file).map(|(_, tag)| format!("#{tag}")).unwrap_or_default())
            })
            .join("\n")
        } else {
            let file = &files[0];
            let mut lines = Vec::new();
            if let Some((_, tag)) = tags.get(file) {
                lines.push(ara_edit::modes::hashline::format::format_hashline_header(file, tag));
            }
            lines.extend(bodies[file].iter().cloned());
            lines.join("\n")
        };
        if limit_reached {
            output.push_str("\n\nResult limit reached; narrow path or use `skip` for the next page.");
        }
        if !parse_text.is_empty() {
            output.push_str("\n\n");
            output.push_str(&parse_text);
        }
        if let Some(note)=grammar_note {output.push_str("\n\n");output.push_str(&note);}
        let truncated = crate::output::truncate_head(&output, usize::MAX, DEFAULT_MAX_BYTES);
        // The formatter can reorder grouped files. If output is truncated, a
        // partially displayed body cannot safely be attributed to a file, so
        // leave its snapshot unmarked rather than claim unseen lines were read.
        if !truncated.truncated {
            for (file, (absolute, tag)) in tags {
                let seen = ara_edit::store::seen_lines_from_body(&bodies[&file].join("\n"));
                if !seen.is_empty() {
                    self.ctx.edit_store.record_seen_lines(
                        &ara_edit::path_policy::canonical_key(&absolute),
                        &tag,
                        &seen,
                    );
                }
            }
        }
        if truncated.truncated {
            details["truncation"] = truncated.details();
        }
        let mut notice = crate::output::Notice::default();
        notice.truncation(&truncated);
        Ok(ToolOutput::text(format!("{}{}", truncated.content, notice.render())).with_details(details))
    }
}

#[async_trait]
impl AgentTool for AstGrepTool {
    fn definition(&self) -> Tool {
        Tool {
            name: "ast_grep".into(),
            description: DESCRIPTION.into(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "pat": {"type": "string", "description": "AST pattern"},
                    "lang": {"type": "string", "description": "language override (alias)"},
                    "path": {"type": "string", "description": "file, directory, glob, or internal URL; several as semicolon-delimited list"},
                    "skip": {"type": "number", "description": "matches to skip"}
                },
                "required": ["pat"],
                "additionalProperties": false
            }),
            ..Default::default()
        }
    }

    async fn execute(
        &self,
        _id: &str,
        args: JsonObject,
        cancel: CancellationToken,
        _update: UpdateFn,
    ) -> Result<ToolOutput, ToolError> {
        let pattern = args.get("pat").and_then(Value::as_str).unwrap_or_default().trim().to_owned();
        if pattern.is_empty() {
            return Err(ToolError("`pat` must be a non-empty pattern".into()));
        }
        let skip = parse_skip(&args)?;
        self.execute_os_values(
            AstGrepOsValues {
                pattern,
                language: args.get("lang").and_then(Value::as_str).map(str::to_owned),
                path: args.get("path").and_then(Value::as_str).map(str::to_owned),
                skip: skip as f64,
            },
            cancel,
        )
        .await
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn call(ctx: &ToolContext, args: Value) -> Result<ToolOutput, ToolError> {
        AstGrepTool { ctx: ctx.clone() }.execute_blocking(args.as_object().unwrap(), &CancellationToken::new())
    }

    fn body(output: &ToolOutput) -> String {
        output
            .content
            .iter()
            .filter_map(|block| match block {
                ara_ai::UserBlock::Text(content) => Some(content.text.as_str()),
                _ => None,
            })
            .collect::<Vec<_>>()
            .join("\n")
    }

    #[test]
    fn structural_match_and_global_paging() {
        let temp = tempfile::tempdir().unwrap();
        std::fs::create_dir(temp.path().join("a")).unwrap();
        std::fs::create_dir(temp.path().join("z")).unwrap();
        std::fs::write(temp.path().join("a/early.ts"), "console.log(one);\nconsole.log(two);\n").unwrap();
        std::fs::write(temp.path().join("z/late.ts"), "console.log(three);\nconsole.log(four);\n").unwrap();
        let ctx = ToolContext::new(temp.path());
        let output = call(&ctx, json!({"pat": "console.log($X)", "path": "a;z", "skip": 2})).unwrap();
        assert_eq!(output.details.as_ref().unwrap()["matchCount"], 4);
        assert_eq!(output.details.as_ref().unwrap()["fileCount"], 2);
        assert!(body(&output).contains("late.ts"));
        assert!(!body(&output).contains("early.ts"));
        assert!(body(&output).contains("meta: X=three"));
    }

    #[test]
    fn parse_error_cap_and_no_match_guidance() {
        let temp = tempfile::tempdir().unwrap();
        for i in 0..21 {
            std::fs::write(temp.path().join(format!("broken-{i}.ts")), "export function broken( { return 1; }")
                .unwrap();
        }
        let output = call(&ToolContext::new(temp.path()), json!({"pat": "missingCall($X)"})).unwrap();
        let details = output.details.as_ref().unwrap();
        assert_eq!(details["matchCount"], 0);
        assert_eq!(details["parseErrorsTotal"], 21);
        assert_eq!(details["parseErrors"].as_array().unwrap().len(), 20);
        assert!(body(&output).contains("Parse issues mean the query may be mis-scoped"));
    }

    #[test]
    fn glob_and_validation() {
        let temp = tempfile::tempdir().unwrap();
        std::fs::write(temp.path().join("code.ts"), "console.log(1);\n").unwrap();
        std::fs::write(temp.path().join("other.js"), "console.log(2);\n").unwrap();
        let ctx = ToolContext::new(temp.path());
        let output = call(&ctx, json!({"pat": "console.log($X)", "path": "**/*.ts"})).unwrap();
        assert_eq!(output.details.as_ref().unwrap()["matchCount"], 1);
        assert!(call(&ctx, json!({"pat": " "})).is_err());
        assert!(call(&ctx, json!({"pat": "console.log($X)", "skip": -1})).is_err());
    }

    #[test]
    fn page_limit_preserves_total_and_cancellation_stops_search() {
        let temp = tempfile::tempdir().unwrap();
        let source = (0..70).map(|i| format!("console.log({i});")).collect::<Vec<_>>().join("\n");
        std::fs::write(temp.path().join("many.ts"), source).unwrap();
        let ctx = ToolContext::new(temp.path());
        let first = call(&ctx, json!({"pat": "console.log($X)"})).unwrap();
        assert_eq!(first.details.as_ref().unwrap()["matchCount"], 70);
        assert_eq!(first.details.as_ref().unwrap()["limitReached"], true);
        assert_eq!(body(&first).matches("meta: X=").count(), 50);
        let next = call(&ctx, json!({"pat": "console.log($X)", "skip": 50})).unwrap();
        assert_eq!(next.details.as_ref().unwrap()["matchCount"], 70);
        assert_eq!(next.details.as_ref().unwrap()["limitReached"], false);
        assert_eq!(body(&next).matches("meta: X=").count(), 20);
        let cancel = CancellationToken::new();
        cancel.cancel();
        let failure =
            AstGrepTool { ctx }.execute_blocking(json!({"pat": "console.log($X)"}).as_object().unwrap(), &cancel);
        assert!(failure.unwrap_err().0.contains("aborted"));
    }

    #[test]
    fn skill_url_is_read_only_and_regular_match_records_seen_line() {
        let temp = tempfile::tempdir().unwrap();
        let skill = temp.path().join("SKILL.md");
        let script = temp.path().join("sample.ts");
        std::fs::write(&skill, "Skill\n").unwrap();
        std::fs::write(&script, "console.log(value);\n").unwrap();
        let ctx = ToolContext::new(temp.path()).with_skills(vec![internal_urls::SkillRef {
            name: "demo".into(),
            file_path: skill,
            base_dir: temp.path().to_path_buf(),
        }]);
        let regular = call(&ctx, json!({"pat": "console.log($X)", "path": "sample.ts"})).unwrap();
        let text = body(&regular);
        assert!(text.contains("#"));
        assert!(text.contains("*1:console.log(value)"));
        let snapshot = ctx.edit_store.head(&ara_edit::path_policy::canonical_key(&script)).unwrap();
        assert!(snapshot.seen_lines.as_ref().is_some_and(|lines| lines.contains(&1)));

        let fresh = ToolContext::new(temp.path()).with_skills(vec![internal_urls::SkillRef {
            name: "demo".into(),
            file_path: temp.path().join("SKILL.md"),
            base_dir: temp.path().to_path_buf(),
        }]);
        let immutable = call(&fresh, json!({"pat": "console.log($X)", "path": "skill://demo/sample.ts"})).unwrap();
        assert_eq!(immutable.details.as_ref().unwrap()["matchCount"], 1);
        assert!(body(&immutable).contains("*1|console.log(value)"));
        assert!(fresh.edit_store.head(&ara_edit::path_policy::canonical_key(&script)).is_none());
    }

    #[test]
    fn directory_output_uses_grouped_paths_and_hashline_suffixes() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("packages/pkg/src");
        std::fs::create_dir_all(root.join("nested")).unwrap();
        std::fs::write(root.join("root.ts"), "const providerOptions = {};\n").unwrap();
        std::fs::write(root.join("nested/child.ts"), "const providerOptions = {};\n").unwrap();
        let output = call(
            &ToolContext::new(temp.path()),
            json!({
                "pat": "providerOptions", "path": "packages/pkg/src/**/*.ts"
            }),
        )
        .unwrap();
        let text = body(&output);
        assert!(text.contains("# packages/pkg/src/"), "{text}");
        assert!(text.lines().any(|line| line.starts_with("## root.ts#")), "{text}");
        assert!(text.lines().any(|line| line.starts_with("### child.ts#")), "{text}");
        assert_eq!(output.details.as_ref().unwrap()["matchCount"], 2);
    }

    #[test]
    fn pluscal_in_tla_file_uses_the_tlaplus_parser() {
        let temp = tempfile::tempdir().unwrap();
        std::fs::write(
            temp.path().join("algo.tla"),
            "---- MODULE Algo ----\n(*--algorithm Demo\nvariables x = 0;\nbegin\n  x := x + 1;\nend algorithm;*)\n====\n",
        )
        .unwrap();
        let output = call(&ToolContext::new(temp.path()), json!({"pat": "x", "path": "algo.tla"})).unwrap();
        assert!(output.details.as_ref().unwrap()["matchCount"].as_u64().unwrap() > 0);
    }
}

#[cfg(test)]
mod grammar_source_review_tests {
    use super::*;
    #[tokio::test]
    async fn target_language_filter_count_and_missing_order_bank() {
        let dir=tempfile::tempdir().unwrap();let cache=tempfile::tempdir().unwrap();
        ara_ast::language::grammar::set_grammar_dir(cache.path().to_owned());
        let ctx=ToolContext::new(dir.path());let tool=AstGrepTool{ctx:ctx.clone()};
        std::fs::write(dir.path().join("calls.ts"),"foo(1);\n").unwrap();
        std::fs::write(dir.path().join("README.txt"),"plain text").unwrap();
        for language in ["","   "] {
            let result=tool.execute_os_values(AstGrepOsValues{pattern:"foo($A)".into(),language:Some(language.into()),path:Some(".".into()),skip:0.0},CancellationToken::new()).await.unwrap();
            let details=result.details.as_ref().unwrap();assert_eq!(details["filesSearched"],1);assert!(details.get("parseErrors").is_none());
        }
        let result=tool.execute_os_values(AstGrepOsValues{pattern:"foo($A)".into(),language:Some("not-a-language".into()),path:Some("calls.ts".into()),skip:0.0},CancellationToken::new()).await.unwrap();
        let details=result.details.as_ref().unwrap();assert_eq!(details["filesSearched"],1);assert_eq!(details["parseErrorsTotal"],1);
        for directory in ["first","second"] {std::fs::create_dir(dir.path().join(directory)).unwrap();}
        let ini=dir.path().join("first/z.ini");let hcl=dir.path().join("second/a.tf");let repeated=dir.path().join("second/z.ini");
        std::fs::write(&ini,"[core]\nx=1\n").unwrap();std::fs::write(&hcl,"name = 1\n").unwrap();std::fs::write(&repeated,"[core]\nx=2\n").unwrap();
        let scope=SearchScope{search_path:dir.path().to_owned(),scope_path:".".into(),glob:None,is_directory:true,multi_targets:None,exact_file_paths:Some(vec![ini.clone(),hcl.clone()]),missing_paths:Vec::new()};
        let (_,_,result)=tool.search_resolved_scope(scope,"foo($A)",0,Vec::new(),None,&CancellationToken::new()).unwrap();
        assert_eq!(result.files_searched,0);assert_eq!(result.missing_grammars,vec!["hcl","ini"]);
        let scope=SearchScope{search_path:dir.path().to_owned(),scope_path:".".into(),glob:None,is_directory:true,multi_targets:Some(vec![paths::SearchTarget{base:ini,glob:None},paths::SearchTarget{base:dir.path().join("second"),glob:None}]),exact_file_paths:None,missing_paths:Vec::new()};
        let (_,_,result)=tool.search_resolved_scope(scope,"foo($A)",0,Vec::new(),None,&CancellationToken::new()).unwrap();
        assert_eq!(result.files_searched,0);assert_eq!(result.missing_grammars,vec!["ini","hcl"]);
    }
}
