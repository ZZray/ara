// Ported from Oh My Pi 596f2da7101178214aa27a753529d15e6b7ad91d, MIT.
// Copyright (c) 2025 Mario Zechner; 2025-2026 Can Bölük; 2026 Stencil Labs, Inc.
// The upstream MIT permission and disclaimer are reproduced in types.rs.

use super::{
    client::{Client, ClientPool},
    config::{ServerConfig, has_root_marker_ancestor, project_aware},
    custom,
    types::*,
};
use futures::future::join_all;
use serde_json::{Value, json};
use std::{
    collections::{HashMap, HashSet},
    io,
    path::{Path, PathBuf},
    sync::Arc,
    time::{Duration, Instant},
};
use tokio_util::sync::CancellationToken;

async fn wait(
    client: Arc<Client>,
    uri: String,
    min: Option<u64>,
    expected: Option<u64>,
    wait_ms: u64,
    cancel: Option<CancellationToken>,
) -> io::Result<Vec<Diagnostic>> {
    let deadline = Instant::now() + Duration::from_millis(wait_ms);
    let key = uri_key(&uri);
    let pull_supported = {
        let state = client.state.lock().unwrap();
        state.capabilities.get("diagnosticProvider").is_some_and(|v| !v.is_null() && v != &Value::Bool(false))
            || state.dynamic.values().any(|v| v == "textDocument/diagnostic")
    };
    let pull_client = client.clone();
    let pull_uri = uri.clone();
    let pull_cancel = cancel.clone();
    let mut pull = Box::pin(async move {
        if !pull_supported {
            std::future::pending::<()>().await;
        }
        pull_client
            .request(
                "textDocument/diagnostic",
                json!({"textDocument":{"uri":pull_uri}}),
                pull_cancel,
                Some(wait_ms.max(1)),
            )
            .await
    });
    let mut pull_done = !pull_supported;
    let mut pull_failure = None;
    let mut settled = None;
    let mut settled_at = Instant::now();
    loop {
        check_cancel(cancel.as_ref())?;
        if !client.alive() {
            return Err(error("LSP exited before diagnostics settled"));
        }
        let published = client.state.lock().unwrap().diagnostics.get(&key).cloned();
        if let Some(published) = published.filter(|v| min.is_none_or(|min| v.publication > min)) {
            if expected.is_some_and(|version| published.document_version == Some(version)) {
                return Ok(published.items);
            }
            if settled != Some(published.publication) {
                settled = Some(published.publication);
                settled_at = Instant::now();
            } else if settled_at.elapsed() >= Duration::from_millis(250) {
                return Ok(published.items);
            }
        }
        if Instant::now() >= deadline {
            break;
        }
        let poll = Duration::from_millis(100).min(deadline.saturating_duration_since(Instant::now()));
        tokio::select! {
            result = &mut pull, if !pull_done => {
                pull_done = true;
                match result {
                    Ok(report) if report["kind"] == "full" && report["items"].is_array() => {
                        let items = serde_json::from_value::<Vec<Diagnostic>>(report["items"].clone()).map_err(io::Error::other)?;
                        let mut state = client.state.lock().unwrap(); state.diagnostic_version += 1;
                        let publication = state.diagnostic_version;
                        let version = expected.or_else(|| state.documents.get(&key).map(|v| v.version));
                        state.diagnostics.insert(key.clone(), PublishedDiagnostics { items: items.clone(), document_version: version, publication });
                        return Ok(items);
                    },
                    Ok(_) => {},
                    Err(error) => pull_failure = Some(error),
                }
            },
            _ = tokio::time::sleep(poll) => {},
            _ = cancellation(cancel.as_ref()) => return Err(cancelled()),
            _ = client.stop.cancelled() => return Err(error("LSP exited while waiting for diagnostics")),
        }
    }
    if !pull_done {
        match pull.await {
            Ok(report) if report["kind"] == "full" && report["items"].is_array() => {
                return serde_json::from_value(report["items"].clone()).map_err(io::Error::other);
            }
            Err(error) => pull_failure = Some(error),
            _ => {}
        }
    }
    check_cancel(cancel.as_ref())?;
    Err(pull_failure.unwrap_or_else(|| error("No fresh authoritative LSP diagnostic report")))
}

