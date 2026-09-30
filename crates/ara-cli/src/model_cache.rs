//! Native model metadata cache from fixed OMP
//! `596f2da7101178214aa27a753529d15e6b7ad91d`,
//! `packages/catalog/src/model-cache.ts`.
//!
//! Hosts supply the path and own the shared instance. Stored rows contain
//! sparse ModelSpec JSON, never executable routes. Network discovery, rebuilding
//! resolved models, restoring headers and computing static fingerprints belong
//! to the model manager. Unlike upstream best-effort catches, failures return
//! Result so a host can report them while continuing model resolution.
//!
//! MIT License
//! Copyright (c) 2025 Mario Zechner
//! Copyright (c) 2025-2026 Can Bölük
//! Copyright (c) 2026 Stencil Labs, Inc.
//! Permission is hereby granted, free of charge, to any person obtaining a copy
//! of this software and associated documentation files (the "Software"), to deal
//! in the Software without restriction, including without limitation the rights
//! to use, copy, modify, merge, publish, distribute, sublicense, and/or sell
//! copies of the Software, and to permit persons to whom the Software is
//! furnished to do so, subject to the following conditions:
//! The above copyright notice and this permission notice shall be included in
//! all copies or substantial portions of the Software.
//! THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF ANY KIND, EXPRESS OR
//! IMPLIED, INCLUDING BUT NOT LIMITED TO THE WARRANTIES OF MERCHANTABILITY,
//! FITNESS FOR A PARTICULAR PURPOSE AND NONINFRINGEMENT. IN NO EVENT SHALL THE
//! AUTHORS OR COPYRIGHT HOLDERS BE LIABLE FOR ANY CLAIM, DAMAGES OR OTHER
//! LIABILITY, WHETHER IN AN ACTION OF CONTRACT, TORT OR OTHERWISE, ARISING FROM,
//! OUT OF OR IN CONNECTION WITH THE SOFTWARE OR THE USE OR OTHER DEALINGS IN
//! THE SOFTWARE.

use anyhow::{Context, Result};
use rusqlite::{Connection, OptionalExtension, params};
use serde_json::{Map, Value};
use std::cell::RefCell;
use std::collections::{BTreeSet, HashMap};
use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};
use std::time::Duration;

pub const CACHE_SCHEMA_VERSION: i64 = 12;
pub const HEADER_RESTORE_VERSION: i64 = 1;
const BUSY_TIMEOUT: Duration = Duration::from_millis(3000);

/// All model metadata survives as JSON. As in the upstream JSON.parse contract,
/// a malformed-but-valid non-array value is not normalized by the cache reader.
/// The manager owns model-list validation and rebuilding.
#[derive(Clone)]
pub struct CacheEntry {
    pub models: Value,
    pub fresh: bool,
    pub authoritative: bool,
    pub updated_at: f64,
    pub header_omitted_model_ids: Vec<String>,
    pub unrestorable_header_model_ids: Vec<String>,
    pub legacy_header_restore_markers: bool,
    pub static_fingerprint: String,
}

/// Runtime headers never enter the persisted metadata. The optional trusted
/// fallback is compared only when no same-id or request-model static source exists.
#[derive(Default)]
pub struct ModelCacheWriteOptions<'a> {
    pub authoritative: bool,
    pub static_fingerprint: &'a str,
    pub static_header_sources: &'a [Value],
    pub restorable_header_fallback: Option<&'a Map<String, Value>>,
}

/// A host can log the first corruption at error level, subsequent recoveries at
/// debug level, matching upstream's process-wide reported-path policy. This
/// receipt contains paths and SQLite/IO categories, never cached model values.
#[derive(Clone, Debug)]
pub struct ModelCacheRecovery {
    pub database_path: PathBuf,
    pub sqlite_code: i32,
    pub previously_reported: bool,
    pub quarantined_paths: Vec<PathBuf>,
    pub quarantine_failures: Vec<(PathBuf, std::io::ErrorKind)>,
}

struct CacheRow {
    version: i64,
    updated_at: f64,
    authoritative: i64,
    static_fingerprint: Option<String>,
    models: String,
    header_omitted_model_ids: String,
    unrestorable_header_model_ids: String,
    header_restore_version: i64,
}

