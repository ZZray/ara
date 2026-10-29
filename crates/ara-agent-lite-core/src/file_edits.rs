//! Optional, versioned single-file edit preparation. This is not a filesystem CAS or transaction.
use anyhow::{bail, ensure, Context, Result};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::{
    fs::OpenOptions,
    io::{Seek, SeekFrom, Write},
    path::Path,
};
use tokio_util::sync::CancellationToken;

const MAX_EDITS: usize = 64;
const MAX_ARGUMENT_BYTES: usize = 2 * 1024 * 1024;
const PREVIEW_BYTES: usize = 160;

pub(crate) fn flag(args: &Value, key: &str) -> Result<bool> {
    match args.get(key) {
        None => Ok(false),
        Some(Value::Bool(value)) => Ok(*value),
        Some(_) => bail!("{key} must be a boolean"),
    }
}

fn sha256(text: &str) -> String {
    data_encoding::HEXLOWER.encode(&Sha256::digest(text.as_bytes()))
}

pub(crate) fn snapshot(path: &Path, text: String) -> String {
    json!({"path":path,"sha256":sha256(&text),"bytes":text.len(),
        "content":text,"encoding":"utf-8","truncated":false})
    .to_string()
}

fn field<'a>(value: &'a Value, key: &str) -> Result<&'a str> {
    value.get(key).and_then(Value::as_str).with_context(|| format!("{key} must be a string"))
}

fn preview(text: &str) -> Value {
    let mut end = text.len().min(PREVIEW_BYTES);
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    json!({"text":&text[..end],"truncated":end < text.len()})
}

struct Change<'a> {
    start: usize,
    end: usize,
    old: &'a str,
    new: &'a str,
    /// 0 for the main block, k for `additional_edits[k-1]` (20.6).
    block: usize,
}
struct Prepared {
    content: String,
    ranges: Vec<Value>,
    before: String,
    candidate: String,
}

fn prepare(original: &str, args: &Value, expected: Option<&str>) -> Result<Prepared> {
    let before = sha256(original);
    if let Some(expected) = expected {
        ensure!(
            before == expected,
            "edit precondition failed: file SHA256 changed; read a fresh snapshot; no edit committed; current_sha256={before}"
        );
    }
    let mut blocks = vec![(field(args, "old_text")?, field(args, "new_text")?)];
    if let Some(value) = args.get("additional_edits") {
        let additional = value.as_array().context("additional_edits must be an array")?;
        ensure!(additional.len() < MAX_EDITS, "at most 64 edit blocks are allowed");
        for block in additional {
            ensure!(
                block
                    .as_object()
                    .is_some_and(|o| o.len() == 2 && o.contains_key("old_text") && o.contains_key("new_text")),
                "each additional edit must contain exactly old_text and new_text"
            );
            blocks.push((field(block, "old_text")?, field(block, "new_text")?));
        }
    }
    let mut bytes = 0usize;
    let mut changes = Vec::new();
    for (block, (old, new)) in blocks.into_iter().enumerate() {
        bytes = bytes
            .checked_add(old.len())
            .and_then(|n| n.checked_add(new.len()))
            .context("edit argument size overflow")?;
        ensure!(bytes <= MAX_ARGUMENT_BYTES, "edit text arguments exceed 2 MiB");
        ensure!(!old.is_empty(), "old_text must be non-empty");
        let start =
            original.find(old).with_context(|| format!("old_text was not found; no edit committed; block={block}"))?;
        // Search one Unicode boundary after the first start, so overlapping occurrences count too.
        let next = start + original[start..].chars().next().unwrap().len_utf8();
        ensure!(
            !original[next..].contains(old),
            "old_text must have exactly one occurrence, including overlaps; no edit committed; block={block}"
        );
        changes.push(Change { start, end: start + old.len(), old, new, block });
    }
    changes.sort_by_key(|change| change.start);
    if let Some(pair) = changes.windows(2).find(|pair| pair[0].end > pair[1].start) {
        bail!("edit blocks overlap; no edit committed; block={}", pair[0].block.max(pair[1].block));
    }
    let mut resulting_bytes = original.len();
    for change in &changes {
        resulting_bytes = resulting_bytes
            .checked_sub(change.old.len())
            .and_then(|n| n.checked_add(change.new.len()))
            .context("edited size overflow")?;
    }
    ensure!(resulting_bytes <= crate::local_tools::FILE_LIMIT, "edited file exceeds 1 MiB limit; no edit committed");
    let ranges = changes.iter().map(|change| {
        let start_line = original[..change.start].bytes().filter(|b| *b==b'\n').count()+1;
        json!({"start_line":start_line,"original_end_line":start_line+change.old.bytes().filter(|b| *b==b'\n').count(),
            "old":preview(change.old),"new":preview(change.new)})
    }).collect();
    let mut content = original.to_owned();
    for change in changes.into_iter().rev() {
        content.replace_range(change.start..change.end, change.new);
    }
    let candidate = sha256(&content);
    Ok(Prepared { content, ranges, before, candidate })
}

