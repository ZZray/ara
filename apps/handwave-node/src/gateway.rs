//! Model selection follows the current HandWave Hub contract, never a literal "default" model.
use crate::wire::Frame;
use anyhow::{bail, ensure, Context, Result};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::{collections::BTreeMap, time::Duration};

#[derive(Clone, Debug, Default, Deserialize, Serialize, PartialEq)]
pub struct Route {
    pub model: String,
    pub protocol: String,
    pub binding: String,
    pub decision: String,
}

pub fn grant_route(frame: &Frame, grant: &Value, previous: &Route) -> Result<(Route, u32)> {
    let field = |key: &str| grant[key].as_str().unwrap_or_default().trim().to_string();
    if frame.smart {
        ensure!(
            frame.model.trim().is_empty() && frame.protocol.trim().is_empty(),
            "smart mode conflicts with explicit model/protocol"
        );
        ensure!(
            previous.model.is_empty() || !previous.binding.is_empty(),
            "smart mode cannot take over an existing explicit session; use a new state directory"
        );
        let route = Route {
            model: field("bound_model"),
            protocol: field("bound_protocol"),
            binding: field("binding_id"),
            decision: field("bound_decision"),
        };
        ensure!(
            !route.model.is_empty()
                && !route.protocol.is_empty()
                && !route.binding.is_empty()
                && !route.decision.is_empty(),
            "incomplete smart grant binding"
        );
        let expiry = chrono::DateTime::parse_from_rfc3339(&field("bound_decision_exp"))
            .context("invalid smart grant decision expiry")?;
        ensure!(expiry > chrono::Utc::now(), "smart decision expired");
        ensure!(grant["bound_tools"] == true, "smart grant does not allow required tools");
        let cap = grant["bound_max_output"]
            .as_u64()
            .filter(|n| *n > 0 && *n <= u32::MAX as u64)
            .context("invalid smart output bound")? as u32;
        ensure!(previous.binding.is_empty() || previous == &route, "smart binding changed; original session retained");
        Ok((route, cap.min(8192)))
    } else {
        ensure!(
            previous.binding.is_empty(),
            "session has a smart binding; continue in smart mode or use a new state directory"
        );
        let default = field("default_model");
        let model = match frame.model.trim() {
            "" | "default" => default.clone(),
            value => value.into(),
        };
        ensure!(!model.is_empty(), "no model selected and Hub has no default model; select a model explicitly");
        let protocol = if !frame.protocol.trim().is_empty() {
            frame.protocol.trim().into()
        } else if previous.model == model && !previous.protocol.is_empty() {
            previous.protocol.clone()
        } else if model == default {
            field("default_protocol")
        } else {
            String::new()
        };
        Ok((Route { model, protocol, ..Route::default() }, 8192))
    }
}

pub fn headers(route: &Route, run: &str) -> BTreeMap<String, String> {
    if route.binding.is_empty() {
        return BTreeMap::new();
    }
    BTreeMap::from([("X-HandWave-Run".into(), run.into()), ("X-HandWave-Binding".into(), route.binding.clone())])
}