/// One host-owned cache. Share through a host mutex if needed; there are no
/// network calls, command execution, or locks held across await points here.
pub struct SqliteModelCache {
    path: PathBuf,
    shared: bool,
    connection: RefCell<Option<Connection>>,
    recoveries: RefCell<Vec<ModelCacheRecovery>>,
}

impl SqliteModelCache {
    /// Open a retained connection, equivalent to upstream's shared default-path
    /// handle. The host injects the default path instead of this module finding it.
    pub fn open(path: impl AsRef<Path>) -> Result<Self> {
        let cache = Self {
            path: path.as_ref().to_owned(),
            shared: true,
            connection: RefCell::new(None),
            recoveries: RefCell::new(Vec::new()),
        };
        cache.with_db(|_| Ok(()))?;
        Ok(cache)
    }

    /// Upstream's explicit dbPath opens and closes the database per operation.
    /// Opening remains lazy, so both reads and writes exercise corruption healing.
    pub fn for_path(path: impl AsRef<Path>) -> Self {
        Self {
            path: path.as_ref().to_owned(),
            shared: false,
            connection: RefCell::new(None),
            recoveries: RefCell::new(Vec::new()),
        }
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    pub fn take_recovery_notices(&self) -> Vec<ModelCacheRecovery> {
        std::mem::take(&mut *self.recoveries.borrow_mut())
    }

    /// Release a retained handle. A later operation reopens it, as upstream does
    /// after switching the shared path or repairing a corrupt database.
    pub fn release_connection(&self) {
        self.connection.borrow_mut().take();
    }

    fn run_db<T>(&self, use_db: &mut impl FnMut(&Connection) -> Result<T>) -> Result<T> {
        if self.shared {
            let mut slot = self.connection.borrow_mut();
            if slot.is_none() {
                *slot = Some(open_db(&self.path)?);
            }
            use_db(slot.as_ref().expect("connection initialized"))
        } else {
            let db = open_db(&self.path)?;
            use_db(&db)
        }
    }

    fn with_db<T>(&self, mut use_db: impl FnMut(&Connection) -> Result<T>) -> Result<T> {
        match self.run_db(&mut use_db) {
            Ok(value) => Ok(value),
            Err(error) if crate::credential_store::is_sqlite_corruption_error(&error) => {
                self.release_connection();
                let code = error
                    .downcast_ref::<rusqlite::Error>()
                    .and_then(|error| match error {
                        rusqlite::Error::SqliteFailure(code, _) => Some(code.extended_code),
                        _ => None,
                    })
                    .unwrap_or(0);
                let recovery = quarantine_corrupt_cache(&self.path, code);
                self.recoveries.borrow_mut().push(recovery);
                // Exactly one retry. Permission, BUSY and malformed JSON never
                // quarantine a database; failed recovery is returned to the host.
                self.run_db(&mut use_db)
            }
            Err(error) => Err(error),
        }
    }

    pub fn read_model_cache(
        &self,
        provider_id: &str,
        ttl_ms: f64,
        mut now: impl FnMut() -> f64,
    ) -> Result<Option<CacheEntry>> {
        let row = self.with_db(|db| {
            Ok(db.query_row(
                "SELECT version,updated_at,authoritative,static_fingerprint,models,header_omitted_model_ids,unrestorable_header_model_ids,header_restore_version FROM model_cache WHERE provider_id=?1",
                [provider_id],
                |row| Ok(CacheRow {
                    version: row.get(0)?,
                    updated_at: row.get(1)?,
                    authoritative: row.get(2)?,
                    static_fingerprint: row.get(3)?,
                    models: row.get(4)?,
                    header_omitted_model_ids: row.get(5)?,
                    unrestorable_header_model_ids: row.get(6)?,
                    header_restore_version: row.get(7)?,
                }),
            ).optional()?)
        })?;
        let Some(row) = row.filter(|row| row.version == CACHE_SCHEMA_VERSION) else {
            return Ok(None);
        };
        let models = serde_json::from_str(&row.models).context("parse cached model metadata")?;
        let header_omitted_model_ids = parse_string_ids(&row.header_omitted_model_ids)?;
        let unrestorable_header_model_ids = parse_string_ids(&row.unrestorable_header_model_ids)?;
        // The injected clock runs after the SQLite borrow has been released.
        let age_ms = now() - row.updated_at;
        Ok(Some(CacheEntry {
            models,
            fresh: age_ms.is_finite() && age_ms >= 0.0 && age_ms <= ttl_ms,
            authoritative: row.authoritative == 1,
            updated_at: row.updated_at,
            header_omitted_model_ids,
            unrestorable_header_model_ids,
            legacy_header_restore_markers: row.header_restore_version < HEADER_RESTORE_VERSION,
            static_fingerprint: row.static_fingerprint.unwrap_or_default(),
        }))
    }

    pub fn write_model_cache(
        &self,
        provider_id: &str,
        updated_at: f64,
        models: &[Value],
        options: ModelCacheWriteOptions<'_>,
    ) -> Result<()> {
        let mut static_by_id = HashMap::new();
        for model in options.static_header_sources {
            let record = model.as_object().context("static header source is not a model object")?;
            let id =
                record.get("id").and_then(Value::as_str).context("static header source model id is not a string")?;
            static_by_id.insert(id, record);
        }
        let mut omitted = Vec::new();
        let mut unrestorable = Vec::new();
        let mut cached = Vec::with_capacity(models.len());
        let fallback = options.restorable_header_fallback.map(|headers| Value::Object(headers.clone()));
        for model in models {
            let record = model.as_object().context("cache write is not a model object")?;
            let id = record.get("id").and_then(Value::as_str).context("cache model id is not a string")?;
            if has_model_headers(record.get("headers")) {
                omitted.push(id);
                let source = static_by_id.get(id).or_else(|| {
                    record
                        .get("requestModelId")
                        .and_then(Value::as_str)
                        .filter(|id| !id.is_empty())
                        .and_then(|id| static_by_id.get(id))
                });
                let matching = match source {
                    Some(source) => headers_equal(record.get("headers"), source.get("headers")),
                    None => headers_equal(record.get("headers"), fallback.as_ref()),
                };
                if !matching {
                    unrestorable.push(id);
                }
            }
            cached.push(to_cached_model_spec(record));
        }
        let models_json = serde_json::to_string(&cached)?;
        let omitted_json = serde_json::to_string(&omitted)?;
        let unrestorable_json = serde_json::to_string(&unrestorable)?;
        self.with_db(|db| {
            db.execute("INSERT OR REPLACE INTO model_cache(provider_id,version,updated_at,authoritative,static_fingerprint,header_omitted_model_ids,unrestorable_header_model_ids,header_restore_version,models) VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9)",
                params![provider_id,CACHE_SCHEMA_VERSION,updated_at,i64::from(options.authoritative),options.static_fingerprint,omitted_json,unrestorable_json,HEADER_RESTORE_VERSION,models_json])?;
            Ok(())
        })
    }
}

fn open_db(path: &Path) -> Result<Connection> {
    let db = Connection::open(path).context("open model cache")?;
    // Busy handler precedes every lock-taking statement, including WAL/schema.
    db.busy_timeout(BUSY_TIMEOUT)?;
    db.execute_batch(
        "PRAGMA secure_delete=ON; PRAGMA journal_mode=WAL;
        CREATE TABLE IF NOT EXISTS model_cache(
            provider_id TEXT PRIMARY KEY,
            version INTEGER NOT NULL,
            updated_at INTEGER NOT NULL,
            authoritative INTEGER NOT NULL DEFAULT 0,
            static_fingerprint TEXT NOT NULL DEFAULT '',
            header_omitted_model_ids TEXT NOT NULL DEFAULT '[]',
            unrestorable_header_model_ids TEXT NOT NULL DEFAULT '[]',
            header_restore_version INTEGER NOT NULL DEFAULT 0,
            models TEXT NOT NULL
        );",
    )?;
    let mut statement = db.prepare("PRAGMA table_info(model_cache)")?;
    let columns = statement.query_map([], |row| row.get::<_, String>(1))?.collect::<rusqlite::Result<BTreeSet<_>>>()?;
    drop(statement);
    for (name, declaration) in [
        ("static_fingerprint", "TEXT NOT NULL DEFAULT ''"),
        ("header_omitted_model_ids", "TEXT NOT NULL DEFAULT '[]'"),
        ("unrestorable_header_model_ids", "TEXT NOT NULL DEFAULT '[]'"),
        ("header_restore_version", "INTEGER NOT NULL DEFAULT 0"),
    ] {
        if !columns.contains(name) {
            db.execute_batch(&format!("ALTER TABLE model_cache ADD COLUMN {name} {declaration}"))?;
        }
    }
    // Invalidate every non-current version, including future rows, exactly as
    // fixed upstream. Never silently promote a stale row to the current version.
    db.execute("DELETE FROM model_cache WHERE version<>?1", [CACHE_SCHEMA_VERSION])?;
    Ok(db)
}

fn parse_string_ids(text: &str) -> Result<Vec<String>> {
    let value: Value = serde_json::from_str(text).context("parse cached model header markers")?;
    Ok(value
        .as_array()
        .map(|ids| ids.iter().filter_map(Value::as_str).map(str::to_owned).collect())
        .unwrap_or_default())
}

fn has_model_headers(value: Option<&Value>) -> bool {
    match value {
        Some(Value::Object(headers)) => !headers.is_empty(),
        Some(Value::Array(headers)) => !headers.is_empty(),
        Some(Value::String(headers)) => !headers.is_empty(),
        _ => false,
    }
}

fn headers_equal(left: Option<&Value>, right: Option<&Value>) -> bool {
    // Header records are provider-supplied string maps. Map equality is by keys
    // and exact values, independent of insertion order, like the two JS loops.
    left == right
}

fn to_cached_model_spec(model: &Map<String, Value>) -> Value {
    let mut spec = model.clone();
    spec.remove("headers");
    let compat = spec.remove("compatConfig");
    let computer_use = spec.remove("supportsComputerUseConfig");
    // Even an absent Config replaces its materialized value with JS undefined,
    // which JSON.stringify omits. Explicit null/false Config values survive.
    spec.remove("compat");
    spec.remove("supportsComputerUse");
    if let Some(computer_use) = computer_use {
        spec.insert("supportsComputerUse".into(), computer_use);
    }
    if let Some(compat) = compat {
        spec.insert("compat".into(), compat);
    }
    Value::Object(spec)
}

fn with_suffix(path: &Path, suffix: &str) -> PathBuf {
    let mut value: OsString = path.as_os_str().to_owned();
    value.push(suffix);
    value.into()
}

fn quarantine_corrupt_cache(path: &Path, code: i32) -> ModelCacheRecovery {
    static REPORTED: OnceLock<Mutex<BTreeSet<PathBuf>>> = OnceLock::new();
    let reported = REPORTED.get_or_init(|| Mutex::new(BTreeSet::new()));
    let previously_reported = !reported.lock().unwrap_or_else(|error| error.into_inner()).insert(path.to_owned());
    let stamp = chrono::Utc::now().timestamp_millis();
    let mut notice = ModelCacheRecovery {
        database_path: path.to_owned(),
        sqlite_code: code,
        previously_reported,
        quarantined_paths: Vec::new(),
        quarantine_failures: Vec::new(),
    };
    for suffix in ["", "-wal", "-shm"] {
        let original = with_suffix(path, suffix);
        let quarantined = with_suffix(path, &format!(".corrupt-{stamp}{suffix}"));
        match std::fs::rename(&original, &quarantined) {
            Ok(()) => notice.quarantined_paths.push(quarantined),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => notice.quarantine_failures.push((original, error.kind())),
        }
    }
    notice
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn fixture_model(id: &str) -> Value {
        json!({"id":id,"provider":"fixture","name":id,"api":"openai-completions",
            "baseUrl":"https://fixture.invalid/v1","reasoning":false,"input":["text","image"],
            "cost":{"input":0,"output":0,"cacheRead":null,"cacheWrite":0},
            "contextWindow":4096,"maxTokens":1024,"compat":{"resolved":true},
            "supportsComputerUse":true,"tokenizer":"fixture-future","extra":{"nested":[false,null,7]}})
    }

    fn write(cache: &SqliteModelCache, id: &str) {
        cache
            .write_model_cache(
                "fixture",
                1000.0,
                &[fixture_model(id)],
                ModelCacheWriteOptions { authoritative: true, static_fingerprint: "static-v3", ..Default::default() },
            )
            .unwrap();
    }

    fn corrupt_pages(path: &Path) {
        let db = Connection::open(path).unwrap();
        db.execute_batch("PRAGMA wal_checkpoint(TRUNCATE)").unwrap();
        drop(db);
        let mut bytes = std::fs::read(path).unwrap();
        assert!(bytes.len() > 100);
        bytes[100..].fill(0xff);
        std::fs::write(path, bytes).unwrap();
    }

    #[test]
    fn native_round_trip_preserves_metadata_and_sparse_spec_provenance() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("models.db");
        let cache = SqliteModelCache::open(&path).unwrap();
        let mut model = fixture_model("complete");
        model["compatConfig"] = json!({"sparse":false,"future":{"wire":"retained"}});
        model["supportsComputerUseConfig"] = json!(false);
        cache
            .write_model_cache(
                "fixture",
                1000.5,
                &[model.clone()],
                ModelCacheWriteOptions {
                    authoritative: true,
                    static_fingerprint: "literal fingerprint:with metadata",
                    ..Default::default()
                },
            )
            .unwrap();
        cache.release_connection();
        let reopened = SqliteModelCache::open(&path).unwrap();
        let entry = reopened.read_model_cache("fixture", 10.0, || 1005.5).unwrap().unwrap();
        assert!(entry.authoritative && entry.fresh && !entry.legacy_header_restore_markers);
        assert_eq!(entry.updated_at, 1000.5);
        assert_eq!(entry.static_fingerprint, "literal fingerprint:with metadata");
        let mut expected = model.as_object().unwrap().clone();
        expected.remove("compatConfig");
        expected.remove("supportsComputerUseConfig");
        expected.insert("compat".into(), json!({"sparse":false,"future":{"wire":"retained"}}));
        expected.insert("supportsComputerUse".into(), json!(false));
        assert_eq!(entry.models, json!([expected]));
        assert!(entry.header_omitted_model_ids.is_empty());
        assert!(entry.unrestorable_header_model_ids.is_empty());
    }

