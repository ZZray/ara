//! Fixed OMP auth-broker wire contracts, validated before projection.
//! Source: OMP 596f2da7101178214aa27a753529d15e6b7ad91d,
//! packages/ai/src/auth-broker/{types,wire-schemas}.ts.
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

use crate::credential_store::{AuthCredential, DisabledCredentialSummary};
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};
use std::fmt;

pub const REMOTE_REFRESH_SENTINEL: &str = "__remote__";
pub const AUTH_BROKER_CAPABILITIES_HEADER: &str = "OMP-Auth-Broker-Capabilities";
pub const AUTH_BROKER_CAPABILITY_CODEX_METER_BLOCK_SCOPES: &str = "codex-meter-block-scopes";

#[derive(Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BrokerBlock {
    pub provider_key: String,
    pub block_scope: String,
    pub blocked_until_ms: f64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub updated_at_ms: Option<f64>,
}

#[derive(Clone, PartialEq)]
pub struct BrokerRefresher {
    pub enabled: bool,
    pub interval_ms: f64,
    pub skew_ms: f64,
    pub next_sweep_in_ms: f64,
}

impl Default for BrokerRefresher {
    fn default() -> Self {
        Self { enabled: false, interval_ms: 0.0, skew_ms: 0.0, next_sweep_in_ms: 9_007_199_254_740_991.0 }
    }
}

/// Credential-bearing values intentionally have neither Debug nor Serialize.
#[derive(Clone)]
pub struct BrokerSnapshotEntry {
    pub id: i64,
    pub provider: String,
    pub credential: AuthCredential,
    pub identity_key: Option<String>,
    pub rotates_in_ms: Option<f64>,
    pub blocks: Vec<BrokerBlock>,
}

#[derive(Clone, Default)]
pub struct BrokerSnapshot {
    pub generation: i64,
    pub generated_at: f64,
    pub server_now_ms: f64,
    pub refresher: BrokerRefresher,
    pub credentials: Vec<BrokerSnapshotEntry>,
}

#[derive(Clone)]
pub enum BrokerSnapshotResult {
    Snapshot { snapshot: BrokerSnapshot, generation: i64 },
    NotModified { generation: i64 },
}

#[derive(Clone)]
pub enum BrokerStreamEvent {
    Snapshot(BrokerSnapshot),
    Entry { entry: BrokerSnapshotEntry, generation: i64, server_now_ms: f64, refresher: BrokerRefresher },
    Removed { id: i64, generation: i64, server_now_ms: f64, refresher: BrokerRefresher },
}

/// The 31 native public schema entrypoints. Request/block alias validation is
/// deliberately identical; extensible OAuth and usage objects retain their keys.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum BrokerWireSchema {
    OAuthCredential,
    RemoteOAuthCredential,
    ApiKeyCredential,
    WritableAuthCredential,
    SnapshotCredential,
    CredentialSnapshotEntry,
    CredentialBlockSnapshot,
    SnapshotEntry,
    RefresherSchedule,
    SnapshotResponse,
    SnapshotStreamSnapshotEvent,
    SnapshotStreamEntryEvent,
    SnapshotStreamRemovedEvent,
    SnapshotStreamEvent,
    HealthzResponse,
    UsageResponse,
    UsageHistoryResponse,
    ClientUsageReportRequest,
    ClientUsageReportResponse,
    ClientUsageSummaryResponse,
    CredentialRefreshResponse,
    CredentialDisableRequest,
    CredentialDisableResponse,
    DisabledCredentialSummary,
    DisabledCredentialsResponse,
    CredentialBlockRequest,
    CredentialBlockResponse,
    CredentialBlocksDeleteResponse,
    UsageStaleResponse,
    CredentialUploadRequest,
    CredentialUploadResponse,
}

/// Only static explanations are retained: a malformed payload can contain secrets.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct BrokerWireError(&'static str);
impl fmt::Display for BrokerWireError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.0)
    }
}
impl std::error::Error for BrokerWireError {}
type WireResult<T = ()> = Result<T, BrokerWireError>;
fn invalid() -> BrokerWireError {
    BrokerWireError("auth broker wire schema validation failed")
}

