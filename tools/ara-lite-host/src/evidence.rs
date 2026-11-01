//! Explicit, versioned source access. A source never grants execution authority.
use crate::{journal::Journal, types::EvidenceSource};
use anyhow::{ensure, Context, Result};
use serde::Serialize;
use sha2::{Digest, Sha256};
use std::io::Read;

pub const MAX_SOURCE_BYTES: usize = 8 * 1024 * 1024;
pub const MAX_WINDOW_BYTES: usize = 16 * 1024;

#[derive(Debug, Serialize)]
pub struct Window {
    pub source_id: String,
    pub version: u64,
    pub sha256: String,
    pub offset: usize,
    pub next_offset: usize,
    pub bytes: usize,
    pub truncated: bool,
    pub text: String,
}

fn snapshot(
    journal: &Journal,
    reader: &str,
    generation: i64,
    id: &str,
    version: u64,
) -> Result<(EvidenceSource, String)> {
    let binding = journal.binding(reader)?.context("unknown reader Task")?;
    ensure!(binding.generation == generation, "reader generation changed");
    let source = journal.source(id)?.context("unknown source")?;
    ensure!(source.active && source.version == version, "source revoked or version changed");
    let owner = journal.binding(&source.task_id)?.context("source owner unavailable")?;
    ensure!(owner.generation == source.generation, "source owner generation changed");
    ensure!(source.task_id == reader || source.allowed_tasks.iter().any(|task| task == reader), "source access denied");
    let file = std::fs::File::open(&source.path)?;
    let mut bytes = Vec::new();
    file.take((MAX_SOURCE_BYTES + 1) as u64).read_to_end(&mut bytes)?;
    ensure!(bytes.len() <= MAX_SOURCE_BYTES, "source exceeds snapshot limit");
    ensure!(format!("{:x}", Sha256::digest(&bytes)) == source.sha256, "source content changed");
    Ok((source, String::from_utf8(bytes).context("source is not UTF-8")?))
}

pub fn read(
    journal: &Journal,
    task: &str,
    generation: i64,
    id: &str,
    version: u64,
    offset: usize,
    limit: usize,
) -> Result<Window> {
    ensure!((1..=MAX_WINDOW_BYTES).contains(&limit), "invalid source window limit");
    let (source, text) = snapshot(journal, task, generation, id, version)?;
    ensure!(offset <= text.len() && text.is_char_boundary(offset), "invalid UTF-8 source offset");
    let mut end = offset.saturating_add(limit).min(text.len());
    while end > offset && !text.is_char_boundary(end) {
        end -= 1;
    }
    ensure!(end > offset || offset == text.len(), "window cannot fit next UTF-8 character");
    Ok(Window {
        source_id: id.into(),
        version,
        sha256: source.sha256,
        offset,
        next_offset: end,
        bytes: text.len(),
        truncated: end < text.len(),
        text: text[offset..end].into(),
    })
}

pub fn search(
    journal: &Journal,
    task: &str,
    generation: i64,
    id: &str,
    version: u64,
    query: &str,
) -> Result<Option<usize>> {
    ensure!(!query.is_empty() && query.len() <= 1024, "invalid source query");
    let (_, text) = snapshot(journal, task, generation, id, version)?;
    Ok(text.find(query))
}
