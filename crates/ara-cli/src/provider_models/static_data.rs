//! Plain model literals materialized by the unchanged fixed OMP module oracle.
//! No JavaScript runs at the native consumer boundary.
use crate::model_collapse::{SpecRef, VariantSpec};
use ara_rpc::{WireString, WireValue};
use std::sync::{Arc, OnceLock};
pub fn fixed_literal(name: &str) -> VariantSpec {
    static DATA: OnceLock<WireValue> = OnceLock::new();
    let data = DATA.get_or_init(|| WireValue::parse(include_str!("seed_models.json")).expect("fixed seeds"));
    let encoded =
        data.get("models").and_then(|m| m.get(name)).unwrap_or_else(|| panic!("unknown fixed literal {name}"));
    let value = WireValue::parse(
        &encoded
            .get("rawWireJSON")
            .and_then(WireValue::as_string)
            .expect("literal JSON")
            .to_utf8()
            .expect("valid literal JSON"),
    )
    .expect("literal parse");
    let undefined_paths = encoded
        .get("undefinedPaths")
        .and_then(WireValue::as_array)
        .unwrap_or_default()
        .iter()
        .map(|path| {
            path.as_array().expect("path").iter().map(|part| part.as_string().expect("path unit").clone()).collect()
        })
        .collect();
    VariantSpec { value, undefined_paths }
}
pub fn fixed_models(name: &str) -> Vec<SpecRef> {
    let data = fixed_literal(name);
    data.value
        .as_array()
        .expect("model array")
        .iter()
        .enumerate()
        .map(|(index, value)| {
            let index: WireString = index.to_string().into();
            Arc::new(VariantSpec {
                value: value.clone(),
                undefined_paths: data
                    .undefined_paths
                    .iter()
                    .filter(|p| p.first() == Some(&index) && p.len() > 1)
                    .map(|p| p[1..].to_vec())
                    .collect(),
            })
        })
        .collect()
}
