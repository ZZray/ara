//! Fixed OMP Cursor discovery, HTTP/2 transport and Connect protobuf framing.
// MIT License
//
// Copyright (c) 2025-2026 Can Bölük
// Copyright (c) 2026 Stencil Labs, Inc.
//
// Permission is hereby granted, free of charge, to any person obtaining a copy
// of this software and associated documentation files (the "Software"), to deal
// in the Software without restriction, including without limitation the rights
// to use, copy, modify, merge, publish, distribute, sublicense, and/or sell
// copies of the Software, and to permit persons to whom the Software is
// furnished to do so, subject to the following conditions:
//
// The above copyright notice and this permission notice shall be included in all
// copies or substantial portions of the Software.
//
// THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF ANY KIND, EXPRESS OR
// IMPLIED, INCLUDING BUT NOT LIMITED TO THE WARRANTIES OF MERCHANTABILITY,
// FITNESS FOR A PARTICULAR PURPOSE AND NONINFRINGEMENT. IN NO EVENT SHALL THE
// AUTHORS OR COPYRIGHT HOLDERS BE LIABLE FOR ANY CLAIM, DAMAGES OR OTHER
// LIABILITY, WHETHER IN AN ACTION OF CONTRACT, TORT OR OTHERWISE, ARISING FROM,
// OUT OF OR IN CONNECTION WITH THE SOFTWARE OR THE USE OR OTHER DEALINGS IN THE
// SOFTWARE.

use super::{
    CatalogContext, DiscoveryError, DiscoveryRequest, DiscoveryResult, HttpMethod, bundled_references, js_trim,
    model_string, trim_trailing_slashes, wire_string, zero_cost,
};
use crate::{
    catalog_proto_schemas::cursor::{GetUsableModelsRequestSchema, GetUsableModelsResponseSchema},
    catalog_protobuf::{ByteView, ProtoMessage, ProtoValue},
    catalog_rules,
    js_regex::JsRegExp,
    model_collapse::{SpecRef, VariantSpec},
    model_wire_policy::classify_wire,
};
use ara_rpc::{WireString, WireValue};
use std::{
    collections::{HashMap, HashSet},
    sync::Arc,
    time::Duration,
};

