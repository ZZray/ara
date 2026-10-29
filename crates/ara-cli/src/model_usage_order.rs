//! Host-owned Native model preference order from fixed OMP
//! `596f2da7101178214aa27a753529d15e6b7ad91d`:
//! `packages/coding-agent/src/session/agent-storage.ts:129,182-188,228-231,475-502`,
//! `packages/coding-agent/src/config/settings.ts:1372`, and
//! `packages/utils/src/dirs.ts:837-840`.
//!
//! The model_usage table records last use in SQLite epoch seconds, not a use
//! count or token/cost usage. The order is cached until this owner's successful
//! record, as in AgentStorage. Hosts supply the actual database path and report
//! returned errors; unlike Native's logged best-effort catches, this seam never
//! silently substitutes an empty order for a real storage error. A missing
//! database is an empty preference order without creating directories or files.
//! Only the Native model_usage table is initialized on record. Other
//! AgentStorage tables, legacy migrations and default/XDG path policy remain
//! with their respective Host owners.
//!
//! MIT License
//! Copyright (c) 2025 Mario Zechner
//! Copyright (c) 2025-2026 Can Bölük
//! Copyright (c) 2026 Stencil Labs, Inc.
//!
//! Permission is hereby granted, free of charge, to any person obtaining a copy
//! of this software and associated documentation files (the "Software"), to deal
//! in the Software without restriction, including without limitation the rights
//! to use, copy, modify, merge, publish, distribute, sublicense, and/or sell copies
//! of the Software, and to permit persons to whom the Software is furnished to
//! do so, subject to the following conditions:
//! The above copyright notice and this permission notice shall be included in
//! all copies or substantial portions of the Software.
//! THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF ANY KIND, EXPRESS OR
//! IMPLIED, INCLUDING BUT NOT LIMITED TO THE WARRANTIES OF MERCHANTABILITY,
//! FITNESS FOR A PARTICULAR PURPOSE AND NONINFRINGEMENT. IN NO EVENT SHALL THE
//! AUTHORS OR COPYRIGHT HOLDERS BE LIABLE FOR ANY CLAIM, DAMAGES OR OTHER
//! LIABILITY, WHETHER IN AN ACTION OF CONTRACT, TORT OR OTHERWISE, ARISING FROM,
//! OUT OF OR IN CONNECTION WITH THE SOFTWARE OR THE USE OR OTHER DEALINGS IN THE
//! SOFTWARE.

use anyhow::{Context, Result, anyhow};
use rusqlite::{Connection, OpenFlags};
use std::{
    fs,
    path::{Path, PathBuf},
    sync::Mutex,
    time::Duration,
};

const HEADLESS_BUSY_TIMEOUT: Duration = Duration::from_millis(1000);
const MODEL_USAGE_SCHEMA: &str = "
CREATE TABLE IF NOT EXISTS model_usage (
    model_key TEXT PRIMARY KEY,
    last_used_at INTEGER NOT NULL DEFAULT (CAST(strftime('%s','now') AS INTEGER))
);";
const RECORD_MODEL_USAGE: &str = "
INSERT INTO model_usage (model_key, last_used_at)
VALUES (?1, CAST(strftime('%s','now') AS INTEGER))
ON CONFLICT(model_key) DO UPDATE SET
    last_used_at = CAST(strftime('%s','now') AS INTEGER)";

/// Share one instance for one Host database to retain Native cache semantics.
/// Operations are synchronous and never retain the lock across an await.
pub struct SqliteModelUsageOrder {
    path: PathBuf,
    busy_timeout: Duration,
    cached_order: Mutex<Option<Vec<String>>>,
}

impl SqliteModelUsageOrder {
    /// The Host has already resolved its persistence path, including any XDG
    /// policy. Construction itself performs no filesystem or database I/O.
    pub fn for_path(path: impl Into<PathBuf>) -> Self {
        Self { path: path.into(), busy_timeout: HEADLESS_BUSY_TIMEOUT, cached_order: Mutex::new(None) }
    }

    /// Native explicit-agentDir path rule; default/XDG resolution is external.
    pub fn for_agent_dir(agent_dir: impl AsRef<Path>) -> Self {
        Self::for_path(agent_dir.as_ref().join("agent.db"))
    }

    /// Native interactive hosts use 5000ms; print/RPC hosts use 1000ms.
    pub fn with_busy_timeout(mut self, timeout: Duration) -> Self {
        self.busy_timeout = timeout;
        self
    }

    pub fn database_path(&self) -> &Path {
        &self.path
    }

