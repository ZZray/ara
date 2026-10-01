//! Fixed catalog/discovery/openai-compatible.ts, including injected mapper and
//! filter order, authoritative empty results and caller-owned abort signals.

use super::*;
use std::collections::HashMap;

pub const DEFAULT_OPENAI_COMPATIBLE_DISCOVERY_TIMEOUT_MS: f64 = 10_000.0;

#[derive(Debug, Clone)]
pub struct OpenAiMapperContext {
    pub api: WireString,
    pub provider: WireString,
    pub base_url: WireString,
}
pub type OpenAiModelMapper =
    Arc<dyn Fn(&VariantSpec, SpecRef, &OpenAiMapperContext) -> Result<Option<SpecRef>, DiscoveryError> + Send + Sync>;
pub type OpenAiModelFilter = Arc<dyn Fn(&VariantSpec, &SpecRef) -> Result<bool, DiscoveryError> + Send + Sync>;

#[derive(Clone)]
pub struct OpenAiCompatibleOptions {
    pub api: WireString,
    pub provider: WireString,
    pub base_url: WireString,
    pub api_key: Option<WireString>,
    pub headers: Vec<(WireString, WireString)>,
    pub signal: Option<DiscoverySignal>,
    pub timeout_ms: Option<f64>,
    pub map_model: Option<OpenAiModelMapper>,
    pub filter_model: Option<OpenAiModelFilter>,
}
impl OpenAiCompatibleOptions {
    pub fn new(api: impl Into<WireString>, provider: impl Into<WireString>, base_url: impl Into<WireString>) -> Self {
        Self {
            api: api.into(),
            provider: provider.into(),
            base_url: base_url.into(),
            api_key: None,
            headers: Vec::new(),
            signal: None,
            timeout_ms: None,
            map_model: None,
            filter_model: None,
        }
    }
}

struct TimerGuard(tokio::task::JoinHandle<()>);
impl Drop for TimerGuard {
    fn drop(&mut self) {
        self.0.abort();
    }
}