fn object(value: &Value) -> WireResult<&Map<String, Value>> {
    value.as_object().ok_or_else(invalid)
}
fn shape<'a>(value: &'a Value, required: &[&str], optional: &[&str]) -> WireResult<&'a Map<String, Value>> {
    let value = object(value)?;
    if required.iter().any(|key| !value.contains_key(*key))
        || value.keys().any(|key| !required.contains(&key.as_str()) && !optional.contains(&key.as_str()))
    {
        return Err(invalid());
    }
    Ok(value)
}
fn field<'a>(object: &'a Map<String, Value>, key: &str) -> WireResult<&'a Value> {
    object.get(key).ok_or_else(invalid)
}
fn string(value: &Value) -> WireResult<&str> {
    value.as_str().ok_or_else(invalid)
}
fn nonempty(value: &Value) -> WireResult<&str> {
    let text = string(value)?;
    if text.is_empty() { Err(invalid()) } else { Ok(text) }
}
fn number(value: &Value) -> WireResult<f64> {
    value.as_f64().filter(|v| v.is_finite()).ok_or_else(invalid)
}
fn integer(value: &Value) -> WireResult<i64> {
    // JSON.parse exposes IEEE-754 numbers even when a JSON token is written
    // without a decimal point; retain its integer rounding before projection.
    // Rust's existing credential IDs/generation are i64, so values outside
    // that host contract cannot be projected and are rejected explicitly.
    let value = number(value)?;
    if value.fract() != 0.0 || !(-9_223_372_036_854_775_808.0..9_223_372_036_854_775_808.0).contains(&value) {
        return Err(invalid());
    }
    Ok(value as i64)
}
fn boolean(value: &Value) -> WireResult<bool> {
    value.as_bool().ok_or_else(invalid)
}
fn array(value: &Value) -> WireResult<&Vec<Value>> {
    value.as_array().ok_or_else(invalid)
}
fn optional(object: &Map<String, Value>, key: &str, validate: impl FnOnce(&Value) -> WireResult) -> WireResult {
    match object.get(key) {
        Some(value) => validate(value),
        None => Ok(()),
    }
}
fn optional_strings(object: &Map<String, Value>, keys: &[&str]) -> WireResult {
    for key in keys {
        optional(object, key, |value| string(value).map(|_| ()))?;
    }
    Ok(())
}
fn optional_numbers(object: &Map<String, Value>, keys: &[&str]) -> WireResult {
    for key in keys {
        optional(object, key, |value| number(value).map(|_| ()))?;
    }
    Ok(())
}
fn strings(value: &Value) -> WireResult {
    for item in array(value)? {
        string(item)?;
    }
    Ok(())
}
fn enumeration(value: &Value, allowed: &[&str]) -> WireResult {
    if allowed.contains(&string(value)?) { Ok(()) } else { Err(invalid()) }
}
fn items(value: &Value, validate: impl Fn(&Value) -> WireResult) -> WireResult {
    for item in array(value)? {
        validate(item)?;
    }
    Ok(())
}