    pub fn get_model_usage_order(&self) -> Result<Vec<String>> {
        let mut cache = self.cached_order.lock().map_err(|_| anyhow!("model usage order cache lock poisoned"))?;
        if let Some(order) = cache.as_ref() {
            return Ok(order.clone());
        }
        match fs::metadata(&self.path) {
            Ok(_) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
            Err(error) => return Err(error).context("inspect model usage database"),
        }
        let db = Connection::open_with_flags(&self.path, OpenFlags::SQLITE_OPEN_READ_ONLY)
            .context("open model usage database for reading")?;
        db.busy_timeout(self.busy_timeout).context("configure model usage database read timeout")?;
        // Native deliberately defines no secondary tie-break for equal seconds.
        let mut statement = db
            .prepare("SELECT model_key, last_used_at FROM model_usage ORDER BY last_used_at DESC")
            .context("prepare native model usage order query")?;
        let order = statement
            .query_map([], |row| row.get::<_, String>(0))
            .context("query native model usage order")?
            .collect::<rusqlite::Result<Vec<_>>>()
            .context("read native model usage order rows")?;
        *cache = Some(order.clone());
        Ok(order)
    }

    /// Call only when the actual Host model selection was adopted. Native
    /// records the already-selected provider/modelId string without parsing,
    /// normalization, counters, or token/cost attribution.
    pub fn record_model_usage(&self, model_key: &str) -> Result<()> {
        let mut cache = self.cached_order.lock().map_err(|_| anyhow!("model usage order cache lock poisoned"))?;
        if let Some(parent) = self.path.parent().filter(|parent| !parent.as_os_str().is_empty()) {
            fs::create_dir_all(parent).context("create model usage database directory")?;
        }
        let db = Connection::open(&self.path).context("open model usage database for recording")?;
        // Install the busy handler before lock-taking schema/WAL statements.
        db.busy_timeout(self.busy_timeout).context("configure model usage database write timeout")?;
        db.execute_batch("PRAGMA journal_mode=WAL; PRAGMA synchronous=NORMAL;")
            .context("configure native model usage database journal")?;
        db.execute_batch(MODEL_USAGE_SCHEMA).context("initialize native model usage table")?;
        db.execute(RECORD_MODEL_USAGE, [model_key]).context("record native model last use")?;
        *cache = None;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rusqlite::params;

    fn native_fixture(path: &Path, rows: &[(&str, i64)]) -> Connection {
        let db = Connection::open(path).unwrap();
        db.execute_batch(MODEL_USAGE_SCHEMA).unwrap();
        for (key, last_used) in rows {
            db.execute("INSERT INTO model_usage(model_key,last_used_at) VALUES(?1,?2)", params![key, last_used])
                .unwrap();
        }
        db
    }

    #[test]
    fn reads_native_schema_mru_without_mutating_legacy_or_other_tables() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("agent.db");
        let db = Connection::open(&path).unwrap();
        db.execute_batch(
            "CREATE TABLE model_usage(model_key TEXT PRIMARY KEY,last_used_at INTEGER NOT NULL DEFAULT(unixepoch()));
             INSERT INTO model_usage VALUES('openai/older',10),('custom/newer',30),('anthropic/middle',20);
             CREATE TABLE settings(key TEXT PRIMARY KEY,value TEXT NOT NULL);
             INSERT INTO settings VALUES('untouched','sentinel');
             CREATE TABLE schema_version(version INTEGER PRIMARY KEY);
             INSERT INTO schema_version VALUES(4);",
        )
        .unwrap();
        let store = SqliteModelUsageOrder::for_path(&path);
        assert_eq!(store.get_model_usage_order().unwrap(), ["custom/newer", "anthropic/middle", "openai/older"]);
        assert_eq!(db.query_row("SELECT value FROM settings", [], |row| row.get::<_, String>(0)).unwrap(), "sentinel");
        assert_eq!(db.query_row("SELECT version FROM schema_version", [], |row| row.get::<_, i64>(0)).unwrap(), 4);
        assert_eq!(
            db.query_row("SELECT sql FROM sqlite_master WHERE name='model_usage'", [], |row| row.get::<_, String>(0))
                .unwrap(),
            "CREATE TABLE model_usage(model_key TEXT PRIMARY KEY,last_used_at INTEGER NOT NULL DEFAULT(unixepoch()))"
        );
    }

