//! Native SQLite credential persistence, ported from fixed OMP
//! `596f2da7101178214aa27a753529d15e6b7ad91d`,
//! `packages/ai/src/auth/sqlite-credential-store.ts`.
//!
//! The host supplies the database path. This module never opens a user's
//! installation implicitly and does not resolve keys, refresh OAuth, or log
//! credentials. Unlike OMP's best-effort methods, SQL failures are returned.
//
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
// The above copyright notice and this permission notice shall be included in
// all copies or substantial portions of the Software.
// THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF ANY KIND, EXPRESS OR
// IMPLIED, INCLUDING BUT NOT LIMITED TO THE WARRANTIES OF MERCHANTABILITY,
// FITNESS FOR A PARTICULAR PURPOSE AND NONINFRINGEMENT. IN NO EVENT SHALL THE
// AUTHORS OR COPYRIGHT HOLDERS BE LIABLE FOR ANY CLAIM, DAMAGES OR OTHER
// LIABILITY, WHETHER IN AN ACTION OF CONTRACT, TORT OR OTHERWISE, ARISING FROM,
// OUT OF OR IN CONNECTION WITH THE SOFTWARE OR THE USE OR OTHER DEALINGS IN
// THE SOFTWARE.

use anyhow::{Context, Result};
use base64::Engine as _;
use rusqlite::{Connection, OptionalExtension, Transaction, TransactionBehavior, params};
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value, json};
use std::cell::{Cell, RefCell};
use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;
use std::time::Duration;

pub const AUTH_SCHEMA_VERSION: i64 = 7;
pub const USAGE_REPORT_TTL_MS: i64 = 300_000;
const USAGE_HISTORY_BUCKET_MS: i64 = 3_600_000;
const CLIENT_USAGE_BUCKET_MS: i64 = 300_000;
const NOW_SQL: &str = "CAST(strftime('%s','now') AS INTEGER)";
const CODEX_PROVIDER: &str = "openai-codex:oauth";

/// Sensitive: intentionally has no Debug or Display implementation.
#[derive(Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum AuthCredential {
    #[serde(rename = "api_key")]
    ApiKey {
        key: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        source: Option<String>,
    },
    #[serde(rename = "oauth")]
    OAuth {
        #[serde(flatten)]
        fields: Map<String, Value>,
    },
}

impl AuthCredential {
    pub fn api_key(key: impl Into<String>) -> Self {
        Self::ApiKey { key: key.into(), source: None }
    }
    pub fn oauth(fields: Map<String, Value>) -> Self {
        Self::OAuth { fields }
    }
    pub fn credential_type(&self) -> &'static str {
        match self {
            Self::ApiKey { .. } => "api_key",
            Self::OAuth { .. } => "oauth",
        }
    }
}

/// `serialized_data` is the exact stored value for refresh CAS fencing. It
/// must not be reconstructed from an OAuth map, whose key order can differ.
#[derive(Clone)]
pub struct StoredAuthCredential {
    pub id: i64,
    pub provider: String,
    pub credential: AuthCredential,
    pub disabled_cause: Option<String>,
    pub serialized_data: String,
    pub revision: i64,
}

#[derive(Clone)]
pub struct SerializedCredentialRecord {
    pub credential_type: String,
    pub data: String,
    pub identity_key: Option<String>,
}

#[derive(Clone)]
pub struct CredentialRefreshLeaseFence {
    pub owner: String,
    pub now_ms: i64,
}

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DisabledCredentialSummary {
    pub id: i64,
    pub provider: String,
    #[serde(rename = "type")]
    pub credential_type: String,
    pub cause: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub email: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub account_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub org_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub org_name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub disabled_at_ms: Option<f64>,
}

#[derive(Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct StoredCredentialBlock {
    pub credential_id: i64,
    pub provider_key: String,
    pub block_scope: String,
    pub blocked_until_ms: i64,
    #[serde(default)]
    pub updated_at_ms: i64,
}

#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UsageHistoryEntry {
    pub recorded_at: i64,
    pub provider: String,
    pub account_key: String,
    pub email: Option<String>,
    pub account_id: Option<String>,
    pub limit_id: String,
    pub label: String,
    pub window_label: Option<String>,
    pub used_fraction: Option<f64>,
    pub status: Option<String>,
    pub resets_at: Option<i64>,
}

#[derive(Default)]
pub struct UsageHistoryQuery {
    pub since_ms: Option<i64>,
    pub provider: Option<String>,
}

#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ClientUsageEntry {
    pub at: i64,
    pub provider: String,
    pub model: String,
    pub requests: i64,
    pub input_tokens: i64,
    pub output_tokens: i64,
    pub cache_read_tokens: i64,
    pub cache_write_tokens: i64,
    pub cost_usd: f64,
}

#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ClientUsageReport {
    pub install_id: String,
    pub hostname: Option<String>,
    pub app: Option<String>,
    pub entries: Vec<ClientUsageEntry>,
}

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ClientProviderUsage {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub app: Option<String>,
    pub provider: String,
    pub requests: i64,
    pub input_tokens: i64,
    pub output_tokens: i64,
    pub cache_read_tokens: i64,
    pub cache_write_tokens: i64,
    pub cost_usd: f64,
}

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ClientUsageClient {
    pub install_id: String,
    pub hostname: Option<String>,
    pub first_seen: i64,
    pub last_seen: i64,
    pub providers: Vec<ClientProviderUsage>,
}

#[derive(Clone, Serialize)]
pub struct ClientUsageSummary {
    pub clients: Vec<ClientUsageClient>,
}

struct AuthRow {
    id: i64,
    provider: String,
    credential_type: String,
    data: String,
    disabled_cause: Option<String>,
    identity_key: Option<String>,
    updated_at: Option<i64>,
}

pub fn serialize_credential(provider: &str, credential: &AuthCredential) -> Result<SerializedCredentialRecord> {
    let value = match credential {
        AuthCredential::ApiKey { key, source } => {
            if source.as_deref() == Some("login") {
                json!({"key":key,"source":"login"})
            } else {
                json!({"key":key})
            }
        }
        AuthCredential::OAuth { fields } => Value::Object(fields.clone()),
    };
    Ok(SerializedCredentialRecord {
        credential_type: credential.credential_type().into(),
        data: serde_json::to_string(&value)?,
        identity_key: resolve_credential_identity_key(provider, credential),
    })
}

fn deserialize_credential(row: &AuthRow) -> Option<AuthCredential> {
    // Corrupt/unknown credential rows are omitted by the fixed upstream list
    // contract. This does not swallow database or parameter type errors.
    let value: Value = serde_json::from_str(&row.data).ok()?;
    let fields = value.as_object()?;
    match row.credential_type.as_str() {
        "api_key" => Some(AuthCredential::ApiKey {
            key: fields.get("key")?.as_str()?.into(),
            source: (fields.get("source").and_then(Value::as_str) == Some("login")).then(|| "login".into()),
        }),
        "oauth" => Some(AuthCredential::OAuth { fields: fields.clone() }),
        _ => None,
    }
}

fn normalized(value: Option<&str>) -> Option<String> {
    value.map(ara_prompt::js::trim).filter(|v| !v.is_empty()).map(str::to_owned)
}

fn identifiers(credential: &AuthCredential) -> Vec<String> {
    let AuthCredential::OAuth { fields } = credential else { return Vec::new() };
    let mut ids = Vec::new();
    let mut add = |kind: &str, value: Option<&str>| {
        if let Some(mut value) = normalized(value) {
            if kind == "email" {
                value = value.to_lowercase();
            }
            let id = format!("{kind}:{value}");
            if !ids.contains(&id) {
                ids.push(id);
            }
        }
    };
    for (field, kind) in [("accountId", "account"), ("email", "email"), ("projectId", "project"), ("orgId", "org")] {
        add(kind, fields.get(field).and_then(Value::as_str));
    }
    for field in ["access", "refresh"] {
        let Some(token) = fields.get(field).and_then(Value::as_str) else { continue };
        let parts = token.split('.').collect::<Vec<_>>();
        if parts.len() != 3 {
            continue;
        }
        let decoded = base64::engine::general_purpose::URL_SAFE_NO_PAD
            .decode(parts[1])
            .or_else(|_| base64::engine::general_purpose::URL_SAFE.decode(parts[1]));
        let Ok(decoded) = decoded else { continue };
        let Ok(payload) = serde_json::from_slice::<Value>(&decoded) else { continue };
        add("email", payload.get("email").and_then(Value::as_str));
        add(
            "email",
            payload.get("https://api.openai.com/profile").and_then(|v| v.get("email")).and_then(Value::as_str),
        );
        let account = ["account_id", "accountId", "user_id", "sub"]
            .iter()
            .find_map(|k| payload.get(k).and_then(Value::as_str))
            .or_else(|| {
                payload
                    .get("https://api.openai.com/auth")
                    .and_then(|v| v.get("chatgpt_account_id"))
                    .and_then(Value::as_str)
            });
        add("account", account);
    }
    ids
}

pub fn resolve_credential_identity_key(provider: &str, credential: &AuthCredential) -> Option<String> {
    let ids = identifiers(credential);
    let find = |kind: &str| ids.iter().find(|id| id.starts_with(kind)).cloned();
    if matches!(provider, "anthropic" | "openai-codex") {
        let base = find("email:").or_else(|| find("account:")).or_else(|| find("project:"));
        match (base, find("org:")) {
            (Some(base), Some(org)) => Some(format!("{base}|{org}")),
            (base, org) => base.or(org),
        }
    } else {
        find("account:").or_else(|| find("email:")).or_else(|| find("project:"))
    }
}

fn row_identity(provider: &str, row: &AuthRow) -> Option<String> {
    normalized(row.identity_key.as_deref())
        .or_else(|| deserialize_credential(row).and_then(|c| resolve_credential_identity_key(provider, &c)))
}

fn provider_token(provider: &str, key: &str) -> Option<String> {
    let text = key.trim();
    if text.is_empty() {
        return None;
    }
    let token = if text.starts_with('{') {
        let value: Value = serde_json::from_str(text).ok()?;
        let fields = value.as_object()?;
        let optional =
            if provider == "alibaba-token-plan" { ["cookie", "baseUrl"] } else { ["accountId", "gatewayId"] };
        if optional.iter().any(|k| fields.get(*k).is_some_and(|v| !v.is_string())) {
            return None;
        }
        normalized(fields.get("token").and_then(Value::as_str))?
    } else {
        text.into()
    };
    if provider == "alibaba-token-plan" {
        let rest = token.strip_prefix("sk-")?;
        let body = rest.trim_end_matches('=');
        if body.is_empty()
            || rest.len() - body.len() > 2
            || !body.bytes().all(|b| b.is_ascii_alphanumeric() || b"._~+/-".contains(&b))
        {
            return None;
        }
    }
    Some(token)
}

fn matches_replacement(
    provider: &str,
    existing: Option<&AuthCredential>,
    identity: Option<&str>,
    incoming: &AuthCredential,
) -> bool {
    let Some(existing) = existing else { return false };
    match (existing, incoming) {
        (AuthCredential::ApiKey { key: old, .. }, AuthCredential::ApiKey { key: new, .. }) => {
            old == new
                || (matches!(provider, "alibaba-token-plan" | "cloudflare-ai-gateway")
                    && provider_token(provider, old).is_some_and(|token| Some(token) == provider_token(provider, new)))
        }
        (AuthCredential::OAuth { .. }, AuthCredential::OAuth { .. }) => {
            let ids = identifiers(incoming);
            let Some(incoming_identity) = resolve_credential_identity_key(provider, incoming) else { return false };
            let Some(identity) = identity else { return false };
            if incoming_identity == identity {
                return true;
            }
            let Some(org) = ids.iter().find(|id| id.starts_with("org:")) else { return false };
            if incoming_identity != *org && !incoming_identity.ends_with(&format!("|{org}")) {
                return false;
            }
            if identity == org {
                return true;
            }
            let existing_ids = if identity.ends_with(&format!("|{org}")) { identifiers(existing) } else { Vec::new() };
            ids.iter().any(|id| {
                ["email:", "account:", "project:"].iter().any(|prefix| id.starts_with(prefix))
                    && id.split_once(':').map(|(_, v)| v) != Some(&org[4..])
                    && (identity == id || identity == format!("{id}|{org}") || existing_ids.contains(id))
            })
        }
        _ => false,
    }
}