pub const CURSOR_DEFAULT_BASE_URL: &str = "https://api2.cursor.sh";
pub const CURSOR_DEFAULT_CLIENT_VERSION: &str = "cli-2026.02.13-41ac335";
pub const CURSOR_GET_USABLE_MODELS_PATH: &str = "/agent.v1.AgentService/GetUsableModels";
#[derive(Clone)]
pub struct CursorModelDiscoveryOptions {
    pub api_key: WireString,
    pub base_url: Option<WireString>,
    pub client_version: Option<WireString>,
    pub timeout_ms: Option<f64>,
    pub custom_model_ids: Option<Vec<ProtoValue>>,
}
impl Default for CursorModelDiscoveryOptions {
    fn default() -> Self {
        Self { api_key: "".into(), base_url: None, client_version: None, timeout_ms: None, custom_model_ids: None }
    }
}
pub fn timer_duration(value: f64) -> Duration {
    Duration::from_millis(if !value.is_finite() || !(1.0..=f64::from(i32::MAX)).contains(&value) {
        1
    } else {
        value.trunc() as u64
    })
}
pub fn normalize_custom_model_ids(values: Option<&[ProtoValue]>) -> Vec<WireString> {
    let mut seen = HashSet::new();
    values
        .unwrap_or_default()
        .iter()
        .filter_map(ProtoValue::as_string)
        .map(|value| js_trim(&value))
        .filter(|value| !value.is_empty() && seen.insert(value.clone()))
        .collect()
}
pub async fn fetch_cursor_usable_models(
    context: &CatalogContext,
    options: &CursorModelDiscoveryOptions,
) -> DiscoveryResult {
    match fetch_inner(context, options).await {
        Ok(value) => Ok(value),
        Err(_) => Ok(None),
    }
}
async fn fetch_inner(context: &CatalogContext, options: &CursorModelDiscoveryOptions) -> DiscoveryResult {
    let ids = normalize_custom_model_ids(options.custom_model_ids.as_deref());
    let request = GetUsableModelsRequestSchema.create(Some(&ProtoMessage::from_entries(&[(
        "customModelIds",
        ProtoValue::array(ids.into_iter().map(ProtoValue::String).collect()),
    )])));
    let body =
        GetUsableModelsRequestSchema.encode(&request).map_err(|e| DiscoveryError::named(e.name, e.message))?.bytes();
    let base = trim_trailing_slashes(&options.base_url.clone().unwrap_or_else(|| CURSOR_DEFAULT_BASE_URL.into()));
    let text = String::from_utf16_lossy(base.units());
    let mut url = reqwest::Url::parse(&text).map_err(|_| DiscoveryError::new("Invalid URL"))?;
    url.set_path(CURSOR_GET_USABLE_MODELS_PATH);
    url.set_query(None);
    url.set_fragment(None);
    let bearer = WireString::from_units([WireString::from("Bearer ").units(), options.api_key.units()].concat());
    let req = DiscoveryRequest {
        url: url.as_str().into(),
        method: HttpMethod::Post,
        headers: vec![
            ("content-type".into(), "application/proto".into()),
            ("te".into(), "trailers".into()),
            ("authorization".into(), bearer),
            ("x-ghost-mode".into(), "true".into()),
            (
                "x-cursor-client-version".into(),
                options.client_version.clone().unwrap_or_else(|| CURSOR_DEFAULT_CLIENT_VERSION.into()),
            ),
            ("x-cursor-client-type".into(), "cli".into()),
        ],
        body: Some(body),
        signal: None,
        http2: true,
        tls: None,
        native_init: None,
        body_policy: super::DiscoveryBodyPolicy::default(),
    };
    let reply = tokio::time::timeout(
        timer_duration(options.timeout_ms.unwrap_or(5000.0)),
        context.http2_transport().fetch(req),
    )
    .await
    .map_err(|_| DiscoveryError::new("Cursor discovery timed out"))??;
    if !reply.ok() {
        return Ok(None);
    }
    let Some(decoded) = decode_get_usable_models_response(&ByteView::new(reply.body)) else { return Ok(None) };
    Ok(Some(normalize_cursor_models(
        context,
        &decoded.get("models").items(),
        options.base_url.as_ref(),
        &bundled_references(&"cursor".into()),
    )?))
}
pub fn decode_connect_unary_body(payload: &ByteView) -> Option<ByteView> {
    if payload.len() < 5 {
        return None;
    }
    let bytes = payload.bytes();
    let mut offset = 0usize;
    while offset + 5 <= bytes.len() {
        let flags = bytes[offset];
        let length = u32::from_be_bytes(bytes[offset + 1..offset + 5].try_into().ok()?) as usize;
        let end = offset.checked_add(5)?.checked_add(length)?;
        if end > bytes.len() || flags & 1 != 0 {
            return None;
        }
        if flags & 2 == 0 {
            return Some(payload.slice(offset + 5, end));
        }
        offset = end;
    }
    None
}
pub fn decode_get_usable_models_response(payload: &ByteView) -> Option<ProtoMessage> {
    if payload.is_empty() {
        return None;
    }
    if let Some(framed) = decode_connect_unary_body(payload) {
        return GetUsableModelsResponseSchema.decode(&framed).ok();
    }
    GetUsableModelsResponseSchema.decode(payload).ok()
}
fn class(id: &WireString) -> Result<VariantSpec, DiscoveryError> {
    classify_wire(&"cursor".into(), id, true).map_err(Into::into)
}
fn text_eq(identity: &VariantSpec, key: &str, s: &str) -> bool {
    identity.get(key).and_then(WireValue::as_string).is_some_and(|v| v.equals_ascii(s))
}
fn revision_at_least(identity: &VariantSpec, floor: &str) -> bool {
    identity
        .get("revision")
        .and_then(WireValue::as_string)
        .and_then(|v| v.to_utf8().ok())
        .and_then(|s| catalog_rules::parse_revision(&s))
        .zip(catalog_rules::parse_revision(floor))
        .is_some_and(|(v, f)| catalog_rules::compare_revision(v, f) >= 0)
}
fn has_1m(value: &WireString) -> bool {
    JsRegExp::new("\\b1m\\b".into(), "i").expect("fixed regex").exec(value).is_some()
}
/// A provided reference array retains its identity, including an empty array.
pub fn resolve_cursor_input(
    id: &WireString,
    reference: Option<&Arc<Vec<WireString>>>,
) -> Result<Arc<Vec<WireString>>, DiscoveryError> {
    if let Some(reference) = reference {
        return Ok(reference.clone());
    }
    let identity = class(id)?;
    Ok(Arc::new(if ["anthropic", "gemini", "openai"].iter().any(|c| text_eq(&identity, "class", c)) {
        vec!["text".into(), "image".into()]
    } else {
        vec!["text".into()]
    }))
}
pub fn normalize_cursor_models(
    context: &CatalogContext,
    models: &[ProtoValue],
    base_url: Option<&WireString>,
    references: &HashMap<WireString, SpecRef>,
) -> Result<Vec<SpecRef>, DiscoveryError> {
    let mut output: Vec<SpecRef> = vec![];
    for value in models {
        let Some(details) = value.as_object() else { continue };
        let Some(id) = details.get("modelId").as_string().map(|v| js_trim(&v)) else { continue };
        if id.is_empty() {
            continue;
        }
        let max_mode = match details.get("maxMode") {
            ProtoValue::Undefined => false,
            ProtoValue::Bool(v) => v,
            _ => continue,
        };
        let mut candidates =
            vec![details.get("displayName"), details.get("displayNameShort"), details.get("displayModelId")];
        candidates.extend(details.get("aliases").items());
        candidates.push(ProtoValue::String(id.clone()));
        let name = candidates
            .iter()
            .filter_map(ProtoValue::as_string)
            .map(|v| js_trim(&v))
            .find(|v| !v.is_empty())
            .unwrap_or_else(|| id.clone());
        let identity = class(&id)?;
        let reference = references.get(&id);
        let reasoning = (text_eq(&identity, "class", "kimi") && text_eq(&identity, "family", "k3"))
            || (text_eq(&identity, "class", "xai") && revision_at_least(&identity, "4"))
            || details.get("thinkingDetails").truthy()
            || reference.is_some_and(|r| r.get("reasoning") == Some(&WireValue::Bool(true)));
        let labeled = has_1m(&id) || candidates.iter().filter_map(ProtoValue::as_string).any(|s| has_1m(&s));
        let glm = text_eq(&identity, "class", "glm")
            && revision_at_least(&identity, "5.2")
            && (identity.get("family").is_none()
                || text_eq(&identity, "family", "air")
                || text_eq(&identity, "family", "turbo"));
        let promote = labeled
            || glm
            || (max_mode && (text_eq(&identity, "class", "anthropic") || text_eq(&identity, "class", "gemini")));
        let fallback = reference.and_then(|r| r.get("contextWindow")).cloned().unwrap_or(WireValue::Number(200000.0));
        let context_window = if promote {
            let n = match fallback {
                WireValue::Number(n) => n,
                _ => 0.0,
            };
            WireValue::Number(if n.is_nan() { n } else { n.max(1000000.0) })
        } else {
            fallback
        };
        let mut spec =
            reference.map(|v| v.as_ref().clone()).unwrap_or_else(|| VariantSpec::from_wire(WireValue::Object(vec![])));
        spec.set("id", wire_string(id.clone()));
        spec.set("name", wire_string(name));
        if reference.is_none() {
            spec.set("api", wire_string("cursor-agent"));
            spec.set("provider", wire_string("cursor"));
        }
        spec.set(
            "baseUrl",
            wire_string(
                base_url
                    .cloned()
                    .or_else(|| reference.and_then(|r| r.get("baseUrl")).and_then(WireValue::as_string).cloned())
                    .unwrap_or_else(|| CURSOR_DEFAULT_BASE_URL.into()),
            ),
        );
        spec.set("reasoning", WireValue::Bool(reasoning));
        if reference.is_none() {
            let input = resolve_cursor_input(&id, None)?;
            spec.set("input", WireValue::Array(input.iter().cloned().map(wire_string).collect()));
            spec.set("cost", zero_cost());
        }
        spec.set("contextWindow", context_window);
        if reference.is_none() {
            spec.set("maxTokens", WireValue::Number(64000.0));
        }
        spec.set("cursorMaxMode", WireValue::Bool(max_mode));
        if let Some(i) = output.iter().position(|v| model_string(v, "id") == id) {
            output[i] = Arc::new(spec);
        } else {
            output.push(Arc::new(spec));
        }
    }
    context.sort_by_id(&mut output);
    Ok(output)
}
