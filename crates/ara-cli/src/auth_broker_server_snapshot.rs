//! Fixed OMP owned snapshot, capability projection and SSE wire builders.

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

use crate::auth_broker_refresher::RefresherSchedule;
use crate::auth_storage::{AuthStorageError, BrokerCredentialEntry, BrokerCredentialSnapshot, BrokerStorageError};
use crate::catalog_discovery::CatalogCollation;
use crate::credential_store::AuthCredential;
use ara_rpc::WireString;
use serde_json::{Value, json};
use std::collections::BTreeMap;

pub(crate) fn entry_value(entry: &BrokerCredentialEntry) -> Value {
    let mut credential = entry.credential.clone();
    if let AuthCredential::OAuth { fields } = &mut credential {
        fields.insert("refresh".into(), json!("__remote__"));
    }
    json!({"id":entry.id,"provider":entry.provider,"credential":credential,"identityKey":entry.identity_key})
}
pub(crate) fn snapshot_value(
    snapshot: BrokerCredentialSnapshot,
    schedule: Option<RefresherSchedule>,
    capable: bool,
    now: f64,
) -> Result<Value, BrokerStorageError> {
    if snapshot.generation > 9_007_199_254_740_991 {
        return Err(AuthStorageError::Storage.into());
    }
    let wire = match schedule {
        Some(s) => {
            json!({"enabled":s.enabled,"intervalMs":s.interval_ms,"skewMs":s.skew_ms,"nextSweepInMs":(s.next_sweep_at_ms-now).max(0.0)})
        }
        None => json!({"enabled":false,"intervalMs":0,"skewMs":0,"nextSweepInMs":9_007_199_254_740_991_u64}),
    };
    let collator = CatalogCollation::for_host().map_err(|_| AuthStorageError::Configuration)?;
    let mut groups = BTreeMap::<i64, Vec<Value>>::new();
    for block in snapshot.blocks.iter().filter(|b| b.blocked_until_ms as f64 > now) {
        groups.entry(block.credential_id).or_default().push(json!({"providerKey":block.provider_key,"blockScope":block.block_scope,"blockedUntilMs":block.blocked_until_ms,"updatedAtMs":block.updated_at_ms}));
    }
    for blocks in groups.values_mut() {
        if !capable {
            let mut shared: Option<Value> = None;
            blocks.retain(|block| {
                if block["providerKey"] == "openai-codex:oauth" && ["chat","spark","shared"].contains(&block["blockScope"].as_str().unwrap_or("")) {
                    let until = block["blockedUntilMs"].as_i64().unwrap_or(0).max(shared.as_ref().and_then(|b| b["blockedUntilMs"].as_i64()).unwrap_or(0));
                    let updated = block["updatedAtMs"].as_i64().unwrap_or(0).max(shared.as_ref().and_then(|b| b["updatedAtMs"].as_i64()).unwrap_or(0));
                    shared = Some(json!({"providerKey":"openai-codex:oauth","blockScope":"shared","blockedUntilMs":until,"updatedAtMs":updated})); false
                } else { true }
            });
            if let Some(shared) = shared {
                blocks.push(shared);
            }
        }
        blocks.sort_by(|a, b| {
            collator
                .compare(
                    &WireString::from(a["providerKey"].as_str().unwrap_or("")),
                    &WireString::from(b["providerKey"].as_str().unwrap_or("")),
                )
                .then_with(|| {
                    collator.compare(
                        &WireString::from(a["blockScope"].as_str().unwrap_or("")),
                        &WireString::from(b["blockScope"].as_str().unwrap_or("")),
                    )
                })
                .then_with(|| a["blockedUntilMs"].as_i64().cmp(&b["blockedUntilMs"].as_i64()))
        });
    }
    let entries = snapshot
        .credentials
        .iter()
        .map(|entry| {
            let mut value = entry_value(entry);
            let expires = match &entry.credential {
                AuthCredential::OAuth { fields } => fields.get("expires").and_then(Value::as_f64),
                _ => None,
            };
            let rotation = schedule
                .and_then(|s| expires.filter(|v| v.is_finite()).and_then(|expires| rotates_in_ms(expires, s, now)));
            value["rotatesInMs"] = rotation.map_or(Value::Null, |v| json!(v));
            if let Some(blocks) = groups.get(&entry.id).filter(|v| !v.is_empty()) {
                value["blocks"] = json!(blocks);
            }
            value
        })
        .collect::<Vec<_>>();
    Ok(
        json!({"generation":snapshot.generation,"generatedAt":snapshot.generated_at_ms,"serverNowMs":now,"refresher":wire,"credentials":entries}),
    )
}
fn rotates_in_ms(expires: f64, s: RefresherSchedule, now: f64) -> Option<f64> {
    if !s.enabled || !s.next_sweep_at_ms.is_finite() || !s.interval_ms.is_finite() || s.interval_ms <= 0.0 {
        return None;
    }
    let due = expires - s.skew_ms;
    let eligible = now.max(due);
    if due <= now && s.next_sweep_at_ms <= now {
        Some(0.0)
    } else if s.next_sweep_at_ms >= eligible {
        Some((s.next_sweep_at_ms - now).max(0.0))
    } else {
        Some(
            (s.next_sweep_at_ms + ((eligible - s.next_sweep_at_ms) / s.interval_ms).ceil() * s.interval_ms - now)
                .max(0.0),
        )
    }
}
pub(crate) fn fingerprint(entry: &Value) -> String {
    json!([
        entry["id"],
        entry["provider"],
        entry["identityKey"],
        entry["rotatesInMs"],
        entry["credential"],
        entry.get("blocks").cloned().unwrap_or_else(|| json!([]))
    ])
    .to_string()
}
pub(crate) fn sse(kind: &str, value: Value) -> bytes::Bytes {
    bytes::Bytes::from(format!("event: {kind}\ndata: {value}\n\n"))
}
