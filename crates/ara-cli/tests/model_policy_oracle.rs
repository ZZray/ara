//! Compares native catalog primitives with independently executed fixed OMP.
//! JSON numbers compare as IEEE doubles, as in Bun, without a tolerance.
use ara_cli::{catalog_behavior as b, catalog_rules as r, model_identity as i, model_policy as p};
use serde_json::{Map, Value, json};
use std::collections::{BTreeMap, BTreeSet};

#[test]
#[ignore = "requires retained fixed OMP artifact in ARA_MODEL_POLICY_ORACLE"]
fn lossless_policy_adapter_replays_all_original_policy_and_build_cases() {
    use ara_cli::{
        model_collapse::{CollapseModelPolicy, VariantSpec},
        model_wire_policy::WireModelPolicy,
    };
    let path = std::env::var_os("ARA_MODEL_POLICY_ORACLE").expect("execute scripts/model_policy_oracle.py first");
    let bytes = std::fs::read(path).unwrap();
    let digest = ring::digest::digest(&ring::digest::SHA256, &bytes);
    let actual = digest.as_ref().iter().map(|byte| format!("{byte:02x}")).collect::<String>();
    assert_eq!(
        actual, "53c29b84b31edde4be7dda3e22ddcb2aa2121e43f66dcc6b2c8e3136cb9b1cad",
        "original complete frozen policy corpus"
    );
    let oracle: Value = serde_json::from_slice(&bytes).unwrap();
    inventory(&oracle).expect("original full source inventory");
    let mut counts = BTreeMap::new();
    let mut differences = Vec::new();
    for case in oracle["cases"].as_array().unwrap() {
        let kind = string(case, "kind");
        let build = match kind {
            "catalog-policy"
            | "fixture-policy"
            | "endpoint-policy"
            | "api-endpoint-policy"
            | "sparse-policy"
            | "prototype-policy" => false,
            "catalog-build" | "fixture-build" | "sparse-build" | "computer-use-build" | "prototype-build" => true,
            _ => continue,
        };
        *counts.entry(kind.to_owned()).or_insert(0usize) += 1;
        let input = VariantSpec::from_json(&case["input"]);
        let outcome = if build { WireModelPolicy.build(&input) } else { WireModelPolicy.resolve(&input) };
        let actual =
            result(outcome.map(|spec| serde_json::from_str::<Value>(&spec.to_wire_json().stringify()).unwrap()));
        if !same(&actual, &case["expected"]) {
            differences.push(json!({"kind":kind,"input":case["input"],"expected":case["expected"],"actual":actual}));
        }
    }
    assert_eq!(counts.values().sum::<usize>(), 11844, "all original policy/build inputs");
    for (kind, count) in &counts {
        assert_eq!(EXPECTED_COUNTS.iter().find(|(name, _)| name == kind).unwrap().1, *count);
    }
    if let Some(path) = std::env::var_os("ARA_WIRE_POLICY_MISMATCHES") {
        std::fs::write(path, serde_json::to_vec(&differences).unwrap()).unwrap();
    }
    assert!(
        differences.is_empty(),
        "{} native wire policy/build differences; first={}",
        differences.len(),
        json!(differences.first())
    );
}