pub async fn fetch_openai_compatible_models(
    context: &CatalogContext,
    options: &OpenAiCompatibleOptions,
) -> DiscoveryResult {
    let mut base_url = js_trim(&options.base_url);
    if base_url.is_empty() {
        return Ok(None);
    }
    if base_url.units().last() == Some(&u16::from(b'/')) {
        base_url = WireString::from_units(base_url.units()[..base_url.len() - 1].to_vec());
    }
    let mut headers: Vec<(WireString, WireString)> = vec![("Accept".into(), "application/json".into())];
    // Object spread overwrites only the same own key, before Fetch's Headers
    // converts differently-cased duplicate names into an appended header.
    for (key, value) in &options.headers {
        set_record_header(&mut headers, key.clone(), value.clone());
    }
    if let Some(key) = options.api_key.as_ref().filter(|key| !key.is_empty()) {
        let mut value = WireString::from("Bearer ");
        let mut units = value.units().to_vec();
        units.extend(key.units());
        value = WireString::from_units(units);
        set_record_header(&mut headers, "Authorization".into(), value);
    }
    let mut url = base_url.clone();
    url.append_str("/models");
    let (signal, timer) = if let Some(signal) = &options.signal {
        (signal.clone(), None)
    } else {
        let signal = DiscoverySignal::default();
        let timer_signal = signal.clone();
        let milliseconds = options.timeout_ms.unwrap_or(DEFAULT_OPENAI_COMPATIBLE_DISCOVERY_TIMEOUT_MS);
        // Bun's setTimeout coerces out-of-range, non-finite and sub-ms delays
        // to 1 ms rather than scheduling a multi-day Rust timer.
        let milliseconds = if milliseconds.is_finite() && (1.0..=2_147_483_647.0).contains(&milliseconds) {
            milliseconds.trunc() as u64
        } else {
            1
        };
        let timer = tokio::spawn(async move {
            tokio::time::sleep(std::time::Duration::from_millis(milliseconds)).await;
            timer_signal.abort(DiscoveryError::named("TimeoutError", "The operation timed out."));
        });
        (signal, Some(TimerGuard(timer)))
    };
    // The timer aborts the signal. An injected fetch may intentionally ignore
    // it and still fulfill, just as the fixed Promise wrapper does.
    let response =
        context.transport.fetch(DiscoveryRequest { url, headers, signal: Some(signal), ..Default::default() }).await;
    let payload = response.ok().filter(DiscoveryReply::ok).and_then(|reply| reply.json().ok());
    drop(timer);
    let Some(payload) = payload else {
        return Ok(None);
    };
    let Some(entries) = extract_model_entries(&payload) else {
        return Ok(None);
    };
    let mapper_context = OpenAiMapperContext { api: options.api.clone(), provider: options.provider.clone(), base_url };
    let mut models = Vec::new();
    let mut positions = HashMap::<WireString, usize>::new();
    for entry in entries {
        let id = model_string(&entry, "id");
        let name = entry
            .get("name")
            .and_then(WireValue::as_string)
            .filter(|name| !name.is_empty())
            .cloned()
            .unwrap_or_else(|| id.clone());
        let defaults = Arc::new(VariantSpec::from_wire(wire_object(&[
            ("id", wire_string(id)),
            ("name", wire_string(name)),
            ("api", wire_string(options.api.clone())),
            ("provider", wire_string(options.provider.clone())),
            ("baseUrl", wire_string(mapper_context.base_url.clone())),
            ("reasoning", WireValue::Bool(false)),
            ("input", WireValue::Array(vec![wire_string("text")])),
            ("cost", zero_cost()),
            ("contextWindow", WireValue::Null),
            ("maxTokens", WireValue::Null),
        ])));
        let mapped =
            if let Some(map) = &options.map_model { map(&entry, defaults, &mapper_context)? } else { Some(defaults) };
        let Some(mapped) = mapped else { continue };
        let Some(id) = mapped.get("id").and_then(WireValue::as_string).filter(|id| !id.is_empty()).cloned() else {
            continue;
        };
        if let Some(filter) = &options.filter_model
            && !filter(&entry, &mapped)?
        {
            continue;
        }
        if let Some(&index) = positions.get(&id) {
            models[index] = mapped;
        } else {
            positions.insert(id, models.len());
            models.push(mapped);
        }
    }
    context.sort_by_id(&mut models);
    Ok(Some(models))
}

pub fn set_record_header(headers: &mut Vec<(WireString, WireString)>, key: WireString, value: WireString) {
    if let Some((_, existing)) = headers.iter_mut().find(|(name, _)| name == &key) {
        *existing = value;
    } else {
        headers.push((key, value));
    }
}

fn child_spec(parent: &VariantSpec, key: &WireString) -> Option<VariantSpec> {
    let value = parent.get_path(std::slice::from_ref(key))?.clone();
    let undefined_paths =
        parent.undefined_paths.iter().filter(|path| path.first() == Some(key)).map(|path| path[1..].to_vec()).collect();
    Some(VariantSpec { value, undefined_paths })
}

fn extract_model_entries(payload: &VariantSpec) -> Option<Vec<VariantSpec>> {
    match &payload.value {
        WireValue::Array(items) => Some(
            items
                .iter()
                .enumerate()
                .filter_map(|(index, _)| {
                    let entry = child_spec(payload, &index.to_string().into())?;
                    if !matches!(entry.value, WireValue::Object(_)) {
                        return None;
                    }
                    if !entry.get("id").and_then(WireValue::as_string).is_some_and(|id| !id.is_empty()) {
                        return None;
                    }
                    if entry.get("name").is_some_and(|name| !matches!(name, WireValue::Null | WireValue::String(_))) {
                        return None;
                    }
                    Some(entry)
                })
                .collect(),
        ),
        WireValue::Object(_) => {
            for key in ["data", "models", "result", "items"] {
                if let Some(node) = child_spec(payload, &key.into())
                    && let Some(entries) = extract_model_entries(&node)
                {
                    return Some(entries);
                }
            }
            None
        }
        _ => None,
    }
}
