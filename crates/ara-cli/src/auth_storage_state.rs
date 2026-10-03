//! Synchronous AuthStorage assignment and block state, owned by the host.
//!
//! Fixed OMP source: 596f2da7101178214aa27a753529d15e6b7ad91d,
//! packages/coding-agent/src/auth-storage.ts: 87–93, 1588–1679,
//! 1744–2087, 2300–2313, 4538–4568, 5936–5984, 6317–6329,
//! 6387–6410, 6497–6525, 6572–6587 and 7052–7109.
//! The host passes its current provider rows and one already-open store while
//! holding its database mutex. No callback, network, or asynchronous work runs
//! here. Usage-cache invalidation and generation notifications belong to the
//! host; the advisory helpers retain native best-effort persistence behavior.
//!
//! MIT License
//!
//! Copyright (c) 2025 Mario Zechner
//! Copyright (c) 2025-2026 Can Bölük
//! Copyright (c) 2026 Stencil Labs, Inc.
//!
//! Permission is hereby granted, free of charge, to any person obtaining a copy
//! of this software and associated documentation files (the "Software"), to deal
//! in the Software without restriction, including without limitation the rights
//! to use, copy, modify, merge, publish, distribute, sublicense, and/or sell
//! copies of the Software, and to permit persons to whom the Software is
//! furnished to do so, subject to the following conditions:
//!
//! The above copyright notice and this permission notice shall be included in all
//! copies or substantial portions of the Software.
//!
//! THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF ANY KIND, EXPRESS OR
//! IMPLIED, INCLUDING BUT NOT LIMITED TO THE WARRANTIES OF MERCHANTABILITY,
//! FITNESS FOR A PARTICULAR PURPOSE AND NONINFRINGEMENT. IN NO EVENT SHALL THE
//! AUTHORS OR COPYRIGHT HOLDERS BE LIABLE FOR ANY CLAIM, DAMAGES OR OTHER
//! LIABILITY, WHETHER IN AN ACTION OF CONTRACT, TORT OR OTHERWISE, ARISING FROM,
//! OUT OF OR IN CONNECTION WITH THE SOFTWARE OR THE USE OR OTHER DEALINGS IN THE
//! SOFTWARE.

#[cfg(test)]
use crate::credential_store::SqliteCredentialStore;
use crate::credential_store::{
    AuthCredential, StoredAuthCredential, StoredCredentialBlock, USAGE_REPORT_TTL_MS, is_sqlite_corruption_error,
};
use crate::credential_store_port::AuthCredentialStore;
use anyhow::{Result, anyhow};
use ara_rpc::WireString;
use base64::{Engine as _, engine::general_purpose::URL_SAFE_NO_PAD};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::{BTreeMap, BTreeSet};

const SESSION_STICKY_CACHE_PREFIX: &str = "session:sticky:";
const STICKY_TTL_SEC: i64 = 30 * 24 * 60 * 60;
const BEARER_HISTORY_LIMIT: usize = 8;

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub enum CredentialKind {
    #[serde(rename = "api_key")]
    ApiKey,
    #[serde(rename = "oauth")]
    OAuth,
}