pub(super) async fn diagnostics(
    pool: &ClientPool,
    path: &Path,
    servers: &[ServerConfig],
    min_versions: Option<&Versions>,
    expected_versions: Option<&Versions>,
    wait_ms: u64,
    cancel: Option<CancellationToken>,
) -> io::Result<Option<Value>> {
    if servers.is_empty() {
        return Ok(None);
    }
    check_cancel(cancel.as_ref())?;
    let pipeline = Duration::from_millis(wait_ms.saturating_add(10_000));
    let results = join_all(servers.iter().map(|server| {
        let cancel = cancel.clone();
        async move {
            let bound = cancel.as_ref().map(CancellationToken::child_token).unwrap_or_default();
            let operation = async {
                check_cancel(Some(&bound))?;
                let diagnostics = if server.custom {
                    custom::lint(server, path, pool.cwd(), Some(&bound)).await?
                } else {
                    let client = pool
                        .get(server, Some(bound.clone()), true)
                        .await?
                        .ok_or_else(|| error("LSP client unavailable"))?;
                    if project_aware(server) {
                        client.wait_project(Some(bound.clone())).await?;
                    }
                    wait(
                        client,
                        file_uri(path),
                        min_versions.and_then(|v| v.get(&server.name)).copied(),
                        expected_versions.and_then(|v| v.get(&server.name)).copied(),
                        wait_ms,
                        Some(bound.clone()),
                    )
                    .await?
                };
                Ok::<_, io::Error>((server, diagnostics))
            };
            let result = tokio::time::timeout(pipeline, operation).await;
            if result.is_err() {
                bound.cancel();
            }
            result.map_err(|_| io::Error::new(io::ErrorKind::TimedOut, "LSP diagnostics pipeline timed out"))?
        }
    }))
    .await;
    check_cancel(cancel.as_ref())?;
    let mut names = Vec::new();
    let mut all = Vec::new();
    for result in results {
        match result {
            Ok((server, diagnostics)) => {
                names.push(server.name.clone());
                let has_root = server.root_markers.is_empty() || has_root_marker_ancestor(path, &server.root_markers);
                all.extend(
                    diagnostics
                        .into_iter()
                        .filter(|diagnostic| has_root || !orphan_typescript(&server.name, diagnostic)),
                );
            }
            Err(error) => eprintln!("LSP diagnostics unavailable: {error}"),
        }
    }
    if names.is_empty() {
        return Ok(None);
    }
    if all.is_empty() {
        return Ok(Some(json!({"server":names.join(", "),"messages":[],"summary":"OK","errored":false})));
    }
    let mut seen = HashSet::new();
    all.retain(|diagnostic| seen.insert((diagnostic.range.start, diagnostic.range.end, diagnostic.message.clone())));
    all.sort_by(|a, b| {
        a.severity
            .unwrap_or(1)
            .cmp(&b.severity.unwrap_or(1))
            .then(a.range.start.cmp(&b.range.start))
            .then(a.message.cmp(&b.message))
    });
    let relative = path.strip_prefix(pool.cwd()).unwrap_or(path).to_string_lossy().replace('\\', "/");
    let messages = all.iter().take(50).map(|diagnostic| format_diagnostic(diagnostic, &relative)).collect::<Vec<_>>();
    let mut counts = [0usize; 4];
    for diagnostic in &all {
        if let Some(index) = usize::from(diagnostic.severity.unwrap_or(1)).checked_sub(1).filter(|v| *v < 4) {
            counts[index] += 1;
        }
    }
    let summary = ["error", "warning", "info", "hint"]
        .into_iter()
        .zip(counts)
        .filter(|(_, count)| *count > 0)
        .map(|(name, count)| format!("{count} {name}(s)"))
        .collect::<Vec<_>>()
        .join(", ");
    Ok(Some(
        json!({"server":names.join(", "),"messages":messages,"summary":if summary.is_empty() {"no issues".into()} else {summary},"errored":all.iter().any(|v|v.severity==Some(1))}),
    ))
}
fn orphan_typescript(name: &str, diagnostic: &Diagnostic) -> bool {
    if diagnostic.source.as_deref() != Some("typescript") && !name.to_lowercase().contains("typescript") {
        return false;
    }
    let code = diagnostic
        .code
        .as_ref()
        .and_then(|value| value.as_u64().or_else(|| value.as_str().and_then(|v| v.parse().ok())));
    matches!(code, Some(1375 | 1378 | 2307 | 2580 | 2591 | 2792 | 2867))
}
fn format_diagnostic(diagnostic: &Diagnostic, path: &str) -> String {
    let severity = match diagnostic.severity.unwrap_or(1) {
        1 => "error",
        2 => "warning",
        3 => "info",
        4 => "hint",
        _ => "unknown",
    };
    let message = diagnostic
        .message
        .lines()
        .filter(|line| {
            let line = line.trim();
            !line.starts_with("for further information visit")
                && !line.starts_with("http://")
                && !line.starts_with("https://")
        })
        .collect::<Vec<_>>()
        .join("\n")
        .trim()
        .to_string();
    let source = diagnostic.source.as_ref().filter(|v| !v.is_empty()).map(|v| format!("[{v}] ")).unwrap_or_default();
    let code = diagnostic
        .code
        .as_ref()
        .filter(|v| !v.is_null() && **v != Value::from(0) && **v != Value::String(String::new()))
        .map(|v| format!(" ({})", v.as_str().map(str::to_string).unwrap_or_else(|| v.to_string())))
        .unwrap_or_default();
    format!(
        "{path}:{}:{} [{severity}] {source}{message}{code}",
        diagnostic.range.start.line + 1,
        diagnostic.range.start.character + 1
    )
}