fn oauth(value: &Value, remote: bool) -> WireResult {
    let value = object(value)?;
    enumeration(field(value, "type")?, &["oauth"])?;
    nonempty(field(value, "access")?)?;
    let refresh = string(field(value, "refresh")?)?;
    if (refresh == REMOTE_REFRESH_SENTINEL) != remote {
        return Err(invalid());
    }
    number(field(value, "expires")?)?;
    optional_strings(value, &["apiEndpoint", "enterpriseUrl", "projectId", "email", "accountId", "orgId", "orgName"])?;
    optional_numbers(value, &["authorizedAt"])
}
fn api_key(value: &Value) -> WireResult {
    let value = shape(value, &["type", "key"], &["source"])?;
    enumeration(field(value, "type")?, &["api_key"])?;
    nonempty(field(value, "key")?)?;
    optional(value, "source", |value| enumeration(value, &["login"]))
}
fn credential(value: &Value, remote: bool) -> WireResult {
    match string(field(object(value)?, "type")?)? {
        "api_key" => api_key(value),
        "oauth" => oauth(value, remote),
        _ => Err(invalid()),
    }
}
fn block(value: &Value) -> WireResult {
    let value = shape(value, &["providerKey", "blockScope", "blockedUntilMs"], &["updatedAtMs"])?;
    nonempty(field(value, "providerKey")?)?;
    string(field(value, "blockScope")?)?;
    number(field(value, "blockedUntilMs")?)?;
    optional_numbers(value, &["updatedAtMs"])
}
fn entry(value: &Value, schedule: bool) -> WireResult {
    let required = if schedule {
        &["id", "provider", "credential", "identityKey", "rotatesInMs"][..]
    } else {
        &["id", "provider", "credential", "identityKey"][..]
    };
    let value = shape(value, required, if schedule { &["blocks"] } else { &[] })?;
    integer(field(value, "id")?)?;
    nonempty(field(value, "provider")?)?;
    credential(field(value, "credential")?, true)?;
    let identity = field(value, "identityKey")?;
    if !identity.is_null() {
        string(identity)?;
    }
    if schedule {
        let rotation = field(value, "rotatesInMs")?;
        if !rotation.is_null() {
            number(rotation)?;
        }
        optional(value, "blocks", |value| items(value, block))?;
    }
    Ok(())
}
fn refresher(value: &Value) -> WireResult {
    let value = shape(value, &["enabled", "intervalMs", "skewMs", "nextSweepInMs"], &[])?;
    boolean(field(value, "enabled")?)?;
    for key in ["intervalMs", "skewMs", "nextSweepInMs"] {
        number(field(value, key)?)?;
    }
    Ok(())
}
fn snapshot(value: &Value, stream: bool) -> WireResult {
    let required = if stream {
        &["kind", "generation", "generatedAt", "serverNowMs", "refresher", "credentials"][..]
    } else {
        &["generation", "generatedAt", "serverNowMs", "refresher", "credentials"][..]
    };
    let value = shape(value, required, &[])?;
    if stream {
        enumeration(field(value, "kind")?, &["snapshot"])?;
    }
    integer(field(value, "generation")?)?;
    number(field(value, "generatedAt")?)?;
    number(field(value, "serverNowMs")?)?;
    refresher(field(value, "refresher")?)?;
    items(field(value, "credentials")?, |value| entry(value, true))
}
fn stream_delta(value: &Value, removed: bool) -> WireResult {
    let key = if removed { "id" } else { "entry" };
    let value = shape(value, &["kind", "generation", "serverNowMs", "refresher", key], &[])?;
    enumeration(field(value, "kind")?, &[if removed { "removed" } else { "entry" }])?;
    integer(field(value, "generation")?)?;
    number(field(value, "serverNowMs")?)?;
    refresher(field(value, "refresher")?)?;
    if removed {
        integer(field(value, key)?)?;
    } else {
        entry(field(value, key)?, true)?;
    }
    Ok(())
}
fn stream(value: &Value) -> WireResult {
    match string(field(object(value)?, "kind")?)? {
        "snapshot" => snapshot(value, true),
        "entry" => stream_delta(value, false),
        "removed" => stream_delta(value, true),
        _ => Err(invalid()),
    }
}
fn ok_response(value: &Value) -> WireResult {
    let value = shape(value, &["ok"], &[])?;
    boolean(field(value, "ok")?)?;
    Ok(())
}
fn status(value: &Value) -> WireResult {
    enumeration(value, &["ok", "warning", "exhausted", "unknown"])
}
fn usage_report(value: &Value) -> WireResult {
    let value = object(value)?;
    string(field(value, "provider")?)?;
    number(field(value, "fetchedAt")?)?;
    items(field(value, "limits")?, usage_limit)?;
    optional(value, "notes", strings)?;
    optional(value, "metadata", |value| object(value).map(|_| ()))?;
    optional(value, "resetCredits", |value| {
        let value = object(value)?;
        number(field(value, "availableCount")?)?;
        optional(value, "credits", |value| {
            items(value, |value| optional_strings(object(value)?, &["grantedAt", "expiresAt", "status"]))
        })
    })
}
fn usage_limit(value: &Value) -> WireResult {
    let value = object(value)?;
    string(field(value, "id")?)?;
    string(field(value, "label")?)?;
    let scope = object(field(value, "scope")?)?;
    string(field(scope, "provider")?)?;
    optional_strings(scope, &["accountId", "projectId", "orgId", "modelId", "tier", "windowId"])?;
    optional(scope, "shared", |value| boolean(value).map(|_| ()))?;
    let amount = object(field(value, "amount")?)?;
    enumeration(
        field(amount, "unit")?,
        &["percent", "tokens", "requests", "credits", "usd", "minutes", "bytes", "unknown"],
    )?;
    optional_numbers(amount, &["used", "limit", "remaining", "usedFraction", "remainingFraction"])?;
    optional(value, "status", status)?;
    optional(value, "notes", strings)?;
    optional(value, "window", |value| {
        let value = object(value)?;
        string(field(value, "id")?)?;
        string(field(value, "label")?)?;
        optional_numbers(value, &["durationMs", "resetsAt"])
    })
}
fn history_entry(value: &Value) -> WireResult {
    let value = object(value)?;
    number(field(value, "recordedAt")?)?;
    for key in ["provider", "accountKey", "limitId", "label"] {
        string(field(value, key)?)?;
    }
    optional_strings(value, &["email", "accountId", "windowLabel"])?;
    optional_numbers(value, &["usedFraction", "resetsAt"])?;
    optional(value, "status", status)
}
fn observed_entry(value: &Value) -> WireResult {
    let value = object(value)?;
    for key in ["provider", "model"] {
        string(field(value, key)?)?;
    }
    for key in ["at", "requests", "inputTokens", "outputTokens", "cacheReadTokens", "cacheWriteTokens", "costUsd"] {
        number(field(value, key)?)?;
    }
    Ok(())
}
fn client_summary(value: &Value) -> WireResult {
    let value = object(value)?;
    string(field(value, "installId")?)?;
    optional_strings(value, &["hostname"])?;
    for key in ["firstSeen", "lastSeen"] {
        number(field(value, key)?)?;
    }
    items(field(value, "providers")?, |value| {
        let value = object(value)?;
        string(field(value, "provider")?)?;
        optional_strings(value, &["app"])?;
        for key in ["requests", "inputTokens", "outputTokens", "cacheReadTokens", "cacheWriteTokens", "costUsd"] {
            number(field(value, key)?)?;
        }
        Ok(())
    })
}
fn generated_array(value: &Value, key: &str, validate: impl Fn(&Value) -> WireResult) -> WireResult {
    let value = shape(value, &["generatedAt", key], &[])?;
    number(field(value, "generatedAt")?)?;
    items(field(value, key)?, validate)
}
fn disabled(value: &Value) -> WireResult {
    let value = shape(
        value,
        &["id", "provider", "type", "cause"],
        &["email", "accountId", "orgId", "orgName", "disabledAtMs"],
    )?;
    integer(field(value, "id")?)?;
    nonempty(field(value, "provider")?)?;
    enumeration(field(value, "type")?, &["oauth", "api_key"])?;
    string(field(value, "cause")?)?;
    optional_strings(value, &["email", "accountId", "orgId", "orgName"])?;
    optional_numbers(value, &["disabledAtMs"])
}

