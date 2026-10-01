//! Native compiled runtime-behavior accessors used by discovery factories.
pub fn model_limits_for(
    provider: &ara_rpc::WireString,
    id: &ara_rpc::WireString,
) -> Option<crate::model_collapse::VariantSpec> {
    for table in
        crate::catalog_rules::compiled_rules()["behavior"]["modelLimits"].as_array().map(Vec::as_slice).unwrap_or(&[])
    {
        if !table["provider"].as_str().is_some_and(|p| provider.equals_ascii(p)) {
            continue;
        }
        if let Some(limit) = table["limits"]
            .as_array()
            .map(Vec::as_slice)
            .unwrap_or(&[])
            .iter()
            .find(|l| l["model"].as_str().is_some_and(|m| id.equals_ascii(m)))
        {
            let mut out = crate::model_identity_wire::empty();
            for key in ["context", "maxTokens"] {
                if let Some(value) = limit.get(key) {
                    out.set(key, ara_rpc::WireValue::parse(&value.to_string()).expect("compiled number"));
                }
            }
            return Some(out);
        }
    }
    None
}
use super::common::{includes, starts};
use crate::{
    catalog_rules,
    model_collapse::{VariantSpec, lower},
};
use ara_rpc::{WireString, WireValue};
fn list<'a>(value: &'a serde_json::Value, key: &str) -> &'a [serde_json::Value] {
    value[key].as_array().map(Vec::as_slice).unwrap_or(&[])
}
fn glob(pattern: &[u16], value: &[u16]) -> bool {
    let mut row = vec![false; value.len() + 1];
    row[0] = true;
    for &unit in pattern {
        let mut next = vec![false; value.len() + 1];
        if unit == 42 {
            next[0] = row[0];
            for i in 1..=value.len() {
                next[i] = row[i] || next[i - 1];
            }
        } else {
            for i in 1..=value.len() {
                next[i] = row[i - 1] && (unit == 63 || unit == value[i - 1]);
            }
        }
        row = next;
    }
    row[value.len()]
}
fn matches(rule: &serde_json::Value, id: &WireString, normalized: &WireString) -> bool {
    if list(rule, "exact").iter().filter_map(|v| v.as_str()).any(|v| id.equals_ascii(v))
        || list(rule, "prefix").iter().filter_map(|v| v.as_str()).any(|v| starts(id, v))
        || list(rule, "substring").iter().filter_map(|v| v.as_str()).any(|v| includes(id, v))
    {
        return true;
    }
    let tokens: Vec<_> = normalized.units().split(|u| !matches!(u,0x61..=0x7a|0x30..=0x39)).collect();
    if list(rule, "token")
        .iter()
        .filter_map(|v| v.as_str())
        .any(|v| tokens.iter().any(|token| *token == v.encode_utf16().collect::<Vec<_>>()))
    {
        return true;
    }
    list(rule, "glob")
        .iter()
        .filter_map(|v| v.as_str())
        .any(|v| glob(&v.encode_utf16().collect::<Vec<_>>(), normalized.units()))
}
pub fn is_excluded_model(provider: &WireString, id: &WireString) -> bool {
    let lower = lower(id);
    list(&catalog_rules::compiled_rules()["behavior"], "excludeModels").iter().any(|rule| {
        rule["provider"].as_str().is_some_and(|p| provider.equals_ascii(p)) && matches(&rule["match"], &lower, &lower)
    })
}
pub fn likely_responses_id(id: &WireString) -> bool {
    let rules = &catalog_rules::compiled_rules()["behavior"]["openaiResponsesHeuristic"];
    !list(rules, "excludePrefixes").iter().filter_map(|v| v.as_str()).any(|v| starts(id, v))
        && !list(rules, "excludeSubstrings").iter().filter_map(|v| v.as_str()).any(|v| includes(id, v))
        && list(rules, "includePrefixes").iter().filter_map(|v| v.as_str()).any(|v| starts(id, v))
}
pub fn api_route_for(provider: &WireString, id: &WireString) -> Option<VariantSpec> {
    let table = list(&catalog_rules::compiled_rules()["behavior"], "apiRoutes")
        .iter()
        .find(|rule| rule["provider"].as_str().is_some_and(|p| provider.equals_ascii(p)))?;
    let lowered = lower(id);
    for route in list(table, "routes") {
        if matches(&route["match"], id, &lowered) {
            let mut out = VariantSpec::from_wire(WireValue::object(vec![(
                "api",
                WireValue::String(route["api"].as_str()?.into()),
            )]));
            if route["stripPrefix"].as_bool() == Some(true)
                && let Some(prefix) =
                    list(&route["match"], "prefix").iter().filter_map(|v| v.as_str()).find(|prefix| starts(id, prefix))
            {
                out.set(
                    "requestModelId",
                    WireValue::String(WireString::from_units(id.units()[prefix.encode_utf16().count()..].to_vec())),
                );
            }
            return Some(out);
        }
    }
    table["default"]
        .as_str()
        .map(|api| VariantSpec::from_wire(WireValue::object(vec![("api", WireValue::String(api.into()))])))
}
pub fn api_route_exact_model_ids(provider: &WireString) -> Vec<WireString> {
    let Some(table) = list(&catalog_rules::compiled_rules()["behavior"], "apiRoutes")
        .iter()
        .find(|r| r["provider"].as_str().is_some_and(|p| provider.equals_ascii(p)))
    else {
        return Vec::new();
    };
    let mut out = Vec::new();
    for route in list(table, "routes") {
        for id in list(&route["match"], "exact").iter().filter_map(|v| v.as_str()) {
            let id: WireString = id.into();
            if !out.contains(&id) {
                out.push(id);
            }
        }
    }
    out
}