const EXPECTED_COUNTS: &[(&str, usize)] = &[
    ("catalog-taxonomy", 4776),
    ("catalog-cascade", 4776),
    ("catalog-policy", 4776),
    ("catalog-build", 4776),
    ("catalog-identity", 4776),
    ("catalog-tokenizer", 4776),
    ("catalog-host", 4776),
    ("catalog-metrics", 4776),
    ("fixture-identity", 215),
    ("fixture-tokenizer", 215),
    ("fixture-taxonomy", 863),
    ("override-taxonomy", 53),
    ("revision", 20),
    ("constraint", 10),
    ("revision-compare", 16),
    ("glob", 63),
    ("cascade-rules", 8),
    ("fixture-policy", 182),
    ("fixture-build", 182),
    ("endpoint-policy", 280),
    ("endpoint-host", 280),
    ("api-endpoint-policy", 1274),
    ("anthropic-url", 18),
    ("xai-map", 6),
    ("compat-apply", 15),
    ("prototype-policy", 1),
    ("prototype-build", 1),
    ("catalog-corrections", 10),
    ("utility-number", 25),
    ("utility-name", 10),
    ("utility-oauth", 5),
    ("axes", 1),
    ("axis-predicates", 13),
    ("vertex-location", 6),
    ("provider-priority", 3),
    ("bundled-reference", 3223),
    ("provider-reference-map", 67),
    ("reference-index", 1),
    ("reference-resolver", 1),
    ("inherit-thinking", 9),
    ("fixture-metrics", 215),
    ("metrics-apply", 1),
    ("auth-global", 1),
    ("taxonomy-vocab", 1),
    ("provider-vocab", 86),
    ("behavior-provider", 86),
    ("sparse-policy", 90),
    ("sparse-build", 90),
    ("computer-use-build", 192),
    ("taxonomy-unicode", 4),
    ("behavior-branches", 86),
];
const BEHAVIOR_ID_COUNT: usize = 3146;
const BEHAVIOR_PAIR_COUNT: usize = 270_556;