pub fn validate_wire(schema: BrokerWireSchema, value: &Value) -> WireResult {
    use BrokerWireSchema as S;
    match schema {
        S::OAuthCredential => oauth(value, false),
        S::RemoteOAuthCredential => oauth(value, true),
        S::ApiKeyCredential => api_key(value),
        S::WritableAuthCredential => credential(value, false),
        S::SnapshotCredential => credential(value, true),
        S::CredentialSnapshotEntry => entry(value, false),
        S::SnapshotEntry => entry(value, true),
        S::CredentialBlockSnapshot | S::CredentialBlockRequest => block(value),
        S::RefresherSchedule => refresher(value),
        S::SnapshotResponse => snapshot(value, false),
        S::SnapshotStreamSnapshotEvent => snapshot(value, true),
        S::SnapshotStreamEntryEvent => stream_delta(value, false),
        S::SnapshotStreamRemovedEvent => stream_delta(value, true),
        S::SnapshotStreamEvent => stream(value),
        S::HealthzResponse => {
            let value = shape(value, &["ok"], &["version"])?;
            boolean(field(value, "ok")?)?;
            optional_strings(value, &["version"])
        }
        S::UsageResponse => generated_array(value, "reports", usage_report),
        S::UsageHistoryResponse => generated_array(value, "entries", history_entry),
        S::ClientUsageSummaryResponse => generated_array(value, "clients", client_summary),
        S::ClientUsageReportRequest => {
            let value = shape(value, &["installId", "entries"], &["hostname", "app"])?;
            string(field(value, "installId")?)?;
            optional_strings(value, &["hostname", "app"])?;
            items(field(value, "entries")?, observed_entry)
        }
        S::ClientUsageReportResponse
        | S::CredentialDisableResponse
        | S::CredentialBlockResponse
        | S::CredentialBlocksDeleteResponse
        | S::UsageStaleResponse => ok_response(value),
        S::CredentialRefreshResponse => {
            let value = shape(value, &["entry"], &[])?;
            entry(field(value, "entry")?, false)
        }
        S::CredentialDisableRequest => {
            let value = shape(value, &[], &["cause"])?;
            optional_strings(value, &["cause"])
        }
        S::DisabledCredentialSummary => disabled(value),
        S::DisabledCredentialsResponse => generated_array(value, "disabled", disabled),
        S::CredentialUploadRequest => {
            let value = shape(value, &["provider", "credential"], &[])?;
            nonempty(field(value, "provider")?)?;
            credential(field(value, "credential")?, false)
        }
        S::CredentialUploadResponse => {
            let value = shape(value, &["entries"], &[])?;
            items(field(value, "entries")?, |value| entry(value, false))
        }
    }
}