pub(super) async fn format(
    pool: &ClientPool,
    path: &Path,
    content: &str,
    servers: &[ServerConfig],
    cancel: Option<CancellationToken>,
) -> FormatResult {
    let mut failed = false;
    for server in servers {
        if check_cancel(cancel.as_ref()).is_err() {
            failed = true;
            break;
        }
        if server.custom {
            return custom::format(server, path, content, pool.cwd(), cancel.as_ref()).await;
        }
        let result = async {
            let client =
                pool.get(server, cancel.clone(), true).await?.ok_or_else(|| error("LSP formatter unavailable"))?;
            let supported = client
                .state
                .lock()
                .unwrap()
                .capabilities
                .get("documentFormattingProvider")
                .is_some_and(|v| !v.is_null() && v != &Value::Bool(false));
            if !supported {
                return Ok::<_, io::Error>(None);
            }
            let edits = client
                .request(
                    "textDocument/formatting",
                    json!({"textDocument":{"uri":file_uri(path)},"options":format_options(path, content)}),
                    cancel.clone(),
                    None,
                )
                .await?;
            let formatted = if edits.is_null() {
                content.into()
            } else {
                apply_text_edits(content, serde_json::from_value(edits).map_err(io::Error::other)?)?
            };
            Ok(Some(formatted))
        }
        .await;
        match result {
            Ok(Some(content)) => {
                return FormatResult { content, failed: false, unsupported: false, external_write_unknown: false };
            }
            Ok(None) => {}
            Err(_) => failed = true,
        }
    }
    FormatResult { content: content.into(), failed, unsupported: !failed, external_write_unknown: false }
}

