//! Fixed OMP 596f2da: catalog/src/discovery/codex.ts.
//! MIT License; Copyright (c) 2025 Mario Zechner; Copyright (c) 2025-2026 Can Bölük;
//! Copyright (c) 2026 Stencil Labs, Inc. See LICENSE for the full license.

use super::*;
use crate::model_collapse::lower;
use crate::{catalog_rules, model_identity_wire, model_wire_policy};
use std::collections::HashSet;

pub const CODEX_BASE_URL: &str = "https://chatgpt.com/backend-api";
pub const CODEX_CLIENT_VERSION: &str = "0.153.0";

#[derive(Clone)]
pub struct CodexModelDiscoveryOptions {
    pub access_token: WireString,
    pub account_id: Option<WireString>,
    pub base_url: Option<WireString>,
    pub client_version: Option<WireString>,
    pub paths: Option<Vec<WireString>>,
    pub headers: Vec<(WireString, WireString)>,
    pub signal: Option<DiscoverySignal>,
}
impl Default for CodexModelDiscoveryOptions {
    fn default() -> Self {
        Self {
            access_token: "".into(),
            account_id: None,
            base_url: None,
            client_version: None,
            paths: None,
            headers: Vec::new(),
            signal: None,
        }
    }
}
#[derive(Debug, Clone)]
pub struct CodexModelDiscoveryResult {
    pub models: Vec<SpecRef>,
    pub etag: Option<WireString>,
    pub rejected_status: Option<u16>,
}

pub async fn fetch_codex_models(
    context: &CatalogContext,
    options: &CodexModelDiscoveryOptions,
) -> Result<Option<CodexModelDiscoveryResult>, DiscoveryError> {
    let base = normalize_base_url(options.base_url.as_ref());
    let version = options
        .client_version
        .as_ref()
        .map(js_trim)
        .filter(|value| {
            let parts: Vec<_> = value.units().split(|unit| *unit == u16::from(b'.')).collect();
            parts.len() == 3
                && parts.iter().all(|part| !part.is_empty() && part.iter().all(|unit| (48..=57).contains(unit)))
        })
        .unwrap_or_else(|| CODEX_CLIENT_VERSION.into());
    let mut headers = DiscoveryHeaders::from_pairs(&options.headers)?;
    let mut bearer = WireString::from("Bearer ").units().to_vec();
    bearer.extend(options.access_token.units());
    headers.set(&"Authorization".into(), &WireString::from_units(bearer))?;
    if let Some(account) = options.account_id.as_ref().filter(|value| !js_trim(value).is_empty()) {
        headers.set(&"chatgpt-account-id".into(), account)?;
    }
    for (key, value) in
        [("OpenAI-Beta", "responses=experimental"), ("originator", "omp"), ("accept", "application/json")]
    {
        headers.set(&key.into(), &value.into())?;
    }
    headers.set(&"version".into(), &version)?;
    let mut paths: Vec<WireString> = options
        .paths
        .as_deref()
        .unwrap_or_default()
        .iter()
        .map(js_trim)
        .filter(|value| !value.is_empty())
        .map(|value| if value.units().first() == Some(&47) { value } else { concat(&"/".into(), &value) })
        .collect();
    if paths.is_empty() {
        paths = vec!["/codex/models".into(), "/models".into()];
    }
    for path in paths {
        // URL construction is outside the upstream fetch catch.
        let mut url = reqwest::Url::parse(&String::from_utf16_lossy(concat(&base, &path).units()))
            .map_err(|_| DiscoveryError::named("TypeError", "Invalid URL"))?;
        url.query_pairs_mut().append_pair("client_version", &String::from_utf16_lossy(version.units()));
        // URLSearchParams.set removes previous matching entries, preserving its first slot.
        let pairs: Vec<_> = url.query_pairs().map(|(key, value)| (key.into_owned(), value.into_owned())).collect();
        let mut seen = false;
        url.set_query(None);
        for (key, value) in pairs {
            if key == "client_version" {
                if seen {
                    continue;
                }
                seen = true;
                url.query_pairs_mut().append_pair(&key, &String::from_utf16_lossy(version.units()));
            } else {
                url.query_pairs_mut().append_pair(&key, &value);
            }
        }
        let Ok(response) = context
            .transport
            .fetch(DiscoveryRequest {
                url: url.as_str().into(),
                headers: headers.entries(),
                signal: options.signal.clone(),
                ..Default::default()
            })
            .await
        else {
            continue;
        };
        if matches!(response.status, 401 | 403) {
            return Ok(Some(CodexModelDiscoveryResult {
                models: Vec::new(),
                etag: None,
                rejected_status: Some(response.status),
            }));
        }
        if !response.ok() {
            continue;
        }
        let Ok(payload) = response.json() else {
            continue;
        };
        let Some(models) = normalize_codex_models(context, &payload, &base)? else {
            continue;
        };
        let etag = response.header("etag").map(js_trim).filter(|value| !value.is_empty());
        return Ok(Some(CodexModelDiscoveryResult { models, etag, rejected_status: None }));
    }
    Ok(None)
}

