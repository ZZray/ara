//! Fixed OMP file-operation lists and grouped-prefix rendering.
use crate::{CompactionDetails, FileOperations};
use regex::Regex;
use std::{
    collections::{BTreeMap, BTreeSet},
    sync::OnceLock,
};

pub fn create_file_ops() -> FileOperations {
    FileOperations::default()
}
pub fn is_url_scheme_path(path: &str) -> bool {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| Regex::new(r"(?i)[a-z][a-z0-9+.-]*://").expect("fixed URL scheme regex")).is_match(path)
}
fn sort_js(values: &mut [String]) {
    values.sort_by(|a, b| a.encode_utf16().cmp(b.encode_utf16()));
}
pub fn compute_file_lists(ops: &FileOperations) -> CompactionDetails {
    let modified: BTreeSet<_> = ops.edited.union(&ops.written).filter(|s| !is_url_scheme_path(s)).cloned().collect();
    let mut read_files: Vec<_> =
        ops.read.iter().filter(|s| !is_url_scheme_path(s) && !modified.contains(*s)).cloned().collect();
    let mut modified_files: Vec<_> = modified.into_iter().collect();
    sort_js(&mut read_files);
    sort_js(&mut modified_files);
    CompactionDetails { read_files, modified_files }
}
#[derive(Default)]
struct Node {
    files: Vec<(String, String)>,
    dirs: Vec<(String, Node)>,
}
impl Node {
    fn child(&mut self, name: &str) -> &mut Node {
        let at = match self.dirs.iter().position(|(n, _)| n == name) {
            Some(i) => i,
            None => {
                self.dirs.push((name.into(), Node::default()));
                self.dirs.len() - 1
            }
        };
        &mut self.dirs[at].1
    }
}
fn walk(node: &Node, depth: usize, mode: &BTreeMap<String, &'static str>, out: &mut Vec<String>) {
    for (name, key) in &node.files {
        out.push(format!("{name} ({})", mode[key]));
    }
    for (name, child) in &node.dirs {
        let mut parts = vec![name.as_str()];
        let mut dir = child;
        while dir.files.is_empty() && dir.dirs.len() == 1 {
            parts.push(&dir.dirs[0].0);
            dir = &dir.dirs[0].1;
        }
        out.push(format!("{} {}/", "#".repeat(depth + 1), parts.join("/")));
        walk(dir, depth + 1, mode, out);
    }
}
pub(crate) fn format_file_list(read: &[String], modified: &[String], read_set: Option<&BTreeSet<String>>) -> String {
    let mut mode = BTreeMap::new();
    for file in read {
        mode.insert(file.clone(), "Read");
    }
    for file in modified {
        mode.insert(file.clone(), if read_set.is_some_and(|s| s.contains(file)) { "RW" } else { "Write" });
    }
    let mut all: Vec<_> = mode.keys().cloned().collect();
    sort_js(&mut all);
    let mut root = Node::default();
    for raw in all.iter().take(20) {
        let normalized = raw.replace('\\', "/");
        let trimmed = normalized.strip_suffix('/').unwrap_or(&normalized);
        if trimmed.is_empty() {
            continue;
        }
        let parts: Vec<_> = trimmed.split('/').collect();
        let is_dir = raw.ends_with('/');
        let dir_count = if is_dir { parts.len() } else { parts.len() - 1 };
        let mut node = &mut root;
        for part in &parts[..dir_count] {
            node = node.child(part);
        }
        if !is_dir {
            let name = parts[parts.len() - 1];
            if !node.files.iter().any(|(n, _)| n == name) {
                node.files.push((name.into(), raw.clone()));
            }
        }
    }
    let mut out = Vec::new();
    walk(&root, 0, &mode, &mut out);
    if all.len() > 20 {
        out.push(format!("[…{} files elided…]", all.len() - 20));
    }
    out.join("\n")
}
pub fn upsert_file_operations(
    summary: &str,
    read: &[String],
    modified: &[String],
    read_set: Option<&BTreeSet<String>>,
) -> String {
    static RE: OnceLock<Regex> = OnceLock::new();
    let re = RE.get_or_init(|| {
        Regex::new(r"(?s)<files>.*?</files>\s*|<read-files>.*?</read-files>\s*|<modified-files>.*?</modified-files>\s*")
            .expect("fixed file tag regex")
    });
    let base = re.replace_all(summary, "");
    let base = ara_prompt::js::trim_end(&base);
    let files = format_file_list(read, modified, read_set);
    if files.is_empty() {
        return base.into();
    }
    let rendered = ara_prompt::render(include_str!("prompts/file-operations.md"), &serde_json::json!({"files": files}))
        .expect("fixed file operations template");
    if base.is_empty() { rendered } else { format!("{base}\n\n{rendered}") }
}