    #[test]
    fn absent_configs_remove_materialized_values_and_explicit_null_survives() {
        let first = fixture_model("derived");
        let spec = to_cached_model_spec(first.as_object().unwrap());
        assert!(spec.get("compat").is_none());
        assert!(spec.get("supportsComputerUse").is_none());
        assert_eq!(spec["extra"], first["extra"]);
        let mut explicit = first;
        explicit["compatConfig"] = Value::Null;
        explicit["supportsComputerUseConfig"] = Value::Null;
        let spec = to_cached_model_spec(explicit.as_object().unwrap());
        assert_eq!(spec.get("compat"), Some(&Value::Null));
        assert_eq!(spec.get("supportsComputerUse"), Some(&Value::Null));
    }

    #[test]
    fn omits_every_header_before_persisting_fixed_issue_5780() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("models.db");
        let cache = SqliteModelCache::for_path(&path);
        let mut model = fixture_model("gated-model");
        model["headers"] = json!({"Authorization":"Bearer standard-secret","X-Goog-Api-Key":"google-secret",
            "X-Access-Token":"access-secret","X-Project-Id":"proj-42","Unknown-Credential-Name":"arbitrary-secret"});
        cache
            .write_model_cache(
                "runtime-ext",
                1000.0,
                &[model],
                ModelCacheWriteOptions { authoritative: true, static_fingerprint: "static-v1", ..Default::default() },
            )
            .unwrap();
        let db = Connection::open(&path).unwrap();
        let raw: String = db.query_row("SELECT models FROM model_cache", [], |row| row.get(0)).unwrap();
        for secret in ["standard-secret", "google-secret", "access-secret", "proj-42", "arbitrary-secret"] {
            assert!(!raw.contains(secret));
        }
        drop(db);
        let entry = cache.read_model_cache("runtime-ext", 100.0, || 1000.0).unwrap().unwrap();
        assert!(entry.models[0].get("headers").is_none());
        assert_eq!(entry.header_omitted_model_ids, ["gated-model"]);
        assert_eq!(entry.unrestorable_header_model_ids, ["gated-model"]);
    }

    #[test]
    fn static_request_model_and_provider_fallback_matching_preserve_priority() {
        let dir = tempfile::tempdir().unwrap();
        let cache = SqliteModelCache::for_path(dir.path().join("models.db"));
        let headers = json!({"X-Route":"trusted","X-Variant":"v1"});
        let mut base = fixture_model("base");
        base["headers"] = headers.clone();
        let mut variant = fixture_model("variant");
        variant["headers"] = json!({"X-Variant":"v1","X-Route":"trusted"});
        variant["requestModelId"] = json!("base");
        let mut mismatched = fixture_model("same-id");
        mismatched["headers"] = headers.clone();
        mismatched["requestModelId"] = json!("base");
        let mut mismatch_source = fixture_model("same-id");
        mismatch_source["headers"] = json!({"X-Route":"different"});
        let mut dynamic = fixture_model("dynamic-only");
        dynamic["headers"] = headers.clone();
        let mut headerless_source = fixture_model("no-static-headers");
        headerless_source["headers"] = headers.clone();
        let sources = [base.clone(), mismatch_source, fixture_model("no-static-headers")];
        cache
            .write_model_cache(
                "fixture",
                1000.0,
                &[base, variant, mismatched, dynamic, headerless_source],
                ModelCacheWriteOptions {
                    authoritative: true,
                    static_fingerprint: "fp",
                    static_header_sources: &sources,
                    restorable_header_fallback: headers.as_object(),
                },
            )
            .unwrap();
        let entry = cache.read_model_cache("fixture", 10.0, || 1000.0).unwrap().unwrap();
        assert_eq!(entry.header_omitted_model_ids, ["base", "variant", "same-id", "dynamic-only", "no-static-headers"]);
        // Existing same-id static sources take priority over requestModelId and
        // provider fallback, even when their headers differ or are absent.
        assert_eq!(entry.unrestorable_header_model_ids, ["same-id", "no-static-headers"]);
    }

    #[test]
    fn duplicate_static_sources_last_wins_and_header_markers_keep_input_order() {
        let dir = tempfile::tempdir().unwrap();
        let cache = SqliteModelCache::for_path(dir.path().join("models.db"));
        let mut live = fixture_model("same");
        live["headers"] = json!({"X-Route":"last"});
        let mut first = live.clone();
        first["headers"] = json!({"X-Route":"first"});
        cache
            .write_model_cache(
                "fixture",
                1000.0,
                &[live.clone(), live.clone()],
                ModelCacheWriteOptions { static_header_sources: &[first, live], ..Default::default() },
            )
            .unwrap();
        let entry = cache.read_model_cache("fixture", 0.0, || 1000.0).unwrap().unwrap();
        assert_eq!(entry.header_omitted_model_ids, ["same", "same"]);
        assert!(entry.unrestorable_header_model_ids.is_empty());
        assert!(!entry.authoritative);
    }

    #[test]
    fn invalidates_and_scrubs_pre_v10_rows_and_every_non_current_version() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("models.db");
        let db = Connection::open(&path).unwrap();
        db.execute_batch(
            "CREATE TABLE model_cache(provider_id TEXT PRIMARY KEY,version INTEGER NOT NULL,
            updated_at INTEGER NOT NULL,authoritative INTEGER NOT NULL DEFAULT 0,models TEXT NOT NULL)",
        )
        .unwrap();
        for version in 0..=13 {
            db.execute(
                "INSERT INTO model_cache VALUES(?1,?2,1000,1,?3)",
                params![
                    format!("version-{version}"),
                    version,
                    if version == 12 { "[]" } else { r#"[{"headers":{"X-Access-Token":"legacy-cached-secret"}}]"# }
                ],
            )
            .unwrap();
        }
        drop(db);
        let cache = SqliteModelCache::for_path(&path);
        assert!(cache.read_model_cache("version-9", 100.0, || 1000.0).unwrap().is_none());
        let raw = std::fs::read(&path).unwrap();
        assert!(!raw.windows(b"legacy-cached-secret".len()).any(|bytes| bytes == b"legacy-cached-secret"));
        let legacy_current = cache.read_model_cache("version-12", 100.0, || 1000.0).unwrap().unwrap();
        assert!(legacy_current.legacy_header_restore_markers);
        assert_eq!(legacy_current.static_fingerprint, "");
        assert!(legacy_current.header_omitted_model_ids.is_empty());
        let db = Connection::open(&path).unwrap();
        assert_eq!(db.query_row("SELECT COUNT(*) FROM model_cache", [], |row| row.get::<_, i64>(0)).unwrap(), 1);
        drop(db);
        write(&cache, "replacement");
        let fresh = cache.read_model_cache("fixture", 100.0, || 1000.0).unwrap().unwrap();
        assert_eq!(fresh.models[0]["id"], "replacement");
        assert_eq!(fresh.static_fingerprint, "static-v3");
    }

    #[test]
    fn ttl_finite_nonnegative_inclusive_and_missing_rows_do_not_call_clock() {
        let dir = tempfile::tempdir().unwrap();
        let cache = SqliteModelCache::open(dir.path().join("models.db")).unwrap();
        let called = std::cell::Cell::new(false);
        assert!(
            cache
                .read_model_cache("missing", 10.0, || {
                    called.set(true);
                    1000.0
                })
                .unwrap()
                .is_none()
        );
        assert!(!called.get());
        write(&cache, "clock");
        for (now, ttl, fresh) in [
            (1000.0, 0.0, true),
            (1010.0, 10.0, true),
            (1010.5, 10.0, false),
            (999.0, 10.0, false),
            (1000.0, -1.0, false),
            (f64::NAN, 10.0, false),
            (f64::INFINITY, 10.0, false),
            (1000.0, f64::NAN, false),
            (1001.0, f64::INFINITY, true),
        ] {
            assert_eq!(cache.read_model_cache("fixture", ttl, || now).unwrap().unwrap().fresh, fresh);
        }
    }

    #[test]
    fn parses_legacy_header_markers_without_normalizing_model_json() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("models.db");
        let cache = SqliteModelCache::for_path(&path);
        write(&cache, "legacy");
        let db = Connection::open(&path).unwrap();
        db.execute("UPDATE model_cache SET header_omitted_model_ids='[\"a\",null,7,\"a\",\"\"]',unrestorable_header_model_ids='{}',header_restore_version=0,models='null'",[]).unwrap();
        drop(db);
        let entry = cache.read_model_cache("fixture", 0.0, || 1000.0).unwrap().unwrap();
        assert_eq!(entry.models, Value::Null);
        assert_eq!(entry.header_omitted_model_ids, ["a", "a", ""]);
        assert!(entry.unrestorable_header_model_ids.is_empty());
        assert!(entry.legacy_header_restore_markers);
    }

    #[test]
    fn malformed_json_is_reported_without_quarantining_healthy_database() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("models.db");
        let cache = SqliteModelCache::for_path(&path);
        write(&cache, "bad-json");
        let db = Connection::open(&path).unwrap();
        db.execute("UPDATE model_cache SET models='broken JSON'", []).unwrap();
        drop(db);
        let called = std::cell::Cell::new(false);
        assert!(
            cache
                .read_model_cache("fixture", 10.0, || {
                    called.set(true);
                    1000.0
                })
                .is_err()
        );
        assert!(!called.get());
        assert!(cache.take_recovery_notices().is_empty());
        write(&cache, "good-json");
        let db = Connection::open(&path).unwrap();
        db.execute("UPDATE model_cache SET header_omitted_model_ids='broken JSON'", []).unwrap();
        drop(db);
        assert!(cache.read_model_cache("fixture", 10.0, || 1000.0).is_err());
        assert!(cache.take_recovery_notices().is_empty());
    }

    #[test]
    fn physical_corruption_read_heals_then_later_handle_observes_new_catalog() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("models.db");
        let cache = SqliteModelCache::for_path(&path);
        write(&cache, "bootstrap");
        corrupt_pages(&path);
        assert!(cache.read_model_cache("fixture", 100.0, || 1000.0).unwrap().is_none());
        let notices = cache.take_recovery_notices();
        assert_eq!(notices.len(), 1);
        assert!(!notices[0].previously_reported);
        assert!(!notices[0].quarantined_paths.is_empty());
        assert!(notices[0].quarantine_failures.is_empty());
        write(&cache, "discovered");
        let later = SqliteModelCache::open(&path).unwrap();
        let entry = later.read_model_cache("fixture", 100.0, || 1000.0).unwrap().unwrap();
        assert_eq!(entry.models[0]["id"], "discovered");
        assert!(later.take_recovery_notices().is_empty());
    }

    #[test]
    fn not_a_database_write_heals_and_repeated_path_has_debug_receipt() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("models.db");
        std::fs::write(&path, "not a database".repeat(64)).unwrap();
        let cache = SqliteModelCache::for_path(&path);
        write(&cache, "first");
        let first = cache.take_recovery_notices();
        assert_eq!(first.len(), 1);
        assert!(!first[0].previously_reported);
        assert_eq!(cache.read_model_cache("fixture", 0.0, || 1000.0).unwrap().unwrap().models[0]["id"], "first");
        corrupt_pages(&path);
        write(&cache, "second");
        let second = cache.take_recovery_notices();
        assert_eq!(second.len(), 1);
        assert!(second[0].previously_reported);
        assert_eq!(cache.read_model_cache("fixture", 0.0, || 1000.0).unwrap().unwrap().models[0]["id"], "second");
    }

    #[test]
    fn native_pragmas_and_busy_failure_never_quarantine() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("models.db");
        let cache = SqliteModelCache::open(&path).unwrap();
        cache
            .with_db(|db| {
                assert_eq!(db.query_row("PRAGMA busy_timeout", [], |row| row.get::<_, i64>(0))?, 3000);
                assert_eq!(db.query_row("PRAGMA secure_delete", [], |row| row.get::<_, i64>(0))?, 1);
                assert_eq!(db.query_row("PRAGMA journal_mode", [], |row| row.get::<_, String>(0))?, "wal");
                Ok(())
            })
            .unwrap();
        cache.release_connection();
        let lock = Connection::open(&path).unwrap();
        lock.execute_batch("PRAGMA journal_mode=DELETE;BEGIN EXCLUSIVE;").unwrap();
        let error = cache.read_model_cache("fixture", 0.0, || 1000.0).err().expect("busy error");
        assert!(crate::credential_store::is_sqlite_busy_error(&error));
        assert!(cache.take_recovery_notices().is_empty());
        lock.execute_batch("ROLLBACK").unwrap();
    }

    #[test]
    fn host_path_errors_and_invalid_writes_are_explicit_and_atomic() {
        let dir = tempfile::tempdir().unwrap();
        let cache = SqliteModelCache::for_path(dir.path().join("missing-parent/models.db"));
        assert!(cache.read_model_cache("fixture", 0.0, || 1000.0).is_err());
        assert!(cache.take_recovery_notices().is_empty());
        assert!(!dir.path().join("missing-parent").exists());
        let cache = SqliteModelCache::for_path(dir.path().join("models.db"));
        write(&cache, "original");
        assert!(
            cache
                .write_model_cache("fixture", 1001.0, &[fixture_model("partial"), Value::Null], Default::default())
                .is_err()
        );
        assert_eq!(cache.read_model_cache("fixture", 1.0, || 1000.0).unwrap().unwrap().models[0]["id"], "original");
        assert!(cache.write_model_cache("fixture", f64::NAN, &[], Default::default()).is_err());
        assert_eq!(cache.read_model_cache("fixture", 1.0, || 1000.0).unwrap().unwrap().models[0]["id"], "original");
        assert!(cache.take_recovery_notices().is_empty());
    }

    #[test]
    fn external_process_model_writer() {
        let dir = tempfile::tempdir().unwrap();
        let path = std::env::var_os("ARA_MODEL_CACHE_TEST_DB")
            .map(PathBuf::from)
            .unwrap_or_else(|| dir.path().join("models.db"));
        write(&SqliteModelCache::for_path(path), "child-native");
    }

    #[test]
    fn cross_process_write_is_visible_to_retained_native_reader() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("models.db");
        let reader = SqliteModelCache::open(&path).unwrap();
        assert!(reader.read_model_cache("fixture", 0.0, || 1000.0).unwrap().is_none());
        let output = std::process::Command::new(std::env::current_exe().unwrap())
            .args(["--exact", "model_cache::tests::external_process_model_writer", "--nocapture"])
            .env("ARA_MODEL_CACHE_TEST_DB", &path)
            .output()
            .unwrap();
        assert!(output.status.success(), "isolated child writer failed");
        let entry = reader.read_model_cache("fixture", 0.0, || 1000.0).unwrap().unwrap();
        assert_eq!(entry.models[0]["id"], "child-native");
        assert_eq!(entry.static_fingerprint, "static-v3");
        assert!(entry.authoritative);
        assert!(reader.take_recovery_notices().is_empty());
    }
}