fn now_ms() -> i64 {
    chrono::Utc::now().timestamp_millis()
}
fn row_from_sql(row: &rusqlite::Row<'_>) -> rusqlite::Result<AuthRow> {
    Ok(AuthRow {
        id: row.get(0)?,
        provider: row.get(1)?,
        credential_type: row.get(2)?,
        data: row.get(3)?,
        disabled_cause: row.get(4)?,
        identity_key: row.get(5)?,
        updated_at: row.get(6)?,
    })
}

/// Host-owned, one connection. Share through a host mutex; no locks are held
/// by this module across network, OAuth or command execution.
pub struct SqliteCredentialStore {
    conn: Option<Connection>,
    data_version: Cell<i64>,
    auth_revision: Cell<i64>,
    local_revision: Cell<i64>,
    reconcile_after: RefCell<BTreeMap<(i64, String, String), i64>>,
    pub newer_schema_version: Option<i64>,
}

impl SqliteCredentialStore {
    pub fn open(path: impl AsRef<Path>) -> Result<Self> {
        Self::open_with_busy_timeout(path, Duration::from_secs(5))
    }

    /// The host may select a shorter headless busy timeout, as OMP's central
    /// timeout policy does. Busy setup precedes every lock-taking statement.
    pub fn open_with_busy_timeout(path: impl AsRef<Path>, timeout: Duration) -> Result<Self> {
        let path = path.as_ref();
        if let Some(parent) = path.parent().filter(|p| !p.as_os_str().is_empty()) {
            let missing = !parent.exists();
            std::fs::create_dir_all(parent).context("create credential database directory")?;
            #[cfg(unix)]
            if missing {
                use std::os::unix::fs::PermissionsExt;
                std::fs::set_permissions(parent, std::fs::Permissions::from_mode(0o700))?;
            }
            #[cfg(not(unix))]
            let _ = missing;
        }
        for attempt in 0..4 {
            let result = (|| {
                let conn = Connection::open(path)?;
                conn.busy_timeout(timeout)?;
                #[cfg(unix)]
                {
                    use std::os::unix::fs::PermissionsExt;
                    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600))?;
                }
                Self::from_connection(conn, timeout)
            })();
            match result {
                Ok(store) => return Ok(store),
                Err(error) if is_sqlite_busy_error(&error) && attempt < 3 => {
                    std::thread::sleep(Duration::from_millis(100 << attempt))
                }
                Err(error) => return Err(error),
            }
        }
        unreachable!("the final attempt always returns")
    }

    pub fn from_connection(conn: Connection, timeout: Duration) -> Result<Self> {
        conn.busy_timeout(timeout)?;
        conn.execute_batch(LEASE_SCHEMA)?;
        let newer_schema_version = initialize_schema(&conn)?;
        let data = conn.query_row("PRAGMA data_version", [], |r| r.get(0))?;
        let auth = conn.query_row("SELECT revision FROM auth_change_revision WHERE id=1", [], |r| r.get(0))?;
        let local = conn.query_row("SELECT revision FROM auth_local_change_revision WHERE id=1", [], |r| r.get(0))?;
        Ok(Self {
            conn: Some(conn),
            data_version: Cell::new(data),
            auth_revision: Cell::new(auth),
            local_revision: Cell::new(local),
            reconcile_after: RefCell::new(BTreeMap::new()),
            newer_schema_version,
        })
    }

    fn db(&self) -> Result<&Connection> {
        self.conn.as_ref().context("credential store is closed")
    }

    /// Internal reset-receipt namespace only; callers hash it before storage.
    /// Disk owners remain stable across restart. An in-memory owner has no
    /// durable credential authority and is isolated for its process lifetime.
    pub(crate) fn reset_receipt_authority(&self) -> Result<String> {
        let path = self.db()?.path().filter(|path| !path.is_empty());
        match path {
            Some(path) => Ok(format!("local:{}", std::fs::canonicalize(path)?.to_string_lossy())),
            None => Ok(format!("memory:{self:p}")),
        }
    }

    pub fn revision(&self) -> Result<i64> {
        Ok(self.db()?.query_row("SELECT revision FROM auth_change_revision WHERE id=1", [], |r| r.get(0))?)
    }

    fn transaction<T>(
        &self,
        behavior: TransactionBehavior,
        f: impl FnOnce(&Transaction<'_>) -> Result<T>,
    ) -> Result<T> {
        let tx = Transaction::new_unchecked(self.db()?, behavior)?;
        let value = f(&tx)?;
        tx.commit()?;
        Ok(value)
    }

    fn rows(&self, provider: Option<&str>, disabled: bool) -> Result<Vec<AuthRow>> {
        let sql="SELECT id,provider,credential_type,data,disabled_cause,identity_key,updated_at FROM auth_credentials
            WHERE (?1 IS NULL OR provider=?1) AND ((?2=1 AND disabled_cause IS NOT NULL) OR (?2=0 AND disabled_cause IS NULL)) ORDER BY id ASC";
        let mut stmt = self.db()?.prepare_cached(sql)?;
        Ok(stmt.query_map(params![provider, disabled], row_from_sql)?.collect::<rusqlite::Result<Vec<_>>>()?)
    }

    fn stored_rows(&self, provider: Option<&str>) -> Result<Vec<StoredAuthCredential>> {
        let revision = self.revision()?;
        Ok(self
            .rows(provider, false)?
            .into_iter()
            .filter_map(|row| {
                let credential = deserialize_credential(&row)?;
                Some(StoredAuthCredential {
                    id: row.id,
                    provider: row.provider,
                    credential,
                    disabled_cause: row.disabled_cause,
                    serialized_data: row.data,
                    revision,
                })
            })
            .collect())
    }

    pub fn list_auth_credentials(&self, provider: Option<&str>) -> Result<Vec<StoredAuthCredential>> {
        // A snapshot includes both row data and revision. The private helper
        // also works within the existing write transaction used by upsert.
        self.transaction(TransactionBehavior::Deferred, |_| self.stored_rows(provider))
    }

    pub fn list_disabled_credentials(&self, provider: Option<&str>) -> Result<Vec<DisabledCredentialSummary>> {
        Ok(self
            .rows(provider, true)?
            .into_iter()
            .map(|row| {
                let credential = deserialize_credential(&row);
                let field = |name: &str| match &credential {
                    Some(AuthCredential::OAuth { fields }) => {
                        fields.get(name).and_then(Value::as_str).filter(|v| !v.is_empty()).map(str::to_owned)
                    }
                    _ => None,
                };
                DisabledCredentialSummary {
                    id: row.id,
                    provider: row.provider,
                    credential_type: if row.credential_type == "api_key" { "api_key" } else { "oauth" }.into(),
                    cause: row.disabled_cause.unwrap_or_else(|| "disabled".into()),
                    email: field("email"),
                    account_id: field("accountId"),
                    org_id: field("orgId"),
                    org_name: field("orgName"),
                    disabled_at_ms: row.updated_at.map(|v| v as f64 * 1000.0),
                }
            })
            .collect())
    }

    fn insert(&self, provider: &str, value: &SerializedCredentialRecord) -> Result<i64> {
        Ok(self.db()?.query_row(
            &format!(
                "INSERT INTO auth_credentials(provider,credential_type,data,identity_key,created_at,updated_at)
            VALUES(?1,?2,?3,?4,{NOW_SQL},{NOW_SQL}) RETURNING id"
            ),
            params![provider, value.credential_type, value.data, value.identity_key],
            |r| r.get(0),
        )?)
    }

    fn update(&self, id: i64, value: &SerializedCredentialRecord) -> Result<()> {
        self.db()?.execute(&format!("UPDATE auth_credentials SET credential_type=?1,data=?2,identity_key=?3,updated_at={NOW_SQL} WHERE id=?4"),
            params![value.credential_type,value.data,value.identity_key,id])?;
        Ok(())
    }

    pub fn replace_auth_credentials_for_provider(
        &self,
        provider: &str,
        items: &[AuthCredential],
    ) -> Result<Vec<StoredAuthCredential>> {
        let result = self.transaction(TransactionBehavior::Deferred, |_| {
            let existing = self.rows(Some(provider), false)?;
            let mut matched = BTreeSet::new();
            let mut result = Vec::new();
            for item in items {
                let value = serialize_credential(provider, item)?;
                let found = existing.iter().find(|row| {
                    !matched.contains(&row.id)
                        && matches_replacement(
                            provider,
                            deserialize_credential(row).as_ref(),
                            row_identity(provider, row).as_deref(),
                            item,
                        )
                });
                let id = if let Some(row) = found {
                    matched.insert(row.id);
                    self.update(row.id, &value)?;
                    row.id
                } else {
                    self.insert(provider, &value)?
                };
                result.push(StoredAuthCredential {
                    id,
                    provider: provider.into(),
                    credential: item.clone(),
                    disabled_cause: None,
                    serialized_data: value.data,
                    revision: 0,
                });
            }
            for row in existing {
                if !matched.contains(&row.id) {
                    self.delete_auth_credential(row.id, "replaced by newer credential")?;
                }
            }
            let revision = self.revision()?;
            for row in &mut result {
                row.revision = revision;
            }
            Ok(result)
        })?;
        self.purge_superseded_disabled_rows(provider, &result)?;
        Ok(result)
    }

    pub fn upsert_auth_credential_for_provider(
        &self,
        provider: &str,
        item: &AuthCredential,
    ) -> Result<Vec<StoredAuthCredential>> {
        let result = self.transaction(TransactionBehavior::Deferred, |_| {
            let value = serialize_credential(provider, item)?;
            let existing = self.rows(Some(provider), false)?;
            if matches!(item, AuthCredential::OAuth { .. }) {
                for row in &existing {
                    if matches!(deserialize_credential(row), Some(AuthCredential::ApiKey { .. })) {
                        self.delete_auth_credential(row.id, "replaced by oauth login")?;
                    }
                }
            }
            let mut target = None;
            for row in &existing {
                if !matches_replacement(
                    provider,
                    deserialize_credential(row).as_ref(),
                    row_identity(provider, row).as_deref(),
                    item,
                ) {
                    continue;
                }
                if target.is_none() {
                    target = Some(row.id);
                    self.update(row.id, &value)?;
                } else {
                    self.delete_auth_credential(row.id, "replaced by newer credential")?;
                }
            }
            if target.is_none() {
                self.insert(provider, &value)?;
            }
            self.stored_rows(Some(provider))
        })?;
        self.purge_superseded_disabled_rows(provider, &result)?;
        Ok(result)
    }

    fn purge_superseded_disabled_rows(&self, provider: &str, active: &[StoredAuthCredential]) -> Result<()> {
        let api = active.iter().any(|r| matches!(r.credential, AuthCredential::ApiKey { .. }));
        let identities = active
            .iter()
            .filter_map(|r| resolve_credential_identity_key(provider, &r.credential))
            .collect::<BTreeSet<_>>();
        if !api && identities.is_empty() {
            return Ok(());
        }
        for row in self.rows(Some(provider), true)? {
            let identity = row_identity(provider, &row);
            let replace = api && row.credential_type == "api_key"
                || identity.as_ref().is_some_and(|key| identities.contains(key))
                || active.iter().any(|r| {
                    matches!(r.credential, AuthCredential::OAuth { .. })
                        && matches_replacement(
                            provider,
                            deserialize_credential(&row).as_ref(),
                            identity.as_deref(),
                            &r.credential,
                        )
                });
            if replace {
                self.db()?.execute("DELETE FROM auth_credentials WHERE id=?1", [row.id])?;
            }
        }
        Ok(())
    }

    fn provider_for_id(&self, id: i64) -> Result<String> {
        Ok(self
            .db()?
            .query_row("SELECT provider FROM auth_credentials WHERE id=?1", [id], |r| r.get(0))
            .optional()?
            .unwrap_or_default())
    }

    pub fn update_auth_credential(&self, id: i64, item: &AuthCredential) -> Result<()> {
        let provider = self.provider_for_id(id)?;
        self.update(id, &serialize_credential(&provider, item)?)?;
        if !provider.is_empty() {
            self.purge_superseded_disabled_rows(&provider, &self.list_auth_credentials(Some(&provider))?)?;
        }
        Ok(())
    }

    pub fn try_update_auth_credential_if_matches(
        &self,
        id: i64,
        expected_data: &str,
        item: &AuthCredential,
        lease: Option<&CredentialRefreshLeaseFence>,
    ) -> Result<bool> {
        let updated = self.try_update_auth_credential_data(id, expected_data, item, lease)?;
        if updated {
            let provider = self.provider_for_id(id)?;
            if !provider.is_empty() {
                self.purge_superseded_disabled_rows(&provider, &self.list_auth_credentials(Some(&provider))?)?;
            }
        }
        Ok(updated)
    }

    fn try_update_auth_credential_data(
        &self,
        id: i64,
        expected_data: &str,
        item: &AuthCredential,
        lease: Option<&CredentialRefreshLeaseFence>,
    ) -> Result<bool> {
        let provider = self.provider_for_id(id)?;
        let value = serialize_credential(&provider, item)?;
        let sql = format!(
            "UPDATE auth_credentials SET credential_type=?1,data=?2,identity_key=?3,updated_at={NOW_SQL}
            WHERE id=?4 AND data=?5 AND disabled_cause IS NULL{}",
            if lease.is_some() {
                " AND EXISTS(SELECT 1 FROM auth_credential_refresh_leases WHERE credential_id=?4 AND owner=?6 AND expires_at_ms>?7)"
            } else {
                ""
            }
        );
        let count = if let Some(lease) = lease {
            self.db()?.execute(
                &sql,
                params![
                    value.credential_type,
                    value.data,
                    value.identity_key,
                    id,
                    expected_data,
                    lease.owner,
                    lease.now_ms
                ],
            )?
        } else {
            self.db()?
                .execute(&sql, params![value.credential_type, value.data, value.identity_key, id, expected_data])?
        };
        Ok(count > 0)
    }

    /// Persist dispatch ownership before sending a rotating OAuth grant. The
    /// opaque key includes the exact row and refresh fingerprint, never tokens.
    /// A surviving record fences an unknown attempt even after lease expiry.
    pub(crate) fn try_begin_oauth_refresh(
        &self,
        id: i64,
        expected_data: &str,
        key: &str,
        lease: &CredentialRefreshLeaseFence,
    ) -> Result<bool> {
        Ok(self.db()?.execute(
            "INSERT INTO cache(key,value,expires_at)
             SELECT ?1,?2,?3 FROM auth_credentials c
             WHERE c.id=?4 AND c.data=?5 AND c.provider='openai-codex'
               AND c.credential_type='oauth' AND c.disabled_cause IS NULL
               AND EXISTS(SELECT 1 FROM auth_credential_refresh_leases l
                          WHERE l.credential_id=c.id AND l.owner=?2 AND l.expires_at_ms>?6)
             ON CONFLICT(key) DO NOTHING",
            params![key, lease.owner, i64::MAX, id, expected_data, lease.now_ms],
        )? == 1)
    }

    /// The submitted grant is settled only when its durable token replacement
    /// and dispatch record removal commit together under the same lease fence.
    pub(crate) fn try_commit_oauth_refresh(
        &self,
        id: i64,
        expected_data: &str,
        item: &AuthCredential,
        key: &str,
        lease: &CredentialRefreshLeaseFence,
    ) -> Result<bool> {
        self.transaction(TransactionBehavior::Immediate, |_| {
            if self.get_cache(key, true)?.as_deref() != Some(lease.owner.as_str()) {
                return Ok(false);
            }
            if !self.try_update_auth_credential_data(id, expected_data, item, Some(lease))? {
                return Ok(false);
            }
            self.clear_oauth_refresh_dispatch(key, &lease.owner)?;
            let provider = self.provider_for_id(id)?;
            self.purge_superseded_disabled_rows(&provider, &self.stored_rows(Some(&provider))?)?;
            Ok(true)
        })
    }

    /// Only a proven pre-dispatch cancellation, confirmed HTTP rejection, or
    /// successful transaction can release a dispatch record, and only its owner.
    pub(crate) fn clear_oauth_refresh_dispatch(&self, key: &str, owner: &str) -> Result<()> {
        self.db()?.execute("DELETE FROM cache WHERE key=?1 AND value=?2", params![key, owner])?;
        Ok(())
    }

    pub fn delete_auth_credential(&self, id: i64, cause: &str) -> Result<()> {
        self.db()?.execute(
            &format!("UPDATE auth_credentials SET disabled_cause=?1,updated_at={NOW_SQL} WHERE id=?2"),
            params![disabled_cause(cause), id],
        )?;
        Ok(())
    }

    pub fn try_disable_auth_credential_if_matches(
        &self,
        id: i64,
        expected_data: &str,
        cause: &str,
        lease: Option<&CredentialRefreshLeaseFence>,
    ) -> Result<bool> {
        let sql = format!(
            "UPDATE auth_credentials SET disabled_cause=?1,updated_at={NOW_SQL} WHERE id=?2 AND data=?3 AND disabled_cause IS NULL{}",
            if lease.is_some() {
                " AND EXISTS(SELECT 1 FROM auth_credential_refresh_leases WHERE credential_id=?2 AND owner=?4 AND expires_at_ms>?5)"
            } else {
                ""
            }
        );
        let count = if let Some(lease) = lease {
            self.db()?.execute(&sql, params![disabled_cause(cause), id, expected_data, lease.owner, lease.now_ms])?
        } else {
            self.db()?.execute(&sql, params![disabled_cause(cause), id, expected_data])?
        };
        Ok(count > 0)
    }

    pub fn delete_auth_credentials_for_provider(&self, provider: &str, cause: &str) -> Result<()> {
        self.db()?.execute(&format!("UPDATE auth_credentials SET disabled_cause=?1,updated_at={NOW_SQL} WHERE provider=?2 AND disabled_cause IS NULL"),params![disabled_cause(cause),provider])?;
        Ok(())
    }

    pub fn get_cache(&self, key: &str, include_expired: bool) -> Result<Option<String>> {
        let sql = if include_expired {
            "SELECT value FROM cache WHERE key=?1".into()
        } else {
            format!("SELECT value FROM cache WHERE key=?1 AND expires_at>{NOW_SQL}")
        };
        Ok(self.db()?.query_row(&sql, [key], |r| r.get(0)).optional()?)
    }
    pub fn set_cache(&self, key: &str, value: &str, expires_at_sec: i64) -> Result<()> {
        self.db()?.execute("INSERT INTO cache(key,value,expires_at) VALUES(?1,?2,?3) ON CONFLICT(key) DO UPDATE SET value=excluded.value,expires_at=excluded.expires_at",params![key,value,expires_at_sec])?;
        Ok(())
    }
    pub fn delete_cache_prefix(&self, prefix: &str) -> Result<()> {
        // Fixed OMP passes JS string.length to SQLite substr. Preserve that
        // UTF-16 count, including its non-BMP prefix behavior.
        self.db()?.execute(
            "DELETE FROM cache WHERE substr(key,1,?1)=?2",
            params![prefix.encode_utf16().count() as i64, prefix],
        )?;
        Ok(())
    }
    pub fn clean_expired_cache(&self) -> Result<()> {
        self.db()?.execute(&format!("DELETE FROM cache WHERE expires_at<={NOW_SQL}"), [])?;
        Ok(())
    }
}

