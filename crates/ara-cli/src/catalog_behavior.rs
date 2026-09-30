//! Fixed OMP `compat/{behavior,auth}.ts` compiled policy accessors.
//!
//! Source: 596f2da7101178214aa27a753529d15e6b7ad91d; MIT notice retained in
//! data/LICENSE.omp-catalog. These are declarative policies, not login flows,
//! credential resolution, model discovery or authorization.

use crate::catalog_rules::{compiled_rules, glob_match};
use serde_json::{Map, Value, json};

fn list(value: &Value) -> &[Value] {
    value.as_array().map(Vec::as_slice).unwrap_or(&[])
}
fn text<'a>(value: &'a Value, key: &str) -> &'a str {
    value[key].as_str().unwrap_or("")
}
fn strings(value: &Value) -> impl Iterator<Item = &str> {
    list(value).iter().filter_map(Value::as_str)
}
fn behavior() -> &'static Value {
    &compiled_rules()["behavior"]
}
fn provider_rule(field: &str, provider: &str) -> Option<&'static Value> {
    list(&behavior()[field]).iter().find(|v| text(v, "provider") == provider)
}

fn matches_list(rule: &Value, model: &str, lower: &str) -> bool {
    strings(&rule["exact"]).any(|id| id == model)
        || strings(&rule["prefix"]).any(|p| model.starts_with(p))
        || strings(&rule["substring"]).any(|s| model.contains(s))
        || strings(&rule["token"]).any(|token| {
            lower.split(|c: char| !c.is_ascii_lowercase() && !c.is_ascii_digit()).any(|part| part == token)
        })
        || strings(&rule["glob"]).any(|pattern| glob_match(pattern, lower))
}

pub fn is_likely_openai_responses_id(model: &str) -> bool {
    let rule = &behavior()["openaiResponsesHeuristic"];
    !rule.is_null()
        && !strings(&rule["excludePrefixes"]).any(|prefix| model.starts_with(prefix))
        && !strings(&rule["excludeSubstrings"]).any(|sub| model.contains(sub))
        && strings(&rule["includePrefixes"]).any(|prefix| model.starts_with(prefix))
}

pub fn model_operation_overrides(provider: &str, model: &str) -> Vec<String> {
    let lower = model.to_lowercase();
    let mut out = Vec::new();
    for rule in list(&behavior()["modelOperations"]) {
        if text(rule, "provider") != provider || !matches_list(&rule["models"], &lower, &lower) {
            continue;
        }
        for operation in strings(&rule["operations"]) {
            if !out.iter().any(|o| o == operation) {
                out.push(operation.to_owned());
            }
        }
    }
    out
}

pub fn cursor_effort_suffix(model: &str) -> Option<Value> {
    let rule = &behavior()["cursorEffort"];
    if rule.is_null() {
        return None;
    }
    for tier in strings(&rule["tiers"]) {
        let Some(prefix) = model.strip_suffix(tier) else {
            continue;
        };
        let Some(base) = prefix.strip_suffix('-') else {
            continue;
        };
        let marker = text(rule, "familyMarker");
        // Walk overlapping occurrences, as String.indexOf(marker,index+1).
        let family = base.char_indices().any(|(index, _)| {
            base[index..].starts_with(marker)
                && base.as_bytes().get(index + marker.len()).is_some_and(u8::is_ascii_digit)
        });
        if !family {
            return None;
        }
        return Some(json!({"base":base,"tier":tier}));
    }
    None
}

pub fn cursor_model_parameters(model: &str) -> Vec<Value> {
    list(&behavior()["cursorParameters"]).iter().filter(|p| text(p, "model") == model).cloned().collect()
}
pub fn quota_tier_for(provider: &str, model: &str) -> Option<&'static str> {
    let rule = provider_rule("quotaTiers", provider)?;
    for tier in list(&rule["tiers"]) {
        if strings(&tier["models"]).any(|id| id == model) {
            return tier["label"].as_str();
        }
    }
    list(&rule["fallbacks"])
        .iter()
        .find(|fallback| model.contains(text(fallback, "substring")))
        .and_then(|v| v["label"].as_str())
}
pub fn has_quota_tier_policy(provider: &str) -> bool {
    provider_rule("quotaTiers", provider).is_some()
}
pub fn hosted_default_model(provider: &str) -> Option<&'static str> {
    provider_rule("hostedDefaults", provider)?.get("model")?.as_str()
}