fn commit(
    file: &mut std::fs::File,
    content: &str,
    candidate: &str,
    write: impl FnOnce(&mut std::fs::File, &[u8]) -> std::io::Result<()>,
) -> Result<()> {
    let result = (|| -> Result<()> {
        write(file, content.as_bytes())?;
        file.set_len(content.len() as u64)?;
        file.sync_all()?;
        file.seek(SeekFrom::Start(0))?;
        let actual = crate::local_tools::read_text_handle(file)?;
        ensure!(sha256(&actual) == candidate, "readback differs from prepared content");
        Ok(())
    })();
    result.map_err(|error| anyhow::anyhow!("effects_unknown: edit commit or readback failed ({error}); the file may be partially changed; read a fresh snapshot and reconcile before any retry"))
}

pub(crate) fn edit(path: &Path, args: &Value, expected: Option<&str>, cancel: &CancellationToken) -> Result<String> {
    let dry_run = flag(args, "dry_run")?;
    ensure!(!cancel.is_cancelled(), "edit canceled before preparation; no edit committed");
    // No create/truncate. Read and commit through the same handle, retaining inode/ACL/hardlink semantics.
    let mut file =
        OpenOptions::new().read(true).write(!dry_run).open(path).context("open edit target; no edit committed")?;
    ensure!(file.metadata()?.is_file(), "edit target must be a regular file");
    let original = crate::local_tools::read_text_handle(&mut file)?;
    let prepared = prepare(&original, args, expected)?;
    ensure!(!cancel.is_cancelled(), "edit canceled before commit; no edit committed");
    let changed = prepared.content != original;
    let mut committed = false;
    let mut observed_after = None;
    if !dry_run && changed {
        file.seek(SeekFrom::Start(0))?;
        let current = crate::local_tools::read_text_handle(&mut file)?;
        ensure!(
            sha256(&current) == prepared.before,
            "file changed during preparation; read a fresh snapshot; no edit committed"
        );
        file.seek(SeekFrom::Start(0))?;
        ensure!(!cancel.is_cancelled(), "edit canceled before commit; no edit committed");
        commit(&mut file, &prepared.content, &prepared.candidate, |file, bytes| file.write_all(bytes))?;
        committed = true;
        observed_after = Some(prepared.candidate.clone());
    }
    // Once committed, cancellation cannot make this receipt claim no write happened.
    Ok(json!({"status":if dry_run {"preview"} else if changed {"applied"} else {"no_change"},
        "path":path,"committed":committed,"before_sha256":prepared.before,"candidate_sha256":prepared.candidate,
        "observed_after_sha256":observed_after,"bytes_before":original.len(),"bytes_candidate":prepared.content.len(),
        "edit_count":prepared.ranges.len(),"changes":prepared.ranges,
        "concurrency":"same_handle_recheck_not_filesystem_CAS; external_writers_or_path_replacement_can_race",
        "commit_policy":"in_place; IO_failure_can_leave_partial_effects"})
    .to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn rejections_name_the_current_hash_and_the_failing_block() {
        let stale = prepare("alpha beta", &json!({"old_text":"alpha","new_text":"a"}), Some(&"0".repeat(64)));
        let message = stale.err().unwrap().to_string();
        assert!(
            message.contains("SHA256 changed") && message.contains(&format!("current_sha256={}", sha256("alpha beta")))
        );
        let edits = |extra: Value| json!({"old_text":"alpha","new_text":"a","additional_edits":extra});
        let cases = [
            (edits(json!([{"old_text":"gamma","new_text":"g"}])), "old_text was not found; no edit committed; block=1"),
            (
                edits(json!([{"old_text":"beta","new_text":"b"},{"old_text":"a","new_text":"x"}])),
                "exactly one occurrence",
            ),
            (edits(json!([{"old_text":"pha b","new_text":"b"}])), "edit blocks overlap; no edit committed; block=1"),
        ];
        for (args, expected) in cases {
            let message = prepare("alpha beta", &args, None).err().unwrap().to_string();
            assert!(message.contains(expected), "{message}");
        }
        let missing = prepare("alpha", &json!({"old_text":"zeta","new_text":"z"}), None).err().unwrap();
        assert!(missing.to_string().ends_with("block=0"));
    }
    #[test]
    fn preparation_is_original_based_and_rejects_overlap_and_late_failure() {
        let args =
            json!({"old_text":"alpha","new_text":"beta","additional_edits":[{"old_text":"beta","new_text":"gamma"}]});
        assert_eq!(prepare("alpha beta", &args, None).unwrap().content, "beta gamma");
        assert!(prepare("aaa", &json!({"old_text":"aa","new_text":"b"}), None).is_err());
        assert!(prepare(
            "abcdef",
            &json!({"old_text":"abc","new_text":"x","additional_edits":[{"old_text":"bcd","new_text":"y"}]}),
            None
        )
        .is_err());
        assert!(prepare("alpha", &args, None).is_err());
    }
    #[test]
    fn source_bytes_limits_and_previews_are_not_normalized() {
        let source = "\u{feff}甲\r\n乙\r\n";
        let prepared = prepare(source, &json!({"old_text":"甲","new_text":"新"}), Some(&sha256(source))).unwrap();
        assert_eq!(prepared.content, "\u{feff}新\r\n乙\r\n");
        assert_ne!(sha256(source), sha256(&source.replace("\r\n", "\n")));
        assert!(prepare(source, &json!({"old_text":"甲","new_text":"x"}), Some(&"0".repeat(64))).is_err());
        assert!(prepare("x", &json!({"old_text":"x","new_text":"z".repeat(crate::local_tools::FILE_LIMIT+1)}), None)
            .is_err());
        let snippet = preview(&"甲".repeat(100));
        assert_eq!(snippet["truncated"], true);
        assert!(snippet["text"].as_str().unwrap().len() <= PREVIEW_BYTES);
    }

    #[test]
    fn injected_writer_failure_has_real_partial_effect_and_unknown_receipt() {
        let path = std::env::temp_dir().join(format!("lite-edit-fault-{}", crate::fresh_id()));
        std::fs::write(&path, b"abcdef").unwrap();
        let mut file = OpenOptions::new().read(true).write(true).open(&path).unwrap();
        let error = commit(&mut file, "xyz", &sha256("xyz"), |file, bytes| {
            file.write_all(&bytes[..1])?;
            Err(std::io::Error::other("controlled write fault after one real byte"))
        })
        .unwrap_err();
        assert!(error.to_string().contains("effects_unknown"));
        assert_eq!(std::fs::read(&path).unwrap(), b"xbcdef");
        drop(file);
        std::fs::remove_file(&path).unwrap();
    }
}
