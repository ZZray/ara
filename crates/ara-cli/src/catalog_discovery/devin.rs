//! Fixed OMP Devin model discovery, native metadata and unary protobuf/gzip.
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
    CatalogContext, DiscoveryError, DiscoveryRequest, DiscoveryResult, DiscoverySignal, HttpMethod, js_trim,
    trim_trailing_slashes, wire_string, zero_cost,
};
use crate::{
    catalog_proto_schemas::devin::{
        DisplayOption, GetCliModelConfigsRequestSchema, GetCliModelConfigsResponseSchema, MetadataSchema,
        ModelDimensionKind,
    },
    catalog_protobuf::{ByteView, MessageCodec, ProtoMessage, ProtoValue},
    js_regex::JsRegExp,
    model_collapse::{SpecRef, VariantCollapseTable, VariantSpec, lower},
};
use ara_rpc::{WireString, WireValue};
use std::{collections::HashSet, io::Read, sync::Arc};

pub const DEVIN_DEFAULT_BASE_URL: &str = "https://server.codeium.com";
pub const DEVIN_GET_CLI_MODEL_CONFIGS_PATH: &str = "/exa.api_server_pb.ApiServerService/GetCliModelConfigs";
const EFFORTS: [&str; 6] = ["minimal", "low", "medium", "high", "xhigh", "max"];
#[derive(Clone, Default)]
pub struct DevinModelDiscoveryOptions {
    pub api_key: Option<WireString>,
    pub base_url: Option<WireString>,
    pub timeout_ms: Option<f64>,
    pub signal: Option<DiscoverySignal>,
}
pub fn normalize_devin_session_token(api_key: Option<&WireString>) -> WireString {
    let Some(key) = api_key.filter(|key| !key.is_empty()) else { return "".into() };
    let prefix = WireString::from("devin-session-token$");
    if key.units().starts_with(prefix.units()) {
        key.clone()
    } else {
        WireString::from_units([prefix.units(), key.units()].concat())
    }
}
fn native_os() -> &'static str {
    if cfg!(target_os = "macos") {
        "darwin"
    } else if cfg!(target_os = "windows") {
        "windows"
    } else {
        "linux"
    }
}
pub fn devin_cli_metadata(api_key: Option<&WireString>, user_jwt: Option<&WireString>) -> ProtoMessage {
    ProtoMessage::from_entries(&[
        ("apiKey", ProtoValue::String(normalize_devin_session_token(api_key))),
        ("userJwt", ProtoValue::String(user_jwt.cloned().unwrap_or_else(|| "".into()))),
        ("ideName", ProtoValue::string("devin-cli")),
        ("ideType", ProtoValue::string("chisel")),
        ("ideVersion", ProtoValue::string("3000.6.2")),
        ("extensionName", ProtoValue::string("chisel")),
        ("extensionVersion", ProtoValue::string("3000.6.2")),
        ("locale", ProtoValue::string("en")),
        ("os", ProtoValue::string(native_os())),
    ])
}
pub fn devin_discovery_metadata(api_key: Option<&WireString>) -> ProtoMessage {
    ProtoMessage::from_entries(&[
        ("apiKey", ProtoValue::String(normalize_devin_session_token(api_key))),
        ("ideName", ProtoValue::string("chisel")),
        ("ideVersion", ProtoValue::string("0.0.0-dev")),
        ("extensionName", ProtoValue::string("chisel")),
        ("extensionVersion", ProtoValue::string("0.0.0-dev")),
        ("locale", ProtoValue::string("en")),
        ("os", ProtoValue::string(native_os())),
    ])
}
pub fn decode_devin_unary_message(schema: &MessageCodec, payload: &ByteView) -> Option<ProtoMessage> {
    if let Ok(value) = schema.decode(payload) {
        return Some(value);
    }
    let bytes = payload.bytes();
    let mut decoder = flate2::read::MultiGzDecoder::new(bytes.as_slice());
    let mut decoded = vec![];
    decoder.read_to_end(&mut decoded).ok()?;
    schema.decode(&ByteView::new(decoded)).ok()
}
pub async fn fetch_devin_models(context: &CatalogContext, options: &DevinModelDiscoveryOptions) -> DiscoveryResult {
    match fetch_inner(context, options).await {
        Ok(v) => Ok(v),
        Err(_) => Ok(None),
    }
}
async fn fetch_inner(context: &CatalogContext, options: &DevinModelDiscoveryOptions) -> DiscoveryResult {
    let controller = DiscoverySignal::default();
    let signal = options
        .signal
        .as_ref()
        .map_or_else(|| controller.clone(), |caller| DiscoverySignal::any(&[controller.clone(), caller.clone()]));
    let timeout_signal = controller;
    let duration = super::cursor::timer_duration(options.timeout_ms.unwrap_or(5000.0));
    struct Timers(Vec<tokio::task::JoinHandle<()>>);
    impl Drop for Timers {
        fn drop(&mut self) {
            for task in &self.0 {
                task.abort();
            }
        }
    }
    let _timers = Timers(vec![tokio::spawn(async move {
        tokio::time::sleep(duration).await;
        timeout_signal.abort(DiscoveryError::named("AbortError", "This operation was aborted."));
    })]);
    let metadata = devin_discovery_metadata(options.api_key.as_ref());
    metadata.set(
        "supportedModelDisplays",
        ProtoValue::array([3, 4, 6, 7, 8].map(|v| ProtoValue::Number(v.into())).to_vec()),
    );
    let metadata = MetadataSchema.create(Some(&metadata));
    let request = GetCliModelConfigsRequestSchema
        .create(Some(&ProtoMessage::from_entries(&[("metadata", ProtoValue::Object(metadata))])));
    let body =
        GetCliModelConfigsRequestSchema.encode(&request).map_err(|e| DiscoveryError::named(e.name, e.message))?.bytes();
    let base = options.base_url.clone().unwrap_or_else(|| DEVIN_DEFAULT_BASE_URL.into());
    let mut url = trim_trailing_slashes(&base);
    url.append_str(DEVIN_GET_CLI_MODEL_CONFIGS_PATH);
    let req = DiscoveryRequest {
        url,
        method: HttpMethod::Post,
        headers: vec![
            ("content-type".into(), "application/proto".into()),
            ("connect-protocol-version".into(), "1".into()),
            ("accept".into(), "*/*".into()),
        ],
        body: Some(body),
        signal: Some(signal),
        http2: false,
        tls: None,
        native_init: None,
        body_policy: super::DiscoveryBodyPolicy::default(),
    };
    let reply = context.transport.fetch(req).await?;
    if !reply.ok() {
        return Ok(None);
    }
    let Some(decoded) =
        decode_devin_unary_message(&GetCliModelConfigsResponseSchema.codec(), &ByteView::new(reply.body))
    else {
        return Ok(None);
    };
    let models =
        normalize_devin_models(context, &decoded.get("clientModelConfigs").items(), options.base_url.as_ref())?;
    // OMP warns with credential-free native metadata; no secret enters the log.
    if models.is_empty() {
        let fields = VariantSpec::from_wire(WireValue::object(vec![(
            "metadata",
            devin_discovery_metadata(None).to_wire().map_err(|e| DiscoveryError::named(e.name, e.message))?,
        )]));
        context.warn("Devin returned an empty native model catalog; the pinned CLI identity may be stale", fields);
        return Ok(None);
    }
    Ok(Some(models))
}
fn string(m: &ProtoMessage, key: &str) -> WireString {
    m.get(key).as_string().unwrap_or_else(|| "".into())
}
fn number(m: &ProtoMessage, key: &str) -> f64 {
    m.get(key).as_number().unwrap_or(0.0)
}
fn boolean(m: &ProtoMessage, key: &str) -> bool {
    matches!(m.get(key), ProtoValue::Bool(true))
}
fn object(m: &ProtoMessage, key: &str) -> Option<ProtoMessage> {
    m.get(key).as_object()
}
fn regex_test(pattern: &str, input: &WireString) -> bool {
    JsRegExp::new(pattern.into(), "i").expect("fixed regex").exec(input).is_some()
}
fn supports_thinking(config: &ProtoMessage) -> bool {
    if let Some(features) = object(config, "modelInfo").and_then(|info| object(&info, "modelFeatures")) {
        return boolean(&features, "supportsThinking");
    }
    let label = string(config, "label");
    !regex_test("\\bno thinking\\b", &label)
        && regex_test("think|thinking|minimal|high|medium|low|xhigh|max|reasoning", &label)
}
fn denominator_tokens(text: &WireString) -> f64 {
    let Some(m) = JsRegExp::new("(\\d+(?:\\.\\d+)?)\\s*([kmb])?".into(), "i").expect("fixed regex").exec(text) else {
        return 1000000.0;
    };
    let count = m
        .captures
        .get(1)
        .and_then(|v| v.as_ref())
        .and_then(|v| v.to_utf8().ok())
        .and_then(|v| v.parse::<f64>().ok())
        .unwrap_or(f64::NAN);
    let suffix = m.captures.get(2).and_then(|v| v.as_ref()).map(lower);
    let scale = match suffix.as_ref().and_then(|s| s.to_utf8().ok()).as_deref() {
        Some("k") => 1000.0,
        Some("m") => 1000000.0,
        Some("b") => 1000000000.0,
        _ => 1.0,
    };
    let n = count * scale;
    if n > 0.0 { n } else { 1000000.0 }
}
fn model_cost(config: &ProtoMessage) -> WireValue {
    let mut cost = VariantSpec::from_wire(zero_cost());
    for dim in config.get("modelDimensions").items() {
        let Some(dim) = dim.as_object() else { continue };
        let kind = number(&dim, "kind");
        if kind != f64::from(ModelDimensionKind::COST) && kind != f64::from(ModelDimensionKind::COST_FUZZY) {
            continue;
        }
        let n = ((number(&dim, "value") * 1000000.0) / denominator_tokens(&string(&dim, "denominator"))) * 1e6;
        let rounded = if n.is_finite() && n.fract() != 0.0 {
            let floor = n.floor();
            let v = if n - floor < 0.5 { floor } else { floor + 1.0 };
            if v == 0.0 && n < 0.0 { -0.0 } else { v }
        } else {
            n
        };
        let per_million = rounded / 1e6;
        let label = lower(&js_trim(&string(&dim, "label")));
        let key = if label.equals_ascii("input") {
            Some("input")
        } else if label.equals_ascii("cached input") {
            Some("cacheRead")
        } else if label.equals_ascii("output") {
            Some("output")
        } else {
            None
        };
        if let Some(key) = key {
            cost.set(key, WireValue::Number(per_million));
        }
    }
    cost.value
}
fn model_spec(config: &ProtoMessage, uid: &WireString, base: &WireString, router: bool) -> VariantSpec {
    let info = object(config, "modelInfo");
    let features = info.as_ref().and_then(|i| object(i, "modelFeatures"));
    let images = features.as_ref().map_or_else(|| boolean(config, "supportsImages"), |f| boolean(f, "supportsImages"))
        && !uid.equals_ascii("swe-1-6")
        && !uid.equals_ascii("swe-1-6-fast");
    let mut spec = VariantSpec::from_wire(WireValue::Object(vec![]));
    spec.set("id", wire_string(uid.clone()));
    let label = js_trim(&string(config, "label"));
    spec.set("name", wire_string(if label.is_empty() { uid.clone() } else { label }));
    spec.set("api", wire_string("devin-agent"));
    spec.set("provider", wire_string("devin"));
    spec.set("baseUrl", wire_string(base.clone()));
    spec.set("reasoning", WireValue::Bool(supports_thinking(config)));
    spec.set(
        "input",
        WireValue::Array(if images {
            vec![wire_string("text"), wire_string("image")]
        } else {
            vec![wire_string("text")]
        }),
    );
    spec.set("supportsTools", WireValue::Bool(features.as_ref().is_none_or(|f| boolean(f, "supportsToolCalls"))));
    spec.set("cost", model_cost(config));
    let context = number(config, "maxTokens");
    spec.set("contextWindow", WireValue::Number(if context > 0.0 { context } else { 200000.0 }));
    let output = info.as_ref().map_or(0.0, |i| number(i, "maxOutputTokens"));
    spec.set("maxTokens", WireValue::Number(if output > 0.0 { output } else { 64000.0 }));
    let mut compat = VariantSpec::from_wire(WireValue::Object(vec![]));
    if router {
        compat.set("modelRouter", WireValue::Bool(true));
    }
    if features.as_ref().is_some_and(|f| boolean(f, "supportsParallelToolCalls")) {
        compat.set("supportsParallelToolCalls", WireValue::Bool(true));
    }
    if !compat.value.entries().unwrap_or_default().is_empty() {
        spec.set("compat", compat.value);
    }
    let desc = js_trim(&string(config, "description"));
    if !desc.is_empty() {
        spec.set("description", wire_string(desc));
    }
    for badge in ["isNew", "isBeta", "isRecommended"] {
        if boolean(config, badge) {
            spec.set(badge, WireValue::Bool(true));
        }
    }
    spec
}
struct Lane {
    id: WireString,
    name: WireString,
    members: Vec<WireString>,
    default_member: Option<WireString>,
    routing: Vec<(&'static str, WireString)>,
}
impl Default for Lane {
    fn default() -> Self {
        Self { id: "".into(), name: "".into(), members: vec![], default_member: None, routing: vec![] }
    }
}
fn ascii_token(value: &WireString, separator: Option<u16>) -> WireString {
    let value = lower(value);
    let mut out = vec![];
    let mut gap = false;
    for &u in value.units() {
        if matches!(u,97..=122|48..=57) {
            if gap
                && !out.is_empty()
                && let Some(s) = separator
            {
                out.push(s);
            }
            out.push(u);
            gap = false;
        } else {
            gap = true;
        }
    }
    WireString::from_units(out)
}
fn collect_lane(lanes: &mut Vec<Lane>, config: &ProtoMessage, uid: &WireString) {
    let Some(metadata) = object(config, "modelFamilyMetadata") else { return };
    let label = js_trim(&string(&metadata, "modelFamilyLabel"));
    if label.is_empty() {
        return;
    }
    let (mut effort, mut thinking, mut fast, mut million) = (None, None, false, false);
    for entry in metadata.get("entries").items() {
        let Some(entry) = entry.as_object() else { continue };
        let Some(value) = object(&entry, "value") else { continue };
        let key = ascii_token(&string(&entry, "key"), Some(32));
        if key.equals_ascii("fast mode") {
            fast = number(&value, "order") == 1.0;
        } else if key.equals_ascii("thinking") {
            thinking = Some(number(&value, "order") == 1.0);
        } else if key.equals_ascii("1m context") {
            million = number(&value, "order") == 1.0;
        } else if key.equals_ascii("effort") || key.equals_ascii("reasoning effort") {
            let name = ascii_token(&string(&value, "name"), None);
            effort = if name.equals_ascii("none") || name.equals_ascii("nothinking") {
                Some("off")
            } else {
                EFFORTS.iter().copied().find(|e| name.equals_ascii(e))
            };
        }
    }
    if thinking == Some(false) {
        effort = Some("off");
    }
    let mut id = ascii_token(&label, Some(45));
    if id.is_empty() {
        return;
    }
    if million {
        id.append_str("-1m");
    }
    if fast {
        id.append_str("-fast");
    }
    let index = lanes.iter().position(|v| v.id == id).unwrap_or_else(|| {
        let mut name = label.clone();
        if million {
            name.append_str(" 1M");
        }
        if fast {
            name.append_str(" Fast");
        }
        lanes.push(Lane { id: id.clone(), name, ..Lane::default() });
        lanes.len() - 1
    });
    let lane = &mut lanes[index];
    lane.members.push(uid.clone());
    if lane.default_member.is_none()
        && (boolean(config, "isDefaultModelInFamily") || boolean(&metadata, "isDefaultModelInFamily"))
    {
        lane.default_member = Some(uid.clone());
    }
    if let Some(e) = effort
        && !lane.routing.iter().any(|(k, _)| *k == e)
    {
        lane.routing.push((e, uid.clone()));
    }
}
fn dynamic_families(lanes: &[Lane]) -> Vec<VariantSpec> {
    let mut families = vec![];
    for lane in lanes {
        let efforts: Vec<_> = EFFORTS.iter().copied().filter(|e| lane.routing.iter().any(|(k, _)| k == e)).collect();
        if efforts.is_empty() {
            continue;
        }
        let members = if let Some(default) = &lane.default_member {
            let mut out = vec![default.clone()];
            out.extend(lane.members.iter().filter(|id| *id != default).cloned());
            out
        } else {
            lane.members.clone()
        };
        let routing =
            WireValue::Object(lane.routing.iter().map(|(k, v)| ((*k).into(), wire_string(v.clone()))).collect());
        let mut thinking = VariantSpec::from_wire(WireValue::Object(vec![]));
        thinking.set("mode", wire_string("effort"));
        thinking.set("efforts", WireValue::Array(efforts.iter().copied().map(wire_string).collect()));
        if let Some(default) = &lane.default_member
            && let Some(level) =
                efforts.iter().find(|effort| lane.routing.iter().any(|(e, id)| e == *effort && id == default))
        {
            thinking.set("defaultLevel", wire_string(*level));
        }
        if !lane.routing.iter().any(|(k, _)| *k == "off") {
            thinking.set("requiresEffort", WireValue::Bool(true));
        }
        let mut family = VariantSpec::from_wire(WireValue::Object(vec![]));
        family.set("id", wire_string(lane.id.clone()));
        family.set("name", wire_string(lane.name.clone()));
        family.set("members", WireValue::Array(members.into_iter().map(wire_string).collect()));
        family.set("routing", routing);
        if let Some(default) = &lane.default_member {
            family.set("defaultMember", wire_string(default.clone()));
        }
        family.set("thinking", thinking.value);
        families.push(family);
    }
    families
}
pub fn normalize_devin_models(
    context: &CatalogContext,
    configs: &[ProtoValue],
    base_override: Option<&WireString>,
) -> Result<Vec<SpecRef>, DiscoveryError> {
    let base = base_override.cloned().unwrap_or_else(|| DEVIN_DEFAULT_BASE_URL.into());
    let mut specs = vec![];
    let mut seen = HashSet::new();
    let mut lanes = vec![];
    for config in configs {
        let Some(config) = config.as_object() else { continue };
        if boolean(&config, "disabled") {
            continue;
        }
        let info = object(&config, "modelInfo");
        let display = info.as_ref().map_or(0.0, |i| number(i, "displayOption"));
        if display == 4.0 || display == 6.0 {
            continue;
        }
        let uid = js_trim(&string(&config, "modelUid"));
        if uid.is_empty() || !seen.insert(uid.clone()) {
            continue;
        }
        let router = display == f64::from(DisplayOption::MODEL_ROUTER)
            || info.as_ref().is_some_and(|i| boolean(i, "isModelRouter"));
        specs.push(Arc::new(model_spec(&config, &uid, &base, router)));
        if !router {
            collect_lane(&mut lanes, &config, &uid);
        }
    }
    let families = dynamic_families(&lanes);
    let mut runtime = context.runtime.collapse.lock().expect("discovery collapse poisoned");
    let dynamic = if families.is_empty() {
        specs
    } else {
        runtime.collapse_variants(&specs, Some(&Arc::new(VariantCollapseTable::new(families))))?
    };
    let mut collapsed = runtime.collapse_variants(&dynamic, None)?;
    drop(runtime);
    context.sort_by_id(&mut collapsed);
    Ok(collapsed)
}