fn parse_refresher(value: &Value) -> WireResult<BrokerRefresher> {
    let value = object(value)?;
    Ok(BrokerRefresher {
        enabled: boolean(field(value, "enabled")?)?,
        interval_ms: number(field(value, "intervalMs")?)?,
        skew_ms: number(field(value, "skewMs")?)?,
        next_sweep_in_ms: number(field(value, "nextSweepInMs")?)?,
    })
}
fn parse_entry_unchecked(value: &Value) -> WireResult<BrokerSnapshotEntry> {
    let value = object(value)?;
    let mut blocks = Vec::new();
    if let Some(items) = value.get("blocks") {
        for item in array(items)? {
            let item = object(item)?;
            blocks.push(BrokerBlock {
                provider_key: string(field(item, "providerKey")?)?.to_owned(),
                block_scope: string(field(item, "blockScope")?)?.to_owned(),
                blocked_until_ms: number(field(item, "blockedUntilMs")?)?,
                updated_at_ms: item.get("updatedAtMs").map(number).transpose()?,
            });
        }
    }
    Ok(BrokerSnapshotEntry {
        id: integer(field(value, "id")?)?,
        provider: string(field(value, "provider")?)?.to_owned(),
        credential: serde_json::from_value(field(value, "credential")?.clone()).map_err(|_| invalid())?,
        identity_key: field(value, "identityKey")?.as_str().map(str::to_owned),
        rotates_in_ms: value.get("rotatesInMs").filter(|v| !v.is_null()).map(number).transpose()?,
        blocks,
    })
}
pub fn parse_credential_entry(value: &Value) -> WireResult<BrokerSnapshotEntry> {
    validate_wire(BrokerWireSchema::CredentialSnapshotEntry, value)?;
    parse_entry_unchecked(value)
}
fn parse_snapshot_unchecked(value: &Value) -> WireResult<BrokerSnapshot> {
    let value = object(value)?;
    Ok(BrokerSnapshot {
        generation: integer(field(value, "generation")?)?,
        generated_at: number(field(value, "generatedAt")?)?,
        server_now_ms: number(field(value, "serverNowMs")?)?,
        refresher: parse_refresher(field(value, "refresher")?)?,
        credentials: array(field(value, "credentials")?)?
            .iter()
            .map(parse_entry_unchecked)
            .collect::<WireResult<_>>()?,
    })
}
pub fn parse_snapshot(value: &Value) -> WireResult<BrokerSnapshot> {
    validate_wire(BrokerWireSchema::SnapshotResponse, value)?;
    parse_snapshot_unchecked(value)
}
pub fn parse_stream_event(value: &Value) -> WireResult<BrokerStreamEvent> {
    validate_wire(BrokerWireSchema::SnapshotStreamEvent, value)?;
    let fields = object(value)?;
    let kind = string(field(fields, "kind")?)?;
    if kind == "snapshot" {
        return Ok(BrokerStreamEvent::Snapshot(parse_snapshot_unchecked(value)?));
    }
    let generation = integer(field(fields, "generation")?)?;
    let server_now_ms = number(field(fields, "serverNowMs")?)?;
    let refresher = parse_refresher(field(fields, "refresher")?)?;
    Ok(if kind == "entry" {
        BrokerStreamEvent::Entry {
            entry: parse_entry_unchecked(field(fields, "entry")?)?,
            generation,
            server_now_ms,
            refresher,
        }
    } else {
        BrokerStreamEvent::Removed { id: integer(field(fields, "id")?)?, generation, server_now_ms, refresher }
    })
}
pub fn parse_disabled_summary(value: &Value) -> WireResult<DisabledCredentialSummary> {
    validate_wire(BrokerWireSchema::DisabledCredentialSummary, value)?;
    let value = object(value)?;
    Ok(DisabledCredentialSummary {
        id: integer(field(value, "id")?)?,
        provider: string(field(value, "provider")?)?.to_owned(),
        credential_type: string(field(value, "type")?)?.to_owned(),
        cause: string(field(value, "cause")?)?.to_owned(),
        email: value.get("email").and_then(Value::as_str).map(str::to_owned),
        account_id: value.get("accountId").and_then(Value::as_str).map(str::to_owned),
        org_id: value.get("orgId").and_then(Value::as_str).map(str::to_owned),
        org_name: value.get("orgName").and_then(Value::as_str).map(str::to_owned),
        disabled_at_ms: value.get("disabledAtMs").map(number).transpose()?,
    })
}

#[cfg(test)]
#[path = "auth_broker_wire_tests.rs"]
mod tests;
