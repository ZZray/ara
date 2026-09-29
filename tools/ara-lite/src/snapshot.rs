//! Git workspace snapshot (Go: `gitSnapshot` / `gitSnapshotOnce` in `main.go`).
//!
//! The client captures repository evidence; the server never runs submitted
//! commands. The digest covers HEAD, branch, the staged diff, the index, and the
//! name, mode and content of every tracked or untracked-not-ignored file. The hash
//! input layout differs from the Go tool, so snapshots are stable within this
//! implementation but not byte-identical to Go's.

use crate::err;
use crate::error::{Error, Result};
use sha2::{Digest, Sha256};
use std::fs;
use std::io::Read;
use std::path::Path;
use std::process::{Command, Stdio};
use std::thread;
use std::time::{Duration, Instant};

const GIT_TIMEOUT: Duration = Duration::from_secs(20);

fn run_git(dir: &Path, args: &[&str]) -> Result<Vec<u8>> {
    let fail = |detail: String| err!("git snapshot: {detail} (workspace {})", dir.display());
    let mut child = Command::new("git")
        .arg("-C")
        .arg(dir)
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| fail(format!("cannot run git: {e}")))?;
    let mut stdout = child.stdout.take().ok_or_else(|| fail("no stdout".into()))?;
    let mut stderr = child.stderr.take().ok_or_else(|| fail("no stderr".into()))?;
    let out_reader = thread::spawn(move || {
        let mut out = Vec::new();
        let _ = stdout.read_to_end(&mut out);
        out
    });
    let err_reader = thread::spawn(move || {
        let mut out = Vec::new();
        let _ = stderr.read_to_end(&mut out);
        out
    });
    let deadline = Instant::now() + GIT_TIMEOUT;
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break status,
            Ok(None) if Instant::now() >= deadline => {
                let _ = child.kill();
                let _ = child.wait();
                return Err(fail(format!("git {} timed out after {}s", args.join(" "), GIT_TIMEOUT.as_secs())));
            }
            Ok(None) => thread::sleep(Duration::from_millis(5)),
            Err(e) => return Err(fail(format!("wait for git: {e}"))),
        }
    };
    let out = out_reader.join().unwrap_or_default();
    let stderr_text = err_reader.join().unwrap_or_default();
    if !status.success() {
        let text = String::from_utf8_lossy(&stderr_text);
        return Err(fail(format!("git {} failed: {status}: {}", args.join(" "), text.trim())));
    }
    Ok(out)
}

#[cfg(unix)]
fn permission_bits(meta: &fs::Metadata) -> u32 {
    use std::os::unix::fs::PermissionsExt;
    meta.permissions().mode() & 0o777
}

#[cfg(not(unix))]
fn permission_bits(meta: &fs::Metadata) -> u32 {
    if meta.permissions().readonly() { 0o444 } else { 0o666 }
}

fn snapshot_once(dir: &Path) -> Result<String> {
    let root = run_git(dir, &["rev-parse", "--show-toplevel"])?;
    let root_text = String::from_utf8_lossy(&root).trim().to_string();
    let root = Path::new(&root_text);
    let mut hasher = Sha256::new();
    for args in [
        &["rev-parse", "HEAD"][..],
        &["symbolic-ref", "--short", "HEAD"],
        &["diff", "--cached", "--binary"],
        &["ls-files", "--stage", "-z"],
    ] {
        hasher.update(run_git(root, args)?);
        hasher.update([0u8]);
    }
    let listing = run_git(root, &["ls-files", "--cached", "--others", "--exclude-standard", "-z"])?;
    let mut names: Vec<&[u8]> = listing.split(|b| *b == 0).collect();
    names.sort();
    let mut last: &[u8] = &[];
    let mut buf = vec![0u8; 64 << 10];
    for name in names {
        if name.is_empty() || name == last {
            continue;
        }
        last = name;
        hasher.update(name);
        hasher.update([0u8]);
        let text = std::str::from_utf8(name).map_err(|_| err!("snapshot unsupported file name (not UTF-8)"))?;
        let path = root.join(text);
        let meta = match fs::symlink_metadata(&path) {
            Ok(m) => m,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                hasher.update(b"deleted\0");
                continue;
            }
            Err(e) => return Err(Error::from(e)),
        };
        let kind = meta.file_type();
        hasher.update(format!("{}{:o}", if kind.is_symlink() { 'l' } else { '-' }, permission_bits(&meta)).as_bytes());
        if kind.is_symlink() {
            let target = fs::read_link(&path)?;
            hasher.update(target.to_string_lossy().as_bytes());
        } else if kind.is_file() {
            let mut file = fs::File::open(&path)?;
            loop {
                let n = file.read(&mut buf)?;
                if n == 0 {
                    break;
                }
                hasher.update(&buf[..n]);
            }
        } else {
            return Err(err!("snapshot unsupported file {text}"));
        }
        hasher.update([0u8]);
    }
    let head = run_git(root, &["rev-parse", "HEAD"])?;
    let branch = run_git(root, &["symbolic-ref", "--short", "HEAD"])?;
    let digest: String = hasher.finalize().iter().map(|b| format!("{b:02x}")).collect();
    Ok(format!("{}:{}:{digest}", String::from_utf8_lossy(&head).trim(), String::from_utf8_lossy(&branch).trim()))
}

/// Snapshot the repository containing `dir`. Captured twice; a difference means a
/// writer is active and the snapshot cannot be trusted.
pub fn git_snapshot(dir: &Path) -> Result<String> {
    let first = snapshot_once(dir)?;
    let second = snapshot_once(dir)?;
    if first != second {
        return Err(Error::new("workspace changed while capturing snapshot; freeze writers and retry"));
    }
    Ok(second)
}
