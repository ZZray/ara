//! Handoff file-operation tracking from fixed OMP `compaction/utils.ts:17-192`,
//! `compaction.ts::extractFileOperations`, and `utils/path-tree.ts` at
//! 596f2da7101178214aa27a753529d15e6b7ad91d (MIT; THIRD_PARTY_NOTICES.md).

use std::collections::{HashMap, HashSet};

use ara_ai::{AssistantBlock, Context, Message};
use serde_json::json;

/// Previous native compaction details, supplied only when not fromExtension.
/// Native tracking rehydrates these fields; it does not scrape summary text.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct HandoffFileDetails {
    pub read_files: Vec<String>,
    pub modified_files: Vec<String>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PreparedHandoffSummary {
    pub summary: String,
    pub read_files: Vec<String>,
    pub modified_files: Vec<String>,
}

fn is_url_scheme(path: &str) -> bool {
    path.match_indices("://").any(|(index, _)| {
        path.as_bytes()[..index]
            .iter()
            .rev()
            .take_while(|byte| byte.is_ascii_alphanumeric() || matches!(**byte, b'+' | b'.' | b'-'))
            .any(u8::is_ascii_alphabetic)
    })
}

fn range_selector(value: &str) -> bool {
    fn number(bytes: &[u8], index: &mut usize) -> bool {
        if bytes.get(*index).is_some_and(|byte| *byte == b'l' || *byte == b'L') {
            *index += 1;
        }
        let start = *index;
        while bytes.get(*index).is_some_and(u8::is_ascii_digit) {
            *index += 1;
        }
        *index > start
    }
    !value.is_empty()
        && value.split(',').all(|chunk| {
            let bytes = chunk.as_bytes();
            let mut index = 0;
            if !number(bytes, &mut index) {
                return false;
            }
            if index == bytes.len() {
                return true;
            }
            let trailing = match bytes[index] {
                b'-' => {
                    index += 1;
                    true
                }
                b'+' => {
                    index += 1;
                    false
                }
                b'.' if bytes.get(index + 1) == Some(&b'.') => {
                    index += 2;
                    true
                }
                _ => return false,
            };
            if index == bytes.len() {
                return trailing;
            }
            number(bytes, &mut index) && index == bytes.len()
        })
}

fn strip_read_selector(path: &str) -> &str {
    let Some(colon) = path.rfind(':').filter(|colon| *colon > 0) else { return path };
    let candidate = &path[colon + 1..];
    let raw = candidate.eq_ignore_ascii_case("raw");
    let range = range_selector(candidate);
    if !raw && !range && !candidate.eq_ignore_ascii_case("conflicts") {
        return path;
    }
    let base = &path[..colon];
    if let Some(inner) = base.rfind(':').filter(|inner| *inner > 0) {
        let inner_candidate = &base[inner + 1..];
        if inner_candidate.eq_ignore_ascii_case("raw") && range || range_selector(inner_candidate) && raw {
            return &base[..inner];
        }
    }
    base
}

fn js_whitespace(character: char) -> bool {
    matches!(
        character,
        '\t' | '\n' | '\u{000b}' | '\u{000c}' | '\r' | ' ' | '\u{00a0}' | '\u{1680}' | '\u{2000}'
            ..='\u{200a}' | '\u{2028}' | '\u{2029}' | '\u{202f}' | '\u{205f}' | '\u{3000}' | '\u{feff}'
    )
}

fn strip_tag(summary: &str, opening: &str, closing: &str) -> String {
    let mut output = String::new();
    let mut rest = summary;
    while let Some(start) = rest.find(opening) {
        let after_opening = start + opening.len();
        let Some(end) = rest[after_opening..].find(closing) else { break };
        output.push_str(&rest[..start]);
        rest = rest[after_opening + end + closing.len()..].trim_start_matches(js_whitespace);
    }
    output.push_str(rest);
    output
}

fn strip_file_operation_tags(summary: &str) -> String {
    let summary = strip_tag(summary, "<files>", "</files>");
    let summary = strip_tag(&summary, "<read-files>", "</read-files>");
    strip_tag(&summary, "<modified-files>", "</modified-files>").trim_end_matches(js_whitespace).into()
}

#[derive(Default)]
struct PathNode {
    files: Vec<(String, String)>,
    subdirs: Vec<(String, PathNode)>,
}

