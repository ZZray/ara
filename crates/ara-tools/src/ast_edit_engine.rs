//! Fixed OMP 596f2da crates/pi-natives/src/ast.rs ast_edit_blocking (MIT).
//! The vendor AST library supplies parsing/matching/apply_edits. All rewrite
//! patterns match the original tree; rules are never applied sequentially.

use crate::{
    engine,
    paths::{SearchScope, SearchTarget},
};
use ara_agent::ToolError;
use ara_ast::{language::SupportLang, ops};
use ara_walk::{Budget, EntryKind, Visit, WalkOptions};
use ast_grep_core::{MatchStrictness, matcher::Pattern, source::Edit, tree_sitter::LanguageExt};
use std::{
    collections::{BTreeMap, BTreeSet},
    path::{Path, PathBuf},
    time::{Duration, Instant},
};
use tokio_util::sync::CancellationToken;

#[derive(Clone, Debug)]
pub struct AstChange {
    pub path: PathBuf,
    pub before: String,
    pub after: String,
    pub byte_start: usize,
    pub byte_end: usize,
    pub deleted_length: usize,
    pub start_line: usize,
    pub start_column: usize,
    pub end_line: usize,
    pub end_column: usize,
}
#[derive(Clone, Debug, Default)]
pub struct AstResult {
    pub changes: Vec<AstChange>,
    pub file_changes: Vec<(PathBuf, usize)>,
    pub total_replacements: usize,
    pub files_touched: usize,
    pub files_searched: usize,
    pub applied: bool,
    pub limit_reached: bool,
    pub parse_errors: Vec<String>,
    pub missing_grammars: Vec<String>,
}
fn check(budget: &Budget) -> Result<(), ToolError> {
    budget.check().map_err(|e| {
        ToolError(
            match e {
                ara_walk::WalkError::Aborted => "Aborted: Cancelled",
                ara_walk::WalkError::Timeout => "Aborted: Timeout",
            }
            .into(),
        )
    })
}
fn collect(target: &SearchTarget, budget: &Budget) -> Result<Vec<(PathBuf, String, SupportLang)>, ToolError> {
    let meta = std::fs::metadata(&target.base).map_err(|e| ToolError(format!("Path not found: {e}")))?;
    let mut candidates = Vec::new();
    if meta.is_file() {
        if let Some(lang) = SupportLang::from_path(&target.base) {
            candidates.push((
                std::fs::canonicalize(&target.base).unwrap_or(target.base.clone()),
                target.base.file_name().unwrap_or_default().to_string_lossy().into_owned(),
                lang,
            ));
        }
    } else if meta.is_dir() {
        let matcher = target
            .glob
            .as_deref()
            .filter(|g| !g.trim().is_empty())
            .map(|g| engine::compile_glob(&engine::build_glob_pattern(g, false)))
            .transpose()
            .map_err(ToolError::from)?;
        ara_walk::walk(
            &target.base,
            WalkOptions {
                include_hidden: true,
                use_gitignore: true,
                skip_node_modules: !target.glob.as_ref().is_some_and(|g| g.contains("node_modules")),
                max_depth: usize::MAX,
            },
            budget,
            |entry| {
                if entry.kind == EntryKind::File && matcher.as_ref().is_none_or(|g| g.is_match(&entry.relative)) {
                    if let Some(lang) = SupportLang::from_path(&entry.path) {
                        candidates.push((entry.path, entry.relative, lang));
                    }
                }
                Visit::Continue
            },
        )
        .map_err(|e| {
            ToolError(
                match e {
                    ara_walk::WalkError::Aborted => "Aborted: Cancelled",
                    ara_walk::WalkError::Timeout => "Aborted: Timeout",
                }
                .into(),
            )
        })?;
    } else {
        return Err(ToolError(format!("Search path must be a file or directory: {}", target.base.display())));
    }
    candidates.sort_by(|a, b| a.1.cmp(&b.1));
    if candidates.is_empty() {
        return Err(ToolError("ast_edit found no supported source files for the given path/glob".into()));
    }
    Ok(candidates)
}
struct Rule {
    pat: String,
    out: String,
    compiled: BTreeMap<String, Pattern>,
    errors: BTreeMap<String, String>,
}
fn one(
    target: &SearchTarget,
    rewrites: &[(String, String)],
    dry_run: bool,
    max_files: usize,
    budget: &Budget,
) -> Result<AstResult, ToolError> {
    let mut candidates = collect(target, budget)?;
    let mut missing = BTreeSet::new();
    candidates.retain(|(_, _, language)| {
        if language.wasm_grammar().is_some_and(|grammar| !grammar.is_installed()) {
            missing.insert(language.canonical_name().to_owned()); false
        } else { true }
    });
    let mut result = AstResult { files_searched: candidates.len(), applied: !dry_run, missing_grammars: missing.into_iter().collect(), ..Default::default() };
    let languages: BTreeMap<_, _> = candidates.iter().map(|(_, _, l)| (l.canonical_name().to_owned(), *l)).collect();
    let mut rules = Vec::new();
    for (pat, out) in rewrites {
        check(budget)?;
        let mut rule = Rule { pat: pat.clone(), out: out.clone(), compiled: BTreeMap::new(), errors: BTreeMap::new() };
        for (key, lang) in &languages {
            check(budget)?;
            match ops::compile_pattern(pat, None, &MatchStrictness::Smart, *lang) {
                Ok(pattern) => {
                    rule.compiled.insert(key.clone(), pattern);
                }
                Err(error) => {
                    rule.errors.insert(key.clone(), error.to_string());
                }
            }
        }
        if rule.compiled.is_empty() {
            for (key, error) in rule.errors {
                result.parse_errors.push(if languages.len() > 1 {
                    format!("{pat} ({key}): {error}")
                } else {
                    format!("{pat}: {error}")
                });
            }
        } else {
            rules.push(rule);
        }
    }
    if rules.is_empty() {
        return Ok(result);
    }
    let mut writes = Vec::new();
    let mut counts = BTreeMap::new();
    for (absolute, display, language) in candidates {
        check(budget)?;
        let key = language.canonical_name();
        let runnable: Vec<_> = rules
            .iter()
            .filter_map(|rule| {
                if let Some(error) = rule.errors.get(key) {
                    result.parse_errors.push(format!("{}: {display}: {error}", rule.pat));
                }
                rule.compiled.get(key).map(|pattern| (&rule.out, pattern))
            })
            .collect();
        if runnable.is_empty() {
            continue;
        }
        let source = match std::fs::read_to_string(&absolute) {
            Ok(source) => source,
            Err(error) => {
                result.parse_errors.push(format!("{display}: {error}"));
                continue;
            }
        };
        let ast = language.ast_grep(&source);
        if ast.root().dfs().any(|node| node.is_error()) {
            result.parse_errors.push(format!("{display}: parse error (syntax tree contains error nodes)"));
            continue;
        }
        let mut edits: Vec<Edit<String>> = Vec::new();
        let mut changes = Vec::new();
        let mut reached_max = false;
        'rules: for (out, pattern) in runnable {
            for matched in ast.root().find_all(pattern.clone()) {
                check(budget)?;
                let edit = matched.replace_by(out.as_str());
                if edits.iter().any(|old| {
                    old.position == edit.position
                        && old.deleted_length == edit.deleted_length
                        && old.inserted_text == edit.inserted_text
                }) {
                    continue;
                }
                if result.changes.len() + changes.len() >= u32::MAX as usize {
                    result.limit_reached = true;
                    reached_max = true;
                    break 'rules;
                }
                let range = matched.range();
                let start = matched.start_pos();
                let end = matched.end_pos();
                let after = String::from_utf8(edit.inserted_text.clone())
                    .map_err(|e| ToolError(format!("{display}: replacement text is not valid UTF-8: {e}")))?;
                changes.push(AstChange {
                    path: absolute.clone(),
                    before: matched.text().into_owned(),
                    after,
                    byte_start: range.start,
                    byte_end: range.end,
                    deleted_length: edit.deleted_length,
                    start_line: start.line() + 1,
                    start_column: start.column(matched.get_node()) + 1,
                    end_line: end.line() + 1,
                    end_column: end.column(matched.get_node()) + 1,
                });
                edits.push(edit);
            }
        }
        if changes.is_empty() {
            if reached_max {
                break;
            }
            continue;
        }
        if result.files_touched >= max_files.max(1) {
            result.limit_reached = true;
            break;
        }
        result.files_touched += 1;
        counts.insert(absolute.clone(), changes.len());
        if !dry_run {
            let output = ops::apply_edits(&source, &edits).map_err(|e| ToolError(e.to_string()))?;
            if output != source {
                writes.push((absolute, output));
            }
        }
        result.changes.extend(changes);
        if reached_max {
            break;
        }
    }
    // A compute/overlap failure above writes zero files in this target. Write
    // failures or cancellation here may have effects and are not replayed.
    for (path, output) in writes {
        check(budget)?;
        std::fs::write(&path, output).map_err(|e| ToolError(format!("Failed to write {}: {e}", path.display())))?;
    }
    result.file_changes = counts.into_iter().collect();
    result.total_replacements = result.changes.len();
    Ok(result)
}

