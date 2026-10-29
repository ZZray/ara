//! Process-shared owned-entry scans, ported from fixed OMP
//! `crates/pi-walker/src/cache.rs` at 596f2da7101178214aa27a753529d15e6b7ad91d.
//! Consumers filter/rank/limit outside this cache. A fresh cache=false scan
//! neither consumes nor updates the shared owner. No OS sandbox is implied.
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

use crate::{Budget, CollectOptions, CollectedEntries, CollectedEntry, FollowLinks, WalkError};
use std::{
    collections::HashMap,
    path::{Component, Path, PathBuf},
    sync::{LazyLock, Mutex, MutexGuard},
    time::{Duration, Instant},
};

#[derive(Clone, Debug, Eq, Hash, PartialEq)]
struct CacheKey {
    root: PathBuf,
    options: CollectOptions,
    // Canonical aliases normally share scans. Never on a symlink operand is
    // an empty traversal, distinct from Never on its canonical directory.
    unfollowed_root: bool,
}

#[derive(Clone)]
struct CacheEntry {
    created_at: Instant,
    sequence: u64,
    entries: Vec<CollectedEntry>,
}

#[derive(Default)]
struct ScanCache {
    entries: HashMap<CacheKey, CacheEntry>,
    epoch: u64,
    sequence: u64,
}

static SCAN_CACHE: LazyLock<Mutex<ScanCache>> = LazyLock::new(|| Mutex::new(ScanCache::default()));
// ARA naming is explicit; upstream names are evidence, not alternate controls.
// FS_SCAN_CACHE_TTL_MS -> ARA_FS_SCAN_CACHE_TTL_MS
// FS_SCAN_EMPTY_RECHECK_MS -> ARA_FS_SCAN_EMPTY_RECHECK_MS
// FS_SCAN_CACHE_MAX_ENTRIES -> ARA_FS_SCAN_CACHE_MAX_ENTRIES
// PI_WALK_WORKERS -> ARA_WALK_WORKERS
static CACHE_TTL_MS: LazyLock<u64> = LazyLock::new(|| env_uint("ARA_FS_SCAN_CACHE_TTL_MS", 1_000));
static EMPTY_RECHECK_MS: LazyLock<u64> = LazyLock::new(|| env_uint("ARA_FS_SCAN_EMPTY_RECHECK_MS", 200));
static MAX_CACHE_ENTRIES: LazyLock<usize> = LazyLock::new(|| env_uint("ARA_FS_SCAN_CACHE_MAX_ENTRIES", 16));
static WALK_WORKERS: LazyLock<usize> = LazyLock::new(|| {
    let configured = env_uint("ARA_WALK_WORKERS", 4_usize);
    if configured == 0 { std::thread::available_parallelism().map_or(4, usize::from).max(1) } else { configured.max(1) }
});

fn env_uint<T: Copy + std::str::FromStr>(name: &str, default: T) -> T {
    std::env::var(name).ok().and_then(|value| value.parse().ok()).unwrap_or(default)
}
fn state() -> MutexGuard<'static, ScanCache> {
    SCAN_CACHE.lock().unwrap_or_else(std::sync::PoisonError::into_inner)
}

pub fn cache_ttl_ms() -> u64 {
    *CACHE_TTL_MS
}
pub fn empty_recheck_ms() -> u64 {
    *EMPTY_RECHECK_MS
}
pub fn max_cache_entries() -> usize {
    *MAX_CACHE_ENTRIES
}
/// Effective configured count. 0 selects available parallelism. This does not
/// claim a parallel scanner/pool: the existing visitor scanner remains serial.
pub fn walk_workers() -> usize {
    *WALK_WORKERS
}