fn string<'a>(value: &'a Value, key: &str) -> &'a str {
    value[key].as_str().expect("fixture string")
}
fn field(out: &mut Map<String, Value>, key: &str, value: Option<Value>) {
    if let Some(value) = value {
        out.insert(key.into(), value);
    }
}
fn answer(value: Option<Value>) -> Value {
    value.map_or_else(|| json!({"status":"undefined"}), |value| json!({"status":"ok","value":value}))
}
fn result<E: std::fmt::Display>(value: Result<Value, E>) -> Value {
    match value {
        Ok(value) => answer(Some(value)),
        Err(error) => json!({"status":"error","error":error.to_string()}),
    }
}
fn same(left: &Value, right: &Value) -> bool {
    match (left, right) {
        (Value::Number(a), Value::Number(b)) => a.as_f64() == b.as_f64(),
        (Value::Array(a), Value::Array(b)) => a.len() == b.len() && a.iter().zip(b).all(|(a, b)| same(a, b)),
        (Value::Object(a), Value::Object(b)) => {
            a.len() == b.len() && a.iter().all(|(k, v)| b.get(k).is_some_and(|w| same(v, w)))
        }
        _ => left == right,
    }
}
fn inventory(oracle: &Value) -> Result<(), String> {
    if oracle["upstreamCommit"] != "596f2da7101178214aa27a753529d15e6b7ad91d" || oracle["bunVersion"] != "1.4.0" {
        return Err("wrong fixed runtime/source".into());
    }
    let cases = oracle["cases"].as_array().ok_or("missing cases")?;
    let mut counts = BTreeMap::new();
    for case in cases {
        let kind = case["kind"].as_str().ok_or("missing category")?;
        *counts.entry(kind).or_insert(0usize) += 1;
    }
    let expected: BTreeMap<_, _> = EXPECTED_COUNTS.iter().copied().collect();
    if counts != expected {
        return Err(format!("incomplete/unknown oracle categories: {counts:?}"));
    }
    let expected_keys: BTreeSet<_> = i::bundled_model_list()
        .iter()
        .map(|row| format!("{}/{}", string(row, "provider"), string(row, "id")))
        .collect();
    let keys: BTreeSet<_> = oracle["catalogKeys"]
        .as_array()
        .ok_or("missing catalog keys")?
        .iter()
        .map(|k| k.as_str().ok_or("invalid catalog key"))
        .collect::<Result<_, _>>()?;
    if expected_keys.len() != 4776 || keys.len() != 4776 || !expected_keys.iter().all(|key| keys.contains(key.as_str()))
    {
        return Err("incomplete fixed catalog keys".into());
    }
    for kind in [
        "catalog-taxonomy",
        "catalog-cascade",
        "catalog-policy",
        "catalog-build",
        "catalog-identity",
        "catalog-tokenizer",
        "catalog-host",
        "catalog-metrics",
    ] {
        let labels: BTreeSet<_> = cases
            .iter()
            .filter(|case| case["kind"] == kind)
            .map(|case| case["label"].as_str().ok_or("missing catalog label"))
            .collect::<Result<_, _>>()?;
        if labels != keys {
            return Err(format!("missing/duplicate catalog coverage for {kind}"));
        }
        for case in cases.iter().filter(|case| case["kind"] == kind) {
            let label = case["label"].as_str().ok_or("missing catalog label")?;
            let id_key = if kind == "catalog-cascade" { "model" } else { "id" };
            let id = case["input"][id_key].as_str().ok_or("missing catalog model id")?;
            let provider = case["input"]["provider"].as_str();
            let (label_provider, label_id) = label.split_once('/').ok_or("invalid catalog label")?;
            if label_id != id || provider.is_some_and(|provider| provider != label_provider) {
                return Err(format!("wrong label/input binding for {kind}/{label}"));
            }
        }
    }
    let provider_cases: Vec<_> = cases.iter().filter(|case| case["kind"] == "behavior-provider").collect();
    let providers: BTreeSet<_> = oracle["behaviorProviders"]
        .as_array()
        .ok_or("missing behavior providers")?
        .iter()
        .map(|v| v.as_str().ok_or("invalid provider"))
        .collect::<Result<_, _>>()?;
    let native_providers: BTreeSet<_> = i::bundled_model_list()
        .iter()
        .map(|row| string(row, "provider"))
        .chain(b::auth_providers().iter().map(|row| string(row, "id")))
        .chain(
            r::compiled_rules()["behavior"]["retiredProviders"]
                .as_array()
                .unwrap()
                .iter()
                .map(|value| value.as_str().unwrap()),
        )
        .chain(["fixture", "OPENAI"])
        .collect();
    if providers != native_providers || provider_cases.len() != 86 {
        return Err("missing/wrong behavior providers".into());
    }
    let bundled_ids: BTreeSet<_> = i::bundled_model_list().iter().map(|row| string(row, "id")).collect();
    let behavior_ids = oracle["behaviorIds"].as_array().ok_or("missing behavior id inventory")?;
    let expected_ids: BTreeSet<_> =
        behavior_ids.iter().map(|id| id.as_str().ok_or("invalid behavior inventory id")).collect::<Result<_, _>>()?;
    if behavior_ids.len() != BEHAVIOR_ID_COUNT
        || expected_ids.len() != BEHAVIOR_ID_COUNT
        || !bundled_ids.is_subset(&expected_ids)
    {
        return Err("incomplete/duplicate behavior id inventory".into());
    }
    let mut pairs = 0;
    let mut found = BTreeSet::new();
    for case in provider_cases {
        let provider = case["input"]["provider"].as_str().ok_or("missing behavior provider")?;
        if !providers.contains(provider) || !found.insert(provider) {
            return Err("duplicate/unknown behavior provider".into());
        }
        let ids = case["input"]["ids"].as_array().ok_or("missing behavior ids")?;
        let id_set: BTreeSet<_> =
            ids.iter().map(|id| id.as_str().ok_or("invalid behavior id")).collect::<Result<_, _>>()?;
        if ids.len() != BEHAVIOR_ID_COUNT
            || id_set != expected_ids
            || case["expected"]["value"].as_array().is_none_or(|v| v.len() != ids.len())
        {
            return Err("incomplete behavior model coverage".into());
        }
        pairs += ids.len();
    }
    if pairs != BEHAVIOR_PAIR_COUNT || oracle["behaviorPairs"].as_u64() != Some(BEHAVIOR_PAIR_COUNT as u64) {
        return Err("behavior pair receipt mismatch".into());
    }
    Ok(())
}
fn identity(id: &str) -> Value {
    let mut out=json!({"bare":i::bare_model_id(id),"segments":i::get_model_like_id_segments(id),"brackets":i::get_bracket_stripped_model_id_candidates(id),"candidates":i::get_reference_candidate_ids(id),"dialect":i::preferred_dialect(id)}).as_object().unwrap().clone();
    field(&mut out, "longest", i::get_longest_model_like_id_segment(id).map(Value::from));
    field(&mut out, "stripped", i::strip_bracketed_model_id_affixes(id).map(Value::from));
    Value::Object(out)
}
fn taxonomy(input: &Value) -> Result<Value, r::CatalogPolicyError> {
    let provider = string(input, "provider");
    let id = string(input, "id");
    let options = r::ClassifyOptions {
        lenient: input["options"]["lenient"] == true,
        observed_at_ms: input["options"]["observedAtMs"].as_f64(),
    };
    let mut out=json!({"identity":r::classify_model(provider,id,options)?,"collapse":r::collapse_variant_id(provider,id),"lane":r::strip_effort_lane(provider,id)}).as_object().unwrap().clone();
    field(&mut out, "thinking", r::strip_thinking_variant_suffix(id)?.map(Value::from));
    field(&mut out, "billing", r::billing_variant_plain(id).map(Value::from));
    field(&mut out, "routing", r::routing_variant_plain(provider, id).map(Value::from));
    Ok(Value::Object(out))
}
fn host(model: &Value) -> Value {
    let url = model["baseUrl"].as_str().unwrap_or("");
    json!({"url":i::known_hosts().map(|name|(name.into(),json!(i::host_matches_url(url,name)))).collect::<Map<String,Value>>(),
        "model":i::known_hosts().map(|name|(name.into(),json!(i::model_matches_host(model,name)))).collect::<Map<String,Value>>(),
        "vertexExpress":i::is_vertex_express_openai_url(url),"vertexRaw":i::is_vertex_raw_predict_url(url),"azure":i::is_azure_deployments_url(url),"dashscope":i::is_dashscope_compatible_mode_url(url)})
}
fn behavior(provider: &str, id: &str) -> Value {
    let mut out=json!({"responses":b::is_likely_openai_responses_id(id),"operations":b::model_operation_overrides(provider,id),"parameters":b::cursor_model_parameters(id),"excluded":b::is_excluded_model(provider,id)}).as_object().unwrap().clone();
    field(&mut out, "effort", b::cursor_effort_suffix(id));
    field(&mut out, "quota", b::quota_tier_for(provider, id).map(Value::from));
    field(&mut out, "route", b::api_route_for(provider, id));
    field(&mut out, "limits", b::model_limits_for(provider, id));
    field(&mut out, "plan", b::plan_requirement_for(provider, id).map(Value::from));
    field(&mut out, "pricingPeer", b::pricing_peer_for(provider, id));
    Value::Object(out)
}
fn provider(provider: &str) -> Value {
    let mut out=json!({"routing":r::has_routing_variants(provider),"recovery":r::recovers_canonical_params(provider),"siblings":r::supports_dynamic_effort_siblings(provider),"families":r::effort_families_for(provider),"quota":b::has_quota_tier_policy(provider),"exactRoutes":b::api_route_exact_model_ids(provider),"retired":b::is_retired_provider(provider)}).as_object().unwrap().clone();
    field(&mut out, "hint", r::responses_hint_group(provider).cloned());
    field(&mut out, "routes", r::responses_route_models(provider).cloned());
    field(&mut out, "hosted", b::hosted_default_model(provider).map(Value::from));
    field(&mut out, "auth", b::auth_policy_for(provider).cloned());
    Value::Object(out)
}
fn metrics(model: &Value, index: &i::CatalogMetricsIndex) -> Value {
    let mut out = Map::new();
    field(&mut out, "own", i::catalog_metrics_of(model));
    field(&mut out, "resolved", index.resolve(model));
    Value::Object(out)
}
fn replay(case: &Value, index: &i::CatalogMetricsIndex) -> Value {
    let input = &case["input"];
    match string(case, "kind") {
        "catalog-taxonomy" | "fixture-taxonomy" | "override-taxonomy" => result(taxonomy(input)),
        "catalog-cascade" => result(r::resolve_cascade(input)),
        "catalog-policy"
        | "fixture-policy"
        | "endpoint-policy"
        | "api-endpoint-policy"
        | "sparse-policy"
        | "prototype-policy" => result(p::resolve_model_policy(input)),
        "catalog-build" | "fixture-build" | "sparse-build" | "computer-use-build" | "prototype-build" => {
            result(p::build_model(input))
        }
        "catalog-identity" | "fixture-identity" => answer(Some(identity(string(input, "id")))),
        "catalog-tokenizer" | "fixture-tokenizer" => match p::resolve_model_tokenizer(string(input, "id")) {
            Ok(value) => answer(value.map(Value::from)),
            Err(error) => json!({"status":"error","error":error.to_string()}),
        },
        "catalog-host" | "endpoint-host" => answer(Some(host(input))),
        "revision" => {
            let mut out = Map::new();
            field(&mut out, "parse", r::parse_revision(string(input, "value")).map(|v| json!(v)));
            field(&mut out, "prefix", r::parse_revision_prefix(string(input, "value")).map(|v| json!(v)));
            answer(Some(Value::Object(out)))
        }
        "constraint" => answer(r::parse_revision_constraint(string(input, "expression")).map(|v| json!(v))),
        "revision-compare" => {
            let left: r::Revision = serde_json::from_value(input["left"].clone()).unwrap();
            let right: r::Revision = serde_json::from_value(input["right"].clone()).unwrap();
            let terms: Vec<r::RevisionTerm> = serde_json::from_value(input["terms"].clone()).unwrap();
            answer(Some(
                json!({"compare":r::compare_revision(left,right),"format":r::format_revision(left),"satisfies":terms.iter().map(|term|r::revision_satisfies(left,std::slice::from_ref(term))).collect::<Vec<_>>()}),
            ))
        }
        "glob" => answer(Some(json!(r::glob_match(string(input, "pattern"), string(input, "value"))))),
        "cascade-rules" => result(r::resolve_cascade_rules(&input["cascade"], &input["target"])),
        "anthropic-url" => {
            let url = input["baseUrl"].as_str();
            answer(Some(
                json!({"official":p::is_official_anthropic_api_url(url),"azure":p::is_azure_anthropic_route(url),"proxy":p::is_anthropic_signing_proxy_url(url)}),
            ))
        }
        "xai-map" => result(p::xai_responses_reasoning_effort_map(string(input, "id"))),
        "compat-apply" => {
            let mut compat = input["compat"].clone();
            p::apply_compat_overrides(&mut compat, input.get("overrides"));
            answer(Some(compat))
        }
        "catalog-corrections" => {
            let mut model = input["model"].clone();
            p::apply_catalog_corrections(&mut model, &input["catalog"]);
            answer(Some(model))
        }
        "utility-number" => {
            let value = &input["value"];
            let mut out=json!({"positive":p::to_positive_number(value,Some(7.0)),"nullable":p::to_positive_number_or_null(value),"record":p::is_record(value)}).as_object().unwrap().clone();
            field(&mut out, "number", p::to_number(value).map(Value::from));
            field(&mut out, "boolean", p::to_boolean(value).map(Value::from));
            answer(Some(Value::Object(out)))
        }
        "utility-name" => answer(Some(json!(p::clean_model_name(string(input, "name"))))),
        "utility-oauth" => answer(Some(json!(p::is_anthropic_oauth_token(string(input, "key"))))),
        "axes" => answer(Some(
            json!({"axes":p::axes_value(),"apiRecords":p::api_compat_records_value(),"efforts":p::effort_tiers_value()}),
        )),
        "axis-predicates" => {
            let value = string(input, "value");
            answer(Some(json!({"effort":p::is_effort_tier(value),"mode":p::is_thinking_mode(value)})))
        }
        "vertex-location" => answer(Some(json!(i::resolve_vertex_endpoint_host(string(input, "location"))))),
        "provider-priority" => {
            let configured: Vec<String> =
                input.get("configured").map_or_else(Vec::new, |v| serde_json::from_value(v.clone()).unwrap());
            answer(Some(json!(i::build_model_provider_priority_rank(&configured))))
        }
        "bundled-reference" => {
            answer(i::resolve_model_reference(string(input, "id"), i::get_bundled_model_reference_index()).cloned())
        }
        "provider-reference-map" => answer(Some(json!(i::create_bundled_reference_map(string(input, "provider"))))),
        "reference-index" => {
            let models = input["models"].as_array().unwrap();
            let index = i::build_model_reference_index(models);
            answer(Some(json!({"exact":index.exact,"suffixAlias":index.suffix_alias})))
        }
        "reference-resolver" => {
            let refs = input["references"].as_object().unwrap().clone();
            let calls = std::rc::Rc::new(std::cell::Cell::new(0));
            let observed = calls.clone();
            let mut resolver = i::ReferenceResolver::lazy(move || {
                observed.set(observed.get() + 1);
                refs
            });
            let results: Vec<_> = input["ids"]
                .as_array()
                .unwrap()
                .iter()
                .map(|id| answer(resolver.resolve(id.as_str().unwrap())))
                .collect();
            answer(Some(json!({"sourceCalls":calls.get(),"results":results})))
        }
        "inherit-thinking" => answer(i::inherit_reference_thinking(
            input.get("modelThinking"),
            input.get("reference"),
            string(input, "provider"),
        )),
        "catalog-metrics" | "fixture-metrics" => answer(Some(metrics(input, index))),
        "metrics-apply" => {
            let scored = input["scored"].as_array().unwrap();
            let queries = input["queries"].as_array().unwrap();
            let index = i::CatalogMetricsIndex::new(scored);
            answer(Some(
                json!({"empty":index.is_empty(),"models":i::apply_catalog_metrics(queries,&index),"resolved":queries.iter().map(|q|answer(index.resolve(q))).collect::<Vec<_>>()}),
            ))
        }
        "auth-global" => answer(Some(json!({"providers":b::auth_providers(),"hooks":b::auth_hook_names()}))),
        "taxonomy-vocab" => {
            answer(Some(json!({"collapse":r::collapse_vocabulary(),"discovery":r::discovery_vocabulary()})))
        }
        "provider-vocab" => answer(Some(provider(string(input, "provider")))),
        "behavior-provider" | "behavior-branches" => {
            let provider = string(input, "provider");
            answer(Some(Value::Array(
                input["ids"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .map(|id| answer(Some(behavior(provider, id.as_str().unwrap()))))
                    .collect(),
            )))
        }
        "taxonomy-unicode" => {
            let id = string(input, "id");
            let units = r::strip_thinking_variant_suffix_utf16(id);
            match units {
                None => answer(Some(json!({}))),
                Some(units) => {
                    let checked = r::strip_thinking_variant_suffix(id);
                    let representable = String::from_utf16(&units).is_ok();
                    assert_eq!(checked.is_ok(), representable, "checked native string boundary");
                    let raw_json = match String::from_utf16(&units) {
                        Ok(text) => serde_json::to_string(&text).unwrap(),
                        Err(_) => {
                            let mut raw = String::from("\"");
                            for unit in &units {
                                if (0xd800..=0xdfff).contains(unit) {
                                    raw.push_str(&format!("\\u{unit:04x}"));
                                } else {
                                    let escaped =
                                        serde_json::to_string(&char::from_u32(u32::from(*unit)).unwrap().to_string())
                                            .unwrap();
                                    raw.push_str(&escaped[1..escaped.len() - 1]);
                                }
                            }
                            raw.push('"');
                            raw
                        }
                    };
                    answer(Some(json!({"rawJson":raw_json,"units":units,"representable":representable})))
                }
            }
        }
        other => panic!("unknown oracle category {other}"),
    }
}

#[test]
#[ignore = "requires retained fixed-source Bun catalog oracle in ARA_MODEL_POLICY_ORACLE"]
fn fixed_source_model_policy_comparison() {
    let path = std::env::var_os("ARA_MODEL_POLICY_ORACLE").expect("execute scripts/model_policy_oracle.py first");
    let oracle: Value = serde_json::from_slice(&std::fs::read(path).unwrap()).unwrap();
    inventory(&oracle).expect("complete fixed-source inventory");
    let index = i::CatalogMetricsIndex::new(i::bundled_model_list());
    let cases = oracle["cases"].as_array().unwrap();
    let mut mismatch_counts = BTreeMap::new();
    let mut details = Vec::new();
    for (position, case) in cases.iter().enumerate() {
        let actual = replay(case, &index);
        if !same(&actual, &case["expected"]) {
            *mismatch_counts.entry(string(case, "kind").to_owned()).or_insert(0usize) += 1;
            if details.len() < 40 {
                details.push(json!({"position":position,"kind":case["kind"],"label":case["label"],
                    "input":case["input"],"actual":actual,"expected":case["expected"]}));
            }
        }
    }
    if let Some(path) = std::env::var_os("ARA_MODEL_POLICY_MISMATCHES") {
        std::fs::write(path, serde_json::to_vec(&json!({"counts":mismatch_counts,"details":details})).unwrap())
            .unwrap();
    }
    assert!(
        mismatch_counts.is_empty(),
        "native/original differences: {mismatch_counts:?}; first={}",
        details.first().unwrap_or(&Value::Null)
    );
    eprintln!(
        "{} fixed-source policy/build/identity/host/tokenizer/behavior/auth comparisons passed ({} behavior pairs)",
        cases.len(),
        oracle["behaviorPairs"]
    );
}

#[test]
#[ignore = "requires retained fixed-source Bun catalog oracle in ARA_MODEL_POLICY_ORACLE"]
fn malformed_model_policy_oracle_is_rejected() {
    let path = std::env::var_os("ARA_MODEL_POLICY_ORACLE").expect("execute scripts/model_policy_oracle.py first");
    let mut oracle: Value = serde_json::from_slice(&std::fs::read(path).unwrap()).unwrap();
    inventory(&oracle).expect("baseline inventory");
    let provider_index =
        oracle["cases"].as_array().unwrap().iter().position(|case| case["kind"] == "behavior-provider").unwrap();
    let original_case = oracle["cases"][provider_index].clone();
    let bundled_ids: BTreeSet<_> = i::bundled_model_list().iter().map(|row| string(row, "id")).collect();
    let nonbundled_index = original_case["input"]["ids"]
        .as_array()
        .unwrap()
        .iter()
        .position(|id| !bundled_ids.contains(id.as_str().unwrap()))
        .unwrap();
    oracle["cases"][provider_index]["input"]["ids"].as_array_mut().unwrap().remove(nonbundled_index);
    oracle["cases"][provider_index]["expected"]["value"].as_array_mut().unwrap().remove(nonbundled_index);
    oracle["behaviorPairs"] = json!(BEHAVIOR_PAIR_COUNT - 1);
    assert!(inventory(&oracle).is_err(), "deleted nonbundled id with adjusted receipt must fail");
    oracle["cases"][provider_index] = original_case.clone();
    oracle["behaviorPairs"] = json!(BEHAVIOR_PAIR_COUNT);
    oracle["cases"][provider_index]["input"]["ids"][nonbundled_index] = json!("replaced-fixture-id");
    assert!(inventory(&oracle).is_err(), "same-length wrong provider id set must fail");
    oracle["cases"][provider_index] = original_case.clone();
    oracle["cases"][provider_index]["input"]["ids"][0] = original_case["input"]["ids"][1].clone();
    assert!(inventory(&oracle).is_err(), "duplicate provider id must fail");
    oracle["cases"][provider_index] = original_case;
    let original_inventory_id = oracle["behaviorIds"][0].clone();
    oracle["behaviorIds"][0] = oracle["behaviorIds"][1].clone();
    assert!(inventory(&oracle).is_err(), "duplicate inventory id must fail");
    oracle["behaviorIds"][0] = original_inventory_id;
    let original_input = oracle["cases"][0]["input"].clone();
    oracle["cases"][0]["input"]["provider"] = json!("wrong-provider");
    assert!(inventory(&oracle).is_err(), "wrong label/provider binding must fail");
    oracle["cases"][0]["input"] = original_input.clone();
    oracle["cases"][0]["input"]["id"] = json!("wrong-model");
    assert!(inventory(&oracle).is_err(), "wrong label/id binding must fail");
    oracle["cases"][0]["input"] = original_input;
    let original = oracle["cases"][0]["kind"].clone();
    oracle["cases"][0]["kind"] = json!("unknown-category");
    assert!(inventory(&oracle).is_err(), "unknown category must fail");
    oracle["cases"][0]["kind"] = original;
    oracle["cases"].as_array_mut().unwrap().retain(|case| case["kind"] != "catalog-host");
    assert!(inventory(&oracle).is_err(), "missing complete family must fail");
}