pub fn api_route_for(provider: &str, model: &str) -> Option<Value> {
    let table = provider_rule("apiRoutes", provider)?;
    let lower = model.to_lowercase();
    for route in list(&table["routes"]) {
        if !matches_list(&route["match"], model, &lower) {
            continue;
        }
        let mut out = Map::new();
        out.insert("api".into(), route["api"].clone());
        if route["stripPrefix"].as_bool() == Some(true)
            && let Some(prefix) =
                strings(&route["match"]["prefix"]).find(|prefix| model.starts_with(prefix) && !prefix.is_empty())
        {
            out.insert("requestModelId".into(), Value::from(&model[prefix.len()..]));
        }
        return Some(Value::Object(out));
    }
    table.get("default").map(|api| json!({"api":api}))
}
pub fn api_route_exact_model_ids(provider: &str) -> Vec<String> {
    let Some(table) = provider_rule("apiRoutes", provider) else {
        return Vec::new();
    };
    let mut out = Vec::new();
    for route in list(&table["routes"]) {
        for id in strings(&route["match"]["exact"]) {
            if !out.iter().any(|item| item == id) {
                out.push(id.to_owned());
            }
        }
    }
    out
}
pub fn model_limits_for(provider: &str, model: &str) -> Option<Value> {
    for table in list(&behavior()["modelLimits"]) {
        if text(table, "provider") != provider {
            continue;
        }
        if let Some(limit) = list(&table["limits"]).iter().find(|v| text(v, "model") == model) {
            let mut out = Map::new();
            for key in ["context", "maxTokens"] {
                if let Some(value) = limit.get(key) {
                    out.insert(key.into(), value.clone());
                }
            }
            return Some(Value::Object(out));
        }
    }
    None
}
pub fn is_excluded_model(provider: &str, model: &str) -> bool {
    let lower = model.to_lowercase();
    list(&behavior()["excludeModels"])
        .iter()
        .any(|rule| text(rule, "provider") == provider && matches_list(&rule["match"], &lower, &lower))
}
pub fn is_retired_provider(provider: &str) -> bool {
    strings(&behavior()["retiredProviders"]).any(|id| id == provider)
}
pub fn plan_requirement_for(provider: &str, model: &str) -> Option<&'static str> {
    let rule = provider_rule("planRequirements", provider)?;
    let lower = model.to_lowercase();
    list(&rule["tiers"])
        .iter()
        .find(|tier| matches_list(&tier["match"], model, &lower))
        .and_then(|tier| tier["tier"].as_str())
}
pub fn pricing_peer_for(provider: &str, model: &str) -> Option<Value> {
    let rule = provider_rule("pricingPeers", provider)?;
    let peer_id = list(&rule["aliases"])
        .iter()
        .find(|v| text(v, "model") == model)
        .and_then(|v| v.get("peerId"))
        .filter(|v| !v.is_null())
        .cloned()
        .unwrap_or_else(|| Value::from(model));
    Some(json!({"peers":rule["peers"],"peerId":peer_id}))
}

pub fn auth_providers() -> &'static [Value] {
    list(&compiled_rules()["auth"]["providers"])
}
pub fn auth_policy_for(provider: &str) -> Option<&'static Value> {
    // Object.fromEntries uses the last occurrence without changing list order.
    auth_providers().iter().rev().find(|p| text(p, "id") == provider)
}
pub fn login_provider_ids() -> Vec<&'static str> {
    auth_providers().iter().filter(|p| p.get("login").is_some()).filter_map(|p| p["id"].as_str()).collect()
}

fn add_hook(names: &mut Map<String, Value>, kind: &str, value: Option<&Value>) {
    let Some(name) = value.and_then(Value::as_str).filter(|s| !s.is_empty()) else {
        return;
    };
    names.entry(kind).or_insert_with(|| Value::Array(Vec::new())).as_array_mut().unwrap().push(Value::from(name));
}
pub fn auth_hook_names() -> Value {
    let mut names = Map::new();
    for provider in auth_providers() {
        let env = &provider["env"];
        add_hook(&mut names, "env", env.get("hook"));
        let login = &provider["login"];
        match text(login, "kind") {
            "custom" => add_hook(&mut names, "login", login.get("hook")),
            "api-key" if text(&login["validate"], "kind") == "models-endpoint" => {
                add_hook(&mut names, "headers", login["validate"].get("headersHook"))
            }
            "oauth-code" => {
                add_hook(&mut names, "after-exchange", login.get("afterExchange"));
                add_hook(&mut names, "value", login["clientId"].get("hook"));
                add_hook(&mut names, "value", login["authorizeUrl"].get("hook"));
                add_hook(&mut names, "value", login["token"]["url"].get("hook"));
            }
            "device-code" => {
                add_hook(&mut names, "after-exchange", login.get("afterExchange"));
                add_hook(&mut names, "headers", login.get("headersHook"));
                for key in ["clientId", "baseUrl"] {
                    add_hook(&mut names, "value", login[key].get("hook"));
                }
                for key in ["device", "token"] {
                    add_hook(&mut names, "value", login[key]["url"].get("hook"));
                }
            }
            _ => {}
        }
        let refresh = &provider["refresh"];
        match text(refresh, "kind") {
            "hook" => add_hook(&mut names, "refresh", refresh.get("hook")),
            "request" => {
                add_hook(&mut names, "after-exchange", refresh.get("afterRefresh"));
                add_hook(&mut names, "headers", refresh.get("headersHook"));
                add_hook(&mut names, "value", refresh["token"]["url"].get("hook"));
            }
            _ => {}
        }
    }
    Value::Object(names)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn behavior_keeps_match_case_and_cursor_family_gate() {
        assert!(is_likely_openai_responses_id("gpt-5.4"));
        assert!(!is_likely_openai_responses_id("GPT-5.4"));
        assert!(cursor_effort_suffix("gpt-5.4-high").is_some());
        assert!(cursor_effort_suffix("gpt-unknown-high").is_none());
        assert!(cursor_effort_suffix("GPT-5.4-high").is_none());
        assert_eq!(api_route_for("missing", "anything"), None);
        assert_eq!(model_limits_for("missing", "anything"), None);
    }
    #[test]
    fn auth_roster_and_hooks_preserve_complete_declarations() {
        assert_eq!(auth_providers().len(), 80);
        assert!(auth_policy_for("openai-codex").unwrap().get("login").is_some());
        assert!(auth_policy_for("OpenAI-Codex").is_none());
        assert!(login_provider_ids().contains(&"openai-codex-device"));
        assert!(!auth_hook_names().as_object().unwrap().is_empty());
    }
}