fn format_options(path: &Path, content: &str) -> Value {
    let mut spaces = None;
    let mut unit = 0u64;
    for line in content.split('\n').filter(|v| !v.trim().is_empty()) {
        let Some(first) = line.as_bytes().first() else {
            continue;
        };
        if !matches!(first, b' ' | b'\t') {
            continue;
        }
        spaces.get_or_insert(*first == b' ');
        if *first == b' ' {
            let width = line.bytes().take_while(|v| *v == b' ').count() as u64;
            if unit == 0 {
                unit = width;
            } else {
                let mut other = width;
                while other != 0 {
                    let rest = unit % other;
                    unit = other;
                    other = rest;
                }
            }
        }
    }
    let mut config = HashMap::<String, String>::new();
    let mut chain = Vec::<(PathBuf, Vec<(String, HashMap<String, String>)>)>::new();
    if !path.components().any(|part| part.as_os_str().len() > 255) {
        let mut dir = path.parent();
        while let Some(current) = dir {
            if let Ok(text) = std::fs::read_to_string(current.join(".editorconfig")) {
                let mut sections: Vec<(String, HashMap<String, String>)> = Vec::new();
                let mut root = false;
                for line in text.lines().map(str::trim).filter(|v| !v.is_empty() && !v.starts_with(['#', ';'])) {
                    if let Some(pattern) = line.strip_prefix('[').and_then(|v| v.strip_suffix(']')).map(str::trim) {
                        if !pattern.is_empty() {
                            sections.push((pattern.into(), HashMap::new()));
                        }
                        continue;
                    }
                    let Some((key, value)) = line.split_once('=') else {
                        continue;
                    };
                    let key = key.trim().to_lowercase();
                    let value = value.trim().to_lowercase();
                    if let Some((_, properties)) = sections.last_mut() {
                        properties.insert(key, value);
                    } else if key == "root" {
                        root = value == "true";
                    }
                }
                chain.push((current.to_owned(), sections));
                if root {
                    break;
                }
            }
            dir = current.parent();
        }
    }
    for (base, sections) in chain.into_iter().rev() {
        let relative = path.strip_prefix(base).unwrap_or(Path::new(".")).to_string_lossy().replace('\\', "/");
        for (pattern, props) in sections {
            let pattern = pattern.trim_start_matches('/').replace('\\', "/");
            let mut patterns = vec![pattern.clone()];
            if !pattern.contains('/') && !pattern.starts_with("**") {
                patterns.push(format!("**/{pattern}"));
            }
            if patterns
                .into_iter()
                .any(|pattern| globset::Glob::new(&pattern).is_ok_and(|v| v.compile_matcher().is_match(&relative)))
            {
                // Invalid values preserve previous matching declarations (fixed tab-spacing.ts).
                if let Some(style) = props.get("indent_style").filter(|v| matches!(v.as_str(), "space" | "tab")) {
                    config.insert("indent_style".into(), style.clone());
                }
                if let Some(size) =
                    props.get("indent_size").filter(|v| v.as_str() == "tab" || v.parse::<u64>().is_ok_and(|v| v > 0))
                {
                    config.insert("indent_size".into(), size.clone());
                }
                if let Some(width) = props.get("tab_width").filter(|v| v.parse::<u64>().is_ok_and(|v| v > 0)) {
                    config.insert("tab_width".into(), width.clone());
                }
            }
        }
    }
    let integer =
        |key: &str| config.get(key).and_then(|v| v.parse::<u64>().ok()).filter(|v| *v > 0).map(|v| v.clamp(1, 16));
    let from_config = integer("indent_size").or_else(|| integer("tab_width"));
    let insert_spaces = match config.get("indent_style").map(String::as_str) {
        Some("space") => Some(true),
        Some("tab") => Some(false),
        _ if config.get("indent_size").is_some_and(|v| v == "tab") => Some(false),
        _ if integer("indent_size").is_some() => Some(true),
        _ => None,
    };
    json!({"tabSize":from_config.or_else(|| (unit > 0 && spaces == Some(true)).then_some(unit)).unwrap_or(2),"insertSpaces":insert_spaces.or(spaces).unwrap_or(true),"trimTrailingWhitespace":true,"insertFinalNewline":true,"trimFinalNewlines":true})
}
