//! Fixed utils/fetch-retry.ts behavior exercised by the Ollama Cloud factory.
use super::common::to_number;
use crate::{
    catalog_discovery::{CatalogContext, DiscoveryError, DiscoveryReply, DiscoveryRequest},
    js_regex::JsRegExp,
};
use ara_rpc::{WireString, WireValue};
fn number(value: &WireString) -> Option<f64> {
    if crate::catalog_discovery::js_trim(value).is_empty() {
        Some(0.0)
    } else {
        to_number(Some(&WireValue::String(value.clone())))
    }
}
fn date(value: &str) -> Option<f64> {
    chrono::DateTime::parse_from_rfc2822(value)
        .or_else(|_| chrono::DateTime::parse_from_rfc3339(value))
        .ok()
        .map(|date| date.timestamp_millis() as f64)
}
fn unit(value: &WireString) -> Option<f64> {
    match value.to_utf8().ok()?.to_ascii_lowercase().as_str() {
        "ms" => Some(1.0),
        "s" | "sec" => Some(1000.0),
        "m" | "min" | "mins" | "minute" | "minutes" => Some(60000.0),
        "h" | "hr" | "hrs" | "hour" | "hours" => Some(3600000.0),
        _ => None,
    }
}
fn capture(body: &WireString, pattern: &str) -> Option<Vec<Option<WireString>>> {
    JsRegExp::new(pattern.into(), "i").expect("fixed retry regex").exec(body).map(|matched| matched.captures)
}
fn duration(body: &WireString, pattern: &str) -> Option<f64> {
    let captures = capture(body, pattern)?;
    let value = number(captures.get(1)?.as_ref()?)?;
    let factor = unit(captures.get(2)?.as_ref()?)?;
    (value > 0.0).then_some(value * factor)
}
pub fn extract_retry_hint(response: &DiscoveryReply, now: f64) -> Option<f64> {
    if let Some(value) =
        response.header("retry-after-ms").filter(|s| !s.is_empty()).and_then(number).filter(|n| *n >= 0.0)
    {
        return Some(value);
    }
    if let Some(value) = response.header("retry-after").filter(|s| !s.is_empty()) {
        if let Some(seconds) = number(value) {
            return Some((seconds * 1000.0).max(0.0));
        }
        if let Some(time) = value.to_utf8().ok().and_then(|value| date(&value)) {
            return Some((time - now).max(0.0));
        }
    }
    if let Some(value) =
        response.header("x-ratelimit-reset-ms").filter(|s| !s.is_empty()).and_then(number).filter(|n| *n > 0.0)
    {
        let target = if value > 1e12 {
            Some(value)
        } else if value > 1e9 {
            Some(value * 1000.0)
        } else {
            None
        };
        if let Some(target) = target {
            if target > now {
                return Some(target - now);
            }
        } else {
            return Some(value);
        }
    }
    if let Some(value) = response.header("x-ratelimit-reset").filter(|s| !s.is_empty()) {
        let text = value.to_utf8().ok()?;
        let text = text.trim_start();
        let digits = text.strip_prefix(['+', '-']).unwrap_or(text).bytes().take_while(u8::is_ascii_digit).count();
        if digits > 0 {
            let prefix = &text[..digits + usize::from(text.starts_with(['+', '-']))];
            if let Ok(reset) = prefix.parse::<f64>() {
                let delta = reset * 1000.0 - now;
                if delta > 0.0 {
                    return Some(delta);
                }
            }
        }
    }
    if let Some(value) =
        response.header("x-ratelimit-reset-after").filter(|s| !s.is_empty()).and_then(number).filter(|n| *n > 0.0)
    {
        return Some(value * 1000.0);
    }
    let body = response.text();
    if body.is_empty() {
        return None;
    }
    if let Some(captures) = capture(&body, r"reset after (?:(\d+)h)?(?:(\d+)m)?(\d+(?:\.\d+)?)s") {
        let part = |index: usize| captures.get(index).and_then(|value| value.as_ref()).and_then(number).unwrap_or(0.0);
        let value = ((part(1) * 60.0 + part(2)) * 60.0 + part(3)) * 1000.0;
        if value > 0.0 {
            return Some(value);
        }
    }
    for pattern in [
        r"(?:will\s+)?reset at\s+([0-9]{4}-[0-9]{2}-[0-9]{2}[ T][0-9]{2}:[0-9]{2}:[0-9]{2}(?:\.[0-9]+)?(?:Z|[+-][0-9]{2}:?[0-9]{2})?)",
        r"将在\s*([0-9]{4}-[0-9]{2}-[0-9]{2}\s+[0-9]{2}:[0-9]{2}:[0-9]{2})\s*重置",
    ] {
        if let Some(value) =
            capture(&body, pattern).and_then(|v| v.get(1).cloned().flatten()).and_then(|s| s.to_utf8().ok())
        {
            let mut value = value.replacen(' ', "T", 1);
            if !value.ends_with('Z') && !value[10..].contains(['+', '-']) {
                value.push('Z');
            }
            if let Some(time) = date(&value).filter(|time| *time > now) {
                return Some(time - now);
            }
        }
    }
    if let Some(value) =
        duration(&body, r"(?:will\s+)?reset in\s+~?\s*([0-9.]+)\s*(ms|sec|s|minutes?|mins?|m|hours?|hrs?|h)\b")
    {
        return Some(value);
    }
    if let Some(value) = capture(&body, r"\bretry-after-ms=([0-9]+)\b")
        .and_then(|v| v.get(1).cloned().flatten())
        .as_ref()
        .and_then(number)
        .filter(|value| *value > 0.0)
    {
        return Some(value);
    }
    for pattern in [
        r"Please retry in ([0-9.]+)(ms|s)",
        r#""retryDelay":\s*"([0-9.]+)(ms|s)""#,
        r"try again in\s+~?\s*([0-9.]+)\s*(ms|sec|s|minutes?|mins?|m|hours?|hrs?|h)\b",
    ] {
        if let Some(value) = duration(&body, pattern) {
            return Some(value);
        }
    }
    None
}
pub async fn fetch_with_ollama_retry(
    context: &CatalogContext,
    mut request: DiscoveryRequest,
) -> Result<DiscoveryReply, DiscoveryError> {
    request.body_policy = crate::catalog_discovery::DiscoveryBodyPolicy::Always;
    for attempt in 0..5 {
        let response = context.transport.fetch(request.clone()).await;
        let default = [2000.0, 5000.0, 10000.0][attempt.min(2)];
        let delay = match response {
            Ok(response) => {
                if !(response.status >= 500 || response.status == 408 || response.status == 429) || attempt == 4 {
                    return Ok(response);
                }
                let hint = extract_retry_hint(&response, chrono::Utc::now().timestamp_millis() as f64);
                if hint.is_some_and(|hint| hint > 60000.0) {
                    return Ok(response);
                }
                hint.unwrap_or(default).min(60000.0)
            }
            Err(error) => {
                if attempt == 4 {
                    return Err(
                        if error.name.equals_ascii("AbortError") || error.message.equals_ascii("Request was aborted") {
                            DiscoveryError::new("Request was aborted")
                        } else {
                            error
                        },
                    );
                }
                default
            }
        };
        tokio::time::sleep(std::time::Duration::from_millis(delay.max(0.0).trunc() as u64)).await;
    }
    unreachable!("bounded retry loop returns")
}
