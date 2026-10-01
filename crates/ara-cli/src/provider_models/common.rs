use ara_rpc::{WireString, WireValue};
use async_trait::async_trait;
use std::{future::Future, sync::Arc};

use super::descriptor_types::ModelManagerConfig;
use crate::{
    catalog_discovery::{CatalogContext, DiscoveryError, DiscoveryReply, DiscoveryRequest, DiscoverySignal, js_trim},
    model_collapse::VariantSpec,
    model_identity_wire::{copy_field, str_value, text},
    model_manager::{DynamicModelFetcher, RawModelValue},
};
pub struct FnDynamic<F>(F);
#[async_trait]
impl<F, Fut> DynamicModelFetcher for FnDynamic<F>
where
    F: Fn() -> Fut + Send + Sync,
    Fut: Future<Output = Result<RawModelValue, DiscoveryError>> + Send,
{
    async fn fetch(&self) -> Result<RawModelValue, DiscoveryError> {
        (self.0)().await
    }
}
pub fn dynamic<F, Fut>(callback: F) -> Arc<dyn DynamicModelFetcher>
where
    F: Fn() -> Fut + Send + Sync + 'static,
    Fut: Future<Output = Result<RawModelValue, DiscoveryError>> + Send + 'static,
{
    Arc::new(FnDynamic(callback))
}
pub fn context(default: &CatalogContext, config: &ModelManagerConfig) -> CatalogContext {
    config.context.as_ref().unwrap_or(default).clone()
}
pub fn to_number(value: Option<&WireValue>) -> Option<f64> {
    match value? {
        WireValue::Number(number) if number.is_finite() => Some(*number),
        WireValue::String(value) => {
            let value = js_trim(value);
            if value.is_empty() {
                return None;
            }
            let text = value.to_utf8().ok()?;
            let radix = if text.starts_with("0x") || text.starts_with("0X") {
                Some(16)
            } else if text.starts_with("0b") || text.starts_with("0B") {
                Some(2)
            } else if text.starts_with("0o") || text.starts_with("0O") {
                Some(8)
            } else {
                None
            };
            let n = if let Some(radix) = radix {
                let digits = &text[2..];
                if digits.is_empty() {
                    return None;
                }
                let mut number = 0.0;
                for c in digits.chars() {
                    number = number * f64::from(radix) + f64::from(c.to_digit(radix)?);
                }
                number
            } else {
                text.parse().ok()?
            };
            n.is_finite().then_some(n)
        }
        _ => None,
    }
}
pub fn positive(value: Option<&WireValue>, fallback: Option<&WireValue>) -> WireValue {
    to_number(value)
        .filter(|n| *n > 0.0)
        .map_or_else(|| fallback.cloned().unwrap_or(WireValue::Null), WireValue::Number)
}
pub fn name(value: Option<&WireValue>, fallback: &WireString) -> WireString {
    value
        .and_then(WireValue::as_string)
        .map(js_trim)
        .filter(|name| !name.is_empty())
        .unwrap_or_else(|| fallback.clone())
}
pub fn input(value: Option<&WireValue>) -> WireValue {
    WireValue::Array(
        if value.and_then(WireValue::as_array).is_some_and(|values| {
            values.iter().any(|value| value.as_string().is_some_and(|value| value.equals_ascii("image")))
        }) {
            vec![str_value("text"), str_value("image")]
        } else {
            vec![str_value("text")]
        },
    )
}
pub fn trim_one_slash(value: &WireString) -> WireString {
    if value.units().last() == Some(&47) { value.slice_prefix(value.len() - 1) } else { value.clone() }
}
pub fn append(value: &WireString, suffix: &str) -> WireString {
    let mut value = value.clone();
    value.append_str(suffix);
    value
}
pub fn replace_first(value: &WireString, needle: &WireString, replacement: &WireString) -> WireString {
    let at =
        if needle.is_empty() { Some(0) } else { value.units().windows(needle.len()).position(|s| s == needle.units()) };
    let Some(at) = at else {
        return value.clone();
    };
    let mut units = value.units()[..at].to_vec();
    units.extend_from_slice(replacement.units());
    units.extend_from_slice(&value.units()[at + needle.len()..]);
    WireString::from_units(units)
}
pub fn ends(value: &WireString, suffix: &str) -> bool {
    value.units().ends_with(&suffix.encode_utf16().collect::<Vec<_>>())
}
pub fn starts(value: &WireString, prefix: &str) -> bool {
    value.units().starts_with(&prefix.encode_utf16().collect::<Vec<_>>())
}
pub fn includes(value: &WireString, needle: &str) -> bool {
    let needle: Vec<_> = needle.encode_utf16().collect();
    value.units().windows(needle.len()).any(|window| window == needle)
}
pub fn js_value_string(value: &WireValue) -> WireString {
    match value {
        WireValue::String(s) => s.clone(),
        WireValue::Null => "null".into(),
        WireValue::Bool(b) => {
            if *b {
                "true".into()
            } else {
                "false".into()
            }
        }
        WireValue::Number(n) => {
            if n.is_nan() {
                "NaN".into()
            } else if n.is_infinite() {
                if n.is_sign_negative() { "-Infinity".into() } else { "Infinity".into() }
            } else if *n == 0.0 {
                "0".into()
            } else {
                value.stringify().into()
            }
        }
        WireValue::Object(_) => "[object Object]".into(),
        WireValue::Array(a) => {
            let mut out = WireString::from("");
            for (i, v) in a.iter().enumerate() {
                if i > 0 {
                    out.append_str(",");
                }
                if !matches!(v, WireValue::Null) {
                    out = super::cache_provider_id::join(&out, &js_value_string(v));
                }
            }
            out
        }
    }
}
pub fn remove_regex_match(value: &WireString, pattern: &str, flags: &str) -> WireString {
    let mut regex = crate::js_regex::JsRegExp::new(pattern.into(), flags).expect("fixed regex");
    if let Some(m) = regex.exec(value) {
        let mut units = value.units()[..m.index].to_vec();
        units.extend_from_slice(&value.units()[m.end..]);
        WireString::from_units(units)
    } else {
        value.clone()
    }
}
pub fn math_round(value: f64) -> f64 {
    if !value.is_finite() || value == 0.0 {
        return value;
    }
    if (-0.5..0.0).contains(&value) {
        return -0.0;
    }
    let floor = value.floor();
    if value - floor < 0.5 { floor } else { floor + 1.0 }
}
pub(super) fn js_min(left: f64, right: f64) -> f64 {
    if left.is_nan() || right.is_nan() {
        f64::NAN
    } else if left == 0.0 && right == 0.0 && (left.is_sign_negative() || right.is_sign_negative()) {
        -0.0
    } else {
        left.min(right)
    }
}
pub fn parse_float(value: Option<&WireValue>) -> f64 {
    let text = value.filter(|v| !matches!(v, WireValue::Null)).map(js_value_string).unwrap_or_else(|| "0".into());
    let mut regex = crate::js_regex::JsRegExp::new(
        r"^[\s]*([+-]?(?:Infinity|(?:[0-9]+\.?[0-9]*|\.[0-9]+)(?:[eE][+-]?[0-9]+)?))".into(),
        "",
    )
    .expect("parseFloat regex");
    regex
        .exec(&text)
        .and_then(|m| m.captures.get(1).cloned().flatten())
        .and_then(|s| s.to_utf8().ok())
        .and_then(|s| s.parse().ok())
        .unwrap_or(f64::NAN)
}
pub fn map_with_bundled_reference(
    entry: &VariantSpec,
    defaults: &VariantSpec,
    reference: Option<&VariantSpec>,
) -> VariantSpec {
    let fallback =
        reference.and_then(|r| text(r, "name")).or_else(|| text(defaults, "name")).unwrap_or_else(|| "".into());
    let model_name = name(entry.get("name"), &fallback);
    let Some(reference) = reference else {
        let mut out = defaults.clone();
        out.set("name", str_value(model_name));
        return out;
    };
    let mut out = reference.clone();
    for key in ["id", "api", "provider", "baseUrl"] {
        copy_field(&mut out, key, defaults, key);
    }
    out.set("name", str_value(model_name));
    out.set("contextWindow", positive(entry.get("context_length"), reference.get("contextWindow")));
    out.set("maxTokens", positive(entry.get("max_completion_tokens"), reference.get("maxTokens")));
    out
}
pub fn thinking(mode: &str, efforts: &[&str]) -> VariantSpec {
    VariantSpec::from_wire(WireValue::object(vec![
        ("mode", str_value(mode)),
        ("efforts", WireValue::Array(efforts.iter().map(|v| str_value(*v)).collect())),
    ]))
}
pub(super) struct DiscoveryTimeoutTimer(tokio::task::JoinHandle<()>);
impl Drop for DiscoveryTimeoutTimer {
    fn drop(&mut self) {
        self.0.abort();
    }
}
pub(super) fn start_discovery_timeout(signal: &DiscoverySignal, milliseconds: f64) -> DiscoveryTimeoutTimer {
    let timer_signal = signal.clone();
    let duration = crate::catalog_discovery::cursor::timer_duration(milliseconds);
    DiscoveryTimeoutTimer(tokio::spawn(async move {
        tokio::time::sleep(duration).await;
        timer_signal.abort(DiscoveryError::named("TimeoutError", "The operation timed out."));
    }))
}
/// JavaScript starts an async call up to its first await before returning its
/// promise. Poll once on the caller so request order has that same boundary;
/// the spawned remainder keeps running if the caller stops awaiting it.
pub async fn eager_background<F>(future: F) -> tokio::task::JoinHandle<F::Output>
where
    F: Future + Send + 'static,
    F::Output: Send + 'static,
{
    let mut future = Box::pin(future);
    let ready = futures::future::poll_fn(|cx| {
        std::task::Poll::Ready(match future.as_mut().poll(cx) {
            std::task::Poll::Ready(value) => Some(value),
            std::task::Poll::Pending => None,
        })
    })
    .await;
    tokio::spawn(async move {
        match ready {
            Some(value) => value,
            None => future.await,
        }
    })
}
/// Start every callback before observing the first rejection, as Promise.all
/// does for an eagerly mapped array. Dropping a remaining join handle leaves
/// its already started promise running.
pub(super) async fn promise_all<T, F>(futures: impl IntoIterator<Item = F>) -> Result<Vec<T>, DiscoveryError>
where
    F: Future<Output = Result<T, DiscoveryError>> + Send + 'static,
    T: Send + 'static,
{
    let mut handles = Vec::new();
    for future in futures {
        handles.push(eager_background(future).await);
    }
    futures::future::try_join_all(
        handles
            .into_iter()
            .map(|handle| async move { handle.await.map_err(|error| DiscoveryError::new(error.to_string()))? }),
    )
    .await
}
pub async fn timed_fetch(
    context: &CatalogContext,
    mut request: DiscoveryRequest,
    milliseconds: f64,
) -> Result<DiscoveryReply, DiscoveryError> {
    let signal = DiscoverySignal::default();
    let timer = start_discovery_timeout(&signal, milliseconds);
    request.signal = Some(signal);
    let result = context.transport.fetch(request).await;
    drop(timer);
    result
}