fn disabled_cause(cause: &str) -> &str {
    if cause.trim().is_empty() { "disabled" } else { cause.trim() }
}

impl SqliteCredentialStore {
    fn block_row(&self, id: i64, provider: &str, scope: &str) -> Result<Option<(i64, i64)>> {
        if provider == CODEX_PROVIDER && scope == "shared" {
            return Ok(None);
        }
        let now = now_ms();
        if provider != CODEX_PROVIDER {
            self.db()?.execute("DELETE FROM auth_credential_blocks WHERE blocked_until_ms<=?1", [now])?;
        }
        Ok(self.db()?.query_row("SELECT blocked_until_ms,updated_at FROM auth_credential_blocks WHERE credential_id=?1 AND provider_key=?2 AND block_scope=?3 AND blocked_until_ms>?4",params![id,provider,scope,now],|r|Ok((r.get(0)?,r.get(1)?))).optional()?)
    }
    pub fn get_credential_block(&self, id: i64, provider: &str, scope: &str) -> Result<Option<i64>> {
        Ok(self.block_row(id, provider, scope)?.map(|r| r.0))
    }
    pub fn get_credential_block_reconcile_after(&self, id: i64, provider: &str, scope: &str) -> Result<Option<i64>> {
        let Some((until, updated)) = self.block_row(id, provider, scope)? else { return Ok(None) };
        let memory = self.reconcile_after.borrow().get(&(id, provider.into(), scope.into())).copied().unwrap_or(0);
        let reconcile = memory.max(updated * 1000 + USAGE_REPORT_TTL_MS);
        Ok((reconcile > now_ms()).then_some(until.min(reconcile)))
    }
    pub fn upsert_credential_block(&self, block: &StoredCredentialBlock) -> Result<()> {
        let legacy = block.provider_key == CODEX_PROVIDER && block.block_scope == "shared";
        let scopes = if legacy { vec!["chat", "spark"] } else { vec![block.block_scope.as_str()] };
        self.transaction(TransactionBehavior::Immediate,|_| {
            for scope in &scopes {self.db()?.execute(&format!("INSERT INTO auth_credential_blocks(credential_id,provider_key,block_scope,blocked_until_ms,updated_at)
                VALUES(?1,?2,?3,?4,{NOW_SQL}) ON CONFLICT(credential_id,provider_key,block_scope) DO UPDATE SET blocked_until_ms=MAX(blocked_until_ms,excluded.blocked_until_ms),updated_at=excluded.updated_at"),params![block.credential_id,block.provider_key,scope,block.blocked_until_ms])?;}Ok(())
        })?;
        let reconcile = block.blocked_until_ms.min(now_ms() + USAGE_REPORT_TTL_MS);
        let mut memory = self.reconcile_after.borrow_mut();
        for scope in scopes {
            memory.insert((block.credential_id, block.provider_key.clone(), scope.into()), reconcile);
        }
        if legacy {
            memory.remove(&(block.credential_id, block.provider_key.clone(), "shared".into()));
        }
        Ok(())
    }
    pub fn delete_credential_block(&self, id: i64, provider: &str, scope: &str) -> Result<()> {
        self.db()?.execute(
            "DELETE FROM auth_credential_blocks WHERE credential_id=?1 AND provider_key=?2 AND block_scope=?3",
            params![id, provider, scope],
        )?;
        self.reconcile_after.borrow_mut().remove(&(id, provider.into(), scope.into()));
        Ok(())
    }
    pub fn delete_credential_blocks(&self, id: i64) -> Result<()> {
        self.db()?.execute("DELETE FROM auth_credential_blocks WHERE credential_id=?1", [id])?;
        self.reconcile_after.borrow_mut().retain(|(credential, ..), _| *credential != id);
        Ok(())
    }
    pub fn clean_expired_credential_blocks(&self, now: i64) -> Result<()> {
        self.db()?.execute("DELETE FROM auth_credential_blocks WHERE blocked_until_ms<=?1", [now])?;
        self.reconcile_after.borrow_mut().retain(|_, reconcile| *reconcile > now);
        Ok(())
    }
    pub fn list_credential_blocks(&self, ids: &[i64]) -> Result<Vec<StoredCredentialBlock>> {
        if ids.is_empty() {
            return Ok(Vec::new());
        }
        let now = now_ms();
        self.clean_expired_credential_blocks(now)?;
        let mut result = Vec::new();
        let mut seen = BTreeSet::new();
        let mut stmt=self.db()?.prepare_cached("SELECT credential_id,provider_key,block_scope,blocked_until_ms,updated_at FROM auth_credential_blocks WHERE credential_id=?1 AND blocked_until_ms>?2 AND NOT(provider_key=?3 AND block_scope='shared') ORDER BY provider_key ASC,block_scope ASC")?;
        for id in ids {
            if seen.insert(*id) {
                result.extend(
                    stmt.query_map(params![id, now, CODEX_PROVIDER], |r| {
                        Ok(StoredCredentialBlock {
                            credential_id: r.get(0)?,
                            provider_key: r.get(1)?,
                            block_scope: r.get(2)?,
                            blocked_until_ms: r.get(3)?,
                            updated_at_ms: r.get::<_, i64>(4)? * 1000,
                        })
                    })?
                    .collect::<rusqlite::Result<Vec<_>>>()?,
                );
            }
        }
        Ok(result)
    }
    pub fn try_acquire_credential_refresh_lease(&self, id: i64, owner: &str, expires_at_ms: i64) -> Result<bool> {
        Ok(self.db()?.execute(&format!("INSERT INTO auth_credential_refresh_leases(credential_id,owner,expires_at_ms,updated_at) VALUES(?1,?2,?3,{NOW_SQL})
            ON CONFLICT(credential_id) DO UPDATE SET owner=excluded.owner,expires_at_ms=excluded.expires_at_ms,updated_at=excluded.updated_at WHERE auth_credential_refresh_leases.expires_at_ms<=?4"),params![id,owner,expires_at_ms,now_ms()])?==1)
    }
    pub fn get_credential_refresh_lease_expires_at(&self, id: i64) -> Result<Option<i64>> {
        let expiry: Option<i64> = self
            .db()?
            .query_row("SELECT expires_at_ms FROM auth_credential_refresh_leases WHERE credential_id=?1", [id], |r| {
                r.get(0)
            })
            .optional()?;
        Ok(expiry.filter(|expiry| *expiry > now_ms()))
    }
    pub fn renew_credential_refresh_lease(&self, id: i64, owner: &str, expires_at_ms: i64) -> Result<bool> {
        Ok(self.db()?.execute(&format!("UPDATE auth_credential_refresh_leases SET expires_at_ms=?1,updated_at={NOW_SQL} WHERE credential_id=?2 AND owner=?3"),params![expires_at_ms,id,owner])?==1)
    }
    pub fn release_credential_refresh_lease(&self, id: i64, owner: &str) -> Result<()> {
        self.db()?.execute(
            "DELETE FROM auth_credential_refresh_leases WHERE credential_id=?1 AND owner=?2",
            params![id, owner],
        )?;
        Ok(())
    }
    pub fn record_usage_snapshots(&self, entries: &[UsageHistoryEntry]) -> Result<()> {
        for e in entries {
            let last:Option<(i64,i64)>=self.db()?.query_row("SELECT id,recorded_at FROM usage_history WHERE provider=?1 AND account_key=?2 AND limit_id=?3 ORDER BY recorded_at DESC LIMIT 1",params![e.provider,e.account_key,e.limit_id],|r|Ok((r.get(0)?,r.get(1)?))).optional()?;
            if let Some((id, at)) = last.filter(|(_, at)| {
                at.div_euclid(USAGE_HISTORY_BUCKET_MS) == e.recorded_at.div_euclid(USAGE_HISTORY_BUCKET_MS)
            }) {
                let _ = at;
                self.db()?.execute("UPDATE usage_history SET recorded_at=?1,email=?2,account_id=?3,label=?4,window_label=?5,used_fraction=?6,status=?7,resets_at=?8 WHERE id=?9",params![e.recorded_at,e.email,e.account_id,e.label,e.window_label,e.used_fraction,e.status,e.resets_at,id])?;
            } else {
                self.db()?.execute("INSERT INTO usage_history(recorded_at,provider,account_key,email,account_id,limit_id,label,window_label,used_fraction,status,resets_at) VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11)",params![e.recorded_at,e.provider,e.account_key,e.email,e.account_id,e.limit_id,e.label,e.window_label,e.used_fraction,e.status,e.resets_at])?;
            }
        }
        Ok(())
    }
    pub fn list_usage_history(&self, query: Option<&UsageHistoryQuery>) -> Result<Vec<UsageHistoryEntry>> {
        let mut stmt=self.db()?.prepare_cached("SELECT recorded_at,provider,account_key,email,account_id,limit_id,label,window_label,used_fraction,status,resets_at FROM usage_history WHERE recorded_at>=?1 AND (?2 IS NULL OR provider=?2) ORDER BY recorded_at ASC")?;
        Ok(stmt
            .query_map(
                params![query.and_then(|q| q.since_ms).unwrap_or(0), query.and_then(|q| q.provider.as_deref())],
                |r| {
                    Ok(UsageHistoryEntry {
                        recorded_at: r.get(0)?,
                        provider: r.get(1)?,
                        account_key: r.get(2)?,
                        email: r.get(3)?,
                        account_id: r.get(4)?,
                        limit_id: r.get(5)?,
                        label: r.get(6)?,
                        window_label: r.get(7)?,
                        used_fraction: r.get(8)?,
                        status: r.get(9)?,
                        resets_at: r.get(10)?,
                    })
                },
            )?
            .collect::<rusqlite::Result<Vec<_>>>()?)
    }
    pub fn record_client_usage(&self, report: &ClientUsageReport) -> Result<()> {
        let now = now_ms();
        self.db()?.execute("INSERT INTO clients(install_id,hostname,first_seen,last_seen) VALUES(?1,?2,?3,?3) ON CONFLICT(install_id) DO UPDATE SET hostname=COALESCE(excluded.hostname,hostname),last_seen=excluded.last_seen",params![report.install_id,report.hostname,now])?;
        let app = report.app.as_deref().unwrap_or("").trim();
        for e in &report.entries {
            let existing:Option<i64>=self.db()?.query_row("SELECT id FROM client_usage WHERE install_id=?1 AND app=?2 AND provider=?3 AND model=?4 AND recorded_at>=?5 ORDER BY recorded_at DESC LIMIT 1",params![report.install_id,app,e.provider,e.model,e.at-CLIENT_USAGE_BUCKET_MS],|r|r.get(0)).optional()?;
            if let Some(id) = existing {
                self.db()?.execute("UPDATE client_usage SET recorded_at=?1,requests=requests+?2,input_tokens=input_tokens+?3,output_tokens=output_tokens+?4,cache_read_tokens=cache_read_tokens+?5,cache_write_tokens=cache_write_tokens+?6,cost_usd=cost_usd+?7 WHERE id=?8",params![e.at,e.requests,e.input_tokens,e.output_tokens,e.cache_read_tokens,e.cache_write_tokens,e.cost_usd,id])?;
            } else {
                self.db()?.execute("INSERT INTO client_usage(recorded_at,install_id,app,provider,model,requests,input_tokens,output_tokens,cache_read_tokens,cache_write_tokens,cost_usd) VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11)",params![e.at,report.install_id,app,e.provider,e.model,e.requests,e.input_tokens,e.output_tokens,e.cache_read_tokens,e.cache_write_tokens,e.cost_usd])?;
            }
        }
        Ok(())
    }
    pub fn get_client_usage_summary(&self, since_ms: i64) -> Result<ClientUsageSummary> {
        let mut aggregates=self.db()?.prepare_cached("SELECT install_id,app,provider,SUM(requests),SUM(input_tokens),SUM(output_tokens),SUM(cache_read_tokens),SUM(cache_write_tokens),SUM(cost_usd) FROM client_usage WHERE recorded_at>=?1 GROUP BY install_id,app,provider ORDER BY install_id,SUM(input_tokens+output_tokens+cache_read_tokens+cache_write_tokens) DESC")?;
        let rows = aggregates
            .query_map([since_ms], |r| {
                Ok((
                    r.get::<_, String>(0)?,
                    ClientProviderUsage {
                        app: Some(r.get::<_, String>(1)?).filter(|v| !v.is_empty()),
                        provider: r.get(2)?,
                        requests: r.get(3)?,
                        input_tokens: r.get(4)?,
                        output_tokens: r.get(5)?,
                        cache_read_tokens: r.get(6)?,
                        cache_write_tokens: r.get(7)?,
                        cost_usd: r.get(8)?,
                    },
                ))
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        let mut grouped: BTreeMap<String, Vec<ClientProviderUsage>> = BTreeMap::new();
        for (install, value) in rows {
            grouped.entry(install).or_default().push(value);
        }
        let mut clients = self
            .db()?
            .prepare_cached("SELECT install_id,hostname,first_seen,last_seen FROM clients ORDER BY last_seen DESC")?;
        let mut result = clients
            .query_map([], |r| {
                Ok(ClientUsageClient {
                    install_id: r.get(0)?,
                    hostname: r.get(1)?,
                    first_seen: r.get(2)?,
                    last_seen: r.get(3)?,
                    providers: Vec::new(),
                })
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        for client in &mut result {
            client.providers = grouped.remove(&client.install_id).unwrap_or_default();
        }
        Ok(ClientUsageSummary { clients: result })
    }
    pub fn save_oauth(&self, provider: &str, fields: Map<String, Value>) -> Result<()> {
        self.upsert_auth_credential_for_provider(provider, &AuthCredential::OAuth { fields })?;
        Ok(())
    }
    pub fn get_oauth(&self, provider: &str) -> Result<Option<Map<String, Value>>> {
        Ok(self.list_auth_credentials(Some(provider))?.into_iter().find_map(|row| match row.credential {
            AuthCredential::OAuth { fields } => Some(fields),
            _ => None,
        }))
    }
    pub fn save_api_key(&self, provider: &str, key: &str) -> Result<()> {
        self.replace_auth_credentials_for_provider(provider, &[AuthCredential::api_key(key)])?;
        Ok(())
    }
    pub fn get_api_key(&self, provider: &str) -> Result<Option<String>> {
        Ok(self.list_auth_credentials(Some(provider))?.into_iter().find_map(|row| match row.credential {
            AuthCredential::ApiKey { key, .. } => Some(key),
            _ => None,
        }))
    }
    pub fn list_providers(&self) -> Result<Vec<String>> {
        let mut result = Vec::new();
        for row in self.rows(None, false)? {
            if !result.contains(&row.provider) {
                result.push(row.provider);
            }
        }
        Ok(result)
    }
    pub fn delete_provider(&self, provider: &str) -> Result<()> {
        self.delete_auth_credentials_for_provider(provider, "deleted by user")
    }
    fn acknowledge_local_auth_changes(&self) -> Result<()> {
        let local: i64 =
            self.db()?.query_row("SELECT revision FROM auth_local_change_revision WHERE id=1", [], |r| r.get(0))?;
        self.auth_revision.set(self.auth_revision.get() + local - self.local_revision.get());
        self.local_revision.set(local);
        Ok(())
    }
    pub fn acknowledge_local_changes(&self) -> Result<()> {
        self.acknowledge_local_auth_changes()
    }
    pub fn poll_external_changes(&self) -> Result<bool> {
        self.acknowledge_local_auth_changes()?;
        let data: i64 = self.db()?.query_row("PRAGMA data_version", [], |r| r.get(0))?;
        if data == self.data_version.get() {
            return Ok(false);
        }
        self.data_version.set(data);
        let auth = self.revision()?;
        if auth == self.auth_revision.get() {
            return Ok(false);
        }
        self.auth_revision.set(auth);
        Ok(true)
    }
    pub fn close(&mut self) -> Result<()> {
        if let Some(conn) = self.conn.take()
            && let Err((conn, error)) = conn.close()
        {
            self.conn = Some(conn);
            return Err(error.into());
        }
        Ok(())
    }
}

fn columns(conn: &Connection, table: &str) -> Result<BTreeSet<String>> {
    // Table names are private constants, never caller-controlled SQL fragments.
    let mut stmt = conn.prepare(&format!("PRAGMA table_info({table})"))?;
    Ok(stmt.query_map([], |r| r.get(1))?.collect::<rusqlite::Result<BTreeSet<_>>>()?)
}
fn initialize_schema(conn: &Connection) -> Result<Option<i64>> {
    conn.execute_batch(BASE_SCHEMA)?;
    if !columns(conn, "client_usage")?.contains("app") {
        conn.execute_batch("ALTER TABLE client_usage ADD COLUMN app TEXT NOT NULL DEFAULT ''")?;
    }
    let exists = conn
        .query_row("SELECT 1 FROM sqlite_master WHERE type='table' AND name='auth_credentials'", [], |r| {
            r.get::<_, i64>(0)
        })
        .optional()?
        .is_some();
    let recorded: Option<i64> =
        conn.query_row("SELECT version FROM auth_schema_version WHERE id=1", [], |r| r.get(0)).optional()?;
    let version = if exists { recorded.unwrap_or(0) } else { AUTH_SCHEMA_VERSION };
    if !exists {
        conn.execute_batch(CREDENTIAL_SCHEMA)?;
        conn.execute_batch(BLOCK_SCHEMA)?;
        conn.execute_batch(LEASE_SCHEMA)?;
        conn.execute_batch(MIRROR_SCHEMA)?;
        conn.execute_batch(COMPATIBILITY_TRIGGERS)?;
        create_revision_objects(conn)?;
        write_schema_version(conn, AUTH_SCHEMA_VERSION)?;
    } else {
        let cols = columns(conn, "auth_credentials")?;
        let inferred = if cols.contains("identity_key") {
            3
        } else if cols.contains("account_id") || cols.contains("email") {
            2
        } else if cols.contains("disabled_cause") {
            1
        } else {
            0
        };
        let version = recorded.unwrap_or(inferred);
        if version < AUTH_SCHEMA_VERSION {
            migrate_schema(conn, version)?;
        }
        conn.execute_batch(CREDENTIAL_INDEXES)?;
        conn.execute_batch(BLOCK_SCHEMA)?;
        conn.execute_batch(LEASE_SCHEMA)?;
        if version <= AUTH_SCHEMA_VERSION {
            conn.execute_batch(MIRROR_SCHEMA)?;
            conn.execute_batch(COMPATIBILITY_TRIGGERS)?;
        }
        create_revision_objects(conn)?;
        if version <= AUTH_SCHEMA_VERSION && recorded != Some(AUTH_SCHEMA_VERSION) {
            write_schema_version(conn, AUTH_SCHEMA_VERSION)?;
        }
    }
    backfill_identities(conn)?;
    Ok((version > AUTH_SCHEMA_VERSION).then_some(version))
}
fn write_schema_version(conn: &Connection, version: i64) -> Result<()> {
    conn.execute("INSERT OR REPLACE INTO auth_schema_version(id,version) VALUES(1,?1)", [version])?;
    Ok(())
}
fn create_revision_objects(conn: &Connection) -> Result<()> {
    conn.execute_batch(REVISION_SCHEMA)?;
    for table in ["auth_credentials", "auth_credential_blocks"] {
        for event in ["INSERT", "UPDATE", "DELETE"] {
            let lower = event.to_lowercase();
            conn.execute_batch(&format!(
                "CREATE TRIGGER IF NOT EXISTS auth_change_revision_{table}_{lower} AFTER {event} ON {table}
            BEGIN UPDATE auth_change_revision SET revision=revision+1 WHERE id=1; END;
            CREATE TEMP TRIGGER IF NOT EXISTS auth_local_change_revision_{table}_{lower} AFTER {event} ON main.{table}
            BEGIN UPDATE auth_local_change_revision SET revision=revision+1 WHERE id=1; END;"
            ))?;
        }
    }
    Ok(())
}
fn migrate_schema(conn: &Connection, from: i64) -> Result<()> {
    if from < 1 {
        let disabled = columns(conn, "auth_credentials")?.contains("disabled");
        let tx = Transaction::new_unchecked(conn, TransactionBehavior::Deferred)?;
        tx.execute_batch("ALTER TABLE auth_credentials RENAME TO auth_credentials_v0")?;
        // v1 has no identity_key; the v3 migration below supplies it.
        tx.execute_batch(&format!("CREATE TABLE auth_credentials(id INTEGER PRIMARY KEY AUTOINCREMENT,provider TEXT NOT NULL,credential_type TEXT NOT NULL,data TEXT NOT NULL,disabled_cause TEXT DEFAULT NULL,created_at INTEGER NOT NULL DEFAULT({NOW_SQL}),updated_at INTEGER NOT NULL DEFAULT({NOW_SQL}));
            INSERT INTO auth_credentials(id,provider,credential_type,data,disabled_cause,created_at,updated_at) SELECT id,provider,credential_type,data,{},created_at,updated_at FROM auth_credentials_v0;DROP TABLE auth_credentials_v0;",if disabled {"CASE WHEN disabled=1 THEN 'disabled' ELSE NULL END"} else {"NULL"}))?;
        tx.commit()?;
    }
    if from < 3 {
        rebuild_credentials(conn, "auth_credentials_legacy", false)?;
    }
    if from < 4 {
        rebuild_credentials(conn, "auth_credentials_v3", true)?;
    }
    if from < 5 {
        let tx = Transaction::new_unchecked(conn, TransactionBehavior::Deferred)?;
        tx.execute_batch(BLOCK_SCHEMA)?;
        tx.commit()?;
    }
    if from < 6 {
        let tx = Transaction::new_unchecked(conn, TransactionBehavior::Deferred)?;
        tx.execute_batch(LEASE_SCHEMA)?;
        tx.commit()?;
    }
    if from < 7 {
        let tx = Transaction::new_unchecked(conn, TransactionBehavior::Immediate)?;
        tx.execute_batch(BLOCK_SCHEMA)?;
        tx.execute_batch(MIRROR_SCHEMA)?;
        tx.execute_batch(MIGRATE_V7)?;
        tx.execute_batch(COMPATIBILITY_TRIGGERS)?;
        write_schema_version(&tx, 7)?;
        tx.commit()?;
    }
    Ok(())
}
fn rebuild_credentials(conn: &Connection, legacy: &str, identity: bool) -> Result<()> {
    let tx = Transaction::new_unchecked(conn, TransactionBehavior::Deferred)?;
    tx.execute_batch(&format!("ALTER TABLE auth_credentials RENAME TO {legacy}"))?;
    tx.execute_batch(CREDENTIAL_SCHEMA)?;
    tx.execute_batch(&format!("INSERT INTO auth_credentials(id,provider,credential_type,data,disabled_cause,identity_key,created_at,updated_at)
        SELECT id,provider,credential_type,data,disabled_cause,{},created_at,updated_at FROM {legacy};DROP TABLE {legacy};",if identity {"identity_key"} else {"NULL"}))?;
    tx.commit()?;
    Ok(())
}
fn backfill_identities(conn: &Connection) -> Result<()> {
    let mut stmt=conn.prepare("SELECT id,provider,credential_type,data,disabled_cause,identity_key,updated_at FROM auth_credentials WHERE identity_key IS NULL ORDER BY id ASC")?;
    let rows = stmt.query_map([], row_from_sql)?.collect::<rusqlite::Result<Vec<_>>>()?;
    for row in rows {
        if let Some(identity) = row_identity(&row.provider, &row) {
            conn.execute("UPDATE auth_credentials SET identity_key=?1 WHERE id=?2", params![identity, row.id])?;
        }
    }
    Ok(())
}

pub fn is_sqlite_busy_error(error: &anyhow::Error) -> bool {
    error.downcast_ref::<rusqlite::Error>().is_some_and(|error|matches!(error,rusqlite::Error::SqliteFailure(code,_) if code.code==rusqlite::ErrorCode::DatabaseBusy || code.code==rusqlite::ErrorCode::DatabaseLocked))
}

pub fn is_sqlite_corruption_error(error: &anyhow::Error) -> bool {
    error.downcast_ref::<rusqlite::Error>().is_some_and(|error|matches!(error,rusqlite::Error::SqliteFailure(code,_) if code.code==rusqlite::ErrorCode::DatabaseCorrupt || code.code==rusqlite::ErrorCode::NotADatabase))
}

// SQL preserved from the fixed upstream schema/migration method.
const BASE_SCHEMA: &str = r#"
PRAGMA journal_mode=WAL;
PRAGMA synchronous=NORMAL;
CREATE TABLE IF NOT EXISTS auth_schema_version (
	id INTEGER PRIMARY KEY CHECK (id = 1),
	version INTEGER NOT NULL
);
CREATE TABLE IF NOT EXISTS cache (
	key TEXT PRIMARY KEY,
	value TEXT NOT NULL,
	expires_at INTEGER NOT NULL
);
CREATE INDEX IF NOT EXISTS idx_cache_expires ON cache(expires_at);
CREATE TABLE IF NOT EXISTS usage_history (
	id INTEGER PRIMARY KEY AUTOINCREMENT,
	recorded_at INTEGER NOT NULL,
	provider TEXT NOT NULL,
	account_key TEXT NOT NULL,
	email TEXT,
	account_id TEXT,
	limit_id TEXT NOT NULL,
	label TEXT NOT NULL,
	window_label TEXT,
	used_fraction REAL,
	status TEXT,
	resets_at INTEGER
);
CREATE INDEX IF NOT EXISTS idx_usage_history_series ON usage_history(provider, account_key, limit_id, recorded_at);
CREATE INDEX IF NOT EXISTS idx_usage_history_recorded ON usage_history(recorded_at);
CREATE TABLE IF NOT EXISTS clients (
	install_id TEXT PRIMARY KEY,
	hostname TEXT,
	first_seen INTEGER NOT NULL,
	last_seen INTEGER NOT NULL
);
CREATE TABLE IF NOT EXISTS client_usage (
	id INTEGER PRIMARY KEY AUTOINCREMENT,
	recorded_at INTEGER NOT NULL,
	install_id TEXT NOT NULL,
	app TEXT NOT NULL DEFAULT '',
	provider TEXT NOT NULL,
	model TEXT NOT NULL,
	requests INTEGER NOT NULL,
	input_tokens INTEGER NOT NULL,
	output_tokens INTEGER NOT NULL,
	cache_read_tokens INTEGER NOT NULL,
	cache_write_tokens INTEGER NOT NULL,
	cost_usd REAL NOT NULL DEFAULT 0
);
CREATE INDEX IF NOT EXISTS idx_client_usage_series ON client_usage(install_id, provider, model, recorded_at);
CREATE INDEX IF NOT EXISTS idx_client_usage_recorded ON client_usage(recorded_at);
"#;

// SQL preserved from the fixed upstream schema/migration method.
const CREDENTIAL_SCHEMA: &str = r#"
CREATE TABLE IF NOT EXISTS auth_credentials (
	id INTEGER PRIMARY KEY AUTOINCREMENT,
	provider TEXT NOT NULL,
	credential_type TEXT NOT NULL,
	data TEXT NOT NULL,
	disabled_cause TEXT DEFAULT NULL,
	identity_key TEXT DEFAULT NULL,
	created_at INTEGER NOT NULL DEFAULT (CAST(strftime('%s','now') AS INTEGER)),
	updated_at INTEGER NOT NULL DEFAULT (CAST(strftime('%s','now') AS INTEGER))
);


CREATE INDEX IF NOT EXISTS idx_auth_provider ON auth_credentials(provider);
CREATE INDEX IF NOT EXISTS idx_auth_provider_identity ON auth_credentials(provider, identity_key) WHERE identity_key IS NOT NULL;
"#;

// SQL preserved from the fixed upstream schema/migration method.
const CREDENTIAL_INDEXES: &str = r#"
CREATE INDEX IF NOT EXISTS idx_auth_provider ON auth_credentials(provider);
CREATE INDEX IF NOT EXISTS idx_auth_provider_identity ON auth_credentials(provider, identity_key) WHERE identity_key IS NOT NULL;
"#;

// SQL preserved from the fixed upstream schema/migration method.
const BLOCK_SCHEMA: &str = r#"
CREATE TABLE IF NOT EXISTS auth_credential_blocks (
	credential_id INTEGER NOT NULL,
	provider_key TEXT NOT NULL,
	block_scope TEXT NOT NULL DEFAULT '',
	blocked_until_ms INTEGER NOT NULL,
	updated_at INTEGER NOT NULL,
	PRIMARY KEY (credential_id, provider_key, block_scope)
);
CREATE INDEX IF NOT EXISTS idx_auth_credential_blocks_expires ON auth_credential_blocks(blocked_until_ms);
"#;

// SQL preserved from the fixed upstream schema/migration method.
const LEASE_SCHEMA: &str = r#"
CREATE TABLE IF NOT EXISTS auth_credential_refresh_leases (
	credential_id INTEGER PRIMARY KEY,
	owner TEXT NOT NULL,
	expires_at_ms INTEGER NOT NULL,
	updated_at INTEGER NOT NULL
);
CREATE INDEX IF NOT EXISTS idx_auth_credential_refresh_leases_expires ON auth_credential_refresh_leases(expires_at_ms);
"#;

// SQL preserved from the fixed upstream schema/migration method.
const REVISION_SCHEMA: &str = r#"
CREATE TABLE IF NOT EXISTS auth_change_revision (
	id INTEGER PRIMARY KEY CHECK (id = 1),
	revision INTEGER NOT NULL
);
INSERT OR IGNORE INTO auth_change_revision (id, revision) VALUES (1, 0);
CREATE TEMP TABLE IF NOT EXISTS auth_local_change_revision (
	id INTEGER PRIMARY KEY CHECK (id = 1),
	revision INTEGER NOT NULL
);
INSERT OR IGNORE INTO auth_local_change_revision (id, revision) VALUES (1, 0);
"#;

// SQL preserved from the fixed upstream schema/migration method.
const MIRROR_SCHEMA: &str = r#"
CREATE TABLE IF NOT EXISTS auth_credential_block_mirror_guard (
	credential_id INTEGER PRIMARY KEY
) WITHOUT ROWID;
"#;

// SQL preserved from the fixed upstream schema/migration method.
const MIGRATE_V7: &str = r#"
DELETE FROM auth_credential_block_mirror_guard;
INSERT OR IGNORE INTO auth_credential_block_mirror_guard (credential_id)
SELECT DISTINCT credential_id
FROM auth_credential_blocks
WHERE provider_key = 'openai-codex:oauth'
	AND block_scope IN ('chat', 'spark', 'shared');

INSERT INTO auth_credential_blocks (
	credential_id,
	provider_key,
	block_scope,
	blocked_until_ms,
	updated_at
)
SELECT credential_id, provider_key, 'chat', blocked_until_ms, updated_at
FROM auth_credential_blocks
WHERE provider_key = 'openai-codex:oauth'
	AND block_scope = 'shared'
ON CONFLICT(credential_id, provider_key, block_scope) DO UPDATE SET
	blocked_until_ms = MAX(auth_credential_blocks.blocked_until_ms, excluded.blocked_until_ms),
	updated_at = MAX(auth_credential_blocks.updated_at, excluded.updated_at);

INSERT INTO auth_credential_blocks (
	credential_id,
	provider_key,
	block_scope,
	blocked_until_ms,
	updated_at
)
SELECT credential_id, provider_key, 'spark', blocked_until_ms, updated_at
FROM auth_credential_blocks
WHERE provider_key = 'openai-codex:oauth'
	AND block_scope = 'shared'
ON CONFLICT(credential_id, provider_key, block_scope) DO UPDATE SET
	blocked_until_ms = MAX(auth_credential_blocks.blocked_until_ms, excluded.blocked_until_ms),
	updated_at = MAX(auth_credential_blocks.updated_at, excluded.updated_at);

INSERT INTO auth_credential_blocks (
	credential_id,
	provider_key,
	block_scope,
	blocked_until_ms,
	updated_at
)
SELECT
	credential_id,
	provider_key,
	'shared',
	MAX(blocked_until_ms),
	MAX(updated_at)
FROM auth_credential_blocks
WHERE provider_key = 'openai-codex:oauth'
	AND block_scope IN ('chat', 'spark')
GROUP BY credential_id, provider_key
ON CONFLICT(credential_id, provider_key, block_scope) DO UPDATE SET
	blocked_until_ms = excluded.blocked_until_ms,
	updated_at = excluded.updated_at;

DELETE FROM auth_credential_block_mirror_guard;
"#;

// SQL preserved from the fixed upstream schema/migration method.
const COMPATIBILITY_TRIGGERS: &str = r#"
CREATE TRIGGER IF NOT EXISTS auth_codex_shared_insert_to_meters
	AFTER INSERT ON auth_credential_blocks
	WHEN NEW.provider_key = 'openai-codex:oauth'
		AND NEW.block_scope = 'shared'
		AND NOT EXISTS (
			SELECT 1 FROM auth_credential_block_mirror_guard
			WHERE credential_id = NEW.credential_id
		)
	BEGIN
		INSERT OR IGNORE INTO auth_credential_block_mirror_guard (credential_id)
		VALUES (NEW.credential_id);
		INSERT INTO auth_credential_blocks (
			credential_id,
			provider_key,
			block_scope,
			blocked_until_ms,
			updated_at
		)
		VALUES (
			NEW.credential_id,
			NEW.provider_key,
			'chat',
			NEW.blocked_until_ms,
			NEW.updated_at
		)
		ON CONFLICT(credential_id, provider_key, block_scope) DO UPDATE SET
			blocked_until_ms = MAX(auth_credential_blocks.blocked_until_ms, excluded.blocked_until_ms),
			updated_at = MAX(auth_credential_blocks.updated_at, excluded.updated_at);
		INSERT INTO auth_credential_blocks (
			credential_id,
			provider_key,
			block_scope,
			blocked_until_ms,
			updated_at
		)
		VALUES (
			NEW.credential_id,
			NEW.provider_key,
			'spark',
			NEW.blocked_until_ms,
			NEW.updated_at
		)
		ON CONFLICT(credential_id, provider_key, block_scope) DO UPDATE SET
			blocked_until_ms = MAX(auth_credential_blocks.blocked_until_ms, excluded.blocked_until_ms),
			updated_at = MAX(auth_credential_blocks.updated_at, excluded.updated_at);
		DELETE FROM auth_credential_block_mirror_guard
		WHERE credential_id = NEW.credential_id;
	END;

	CREATE TRIGGER IF NOT EXISTS auth_codex_meter_insert_to_shared
	AFTER INSERT ON auth_credential_blocks
	WHEN NEW.provider_key = 'openai-codex:oauth'
		AND NEW.block_scope IN ('chat', 'spark')
		AND NOT EXISTS (
			SELECT 1 FROM auth_credential_block_mirror_guard
			WHERE credential_id = NEW.credential_id
		)
	BEGIN
		INSERT OR IGNORE INTO auth_credential_block_mirror_guard (credential_id)
		VALUES (NEW.credential_id);
		DELETE FROM auth_credential_blocks
		WHERE credential_id = NEW.credential_id
			AND provider_key = NEW.provider_key
			AND block_scope = 'shared';
		INSERT INTO auth_credential_blocks (
			credential_id,
			provider_key,
			block_scope,
			blocked_until_ms,
			updated_at
		)
		SELECT
			NEW.credential_id,
			NEW.provider_key,
			'shared',
			MAX(blocked_until_ms),
			MAX(updated_at)
		FROM auth_credential_blocks
		WHERE credential_id = NEW.credential_id
			AND provider_key = NEW.provider_key
			AND block_scope IN ('chat', 'spark')
		GROUP BY credential_id, provider_key;
		DELETE FROM auth_credential_block_mirror_guard
		WHERE credential_id = NEW.credential_id;
	END;


	CREATE TRIGGER IF NOT EXISTS auth_codex_shared_update_to_meters
	AFTER UPDATE ON auth_credential_blocks
	WHEN NEW.provider_key = 'openai-codex:oauth'
		AND NEW.block_scope = 'shared'
		AND NOT EXISTS (
			SELECT 1 FROM auth_credential_block_mirror_guard
			WHERE credential_id = NEW.credential_id
		)
	BEGIN
		INSERT OR IGNORE INTO auth_credential_block_mirror_guard (credential_id)
		VALUES (NEW.credential_id);
		INSERT INTO auth_credential_blocks (
			credential_id,
			provider_key,
			block_scope,
			blocked_until_ms,
			updated_at
		)
		VALUES (
			NEW.credential_id,
			NEW.provider_key,
			'chat',
			NEW.blocked_until_ms,
			NEW.updated_at
		)
		ON CONFLICT(credential_id, provider_key, block_scope) DO UPDATE SET
			blocked_until_ms = MAX(auth_credential_blocks.blocked_until_ms, excluded.blocked_until_ms),
			updated_at = MAX(auth_credential_blocks.updated_at, excluded.updated_at);
		INSERT INTO auth_credential_blocks (
			credential_id,
			provider_key,
			block_scope,
			blocked_until_ms,
			updated_at
		)
		VALUES (
			NEW.credential_id,
			NEW.provider_key,
			'spark',
			NEW.blocked_until_ms,
			NEW.updated_at
		)
		ON CONFLICT(credential_id, provider_key, block_scope) DO UPDATE SET
			blocked_until_ms = MAX(auth_credential_blocks.blocked_until_ms, excluded.blocked_until_ms),
			updated_at = MAX(auth_credential_blocks.updated_at, excluded.updated_at);
		DELETE FROM auth_credential_block_mirror_guard
		WHERE credential_id = NEW.credential_id;
	END;

	CREATE TRIGGER IF NOT EXISTS auth_codex_meter_update_to_shared
	AFTER UPDATE ON auth_credential_blocks
	WHEN NEW.provider_key = 'openai-codex:oauth'
		AND NEW.block_scope IN ('chat', 'spark')
		AND NOT EXISTS (
			SELECT 1 FROM auth_credential_block_mirror_guard
			WHERE credential_id = NEW.credential_id
		)
	BEGIN
		INSERT OR IGNORE INTO auth_credential_block_mirror_guard (credential_id)
		VALUES (NEW.credential_id);
		DELETE FROM auth_credential_blocks
		WHERE credential_id = NEW.credential_id
			AND provider_key = NEW.provider_key
			AND block_scope = 'shared';
		INSERT INTO auth_credential_blocks (
			credential_id,
			provider_key,
			block_scope,
			blocked_until_ms,
			updated_at
		)
		SELECT
			NEW.credential_id,
			NEW.provider_key,
			'shared',
			MAX(blocked_until_ms),
			MAX(updated_at)
		FROM auth_credential_blocks
		WHERE credential_id = NEW.credential_id
			AND provider_key = NEW.provider_key
			AND block_scope IN ('chat', 'spark')
		GROUP BY credential_id, provider_key;
		DELETE FROM auth_credential_block_mirror_guard
		WHERE credential_id = NEW.credential_id;
	END;


CREATE TRIGGER IF NOT EXISTS auth_codex_shared_delete_to_meters
AFTER DELETE ON auth_credential_blocks
WHEN OLD.provider_key = 'openai-codex:oauth'
	AND OLD.block_scope = 'shared'
	AND NOT EXISTS (
		SELECT 1 FROM auth_credential_block_mirror_guard
		WHERE credential_id = OLD.credential_id
	)
BEGIN
	INSERT OR IGNORE INTO auth_credential_block_mirror_guard (credential_id)
	VALUES (OLD.credential_id);
	DELETE FROM auth_credential_blocks
	WHERE credential_id = OLD.credential_id
		AND provider_key = OLD.provider_key
		AND block_scope IN ('chat', 'spark');
	DELETE FROM auth_credential_block_mirror_guard
	WHERE credential_id = OLD.credential_id;
END;

CREATE TRIGGER IF NOT EXISTS auth_codex_meter_delete_to_shared
AFTER DELETE ON auth_credential_blocks
WHEN OLD.provider_key = 'openai-codex:oauth'
	AND OLD.block_scope IN ('chat', 'spark')
	AND NOT EXISTS (
		SELECT 1 FROM auth_credential_block_mirror_guard
		WHERE credential_id = OLD.credential_id
	)
BEGIN
	INSERT OR IGNORE INTO auth_credential_block_mirror_guard (credential_id)
	VALUES (OLD.credential_id);
	DELETE FROM auth_credential_blocks
	WHERE credential_id = OLD.credential_id
		AND provider_key = OLD.provider_key
		AND block_scope = 'shared';
	INSERT INTO auth_credential_blocks (
		credential_id,
		provider_key,
		block_scope,
		blocked_until_ms,
		updated_at
	)
	SELECT
		OLD.credential_id,
		OLD.provider_key,
		'shared',
		MAX(blocked_until_ms),
		MAX(updated_at)
	FROM auth_credential_blocks
	WHERE credential_id = OLD.credential_id
		AND provider_key = OLD.provider_key
		AND block_scope IN ('chat', 'spark')
	GROUP BY credential_id, provider_key;
	DELETE FROM auth_credential_block_mirror_guard
	WHERE credential_id = OLD.credential_id;
END;
"#;

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{Arc, Barrier};

    fn store() -> SqliteCredentialStore {
        SqliteCredentialStore::from_connection(Connection::open_in_memory().unwrap(), Duration::from_millis(200))
            .unwrap()
    }
    fn oauth(email: &str, org: &str, access: &str) -> AuthCredential {
        AuthCredential::oauth(
            json!({"email":email,"orgId":org,"access":access,"refresh":"fixture-refresh","expires":123})
                .as_object()
                .unwrap()
                .clone(),
        )
    }
    #[test]
    fn js_identity_trimming_reuses_bom_account_and_preserves_next_line() {
        let s = store();
        let original = oauth("\u{feff}A@TEST\u{feff}", "\u{feff}org\u{feff}", "old");
        let first = s.upsert_auth_credential_for_provider("anthropic", &original).unwrap().remove(0);
        let renewed = s.upsert_auth_credential_for_provider("anthropic", &oauth("a@test", "org", "new")).unwrap();
        assert_eq!(renewed.len(), 1);
        assert_eq!(renewed[0].id, first.id);
        let account =
            AuthCredential::oauth(json!({"accountId":"\u{feff}account\u{feff}"}).as_object().unwrap().clone());
        assert_eq!(resolve_credential_identity_key("fixture", &account).as_deref(), Some("account:account"));
        let next_line =
            AuthCredential::oauth(json!({"accountId":"\u{0085}account\u{0085}"}).as_object().unwrap().clone());
        assert_eq!(
            resolve_credential_identity_key("fixture", &next_line).as_deref(),
            Some("account:\u{0085}account\u{0085}")
        );
    }
    #[test]
    fn cache_prefix_uses_fixed_upstream_utf16_length() {
        let s = store();
        s.set_cache("ascii:one", "first", i64::MAX).unwrap();
        s.set_cache("other:one", "retained", i64::MAX).unwrap();
        s.delete_cache_prefix("ascii:").unwrap();
        assert!(s.get_cache("ascii:one", true).unwrap().is_none());
        assert!(s.get_cache("other:one", true).unwrap().is_some());
        let prefix = "\u{1f600}";
        let extended = format!("{prefix}one");
        s.set_cache(prefix, "exact", i64::MAX).unwrap();
        s.set_cache(&extended, "extended", i64::MAX).unwrap();
        s.delete_cache_prefix(prefix).unwrap();
        assert!(s.get_cache(prefix, true).unwrap().is_none());
        assert!(s.get_cache(&extended, true).unwrap().is_some());
    }
    #[test]
    fn fresh_native_schema_and_reopen_preserve_original_ids() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("isolated.db");
        let mut s = SqliteCredentialStore::open(&path).unwrap();
        assert_eq!(
            s.db()
                .unwrap()
                .query_row("SELECT version FROM auth_schema_version WHERE id=1", [], |r| r.get::<_, i64>(0))
                .unwrap(),
            7
        );
        for table in [
            "auth_credentials",
            "cache",
            "usage_history",
            "clients",
            "client_usage",
            "auth_credential_refresh_leases",
            "auth_credential_blocks",
            "auth_credential_block_mirror_guard",
            "auth_change_revision",
        ] {
            assert!(
                s.db()
                    .unwrap()
                    .query_row("SELECT 1 FROM sqlite_master WHERE name=?1", [table], |r| r.get::<_, i64>(0))
                    .optional()
                    .unwrap()
                    .is_some()
            );
        }
        s.save_api_key("fixture", "private-key-not-printed").unwrap();
        let first = s.list_auth_credentials(Some("fixture")).unwrap().remove(0);
        s.close().unwrap();
        s.close().unwrap();
        assert!(s.get_api_key("fixture").is_err());
        let reopened = SqliteCredentialStore::open(&path).unwrap();
        let rows = reopened.list_auth_credentials(Some("fixture")).unwrap();
        assert_eq!(rows[0].id, first.id);
        assert!(rows[0].credential == first.credential);
        assert!(rows[0].serialized_data == first.serialized_data);
    }
    #[test]
    fn org_identity_replacement_is_one_way_and_preserves_other_subscription() {
        let s = store();
        let legacy =
            AuthCredential::oauth(json!({"email":"USER@EXAMPLE.test","access":"legacy"}).as_object().unwrap().clone());
        let id = s.upsert_auth_credential_for_provider("anthropic", &legacy).unwrap()[0].id;
        s.upsert_auth_credential_for_provider("anthropic", &oauth("user@example.test", "org-a", "new")).unwrap();
        s.upsert_auth_credential_for_provider("anthropic", &oauth("user@example.test", "org-b", "other")).unwrap();
        let rows = s.list_auth_credentials(Some("anthropic")).unwrap();
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0].id, id);
        s.upsert_auth_credential_for_provider("anthropic", &legacy).unwrap();
        assert_eq!(s.list_auth_credentials(Some("anthropic")).unwrap().len(), 3);
        let member_a = AuthCredential::oauth(
            json!({"email":"member-a@test","accountId":"workspace","orgId":"workspace"}).as_object().unwrap().clone(),
        );
        let member_b =
            AuthCredential::oauth(json!({"accountId":"workspace","orgId":"workspace"}).as_object().unwrap().clone());
        assert!(!matches_replacement(
            "openai-codex",
            Some(&member_a),
            resolve_credential_identity_key("openai-codex", &member_a).as_deref(),
            &member_b
        ));
    }
    #[test]
    fn jwt_and_wrapped_token_identity_match_fixed_source() {
        let payload = base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(br#"{"email":"JWT@TEST","sub":"acct"}"#);
        let c = AuthCredential::oauth(json!({"access":format!("e30.{payload}.sig")}).as_object().unwrap().clone());
        assert_eq!(resolve_credential_identity_key("anthropic", &c).as_deref(), Some("email:jwt@test"));
        assert_eq!(resolve_credential_identity_key("other", &c).as_deref(), Some("account:acct"));
        assert!(matches_replacement(
            "alibaba-token-plan",
            Some(&AuthCredential::api_key("sk-fixture")),
            None,
            &AuthCredential::api_key(r#"{"token":"sk-fixture","cookie":"different"}"#)
        ));
        assert!(!matches_replacement(
            "alibaba-token-plan",
            Some(&AuthCredential::api_key("bad")),
            None,
            &AuthCredential::api_key(r#"{"token":"bad"}"#)
        ));
        assert!(matches_replacement(
            "cloudflare-ai-gateway",
            Some(&AuthCredential::api_key("fixture-token")),
            None,
            &AuthCredential::api_key(r#"{"token":"fixture-token","accountId":"routing"}"#)
        ));
    }
    #[test]
    fn oauth_upsert_tombstones_static_keys_and_purges_only_superseded_disabled_identities() {
        let s = store();
        s.save_api_key("anthropic", "fixture-key").unwrap();
        let key = s.list_auth_credentials(Some("anthropic")).unwrap()[0].id;
        s.upsert_auth_credential_for_provider("anthropic", &oauth("a@test", "org-a", "one")).unwrap();
        assert!(s.list_disabled_credentials(Some("anthropic")).unwrap().iter().any(|r| r.id == key));
        let old = s.list_auth_credentials(Some("anthropic")).unwrap()[0].id;
        s.delete_auth_credential(old, " expired ").unwrap();
        s.upsert_auth_credential_for_provider("anthropic", &oauth("b@test", "org-b", "two")).unwrap();
        assert!(s.list_disabled_credentials(Some("anthropic")).unwrap().iter().any(|r| r.id == old));
        s.upsert_auth_credential_for_provider("anthropic", &oauth("a@test", "org-a", "renewed")).unwrap();
        assert!(!s.list_disabled_credentials(Some("anthropic")).unwrap().iter().any(|r| r.id == old));
    }
    #[test]
    fn original_serialized_data_and_refresh_fence_prevent_peer_clobber() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("isolated.db");
        let a = SqliteCredentialStore::open(&path).unwrap();
        let b = SqliteCredentialStore::open(&path).unwrap();
        let raw = r#"{"z":1,"access":"old","email":"a@test"}"#;
        a.db()
            .unwrap()
            .execute("INSERT INTO auth_credentials(provider,credential_type,data) VALUES('fixture','oauth',?1)", [raw])
            .unwrap();
        let row = a.list_auth_credentials(Some("fixture")).unwrap().remove(0);
        assert!(row.serialized_data == raw);
        assert!(a.try_acquire_credential_refresh_lease(row.id, "owner-a", now_ms() + 60_000).unwrap());
        assert!(!b.try_acquire_credential_refresh_lease(row.id, "owner-b", now_ms() + 60_000).unwrap());
        let replacement = oauth("a@test", "org", "new");
        assert!(
            !b.try_update_auth_credential_if_matches(
                row.id,
                raw,
                &replacement,
                Some(&CredentialRefreshLeaseFence { owner: "owner-b".into(), now_ms: now_ms() })
            )
            .unwrap()
        );
        assert!(
            !a.try_update_auth_credential_if_matches(
                row.id,
                raw,
                &replacement,
                Some(&CredentialRefreshLeaseFence { owner: "owner-a".into(), now_ms: now_ms() + 120_000 })
            )
            .unwrap()
        );
        assert!(
            a.try_update_auth_credential_if_matches(
                row.id,
                raw,
                &replacement,
                Some(&CredentialRefreshLeaseFence { owner: "owner-a".into(), now_ms: now_ms() })
            )
            .unwrap()
        );
        assert!(!b.try_disable_auth_credential_if_matches(row.id, raw, "refresh failed", None).unwrap());
        a.release_credential_refresh_lease(row.id, "wrong-owner").unwrap();
        assert!(a.get_credential_refresh_lease_expires_at(row.id).unwrap().is_some());
        a.release_credential_refresh_lease(row.id, "owner-a").unwrap();
        assert!(b.try_acquire_credential_refresh_lease(row.id, "owner-b", now_ms() + 60_000).unwrap());
    }
    #[test]
    fn multi_connection_refresh_lease_has_one_winner() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("isolated.db");
        let s = SqliteCredentialStore::open(&path).unwrap();
        s.save_api_key("fixture", "key").unwrap();
        let id = s.list_auth_credentials(None).unwrap()[0].id;
        let ready = Arc::new(Barrier::new(4));
        let handles = (0..4)
            .map(|n| {
                let path = path.clone();
                let ready = ready.clone();
                std::thread::spawn(move || {
                    let s = SqliteCredentialStore::open(&path).unwrap();
                    ready.wait();
                    s.try_acquire_credential_refresh_lease(id, &format!("owner-{n}"), now_ms() + 60_000).unwrap()
                })
            })
            .collect::<Vec<_>>();
        assert_eq!(handles.into_iter().map(|h| usize::from(h.join().unwrap())).sum::<usize>(), 1);
    }
    #[test]
    fn revision_ignores_cache_usage_and_own_writes_but_detects_peer_auth() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("isolated.db");
        let a = SqliteCredentialStore::open(&path).unwrap();
        let b = SqliteCredentialStore::open(&path).unwrap();
        assert!(!a.poll_external_changes().unwrap());
        b.set_cache("usage-cache", "value", i64::MAX).unwrap();
        assert!(!a.poll_external_changes().unwrap());
        a.save_api_key("own", "a").unwrap();
        b.save_api_key("peer", "b").unwrap();
        assert!(a.poll_external_changes().unwrap());
        assert!(!a.poll_external_changes().unwrap());
        a.save_api_key("own", "updated").unwrap();
        assert!(!a.poll_external_changes().unwrap());
        b.upsert_credential_block(&StoredCredentialBlock {
            credential_id: 1,
            provider_key: "fixture".into(),
            block_scope: "all".into(),
            blocked_until_ms: now_ms() + 60_000,
            updated_at_ms: 0,
        })
        .unwrap();
        assert!(a.poll_external_changes().unwrap());
    }
    #[test]
    fn external_process_writer() {
        let own = tempfile::tempdir().unwrap();
        let path = std::env::var_os("ARA_CREDENTIAL_TEST_DB")
            .map(std::path::PathBuf::from)
            .unwrap_or_else(|| own.path().join("isolated.db"));
        let s = SqliteCredentialStore::open(&path).unwrap();
        s.save_api_key("child-fixture", "child-key").unwrap();
        assert!(s.get_api_key("child-fixture").unwrap().is_some());
    }
    #[test]
    fn revision_detects_other_process_native_commit() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("isolated.db");
        let s = SqliteCredentialStore::open(&path).unwrap();
        let output = std::process::Command::new(std::env::current_exe().unwrap())
            .args(["--exact", "credential_store::tests::external_process_writer", "--test-threads=1"])
            .env("ARA_CREDENTIAL_TEST_DB", &path)
            .output()
            .unwrap();
        assert!(output.status.success());
        assert!(s.poll_external_changes().unwrap());
        assert_eq!(s.list_providers().unwrap(), ["child-fixture"]);
    }
    #[test]
    fn schema_zero_and_unique_schema_three_migrate_without_losing_ids() {
        for version in [0, 1, 2, 3, 4, 5, 6] {
            let conn = Connection::open_in_memory().unwrap();
            if version == 0 {
                conn.execute_batch("CREATE TABLE auth_credentials(id INTEGER PRIMARY KEY,provider TEXT,credential_type TEXT,data TEXT,disabled INTEGER,created_at INTEGER,updated_at INTEGER); INSERT INTO auth_credentials VALUES(42,'fixture','api_key','{\"key\":\"legacy\"}',1,10,20)").unwrap();
            } else {
                let identity = if version >= 3 { ",identity_key TEXT" } else { "" };
                let extra = if version == 2 { ",account_id TEXT,email TEXT" } else { "" };
                conn.execute_batch(&format!("CREATE TABLE auth_schema_version(id INTEGER PRIMARY KEY,version INTEGER);INSERT INTO auth_schema_version VALUES(1,{version});CREATE TABLE auth_credentials(id INTEGER PRIMARY KEY,provider TEXT UNIQUE,credential_type TEXT,data TEXT,disabled_cause TEXT,created_at INTEGER,updated_at INTEGER{identity}{extra});INSERT INTO auth_credentials(id,provider,credential_type,data,created_at,updated_at) VALUES(42,'fixture','api_key','{{\"key\":\"legacy\"}}',10,20)")).unwrap();
            }
            let s = SqliteCredentialStore::from_connection(conn, Duration::from_millis(100)).unwrap();
            assert_eq!(
                s.db()
                    .unwrap()
                    .query_row("SELECT version FROM auth_schema_version WHERE id=1", [], |r| r.get::<_, i64>(0))
                    .unwrap(),
                7
            );
            assert_eq!(
                s.db().unwrap().query_row("SELECT id FROM auth_credentials", [], |r| r.get::<_, i64>(0)).unwrap(),
                42
            );
            if version == 0 {
                assert_eq!(s.list_disabled_credentials(None).unwrap()[0].cause, "disabled");
            }
            if version <= 3 {
                s.replace_auth_credentials_for_provider(
                    "fixture",
                    &[AuthCredential::api_key("one"), AuthCredential::api_key("two")],
                )
                .unwrap();
                assert_eq!(s.list_auth_credentials(Some("fixture")).unwrap().len(), 2);
            }
        }
    }
    #[test]
    fn schema_six_shared_meter_migration_and_legacy_triggers_are_native() {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(CREDENTIAL_SCHEMA).unwrap();
        conn.execute_batch(BLOCK_SCHEMA).unwrap();
        conn.execute_batch("CREATE TABLE auth_schema_version(id INTEGER PRIMARY KEY,version INTEGER);INSERT INTO auth_schema_version VALUES(1,6);INSERT INTO auth_credential_blocks VALUES(9,'openai-codex:oauth','shared',9999999999999,20);INSERT INTO auth_credential_blocks VALUES(9,'openai-codex:oauth','chat',9999999999998,30)").unwrap();
        let s = SqliteCredentialStore::from_connection(conn, Duration::from_millis(100)).unwrap();
        assert_eq!(s.list_credential_blocks(&[9, 9]).unwrap().len(), 2);
        assert!(s.get_credential_block(9, CODEX_PROVIDER, "shared").unwrap().is_none());
        s.db().unwrap().execute("UPDATE auth_credential_blocks SET blocked_until_ms=9999999999999+100 WHERE credential_id=9 AND block_scope='shared'",[]).unwrap();
        assert_eq!(s.get_credential_block(9, CODEX_PROVIDER, "spark").unwrap(), Some(10000000000099));
        s.delete_credential_block(9, CODEX_PROVIDER, "chat").unwrap();
        assert_eq!(s.list_credential_blocks(&[9]).unwrap().len(), 1);
        s.db()
            .unwrap()
            .execute("DELETE FROM auth_credential_blocks WHERE credential_id=9 AND block_scope='shared'", [])
            .unwrap();
        assert!(s.list_credential_blocks(&[9]).unwrap().is_empty());
    }
    #[test]
    fn cache_blocks_reconcile_usage_buckets_and_client_apps_preserve_contracts() {
        let s = store();
        s.set_cache("a%one", "old", 0).unwrap();
        s.set_cache("a%two", "new", i64::MAX).unwrap();
        s.set_cache("a-other", "keep", i64::MAX).unwrap();
        assert!(s.get_cache("a%one", false).unwrap().is_none());
        assert!(s.get_cache("a%one", true).unwrap().is_some());
        s.delete_cache_prefix("a%").unwrap();
        assert!(s.get_cache("a-other", false).unwrap().is_some());
        let until = now_ms() + 600_000;
        let block = StoredCredentialBlock {
            credential_id: 1,
            provider_key: "fixture".into(),
            block_scope: "model".into(),
            blocked_until_ms: until,
            updated_at_ms: 0,
        };
        s.upsert_credential_block(&block).unwrap();
        s.upsert_credential_block(&StoredCredentialBlock { blocked_until_ms: until - 100_000, ..block.clone() })
            .unwrap();
        assert_eq!(s.get_credential_block(1, "fixture", "model").unwrap(), Some(until));
        assert!(s.get_credential_block_reconcile_after(1, "fixture", "model").unwrap().is_some());
        let mut entry = UsageHistoryEntry {
            recorded_at: 3_600_001,
            provider: "fixture".into(),
            account_key: "a".into(),
            email: None,
            account_id: None,
            limit_id: "week".into(),
            label: "Weekly".into(),
            window_label: None,
            used_fraction: None,
            status: None,
            resets_at: None,
        };
        s.record_usage_snapshots(&[entry.clone()]).unwrap();
        entry.recorded_at += 1;
        entry.used_fraction = Some(0.5);
        s.record_usage_snapshots(&[entry.clone()]).unwrap();
        assert_eq!(s.list_usage_history(None).unwrap().len(), 1);
        entry.recorded_at += 3_600_000;
        s.record_usage_snapshots(&[entry]).unwrap();
        assert_eq!(s.list_usage_history(None).unwrap().len(), 2);
        let report = ClientUsageReport {
            install_id: "fixture-install".into(),
            hostname: Some("fixture-host".into()),
            app: Some(" app-a ".into()),
            entries: vec![ClientUsageEntry {
                at: now_ms(),
                provider: "fixture".into(),
                model: "fixture-model".into(),
                requests: 1,
                input_tokens: 2,
                output_tokens: 3,
                cache_read_tokens: 4,
                cache_write_tokens: 5,
                cost_usd: 0.1,
            }],
        };
        s.record_client_usage(&report).unwrap();
        s.record_client_usage(&report).unwrap();
        let mut other = report.clone();
        other.app = Some("app-b".into());
        s.record_client_usage(&other).unwrap();
        let summary = s.get_client_usage_summary(0).unwrap();
        assert_eq!(summary.clients[0].providers.len(), 2);
        assert_eq!(summary.clients[0].providers[0].requests, 2);
    }
    #[test]
    fn sql_failures_propagate_and_unknown_rows_are_not_keys() {
        let s = store();
        s.db()
            .unwrap()
            .execute("INSERT INTO auth_credentials(provider,credential_type,data) VALUES('bad','oauth','not-json')", [])
            .unwrap();
        assert!(s.list_auth_credentials(Some("bad")).unwrap().is_empty());
        s.db().unwrap().execute_batch("DROP TABLE cache").unwrap();
        assert!(s.get_cache("missing", false).is_err());
        assert!(s.set_cache("key", "value", 100).is_err());
    }
}