fn add_path(node: &mut PathNode, segments: &[&str], directory: bool, original: &str) {
    if segments.len() == 1 && !directory {
        let name = segments[0];
        if !node.files.iter().any(|(existing, _)| existing == name) {
            node.files.push((name.into(), original.into()));
        }
        return;
    }
    let name = segments[0];
    let index = match node.subdirs.iter().position(|(existing, _)| existing == name) {
        Some(index) => index,
        None => {
            node.subdirs.push((name.into(), PathNode::default()));
            node.subdirs.len() - 1
        }
    };
    if segments.len() > 1 {
        add_path(&mut node.subdirs[index].1, &segments[1..], directory, original);
    }
}

fn walk_paths(node: &PathNode, depth: usize, mode: &HashMap<String, &'static str>, lines: &mut Vec<String>) {
    for (name, key) in &node.files {
        lines.push(format!("{name} ({})", mode[key]));
    }
    for (name, child) in &node.subdirs {
        let mut parts = vec![name.as_str()];
        let mut child = child;
        while child.files.is_empty() && child.subdirs.len() == 1 {
            parts.push(child.subdirs[0].0.as_str());
            child = &child.subdirs[0].1;
        }
        lines.push(format!("{} {}/", "#".repeat(depth + 1), parts.join("/")));
        walk_paths(child, depth + 1, mode, lines);
    }
}

fn format_file_operations(read_files: &[String], modified_files: &[String], read: &HashSet<String>) -> String {
    let mut mode = HashMap::new();
    for file in read_files {
        mode.insert(file.clone(), "Read");
    }
    for file in modified_files {
        mode.insert(file.clone(), if read.contains(file) { "RW" } else { "Write" });
    }
    if mode.is_empty() {
        return String::new();
    }
    let mut paths = mode.keys().cloned().collect::<Vec<_>>();
    paths.sort_by(|left, right| left.encode_utf16().cmp(right.encode_utf16()));
    let mut tree = PathNode::default();
    for path in paths.iter().take(20) {
        let normalized = path.replace('\\', "/");
        let trimmed = normalized.strip_suffix('/').unwrap_or(&normalized);
        if !trimmed.is_empty() {
            add_path(&mut tree, &trimmed.split('/').collect::<Vec<_>>(), path.ends_with('/'), path);
        }
    }
    let mut lines = Vec::new();
    walk_paths(&tree, 0, &mode, &mut lines);
    let mut files = lines.join("\n");
    if paths.len() > 20 {
        files.push_str(&format!("\n[…{} files elided…]", paths.len() - 20));
    }
    const TEMPLATE: &str = "{{#if files}}\n{{#xml \"files\"}}\n{{files}}\n{{/xml}}\n{{/if}}\n";
    ara_prompt::render(TEMPLATE, &json!({"files": files})).expect("fixed file-operations template is valid")
}

/// `context.messages` is the prefix consumed by native preparation, INCLUDING
/// a split turn's prefix and EXCLUDING kept/recent messages. It is a different
/// input from the complete live Context used to generate the handoff document.
/// `previous` holds the previous compaction's native details, not its summary.
pub fn prepare_handoff_summary(
    document: &str,
    context: &Context,
    previous: Option<&HandoffFileDetails>,
) -> PreparedHandoffSummary {
    let mut read = HashSet::new();
    let mut modified = HashSet::new();
    if let Some(previous) = previous {
        read.extend(previous.read_files.iter().map(|path| strip_read_selector(path).to_owned()));
        modified.extend(previous.modified_files.iter().cloned());
    }
    for message in &context.messages {
        let Message::Assistant(message) = message else { continue };
        for block in &message.content {
            let AssistantBlock::ToolCall(call) = block else { continue };
            let Some(path) = call.arguments.get("path").and_then(|value| value.as_str()) else { continue };
            if path.is_empty() || is_url_scheme(path) {
                continue;
            }
            match call.name.as_str() {
                "read" => {
                    read.insert(strip_read_selector(path).to_owned());
                }
                "write" | "edit" => {
                    modified.insert(path.to_owned());
                }
                _ => {}
            }
        }
    }
    let mut read_files =
        read.iter().filter(|path| !is_url_scheme(path) && !modified.contains(*path)).cloned().collect::<Vec<_>>();
    let mut modified_files = modified.iter().filter(|path| !is_url_scheme(path)).cloned().collect::<Vec<_>>();
    read_files.sort_by(|left, right| left.encode_utf16().cmp(right.encode_utf16()));
    modified_files.sort_by(|left, right| left.encode_utf16().cmp(right.encode_utf16()));
    let base = strip_file_operation_tags(document);
    let files = format_file_operations(&read_files, &modified_files, &read);
    let summary = if files.is_empty() {
        base
    } else if base.is_empty() {
        files
    } else {
        format!("{base}\n\n{files}")
    };
    PreparedHandoffSummary { summary, read_files, modified_files }
}