/// Actual process cwd for relative paths, canonical-parent fallback for a
/// deleted/moved operand; preserve the leaf even when it no longer exists.
pub(crate) fn absolute_path(path: &Path) -> PathBuf {
    if path.is_absolute() { path.to_owned() } else { std::env::current_dir().unwrap_or_default().join(path) }
}
pub(crate) fn resolve_path(path: &Path) -> PathBuf {
    let absolute = absolute_path(path);
    // Resolve existing symlink/.. components using the filesystem before any
    // lexical cleanup; collapsing them first can select a different directory.
    if let Ok(real) = std::fs::canonicalize(&absolute) {
        return real;
    }
    let mut ancestor = absolute.as_path();
    let mut suffix = Vec::new();
    while let Some(parent) = ancestor.parent() {
        if let Some(name) = ancestor.file_name() {
            suffix.push(name.to_os_string());
        }
        ancestor = parent;
        if let Ok(mut real) = std::fs::canonicalize(ancestor) {
            for name in suffix.into_iter().rev() {
                real.push(name);
            }
            return real;
        }
    }
    let mut normalized = PathBuf::new();
    for component in absolute.components() {
        match component {
            Component::CurDir => {}
            Component::ParentDir => {
                normalized.pop();
            }
            _ => normalized.push(component.as_os_str()),
        }
    }
    normalized
}

pub fn collect_entries(
    root: &Path,
    mut options: CollectOptions,
    budget: &Budget,
) -> Result<CollectedEntries, WalkError> {
    // Pre-check is mandatory even for an already-populated cached hit.
    budget.check()?;
    let operand = absolute_path(root);
    if !options.cache || cache_ttl_ms() == 0 || max_cache_entries() == 0 {
        return crate::collect_entries_uncached(&operand, options, budget);
    }
    options.cache = false;
    let canonical_root = resolve_path(&operand);
    let unfollowed_root = options.follow_links == FollowLinks::Never
        && std::fs::symlink_metadata(&operand).is_ok_and(|metadata| metadata.is_symlink());
    let key = CacheKey { root: canonical_root, options, unfollowed_root };
    budget.check()?;
    let now = Instant::now();
    let epoch = {
        let mut cache = state();
        if let Some(entry) = cache.entries.get(&key) {
            let age = now.saturating_duration_since(entry.created_at);
            if age < Duration::from_millis(cache_ttl_ms()) {
                let result = CollectedEntries {
                    entries: entry.entries.clone(),
                    cache_age_ms: age.as_millis().min(u128::from(u64::MAX)) as u64,
                };
                budget.check()?;
                return Ok(result);
            }
            cache.entries.remove(&key);
        }
        cache.epoch
    };
    let scan = crate::collect_entries_uncached(&operand, options, budget)?;
    budget.check()?;
    let mut cache = state();
    budget.check()?;
    // Invalidating any path also fences earlier in-flight scans. Existing
    // unrelated roots survive, but an old scan cannot install after the hook.
    let mut installed = None;
    if cache.epoch == epoch {
        cache.sequence = cache.sequence.wrapping_add(1);
        let sequence = cache.sequence;
        cache.entries.insert(key.clone(), CacheEntry { created_at: now, sequence, entries: scan.entries.clone() });
        installed = Some(sequence);
        while cache.entries.len() > max_cache_entries() {
            let oldest = cache
                .entries
                .iter()
                .min_by_key(|(_, entry)| (entry.created_at, entry.sequence))
                .map(|(key, _)| key.clone());
            if let Some(oldest) = oldest {
                cache.entries.remove(&oldest);
            } else {
                break;
            }
        }
    }
    if let Err(error) = budget.check() {
        if installed.is_some_and(|sequence| cache.entries.get(&key).is_some_and(|entry| entry.sequence == sequence)) {
            cache.entries.remove(&key);
        }
        return Err(error);
    }
    Ok(scan)
}

/// Invalidate only existing roots containing the resolved target. Epoch fencing
/// prevents an in-flight scan from undoing this invalidation when it finishes.
pub fn invalidate_path(path: &Path) {
    let target = resolve_path(path);
    let mut cache = state();
    cache.epoch = cache.epoch.wrapping_add(1);
    cache.entries.retain(|key, _| !target.starts_with(&key.root));
}
pub fn invalidate_path_string(path: &str) {
    invalidate_path(Path::new(path));
}
pub fn invalidate_all() {
    let mut cache = state();
    cache.epoch = cache.epoch.wrapping_add(1);
    cache.entries.clear();
}