    #[test]
    fn record_uses_sqlite_seconds_upsert_and_only_the_native_table() {
        let temp = tempfile::tempdir().unwrap();
        let agent_dir = temp.path().join("agent");
        let store = SqliteModelUsageOrder::for_agent_dir(&agent_dir);
        let before = Connection::open_in_memory()
            .unwrap()
            .query_row("SELECT CAST(strftime('%s','now') AS INTEGER)", [], |row| row.get::<_, i64>(0))
            .unwrap();
        store.record_model_usage("custom/key/kept:high").unwrap();
        let db = Connection::open(store.database_path()).unwrap();
        db.execute("UPDATE model_usage SET last_used_at=1", []).unwrap();
        store.record_model_usage("custom/key/kept:high").unwrap();
        let after =
            db.query_row("SELECT CAST(strftime('%s','now') AS INTEGER)", [], |row| row.get::<_, i64>(0)).unwrap();
        let (count, last_used): (i64, i64) = db
            .query_row("SELECT count(*),max(last_used_at) FROM model_usage", [], |row| Ok((row.get(0)?, row.get(1)?)))
            .unwrap();
        assert_eq!(count, 1);
        assert!((before..=after).contains(&last_used));
        assert_eq!(store.get_model_usage_order().unwrap(), ["custom/key/kept:high"]);
        let tables = db
            .prepare("SELECT name FROM sqlite_master WHERE type='table' ORDER BY name")
            .unwrap()
            .query_map([], |row| row.get::<_, String>(0))
            .unwrap()
            .collect::<rusqlite::Result<Vec<_>>>()
            .unwrap();
        assert_eq!(tables, ["model_usage"]);
        let sql: String =
            db.query_row("SELECT sql FROM sqlite_master WHERE name='model_usage'", [], |row| row.get(0)).unwrap();
        assert!(sql.contains("strftime('%s','now')"));
        assert!(!sql.contains("unixepoch("));
    }

    #[test]
    fn native_cache_is_invalidated_by_successful_local_record_only() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("agent.db");
        let db = native_fixture(&path, &[("p/first", 10), ("p/second", 5)]);
        let store = SqliteModelUsageOrder::for_path(&path);
        assert_eq!(store.get_model_usage_order().unwrap(), ["p/first", "p/second"]);
        db.execute("UPDATE model_usage SET last_used_at=20 WHERE model_key='p/second'", []).unwrap();
        assert_eq!(store.get_model_usage_order().unwrap(), ["p/first", "p/second"]);
        store.record_model_usage("p/third").unwrap();
        assert_eq!(store.get_model_usage_order().unwrap(), ["p/third", "p/second", "p/first"]);
        db.execute_batch("DROP TABLE model_usage; CREATE TABLE model_usage(wrong TEXT);").unwrap();
        assert!(store.record_model_usage("p/rejected").is_err());
        assert_eq!(store.get_model_usage_order().unwrap(), ["p/third", "p/second", "p/first"]);
    }

    #[test]
    fn missing_database_is_empty_but_real_storage_errors_are_explicit() {
        let temp = tempfile::tempdir().unwrap();
        let missing = temp.path().join("not-created/agent.db");
        let store = SqliteModelUsageOrder::for_path(&missing);
        assert!(store.get_model_usage_order().unwrap().is_empty());
        assert!(!missing.parent().unwrap().exists());
        store.record_model_usage("p/first").unwrap();
        assert_eq!(store.get_model_usage_order().unwrap(), ["p/first"]);

        let corrupt = temp.path().join("corrupt.db");
        fs::write(&corrupt, b"not a SQLite database").unwrap();
        let corrupt_store = SqliteModelUsageOrder::for_path(&corrupt);
        assert!(corrupt_store.get_model_usage_order().is_err());
        assert!(corrupt_store.record_model_usage("p/no-write").is_err());
        assert_eq!(fs::read(&corrupt).unwrap(), b"not a SQLite database");

        let no_schema = temp.path().join("no-schema.db");
        Connection::open(&no_schema).unwrap();
        assert!(SqliteModelUsageOrder::for_path(no_schema).get_model_usage_order().is_err());
    }

    #[test]
    fn lock_contention_returns_error_without_creating_usage_or_cached_success() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("agent.db");
        let db = native_fixture(&path, &[("p/original", 1)]);
        db.execute_batch("BEGIN EXCLUSIVE;").unwrap();
        let store = SqliteModelUsageOrder::for_path(path).with_busy_timeout(Duration::from_millis(5));
        let read_error = store.get_model_usage_order().unwrap_err();
        let write_error = store.record_model_usage("p/rejected").unwrap_err();
        for error in [read_error, write_error] {
            assert!(matches!(
                error.downcast_ref::<rusqlite::Error>(),
                Some(rusqlite::Error::SqliteFailure(code, _))
                    if matches!(code.code, rusqlite::ErrorCode::DatabaseBusy | rusqlite::ErrorCode::DatabaseLocked)
            ));
        }
        db.execute_batch("ROLLBACK;").unwrap();
        assert_eq!(store.get_model_usage_order().unwrap(), ["p/original"]);
    }
}