pub async fn resolve(
    mut route: Route,
    endpoint: &str,
    token: &str,
    run: &str,
) -> Result<(Route, reqwest::Client, crate::model_caps::ModelCaps)> {
    let mut h = reqwest::header::HeaderMap::new();
    for (key, value) in headers(&route, run) {
        h.insert(reqwest::header::HeaderName::from_bytes(key.as_bytes())?, value.parse()?);
    }
    let client = reqwest::Client::builder()
        .connect_timeout(Duration::from_secs(20))
        .redirect(reqwest::redirect::Policy::none())
        .default_headers(h)
        .build()?;
    let mut response = client
        .get(format!("{}/models", endpoint.trim_end_matches('/')))
        .bearer_auth(token)
        .timeout(Duration::from_secs(20))
        .send()
        .await
        .map_err(|_| anyhow::anyhow!("Hub model directory request failed"))?;
    ensure!(
        response.status().is_success(),
        "Hub model directory returned HTTP {} (body withheld)",
        response.status().as_u16()
    );
    let mut bytes = Vec::new();
    while let Some(chunk) = response.chunk().await.map_err(|_| anyhow::anyhow!("Hub model directory read failed"))? {
        ensure!(bytes.len() + chunk.len() <= 2 * 1024 * 1024, "Hub model directory exceeds 2 MiB");
        bytes.extend_from_slice(&chunk);
    }
    let value: Value = serde_json::from_slice(&bytes).context("invalid Hub model directory JSON")?;
    let rows = value["data"].as_array().context("Hub model directory has no data array")?;
    let selected: Vec<_> = rows.iter().filter(|r| r["id"].as_str() == Some(&route.model)).collect();
    ensure!(selected.len() == 1, "selected model is missing or duplicated in Hub directory");
    let row = selected[0];
    let mut protocols: Vec<&str> = match row.get("protocols") {
        None | Some(Value::Null) => Vec::new(),
        Some(v) => v
            .as_array()
            .context("invalid model protocols")?
            .iter()
            .map(|p| {
                let value = p.as_str().context("invalid model protocol")?.trim();
                ensure!(!value.is_empty(), "empty model protocol");
                Ok(value)
            })
            .collect::<Result<_>>()?,
    };
    protocols.sort_unstable();
    protocols.dedup();
    if protocols.is_empty() {
        protocols.push("openai-completions");
    }
    if route.protocol.is_empty() {
        ensure!(protocols.len() == 1, "model has ambiguous protocols; select a protocol explicitly");
        route.protocol = protocols[0].into();
    }
    ensure!(protocols.contains(&route.protocol.as_str()), "selected protocol is not advertised by the Hub model");
    if !matches!(route.protocol.as_str(), "openai-completions" | "openrouter-chat") {
        bail!(
            "this compact node supports openai-completions/openrouter-chat; selected protocol is unsupported: {}",
            route.protocol
        );
    }
    let capabilities = crate::model_caps::ModelCaps::parse(row)?;
    Ok((route, client, capabilities))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    #[test]
    fn default_and_explicit_never_silently_change_protocol() {
        let grant = json!({"default_model":"actual", "default_protocol":"anthropic-messages"});
        let (route, _) = grant_route(&Frame::default(), &grant, &Route::default()).unwrap();
        assert_eq!(route.model, "actual");
        assert_eq!(route.protocol, "anthropic-messages");
        let frame = Frame { model: "other".into(), ..Frame::default() };
        assert!(grant_route(&frame, &grant, &Route::default()).unwrap().0.protocol.is_empty());
        assert!(grant_route(&Frame::default(), &json!({}), &Route::default()).is_err());
    }
    #[test]
    fn smart_requires_complete_trusted_tuple_and_preserves_identity() {
        let frame = Frame { smart: true, ..Frame::default() };
        let grant = json!({"binding_id":"pin","bound_model":"actual","bound_protocol":"openai-completions","bound_decision":"decision","bound_decision_exp":(chrono::Utc::now()+chrono::Duration::minutes(5)).to_rfc3339(),"bound_max_output":2000,"bound_tools":true});
        let (route, cap) = grant_route(&frame, &grant, &Route::default()).unwrap();
        assert_eq!(cap, 2000);
        assert_eq!(headers(&route, "run")["X-HandWave-Run"], "run");
        let mut changed = grant.clone();
        changed["binding_id"] = json!("other");
        assert!(grant_route(&frame, &changed, &route).is_err());
        changed = grant.clone();
        changed["bound_tools"] = json!(false);
        assert!(grant_route(&frame, &changed, &Route::default()).is_err());
        assert!(grant_route(&Frame::default(), &grant, &route).is_err());
    }
}