impl CredentialKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::ApiKey => "api_key",
            Self::OAuth => "oauth",
        }
    }

    pub fn of(credential: &AuthCredential) -> Self {
        match credential {
            AuthCredential::ApiKey { .. } => Self::ApiKey,
            AuthCredential::OAuth { .. } => Self::OAuth,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct SessionCredential {
    pub kind: CredentialKind,
    /// Index in the full provider row list, including API keys.
    pub index: usize,
    pub last_used_at_ms: Option<f64>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct CredentialTarget {
    pub kind: CredentialKind,
    pub index: usize,
    pub explicit: bool,
}

/// Read-only account identity; never includes access or refresh credentials.
#[derive(Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct OAuthAccountSummary {
    pub position: usize,
    pub credential_id: i64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub account_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub email: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub project_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub enterprise_url: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub org_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub org_name: Option<String>,
    pub active: bool,
}

type IndexedTimes = BTreeMap<String, BTreeMap<usize, f64>>;

/// No Debug/Serialize: even token fingerprints stay inside the identity cache.
#[derive(Default)]
pub struct AuthStorageState {
    round_robin: BTreeMap<String, usize>,
    sessions: BTreeMap<String, BTreeMap<String, SessionCredential>>,
    backoff: IndexedTimes,
    probe_after: IndexedTimes,
    // Vec preserves the JS Map's row insertion order for first-match lookup.
    bearer_histories: BTreeMap<String, Vec<(i64, Vec<String>)>>,
    persisted_block_store_damaged: bool,
    block_store_damage_notice_pending: bool,
}

pub fn provider_type_key(provider: &str, kind: CredentialKind) -> String {
    format!("{provider}:{}", kind.as_str())
}

fn sticky_key(provider: &str, session_id: &str) -> String {
    format!("{SESSION_STICKY_CACHE_PREFIX}{provider}:{session_id}")
}

fn scoped_key(provider_key: &str, scope: Option<&str>) -> String {
    match scope.filter(|scope| !scope.is_empty()) {
        Some(scope) => format!("{provider_key}\0{scope}"),
        None => provider_key.to_owned(),
    }
}

fn remove_time(maps: &mut IndexedTimes, key: &str, index: usize) {
    if let Some(map) = maps.get_mut(key) {
        map.remove(&index);
        if map.is_empty() {
            maps.remove(key);
        }
    }
}

// Rust f64::{min,max} discard one NaN; JavaScript Math.{min,max} propagates it.
fn js_max(left: f64, right: f64) -> f64 {
    if left.is_nan() || right.is_nan() { f64::NAN } else { left.max(right) }
}

fn js_min(left: f64, right: f64) -> f64 {
    if left.is_nan() || right.is_nan() { f64::NAN } else { left.min(right) }
}

fn integer_ms(value: f64) -> Option<i64> {
    (value.is_finite() && value.fract() == 0.0 && value >= i64::MIN as f64 && value < -(i64::MIN as f64))
        .then_some(value as i64)
}

fn merge_block(current: &mut Option<f64>, candidate: Option<f64>) {
    if let Some(candidate) = candidate
        && current.is_none_or(|current| candidate > current)
    {
        *current = Some(candidate);
    }
}

fn bearer_fingerprint(bearer: &str) -> String {
    URL_SAFE_NO_PAD.encode(ring::digest::digest(&ring::digest::SHA256, bearer.as_bytes()).as_ref())
}

fn oauth_field(credential: &AuthCredential, name: &str) -> Option<String> {
    match credential {
        AuthCredential::OAuth { fields } => fields.get(name)?.as_str().map(str::to_owned),
        AuthCredential::ApiKey { .. } => None,
    }
}

/// Fixed OMP equality deliberately ignores API-key source and other OAuth
/// fields such as orgId/orgName. Hosts use this for native generation behavior.
pub fn stored_credential_arrays_equal(left: &[StoredAuthCredential], right: &[StoredAuthCredential]) -> bool {
    left.len() == right.len()
        && left.iter().zip(right).all(|(left, right)| {
            left.id == right.id
                && match (&left.credential, &right.credential) {
                    (AuthCredential::ApiKey { key: left, .. }, AuthCredential::ApiKey { key: right, .. }) => {
                        left == right
                    }
                    (AuthCredential::OAuth { fields: left }, AuthCredential::OAuth { fields: right }) => {
                        ["access", "refresh", "expires", "accountId", "email", "projectId", "enterpriseUrl"]
                            .iter()
                            .all(|field| left.get(*field) == right.get(*field))
                    }
                    _ => false,
                }
        })
}

impl AuthStorageState {
    pub fn new() -> Self {
        Self::default()
    }

    /// total=0 also returns [0], matching the fixed native helper's early return.
    pub fn credential_order(&mut self, provider_key: &str, session_id: Option<&str>, total: usize) -> Vec<usize> {
        self.credential_order_wire(provider_key, session_id.map(WireString::from).as_ref(), total)
    }

    pub fn credential_order_wire(
        &mut self,
        provider_key: &str,
        session_id: Option<&WireString>,
        total: usize,
    ) -> Vec<usize> {
        if total <= 1 {
            return vec![0];
        }
        let start = if let Some(session) = session_id.filter(|session| !session.units().is_empty()) {
            let utf8 = String::from_utf16_lossy(session.units());
            xxhash_rust::xxh32::xxh32(utf8.as_bytes(), 0) as usize % total
        } else {
            let next = self.round_robin.get(provider_key).map_or(0, |current| (current + 1) % total);
            self.round_robin.insert(provider_key.to_owned(), next);
            next
        };
        (0..total).map(|offset| (start + offset) % total).collect()
    }

    // Fixed OMP #recordSessionCredential keeps provider/session/type/index
    // independent; the Rust host additionally supplies store, rows and clocks.
    #[allow(clippy::too_many_arguments)]
    pub fn record_session_credential(
        &mut self,
        store: &dyn AuthCredentialStore,
        provider: &str,
        session_id: Option<&str>,
        kind: CredentialKind,
        index: usize,
        rows: &[StoredAuthCredential],
        last_used_at_ms: Option<f64>,
        now_ms: f64,
    ) {
        let Some(session) = session_id.filter(|session| !session.is_empty()) else { return };
        let timestamp = last_used_at_ms.unwrap_or(now_ms);
        self.sessions
            .entry(provider.to_owned())
            .or_default()
            .insert(session.to_owned(), SessionCredential { kind, index, last_used_at_ms: Some(timestamp) });
        // The native write is advisory and follows the memory assignment.
        if let Some(row) = rows.get(index) {
            let value = serde_json::json!({ "type": kind, "index": index,
                "credentialId": row.id, "lastUsedAtMs": timestamp });
            if let Some(expires) =
                integer_ms((timestamp / 1000.0).floor()).and_then(|seconds| seconds.checked_add(STICKY_TTL_SEC))
            {
                let _ = store.set_cache(&sticky_key(provider, session), &value.to_string(), expires);
            }
        }
    }

    pub fn read_session_credential(
        &mut self,
        store: &dyn AuthCredentialStore,
        provider: &str,
        session_id: Option<&str>,
        rows: &[StoredAuthCredential],
    ) -> Option<SessionCredential> {
        let session = session_id.filter(|session| !session.is_empty())?;
        if let Some(cached) = self.sessions.get(provider).and_then(|sessions| sessions.get(session)) {
            // Do not revalidate the in-process cache: reset_provider_assignments
            // is the native mutation boundary for index-bearing assignments.
            return Some(*cached);
        }
        let key = sticky_key(provider, session);
        let raw = store.get_cache(&key, false).ok()??;
        if raw.is_empty() {
            return None;
        }
        let value: Value = serde_json::from_str(&raw).ok()?;
        let id = value.get("credentialId").and_then(|id| id.as_i64().or_else(|| id.as_f64().and_then(integer_ms)));
        let kind = value.get("type").cloned().and_then(|kind| serde_json::from_value::<CredentialKind>(kind).ok());
        let target = id.and_then(|id| rows.iter().position(|row| row.id == id)).and_then(|index| {
            kind.filter(|kind| *kind == CredentialKind::of(&rows[index].credential)).map(|kind| (index, kind))
        });
        let Some((index, kind)) = target else {
            // Reject legacy index-only rows and durable rows removed/type-changed.
            let _ = store.set_cache(&key, "", 0);
            return None;
        };
        let session_value =
            SessionCredential { kind, index, last_used_at_ms: value.get("lastUsedAtMs").and_then(Value::as_f64) };
        self.sessions.entry(provider.to_owned()).or_default().insert(session.to_owned(), session_value);
        Some(session_value)
    }

    /// Native getApiKey cascade forgets the in-memory assignment while keeping
    /// its durable sticky cache available to a later OAuth selection.
    pub fn forget_session_credential(&mut self, provider: &str, session_id: Option<&str>) {
        let Some(session) = session_id.filter(|session| !session.is_empty()) else { return };
        if let Some(sessions) = self.sessions.get_mut(provider) {
            sessions.remove(session);
            if sessions.is_empty() {
                self.sessions.remove(provider);
            }
        }
    }

    pub fn clear_session_credential(
        &mut self,
        store: &dyn AuthCredentialStore,
        provider: &str,
        session_id: Option<&str>,
    ) {
        let Some(session) = session_id.filter(|session| !session.is_empty()) else { return };
        self.forget_session_credential(provider, Some(session));
        let _ = store.set_cache(&sticky_key(provider, session), "", 0);
    }

    pub fn list_oauth_accounts(
        &mut self,
        store: &dyn AuthCredentialStore,
        provider: &str,
        session_id: Option<&str>,
        rows: &[StoredAuthCredential],
        oauth_suppressed: bool,
    ) -> Vec<OAuthAccountSummary> {
        if oauth_suppressed {
            return Vec::new();
        }
        let active_id = self
            .read_session_credential(store, provider, session_id, rows)
            .filter(|session| session.kind == CredentialKind::OAuth)
            .and_then(|session| rows.get(session.index))
            .map(|row| row.id);
        rows.iter()
            .filter(|row| matches!(row.credential, AuthCredential::OAuth { .. }))
            .enumerate()
            .map(|(position, row)| OAuthAccountSummary {
                position,
                credential_id: row.id,
                active: active_id == Some(row.id),
                account_id: oauth_field(&row.credential, "accountId"),
                email: oauth_field(&row.credential, "email"),
                project_id: oauth_field(&row.credential, "projectId"),
                enterprise_url: oauth_field(&row.credential, "enterpriseUrl"),
                org_id: oauth_field(&row.credential, "orgId"),
                org_name: oauth_field(&row.credential, "orgName"),
            })
            .collect()
    }

    // Fixed OMP pinSessionOAuthAccount separates account ID, suppression and
    // last use; store, snapshot and current time remain explicit host inputs.
    #[allow(clippy::too_many_arguments)]
    pub fn pin_session_oauth_account(
        &mut self,
        store: &dyn AuthCredentialStore,
        provider: &str,
        session_id: &str,
        credential_id: i64,
        rows: &[StoredAuthCredential],
        oauth_suppressed: bool,
        last_used_at_ms: Option<f64>,
        now_ms: f64,
    ) -> bool {
        if session_id.is_empty() || oauth_suppressed {
            return false;
        }
        let Some(index) = rows.iter().position(|row| row.id == credential_id) else { return false };
        if !matches!(rows[index].credential, AuthCredential::OAuth { .. }) {
            return false;
        }
        self.record_session_credential(
            store,
            provider,
            Some(session_id),
            CredentialKind::OAuth,
            index,
            rows,
            last_used_at_ms,
            now_ms,
        );
        true
    }

    /// Explicit target attempts never fall through to a sticky credential.
    /// The host refreshes rows and asynchronously resolves stored config keys
    /// before calling this synchronous seam. Missing resolutions do not match.
    // Fixed OMP #resolveCredentialTarget separates explicit ID/bearer from
    // session fallback; store, rows and resolved keys carry distinct evidence.
    #[allow(clippy::too_many_arguments)]
    pub fn resolve_credential_target(
        &mut self,
        store: &dyn AuthCredentialStore,
        provider: &str,
        session_id: Option<&str>,
        rows: &[StoredAuthCredential],
        credential_id: Option<i64>,
        api_key: Option<&str>,
        resolved_api_keys: &[(i64, String)],
    ) -> Option<CredentialTarget> {
        let explicit = credential_id.is_some() || api_key.is_some();
        let target_index = credential_id.and_then(|id| rows.iter().position(|row| row.id == id)).or_else(|| {
            api_key.and_then(|key| rows.iter().position(|row| credential_matches_api_key(row, key, resolved_api_keys)))
        });
        if let Some(index) = target_index {
            return Some(CredentialTarget { kind: CredentialKind::of(&rows[index].credential), index, explicit: true });
        }
        if explicit {
            return None;
        }
        self.read_session_credential(store, provider, session_id, rows).map(|session| CredentialTarget {
            kind: session.kind,
            index: session.index,
            explicit: false,
        })
    }

    pub fn record_oauth_bearer_credential_id(&mut self, provider: &str, bearer: &str, credential_id: Option<i64>) {
        let Some(id) = credential_id else { return };
        let fingerprint = bearer_fingerprint(bearer);
        let rows = self.bearer_histories.entry(provider.to_owned()).or_default();
        let index = rows.iter().position(|(tracked_id, _)| *tracked_id == id).unwrap_or_else(|| {
            rows.push((id, Vec::new()));
            rows.len() - 1
        });
        let history = &mut rows[index].1;
        history.retain(|old| old != &fingerprint);
        history.push(fingerprint);
        if history.len() > BEARER_HISTORY_LIMIT {
            history.remove(0);
        }
    }

    pub fn find_oauth_credential_id_for_bearer(&self, provider: &str, bearer: &str) -> Option<i64> {
        let fingerprint = bearer_fingerprint(bearer);
        self.bearer_histories
            .get(provider)?
            .iter()
            .find(|(_, history)| history.contains(&fingerprint))
            .map(|(id, _)| *id)
    }

    /// Used solely for delayed quota errors, never for invalid-auth handling.
    pub fn find_bearer_quota_target(
        &self,
        provider: &str,
        bearer: &str,
        rows: &[StoredAuthCredential],
    ) -> Option<CredentialTarget> {
        let id = self.find_oauth_credential_id_for_bearer(provider, bearer)?;
        let index =
            rows.iter().position(|row| row.id == id && matches!(row.credential, AuthCredential::OAuth { .. }))?;
        Some(CredentialTarget { kind: CredentialKind::OAuth, index, explicit: true })
    }

    pub fn prune_provider_bearer_history(&mut self, provider: &str, rows: &[StoredAuthCredential]) {
        let active: BTreeSet<_> = rows
            .iter()
            .filter(|row| matches!(row.credential, AuthCredential::OAuth { .. }))
            .map(|row| row.id)
            .collect();
        if let Some(histories) = self.bearer_histories.get_mut(provider) {
            histories.retain(|(id, _)| active.contains(id));
            if histories.is_empty() {
                self.bearer_histories.remove(provider);
            }
        }
    }

    /// Reload does not reset assignments/backoff or clear the corruption latch.
    /// Hosts call this after native snapshot equality/deduplication processing.
    pub fn reload_state(&mut self, all_rows: &[StoredAuthCredential]) {
        let providers: Vec<_> = self.bearer_histories.keys().cloned().collect();
        for provider in providers {
            let active: BTreeSet<_> = all_rows
                .iter()
                .filter(|row| row.provider == provider && matches!(row.credential, AuthCredential::OAuth { .. }))
                .map(|row| row.id)
                .collect();
            if let Some(histories) = self.bearer_histories.get_mut(&provider) {
                histories.retain(|(id, _)| active.contains(id));
                if histories.is_empty() {
                    self.bearer_histories.remove(&provider);
                }
            }
        }
    }

    pub fn reset_provider_assignments(&mut self, store: &dyn AuthCredentialStore, provider: &str) {
        let prefix = format!("{provider}:");
        self.round_robin.retain(|key, _| !key.starts_with(&prefix));
        self.sessions.remove(provider);
        let _ = store.delete_cache_prefix(&format!("{SESSION_STICKY_CACHE_PREFIX}{provider}:"));
        self.backoff.retain(|key, _| !key.starts_with(&prefix));
        // The fixed native reset intentionally does not clear probe_after.
    }

    pub fn persisted_block_store_damaged(&self) -> bool {
        self.persisted_block_store_damaged
    }

    /// Returns true only for the fixed SQLite corruption classifier. Hosts may
    /// call this for a failed reload too; a closed store or BUSY is not damage.
    pub fn observe_store_error(&mut self, error: &anyhow::Error) -> bool {
        if !is_sqlite_corruption_error(error) {
            return false;
        }
        if !self.persisted_block_store_damaged {
            self.persisted_block_store_damaged = true;
            self.block_store_damage_notice_pending = true;
        }
        true
    }

    /// One receipt for the host's operator-facing corruption log.
    pub fn take_block_store_damage_notice(&mut self) -> bool {
        std::mem::take(&mut self.block_store_damage_notice_pending)
    }

    fn assert_block_store_writable(&self) -> Result<()> {
        if self.persisted_block_store_damaged {
            return Err(anyhow!("Persistent credential block store is unavailable after SQLite corruption"));
        }
        Ok(())
    }

    fn memory_blocked_until(&mut self, key: &str, index: usize, now_ms: f64) -> Option<f64> {
        let until = self.backoff.get(key)?.get(&index).copied()?;
        if until == 0.0 || until.is_nan() {
            return None;
        }
        if until <= now_ms {
            remove_time(&mut self.backoff, key, index);
            remove_time(&mut self.probe_after, key, index);
            return None;
        }
        Some(until)
    }

    fn read_persisted_block(
        &mut self,
        store: &dyn AuthCredentialStore,
        id: i64,
        key: &str,
        scope: &str,
    ) -> Option<f64> {
        if self.persisted_block_store_damaged {
            return None;
        }
        match store.get_credential_block(id, key, scope) {
            Ok(until) => until.map(|until| until as f64),
            Err(error) => {
                self.observe_store_error(&error);
                None
            }
        }
    }

    fn read_reconcile_after(&mut self, store: &dyn AuthCredentialStore, id: i64, key: &str, scope: &str) -> f64 {
        if self.persisted_block_store_damaged {
            return 0.0;
        }
        match store.get_credential_block_reconcile_after(id, key, scope) {
            Ok(after) => after.unwrap_or(0) as f64,
            Err(error) => {
                self.observe_store_error(&error);
                0.0
            }
        }
    }

    pub fn blocked_until(
        &mut self,
        store: &dyn AuthCredentialStore,
        provider_key: &str,
        index: usize,
        scopes: &[&str],
        rows: &[StoredAuthCredential],
        now_ms: f64,
    ) -> Option<f64> {
        let mut result = self.memory_blocked_until(provider_key, index, now_ms);
        for scope in scopes.iter().copied().filter(|scope| !scope.is_empty()) {
            merge_block(&mut result, self.memory_blocked_until(&scoped_key(provider_key, Some(scope)), index, now_ms));
        }
        let Some(row) = rows.get(index) else { return result };
        merge_block(&mut result, self.read_persisted_block(store, row.id, provider_key, ""));
        for scope in scopes.iter().copied().filter(|scope| !scope.is_empty()) {
            merge_block(&mut result, self.read_persisted_block(store, row.id, provider_key, scope));
        }
        result
    }

    /// Memory remains authoritative for this process if advisory persistence
    /// fails. The host invalidates provider usage-cache after every invocation.
    // Fixed OMP #markCredentialBlocked separates key/index/scope/expiry;
    // store, row snapshot and current time make host persistence explicit.
    #[allow(clippy::too_many_arguments)]
    pub fn mark_credential_blocked(
        &mut self,
        store: &dyn AuthCredentialStore,
        provider_key: &str,
        index: usize,
        until_ms: f64,
        scope: Option<&str>,
        rows: &[StoredAuthCredential],
        now_ms: f64,
    ) {
        let key = scoped_key(provider_key, scope);
        let map = self.backoff.entry(key.clone()).or_default();
        let next = js_max(map.get(&index).copied().unwrap_or(0.0), until_ms);
        map.insert(index, next);
        self.probe_after.entry(key).or_default().insert(index, js_min(next, now_ms + USAGE_REPORT_TTL_MS as f64));
        if self.persisted_block_store_damaged {
            return;
        }
        if let (Some(row), Some(until)) = (rows.get(index), integer_ms(next)) {
            let block = StoredCredentialBlock {
                credential_id: row.id,
                provider_key: provider_key.to_owned(),
                block_scope: scope.unwrap_or("").to_owned(),
                blocked_until_ms: until,
                updated_at_ms: integer_ms(now_ms).unwrap_or(0),
            };
            if let Err(error) = store.upsert_credential_block(&block) {
                self.observe_store_error(&error);
            }
        }
    }

    pub fn list_credential_blocks(
        &mut self,
        store: &dyn AuthCredentialStore,
        ids: &[i64],
    ) -> Result<Vec<StoredCredentialBlock>> {
        if self.persisted_block_store_damaged {
            return Ok(Vec::new());
        }
        match store.list_credential_blocks(ids) {
            Ok(blocks) => Ok(blocks),
            Err(error) if self.observe_store_error(&error) => Ok(Vec::new()),
            Err(error) => Err(error),
        }
    }

    pub fn upsert_credential_block(
        &mut self,
        store: &dyn AuthCredentialStore,
        block: &StoredCredentialBlock,
    ) -> Result<()> {
        self.assert_block_store_writable()?;
        if let Err(error) = store.upsert_credential_block(block) {
            if self.observe_store_error(&error) {
                self.assert_block_store_writable()?;
            }
            return Err(error);
        }
        Ok(())
    }

    pub fn delete_credential_block(
        &mut self,
        store: &dyn AuthCredentialStore,
        id: i64,
        key: &str,
        scope: &str,
    ) -> Result<()> {
        self.assert_block_store_writable()?;
        if let Err(error) = store.delete_credential_block(id, key, scope) {
            if self.observe_store_error(&error) {
                self.assert_block_store_writable()?;
            }
            return Err(error);
        }
        Ok(())
    }

    pub fn delete_credential_blocks(&mut self, store: &dyn AuthCredentialStore, id: i64) -> Result<()> {
        self.assert_block_store_writable()?;
        if let Err(error) = store.delete_credential_blocks(id) {
            if self.observe_store_error(&error) {
                self.assert_block_store_writable()?;
            }
            return Err(error);
        }
        Ok(())
    }

    pub fn clear_credential_block_scope(
        &mut self,
        store: &dyn AuthCredentialStore,
        id: i64,
        index: usize,
        provider_key: &str,
        scope: Option<&str>,
    ) {
        let key = scoped_key(provider_key, scope);
        remove_time(&mut self.backoff, &key, index);
        remove_time(&mut self.probe_after, &key, index);
        let _ = self.delete_credential_block(store, id, provider_key, scope.unwrap_or(""));
    }

    /// Used after OAuth refresh; clears every scope for only this row/index.
    pub fn clear_credential_blocks(
        &mut self,
        store: &dyn AuthCredentialStore,
        provider: &str,
        id: i64,
        rows: &[StoredAuthCredential],
    ) {
        let _ = self.delete_credential_blocks(store, id);
        let Some(index) = rows.iter().position(|row| row.id == id) else { return };
        let key = provider_type_key(provider, CredentialKind::OAuth);
        let prefix = format!("{key}\0");
        for maps in [&mut self.backoff, &mut self.probe_after] {
            let keys: Vec<_> =
                maps.keys().filter(|candidate| *candidate == &key || candidate.starts_with(&prefix)).cloned().collect();
            for key in keys {
                remove_time(maps, &key, index);
            }
        }
    }

    /// Health assessment belongs to the usage strategy. Call once for each
    /// healthy scope; this gate also honors global and persisted probe windows.
    /// The returned timestamp is the native log's clearedBlockedUntilMs receipt.
    // Fixed OMP #clearHealedBlockScope needs durable ID and memory index plus
    // scope; store, row snapshot and time preserve both independent probe gates.
    #[allow(clippy::too_many_arguments)]
    pub fn clear_healed_block_scope(
        &mut self,
        store: &dyn AuthCredentialStore,
        provider_key: &str,
        id: i64,
        index: usize,
        scope: Option<&str>,
        rows: &[StoredAuthCredential],
        now_ms: f64,
    ) -> Option<f64> {
        let scopes: Vec<_> = scope.into_iter().collect();
        let blocked = self.blocked_until(store, provider_key, index, &scopes, rows, now_ms)?;
        let key = scoped_key(provider_key, scope);
        let global = self.probe_after.get(provider_key).and_then(|map| map.get(&index)).copied().unwrap_or(0.0);
        let scoped = self.probe_after.get(&key).and_then(|map| map.get(&index)).copied().unwrap_or(0.0);
        let persisted_global = self.read_reconcile_after(store, id, provider_key, "");
        let persisted_scoped = self.read_reconcile_after(store, id, provider_key, scope.unwrap_or(""));
        if js_max(js_max(global, scoped), js_max(persisted_global, persisted_scoped)) > now_ms {
            return None;
        }
        self.clear_credential_block_scope(store, id, index, provider_key, scope);
        Some(blocked)
    }
}

fn credential_matches_api_key(row: &StoredAuthCredential, key: &str, resolved_api_keys: &[(i64, String)]) -> bool {
    match &row.credential {
        AuthCredential::ApiKey { .. } => {
            resolved_api_keys.iter().any(|(id, resolved)| *id == row.id && resolved == key)
        }
        AuthCredential::OAuth { fields } => {
            let Some(access) = fields.get("access").and_then(Value::as_str) else { return false };
            if access == key {
                return true;
            }
            if !key.starts_with('{') {
                return false;
            }
            serde_json::from_str::<Value>(key)
                .ok()
                .and_then(|value| value.get("token").and_then(Value::as_str).map(str::to_owned))
                .is_some_and(|token| token == access)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    fn oauth(email: &str, access: &str) -> AuthCredential {
        AuthCredential::oauth(
            serde_json::json!({"access":access,"refresh":"fixture-refresh",
            "expires":9_999_999_999_999_i64,"email":email,"accountId":email,"orgId":"fixture-org"})
            .as_object()
            .unwrap()
            .clone(),
        )
    }

    /// One corpus covering native assignment, durable identity, bearer alias,
    /// scope isolation and fault classification boundaries. Real model/OAuth
    /// requests and a Bun-produced oracle are separate host acceptance gates.
    #[test]
    fn fixed_auth_storage_state_case_corpus() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("auth.db");
        let store = SqliteCredentialStore::open_with_busy_timeout(&path, Duration::from_millis(10)).unwrap();
        store.upsert_auth_credential_for_provider("fixture", &oauth("one@fixture", "one-current")).unwrap();
        store.upsert_auth_credential_for_provider("fixture", &oauth("two@fixture", "two-current")).unwrap();
        let rows = store.list_auth_credentials(Some("fixture")).unwrap();
        assert_eq!(rows.len(), 2);
        let now = chrono::Utc::now().timestamp_millis() as f64;
        let key = provider_type_key("fixture", CredentialKind::OAuth);
        let mut state = AuthStorageState::new();

        // Seed-zero XXH32 published vectors exercise the actual algorithm;
        // the native helper's source comment incorrectly calls this FNV-1a.
        for (session, expected_hash) in [("abc", 852_579_327_u32), ("hello", 4_211_111_929)] {
            assert_eq!(xxhash_rust::xxh32::xxh32(session.as_bytes(), 0), expected_hash);
            let start = expected_hash as usize % 7;
            assert_eq!(
                state.credential_order(&key, Some(session), 7),
                (0..7).map(|i| (start + i) % 7).collect::<Vec<_>>()
            );
        }
        assert_eq!(state.credential_order(&key, None, 0), [0]);
        assert_eq!(state.credential_order(&key, None, 1), [0]);
        assert_eq!(state.credential_order(&key, None, 3), [0, 1, 2]);
        assert_eq!(state.credential_order(&key, Some(""), 3), [1, 2, 0]);
        assert_eq!(state.credential_order(&key, None, 3), [2, 0, 1]);
        assert_eq!(state.credential_order(&key, None, 3), [0, 1, 2]);
        assert_eq!(state.credential_order("other:oauth", None, 3), [0, 1, 2]);
        assert_eq!(
            state.credential_order_wire(&key, Some(&WireString::from_units(vec![0xd800])), 11),
            state.credential_order(&key, Some("\u{fffd}"), 11)
        );

        // Pins persist the durable ID and restore to its new full-list index.
        assert!(state.pin_session_oauth_account(
            &store,
            "fixture",
            "resume",
            rows[1].id,
            &rows,
            false,
            Some(now - 60_000.0),
            now
        ));
        let cached = store.get_cache(&sticky_key("fixture", "resume"), false).unwrap().unwrap();
        let persisted: Value = serde_json::from_str(&cached).unwrap();
        assert_eq!(persisted["credentialId"].as_i64(), Some(rows[1].id));
        assert_eq!(persisted["lastUsedAtMs"].as_f64(), Some(now - 60_000.0));
        let reversed = vec![rows[1].clone(), rows[0].clone()];
        let mut restarted = AuthStorageState::new();
        let restored = restarted.read_session_credential(&store, "fixture", Some("resume"), &reversed).unwrap();
        assert_eq!(restored.index, 0);
        assert_eq!(restored.kind, CredentialKind::OAuth);
        // An already-cached native entry is not revalidated until reset.
        assert_eq!(restarted.read_session_credential(&store, "fixture", Some("resume"), &rows).unwrap().index, 0);
        let accounts = restarted.list_oauth_accounts(&store, "fixture", Some("resume"), &reversed, false);
        assert_eq!(accounts.len(), 2);
        assert_eq!(accounts[0].position, 0);
        assert!(accounts[0].active);
        assert!(!accounts[1].active);
        assert!(restarted.list_oauth_accounts(&store, "fixture", Some("resume"), &rows, true).is_empty());
        assert!(!restarted.pin_session_oauth_account(&store, "fixture", "", rows[0].id, &rows, false, None, now));
        assert!(!restarted.pin_session_oauth_account(&store, "fixture", "resume", rows[0].id, &rows, true, None, now));

        for (session, value) in [
            ("legacy", serde_json::json!({"type":"oauth","index":0})),
            ("missing", serde_json::json!({"type":"oauth","index":0,"credentialId":-9})),
            ("type-change", serde_json::json!({"type":"api_key","index":0,"credentialId":rows[0].id})),
        ] {
            let cache_key = sticky_key("fixture", session);
            store.set_cache(&cache_key, &value.to_string(), i64::MAX).unwrap();
            assert!(restarted.read_session_credential(&store, "fixture", Some(session), &rows).is_none());
            assert_eq!(store.get_cache(&cache_key, true).unwrap().as_deref(), Some(""));
        }
        restarted.clear_session_credential(&store, "fixture", Some("resume"));
        assert!(restarted.read_session_credential(&store, "fixture", Some("resume"), &rows).is_none());

        // Row ID wins, but an absent explicit row may still match the supplied
        // current bearer. No explicit mismatch can adopt the session's account.
        state.record_session_credential(
            &store,
            "fixture",
            Some("selected"),
            CredentialKind::OAuth,
            0,
            &rows,
            None,
            now,
        );
        assert_eq!(
            state
                .resolve_credential_target(
                    &store,
                    "fixture",
                    Some("selected"),
                    &rows,
                    Some(rows[0].id),
                    Some("two-current"),
                    &[]
                )
                .unwrap()
                .index,
            0
        );
        assert_eq!(
            state
                .resolve_credential_target(
                    &store,
                    "fixture",
                    Some("selected"),
                    &rows,
                    Some(-9),
                    Some("two-current"),
                    &[]
                )
                .unwrap()
                .index,
            1
        );
        for candidate in ["two-current", "{\"token\":\"two-current\"}"] {
            assert_eq!(
                state
                    .resolve_credential_target(&store, "fixture", Some("selected"), &rows, None, Some(candidate), &[])
                    .unwrap()
                    .index,
                1
            );
        }
        for candidate in ["removed-bearer", " {\"token\":\"two-current\"}", "{\"token\":4}"] {
            assert!(
                state
                    .resolve_credential_target(&store, "fixture", Some("selected"), &rows, None, Some(candidate), &[])
                    .is_none()
            );
        }
        assert!(
            state.resolve_credential_target(&store, "fixture", Some("selected"), &rows, Some(-9), None, &[]).is_none()
        );
        assert!(
            !state
                .resolve_credential_target(&store, "fixture", Some("selected"), &rows, None, None, &[])
                .unwrap()
                .explicit
        );

        assert_eq!(bearer_fingerprint("abc"), "ungWv48Bz-pBQUDeXa4iI7ADYaOWF3qctBD_YfIAFa0");
        state.record_oauth_bearer_credential_id("fixture", "removed-bearer", Some(rows[1].id));
        assert_eq!(state.find_bearer_quota_target("fixture", "removed-bearer", &rows).unwrap().index, 1);
        // More than eight rotations eject the oldest alias. Repeating a token
        // moves it to the newest slot without growing the bounded history.
        for index in 0..9 {
            state.record_oauth_bearer_credential_id("fixture", &format!("rotated-{index}"), Some(rows[1].id));
        }
        assert!(state.find_oauth_credential_id_for_bearer("fixture", "rotated-0").is_none());
        state.record_oauth_bearer_credential_id("fixture", "rotated-1", Some(rows[1].id));
        state.record_oauth_bearer_credential_id("fixture", "rotated-9", Some(rows[1].id));
        assert!(state.find_oauth_credential_id_for_bearer("fixture", "rotated-1").is_some());
        assert!(state.find_oauth_credential_id_for_bearer("fixture", "rotated-2").is_none());
        state.record_oauth_bearer_credential_id("fixture", "shared-bearer", Some(rows[1].id));
        state.record_oauth_bearer_credential_id("fixture", "shared-bearer", Some(rows[0].id));
        assert_eq!(state.find_oauth_credential_id_for_bearer("fixture", "shared-bearer"), Some(rows[1].id));
        state.prune_provider_bearer_history("fixture", &rows[..1]);
        assert_eq!(state.find_oauth_credential_id_for_bearer("fixture", "shared-bearer"), Some(rows[0].id));
        state.reload_state(&[]);
        assert!(state.find_oauth_credential_id_for_bearer("fixture", "shared-bearer").is_none());

        // Independent scopes + the legacy catch-all combine by longest expiry.
        state.mark_credential_blocked(&store, &key, 0, now + 3_600_000.0, None, &rows, now);
        state.mark_credential_blocked(&store, &key, 0, now + 4_000_000.0, Some("chat"), &rows, now);
        state.mark_credential_blocked(&store, &key, 0, now + 5_000_000.0, Some("spark"), &rows, now);
        assert_eq!(state.blocked_until(&store, &key, 0, &["chat"], &rows, now), Some(now + 4_000_000.0));
        assert_eq!(state.blocked_until(&store, &key, 0, &["", "chat", "spark"], &rows, now), Some(now + 5_000_000.0));
        assert!(state.clear_healed_block_scope(&store, &key, rows[0].id, 0, Some("chat"), &rows, now + 1.0).is_none());
        // The Host's injected time and SQLite's mutation wall clock are
        // independent. Use the actual four probe receipts: a fixed offset
        // from the corpus start can precede the later durable write's gate.
        let memory_global_probe = state.probe_after.get(&key).unwrap().get(&0).copied().unwrap();
        let memory_scoped_probe =
            state.probe_after.get(&scoped_key(&key, Some("chat"))).unwrap().get(&0).copied().unwrap();
        let persisted_global_probe =
            store.get_credential_block_reconcile_after(rows[0].id, &key, "").unwrap().unwrap() as f64;
        let persisted_scoped_probe =
            store.get_credential_block_reconcile_after(rows[0].id, &key, "chat").unwrap().unwrap() as f64;
        let after_probe =
            memory_global_probe.max(memory_scoped_probe).max(persisted_global_probe).max(persisted_scoped_probe);
        assert!(
            state
                .clear_healed_block_scope(&store, &key, rows[0].id, 0, Some("chat"), &rows, after_probe - 1.0)
                .is_none()
        );
        assert_eq!(
            state.clear_healed_block_scope(&store, &key, rows[0].id, 0, Some("chat"), &rows, after_probe),
            Some(now + 4_000_000.0)
        );
        assert!(store.get_credential_block(rows[0].id, &key, "chat").unwrap().is_none());
        assert!(store.get_credential_block(rows[0].id, &key, "spark").unwrap().is_some());
        assert_eq!(state.blocked_until(&store, &key, 0, &["chat"], &rows, after_probe), Some(now + 3_600_000.0));
        state.clear_credential_blocks(&store, "fixture", rows[0].id, &rows);
        assert!(state.blocked_until(&store, &key, 0, &["chat", "spark"], &rows, now).is_none());

        // Index-only memory fallback covers ephemeral candidates. NaN is falsy
        // on read and propagates across subsequent Math.max writes natively.
        for (scope, until, expected) in
            [("expired", now - 1.0, None), ("nan", f64::NAN, None), ("infinite", f64::INFINITY, Some(f64::INFINITY))]
        {
            state.mark_credential_blocked(&store, &key, 7, until, Some(scope), &rows, now);
            assert_eq!(state.blocked_until(&store, &key, 7, &[scope], &rows, now), expected);
        }
        assert!(!state.backoff.contains_key(&scoped_key(&key, Some("expired"))));
        assert!(!state.probe_after.contains_key(&scoped_key(&key, Some("expired"))));
        assert!(state.backoff.contains_key(&scoped_key(&key, Some("nan"))));
        state.mark_credential_blocked(&store, &key, 7, now + 500.0, Some("nan"), &rows, now);
        assert!(state.blocked_until(&store, &key, 7, &["nan"], &rows, now).is_none());

        // Provider reset clears assignments/backoff, preserving fixed native
        // probe residue and other providers' independent round-robin state.
        store.set_cache(&sticky_key("other", "retained"), "retained", i64::MAX).unwrap();
        state.reset_provider_assignments(&store, "fixture");
        assert!(store.get_cache(&sticky_key("fixture", "selected"), true).unwrap().is_none());
        assert!(store.get_cache(&sticky_key("other", "retained"), true).unwrap().is_some());
        assert_eq!(state.credential_order(&key, None, 3), [0, 1, 2]);
        assert_eq!(state.credential_order("other:oauth", None, 3), [1, 2, 0]);
        assert!(state.probe_after.contains_key(&scoped_key(&key, Some("nan"))));
        assert!(!state.backoff.contains_key(&scoped_key(&key, Some("nan"))));

        // SQLite BUSY is a real competing-writer failure. Advisory paths keep
        // memory and public broker writes/list propagate ordinary failures.
        let contender = rusqlite::Connection::open(&path).unwrap();
        contender.execute_batch("BEGIN IMMEDIATE").unwrap();
        let block = StoredCredentialBlock {
            credential_id: rows[1].id,
            provider_key: key.clone(),
            block_scope: "busy".to_owned(),
            blocked_until_ms: (now + 800_000.0) as i64,
            updated_at_ms: now as i64,
        };
        let error = state.upsert_credential_block(&store, &block).err().unwrap();
        assert!(crate::credential_store::is_sqlite_busy_error(&error));
        assert!(!state.persisted_block_store_damaged());
        state.mark_credential_blocked(&store, &key, 1, now + 800_000.0, Some("busy"), &rows, now);
        assert_eq!(state.blocked_until(&store, &key, 1, &["busy"], &rows, now), Some(now + 800_000.0));
        assert!(state.list_credential_blocks(&store, &[rows[1].id]).is_err());
        assert!(!state.take_block_store_damage_notice());
        contender.execute_batch("ROLLBACK").unwrap();

        for code in [rusqlite::ffi::SQLITE_CORRUPT, rusqlite::ffi::SQLITE_NOTADB] {
            let error: anyhow::Error = rusqlite::Error::SqliteFailure(rusqlite::ffi::Error::new(code), None).into();
            let was_damaged = state.persisted_block_store_damaged();
            assert!(state.observe_store_error(&error));
            assert_eq!(state.take_block_store_damage_notice(), !was_damaged);
        }
        assert!(state.persisted_block_store_damaged());
        assert!(state.list_credential_blocks(&store, &[rows[1].id]).unwrap().is_empty());
        assert!(state.upsert_credential_block(&store, &block).is_err());
        assert!(state.delete_credential_block(&store, rows[1].id, &key, "busy").is_err());
        assert!(state.delete_credential_blocks(&store, rows[1].id).is_err());
        assert_eq!(state.blocked_until(&store, &key, 1, &["busy"], &rows, now), Some(now + 800_000.0));
        state.reload_state(&rows);
        assert!(state.persisted_block_store_damaged());
        assert!(!state.take_block_store_damage_notice());

        // Native equality governs generation; OAuth org metadata and API key
        // source alone do not turn an identical snapshot into a credential edit.
        let mut metadata_only = rows.clone();
        if let AuthCredential::OAuth { fields } = &mut metadata_only[0].credential {
            fields.insert("orgId".to_owned(), Value::String("changed-org".to_owned()));
        }
        assert!(stored_credential_arrays_equal(&rows, &metadata_only));
        if let AuthCredential::OAuth { fields } = &mut metadata_only[0].credential {
            fields.insert("access".to_owned(), Value::String("changed-access".to_owned()));
        }
        assert!(!stored_credential_arrays_equal(&rows, &metadata_only));
    }
}