pub fn run(
    scope: &SearchScope,
    rewrites: &[(String, String)],
    dry_run: bool,
    max_files: usize,
    cancel: CancellationToken,
) -> Result<AstResult, ToolError> {
    // No timeout_ms was supplied by the fixed tool. Budget uses a distant
    // representable deadline solely to adapt the existing cancellable walker.
    let budget = Budget { cancel, deadline: Instant::now() + Duration::from_secs(u32::MAX as u64) };
    let targets = native_targets(scope);
    let mut result = AstResult { applied: !dry_run, ..Default::default() };
    for target in &targets {
        // The typed entrance supplies one frozen scalar map. Original Native
        // callers dispatch run_native_target separately after each Graph read.
        merge_native_target(&mut result, one(target, rewrites, dry_run, max_files, &budget)?);
    }
    Ok(result)
}

pub fn native_targets(scope: &SearchScope) -> Vec<SearchTarget> {
    scope
        .multi_targets
        .clone()
        .unwrap_or_else(|| vec![SearchTarget { base: scope.search_path.clone(), glob: scope.glob.clone() }])
}
pub fn merge_native_target(result: &mut AstResult, part: AstResult) {
    result.total_replacements += part.total_replacements;
    result.files_searched += part.files_searched;
    result.applied &= part.applied;
    result.limit_reached |= part.limit_reached;
    result.changes.extend(part.changes);
    result.parse_errors.extend(part.parse_errors);
    for language in part.missing_grammars {
        if !result.missing_grammars.contains(&language) { result.missing_grammars.push(language); }
    }
    for (path, count) in part.file_changes {
        if let Some((_, old)) = result.file_changes.iter_mut().find(|(old, _)| old == &path) {
            *old += count;
        } else {
            result.file_changes.push((path, count));
        }
    }
    result.files_touched = result.file_changes.len();
}
pub fn run_native_target(
    target: SearchTarget,
    mut rewrites: Vec<(String, String)>,
    dry_run: bool,
    max_files: usize,
    cancel: CancellationToken,
) -> Result<AstResult, ToolError> {
    // pi-natives/ast.rs normalize_rewrite_map occurs after NAPI conversion.
    if rewrites.is_empty() {
        return Err(ToolError("`rewrites` is required and must include at least one pattern->rewrite mapping".into()));
    }
    if rewrites.iter().any(|(key, _)| key.is_empty()) {
        return Err(ToolError("`rewrites` keys must be non-empty pattern strings".into()));
    }
    rewrites.sort_by(|a, b| a.0.cmp(&b.0));
    let budget = Budget { cancel, deadline: Instant::now() + Duration::from_secs(u32::MAX as u64) };
    one(&target, &rewrites, dry_run, max_files, &budget)
}

pub fn cap_errors(errors: Vec<String>) -> (Vec<String>, usize) {
    let mut seen = BTreeSet::new();
    let mut unique = Vec::new();
    for error in errors {
        if seen.insert(error.clone()) {
            unique.push(error);
        }
    }
    let total = unique.len();
    unique.truncate(20);
    (unique, total)
}