fn concat(left: &WireString, right: &WireString) -> WireString {
    let mut units = left.units().to_vec();
    units.extend(right.units());
    WireString::from_units(units)
}
fn normalize_base_url(value: Option<&WireString>) -> WireString {
    let value = value.map(js_trim).filter(|value| !value.is_empty()).unwrap_or_else(|| CODEX_BASE_URL.into());
    trim_trailing_slashes(&value)
}

fn nonempty(value: Option<&WireValue>) -> Option<WireString> {
    value.and_then(WireValue::as_string).map(js_trim).filter(|value| !value.is_empty())
}
fn finite(value: Option<&WireValue>) -> Option<f64> {
    value.and_then(WireValue::as_number).filter(|number| number.is_finite())
}
struct ParsedModel {
    slug: WireString,
    name: WireString,
    context: Option<f64>,
    reasoning: bool,
    input: Vec<WireValue>,
    websocket: bool,
    lite: bool,
    tool: bool,
    priority: f64,
}
fn parse_model(value: &WireValue) -> Option<ParsedModel> {
    if !value.is_object() {
        return None;
    }
    let slug = nonempty(value.get("slug")).or_else(|| nonempty(value.get("id")))?;
    if nonempty(value.get("visibility"))
        .map(|value| lower(&value))
        .is_some_and(|value| value.equals_ascii("hide") || value.equals_ascii("hidden"))
    {
        return None;
    }
    let active = |value: Option<&WireValue>| {
        nonempty(value).map(|value| lower(&value)).is_some_and(|value| !value.equals_ascii("none"))
    };
    let reasoning = active(value.get("default_reasoning_level"))
        || value
            .get("supported_reasoning_levels")
            .and_then(WireValue::as_array)
            .is_some_and(|levels| levels.iter().any(|level| level.is_object() && active(level.get("effort"))));
    let modalities = value
        .get("input_modalities")
        .and_then(WireValue::as_array)
        .map(|items| items.iter().filter_map(|item| nonempty(Some(item))).map(|item| lower(&item)).collect::<Vec<_>>());
    let mut input: Vec<_> = ["text", "image"]
        .iter()
        .filter(|mode| modalities.as_ref().is_none_or(|items| items.iter().any(|item| item.equals_ascii(mode))))
        .map(|mode| wire_string(*mode))
        .collect();
    if input.is_empty() {
        input = vec![wire_string("text"), wire_string("image")];
    }
    Some(ParsedModel {
        name: nonempty(value.get("display_name")).unwrap_or_else(|| slug.clone()),
        slug,
        context: finite(value.get("context_window")).filter(|value| *value > 0.0).map(f64::trunc),
        reasoning,
        input,
        websocket: matches!(value.get("prefer_websockets"), Some(WireValue::Bool(true))),
        lite: matches!(value.get("use_responses_lite"), Some(WireValue::Bool(true))),
        tool: value
            .get("tool_mode")
            .and_then(WireValue::as_string)
            .is_some_and(|value| value.equals_ascii("code_mode_only")),
        priority: finite(value.get("priority")).unwrap_or(9_007_199_254_740_991.0),
    })
}
pub fn normalize_codex_models(
    context: &CatalogContext,
    payload: &VariantSpec,
    base_url: &WireString,
) -> DiscoveryResult {
    // The fixed object schema accepts arrays without named envelope fields,
    // while an own optional field whose value is undefined fails unknown[].
    // Inspect raw own slots here; VariantSpec::get hides undefined slots.
    if !matches!(payload.value, WireValue::Object(_) | WireValue::Array(_))
        || ["models", "data"].iter().any(|key| {
            payload.undefined_paths.contains(&vec![(*key).into()])
                || payload.value.get(key).is_some_and(|value| value.as_array().is_none())
        })
    {
        return Ok(None);
    }
    let entries =
        payload.get("models").or_else(|| payload.get("data")).and_then(WireValue::as_array).unwrap_or_default();
    let parsed: Vec<_> = entries.iter().filter_map(parse_model).collect();
    let advertised: HashSet<_> = parsed.iter().map(|item| item.slug.clone()).collect();
    let bundled: HashSet<_> = model_identity_wire::bundled_provider_models(&"openai-codex".into())
        .iter()
        .map(|model| model_string(model, "id"))
        .collect();
    let mut normalized = Vec::new();
    for parsed in &parsed {
        let canonical = parsed
            .slug
            .units()
            .strip_suffix(&[45, 119, 109])
            .map(|units| WireString::from_units(units.to_vec()))
            .filter(|plain| !plain.is_empty() && bundled.contains(plain))
            .unwrap_or_else(|| parsed.slug.clone());
        normalized.push((parsed.priority, build_model(parsed, &parsed.slug, &canonical, base_url)?));
        if canonical != parsed.slug && !advertised.contains(&canonical) {
            normalized.push((parsed.priority, build_model(parsed, &canonical, &canonical, base_url)?));
        }
    }
    normalized.sort_by(|(lp, l), (rp, r)| {
        lp.partial_cmp(rp)
            .unwrap_or(Ordering::Equal)
            .then_with(|| context.locale.compare(&model_string(l, "id"), &model_string(r, "id")))
    });
    Ok(Some(normalized.into_iter().map(|(_, model)| model).collect()))
}
fn build_model(
    parsed: &ParsedModel,
    id: &WireString,
    canonical: &WireString,
    base: &WireString,
) -> Result<SpecRef, DiscoveryError> {
    let identity = model_wire_policy::classify_wire(&"openai-codex".into(), canonical, true)?;
    let revision =
        model_string(&identity, "revision").to_utf8().ok().and_then(|value| catalog_rules::parse_revision(&value));
    let fallback = if model_string(&identity, "class").equals_ascii("openai")
        && revision
            .zip(catalog_rules::parse_revision("5.6"))
            .is_some_and(|(a, b)| catalog_rules::compare_revision(a, b) == 0)
    {
        372_000.0
    } else {
        272_000.0
    };
    let mut window = parsed.context.unwrap_or(fallback);
    if ["gpt-5.6-luna", "gpt-5.6-sol", "gpt-5.6-terra"].iter().any(|id| canonical.equals_ascii(id)) {
        window = window.max(1_000_000.0);
    }
    let mut model = VariantSpec::from_wire(wire_object(&[
        ("id", wire_string(id.clone())),
        ("name", wire_string(parsed.name.clone())),
        ("api", wire_string("openai-codex-responses")),
        ("provider", wire_string("openai-codex")),
        ("baseUrl", wire_string(base.clone())),
        ("reasoning", WireValue::Bool(parsed.reasoning)),
        ("input", WireValue::Array(parsed.input.clone())),
        ("cost", zero_cost()),
        (
            "remoteCompaction",
            wire_object(&[
                ("enabled", WireValue::Bool(true)),
                ("api", wire_string("openai-codex-responses")),
                ("v2StreamingEnabled", WireValue::Bool(true)),
            ]),
        ),
        ("contextWindow", WireValue::Number(window)),
        ("maxTokens", WireValue::Number(window.min(128_000.0))),
    ]));
    if parsed.websocket {
        model.set("preferWebsockets", WireValue::Bool(true));
    }
    if parsed.lite {
        model.set("useResponsesLite", WireValue::Bool(true));
    }
    if parsed.tool {
        model.set("toolMode", wire_string("code_mode_only"));
    }
    if parsed.priority != 9_007_199_254_740_991.0 {
        model.set("priority", WireValue::Number(parsed.priority));
    }
    Ok(Arc::new(model))
}
