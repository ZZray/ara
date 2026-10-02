//! One grouped fixed-OMP provider reference contract, including native
//! registry's Bedrock ARN example. No provider, credential or process is run.

use ara_cli::{
    model_collapse::{CollapseRuntime, VariantSpec},
    model_config_values::{HeaderConfigRecord, HeaderResolutionOptions, HeaderSource, create_live_config_headers},
    model_identity_wire::text,
    model_patch::{HeaderSlot, HostModel, HostModelRef},
    provider_model_reference::ProviderModelReferenceIndex,
};
use ara_rpc::{WireString, WireValue};
use serde_json::{Value, json};
use std::sync::{Arc, Mutex};

fn model(fields: Value, headers: HeaderSlot) -> HostModelRef {
    HostModel::new(Arc::new(VariantSpec::from_json(&fields)), headers).unwrap()
}
fn plain(provider: &str, id: &str) -> HostModelRef {
    model(json!({"provider":provider,"id":id,"name":id}), HeaderSlot::Absent)
}
fn resolve(index: &ProviderModelReferenceIndex, provider: &str, id: &str) -> Option<HostModelRef> {
    index.resolve(&provider.into(), &id.into()).unwrap()
}
fn same(index: &ProviderModelReferenceIndex, provider: &str, id: &str, expected: &HostModelRef) {
    assert!(resolve(index, provider, id).is_some_and(|actual| Arc::ptr_eq(&actual, expected)));
}

#[test]
fn provider_references_preserve_native_precedence_ambiguity_routes_and_synthetic_donors() {
    let logical = plain("google-antigravity", "gemini-3.5-flash");
    let live_variant = plain("google-antigravity", "gemini-3.5-flash-extra-low");
    let opus = plain("devin", "claude-opus-5");
    let thinking_base = plain("custom", "unit");
    let effort_base = plain("custom", "effort");
    let dotted_donor = plain("anthropic", "claude-fable-5-1");
    let aggregator = plain("openrouter", "anthropic/claude-fable-5.1");
    let routed = model(
        json!({"provider":"dynamic","id":"logical","name":"Logical",
        "thinking":{"effortRouting":{"off":"uid","minimal":"uid","future-effort":"other-uid"}}}),
        HeaderSlot::Undefined,
    );
    let wire_live = plain("dynamic", "uid");
    let first_conflict = model(
        json!({"provider":"dynamic","id":"conflict-a","name":"A",
        "thinking":{"effortRouting":{"low":"shared-uid"}}}),
        HeaderSlot::Absent,
    );
    let second_conflict = model(
        json!({"provider":"dynamic","id":"conflict-b","name":"B",
        "thinking":{"effortRouting":{"high":"shared-uid"}}}),
        HeaderSlot::Absent,
    );
    let index = ProviderModelReferenceIndex::new(&[
        logical.clone(),
        live_variant.clone(),
        opus.clone(),
        thinking_base.clone(),
        effort_base,
        dotted_donor.clone(),
        aggregator.clone(),
        routed.clone(),
        wire_live.clone(),
        first_conflict,
        second_conflict,
        plain("p", "r-5.1.2"),
        plain("p", "r-5-1-2"),
        plain("p", "model-a"),
    ])
    .unwrap();
    same(&index, " \u{feff}GOOGLE-ANTIGRAVITY\u{a0}", " GEMINI-3.5-FLASH-EXTRA-LOW ", &live_variant);
    same(&index, "google-antigravity", "gemini-3.5-flash-low", &logical);
    same(&index, "devin", "opus", &opus);
    same(&index, "custom", "unit-thinking", &thinking_base);
    assert!(resolve(&index, "custom", "effort-high").is_none(), "an effort suffix is not thinking grammar");
    assert!(
        resolve(&index, "custom", "unit-thinking-infix").is_none(),
        "provider reference grammar only strips a trailing taxonomy suffix"
    );
    same(&index, "dynamic", "uid", &wire_live);
    same(&index, "dynamic", "OTHER-UID", &routed);
    assert!(resolve(&index, "dynamic", "shared-uid").is_none());
    same(&index, "anthropic", "CLAUDE-FABLE-5.1", &dotted_donor);
    same(&index, "openrouter", "anthropic/claude-fable-5.1", &aggregator);
    assert!(
        resolve(&index, "anthropic", "anthropic/claude-fable-5.1").is_none(),
        "provider scoping cannot rebind to an aggregator"
    );
    assert!(resolve(&index, "p", "r-5.1-2").is_none(), "ambiguous revision spelling stays unresolved");
    assert!(resolve(&index, "p", "model.a").is_none(), "a nondigit-bounded dot stays literal");
    assert!(resolve(&index, " ", "unit").is_none());
    assert!(resolve(&index, "custom", "\u{feff}").is_none());

    // Exact duplicate sentinels never recover on a third occurrence. The
    // wire-route reverse index instead permits repeated routes on one object.
    let duplicate = plain("p", "duplicate");
    let ambiguous = ProviderModelReferenceIndex::new(&[duplicate.clone(), duplicate.clone(), duplicate]).unwrap();
    assert!(resolve(&ambiguous, "P", "DUPLICATE").is_none());
    let route_only = ProviderModelReferenceIndex::new(std::slice::from_ref(&routed)).unwrap();
    same(&route_only, "dynamic", "uid", &routed);
    assert!(matches!(resolve(&route_only, "dynamic", "uid").unwrap().headers(), HeaderSlot::Undefined));
    let route_duplicate = ProviderModelReferenceIndex::new(&[routed.clone(), routed.clone()]).unwrap();
    same(&route_duplicate, "dynamic", "uid", &routed);

    // The exact phase does not build/inspect the reverse index. A malformed
    // foreign route is reached only after an exact miss, as in native code.
    let malformed = model(
        json!({"provider":"broken","id":"literal","name":"Literal",
        "thinking":{"effortRouting":{"low":null}}}),
        HeaderSlot::Absent,
    );
    let malformed_index = ProviderModelReferenceIndex::new(std::slice::from_ref(&malformed)).unwrap();
    same(&malformed_index, "broken", "literal", &malformed);
    assert!(malformed_index.resolve(&"broken".into(), &"missing".into()).is_err());

    // Preserve Host-owned module state: a retired wire alias learned on an
    // earlier snapshot still selects a currently live logical donor.
    let collapse_runtime = Arc::new(Mutex::new(CollapseRuntime::new().unwrap()));
    let old_carrier =
        Arc::new(VariantSpec::from_json(&json!({"provider":"learned-test","id":"logical","name":"Logical",
        "thinking":{"effortRouting":{"high":"retired-wire"}}})));
    collapse_runtime.lock().unwrap().collapse_variants(&[old_carrier], None).unwrap();
    let new_carrier = plain("learned-test", "logical");
    let learned =
        ProviderModelReferenceIndex::with_collapse_runtime(std::slice::from_ref(&new_carrier), collapse_runtime)
            .unwrap();
    same(&learned, "learned-test", "retired-wire", &new_carrier);

    // Authored UTF-16 is never projected through a lossy UTF-8 donor map.
    let mut wire_spec = VariantSpec::from_json(&json!({"provider":"wire","name":"Wire"}));
    wire_spec.set("id", WireValue::String(WireString::from_units(vec![77, 0xd800, 45, 49, 45, 50])));
    let wire_donor = HostModel::new(Arc::new(wire_spec), HeaderSlot::Null).unwrap();
    let wire_index = ProviderModelReferenceIndex::new(std::slice::from_ref(&wire_donor)).unwrap();
    let wire_query = WireString::from_units(vec![32, 109, 0xd800, 45, 49, 46, 50, 32]);
    assert!(
        wire_index
            .resolve(&"WIRE".into(), &wire_query)
            .unwrap()
            .is_some_and(|actual| Arc::ptr_eq(&actual, &wire_donor))
    );

    // A fallback clones the actual donor's built fields and opaque headers;
    // it does not rebuild policy for the new route or consult another provider.
    let live_headers = create_live_config_headers(
        &[Some(HeaderSource::Config(HeaderConfigRecord::default()))],
        HeaderResolutionOptions::default(),
    )
    .unwrap();
    let dated = model(
        json!({"provider":"openrouter","id":"lab/unit-20260901","name":"lab/unit-20260901",
        "contextWindow":123,"compat":{"source":"dated"},"identity":{"class":"donor-only"}}),
        HeaderSlot::Source(HeaderSource::Live(live_headers.clone())),
    );
    let routed_base = model(
        json!({"provider":"openrouter","id":"lab/unit:free","name":"Routed base","contextWindow":456}),
        HeaderSlot::Absent,
    );
    let base = model(
        json!({"provider":"openrouter","id":"lab/base","name":"Display name","contextWindow":789}),
        HeaderSlot::Null,
    );
    let router = ProviderModelReferenceIndex::new(&[dated.clone(), routed_base.clone(), base.clone()]).unwrap();
    let requested = "lab/unit-20260901:free";
    let fallback = resolve(&router, "OPENROUTER", requested).unwrap();
    assert!(!Arc::ptr_eq(&fallback, &dated));
    assert_eq!(text(fallback.spec(), "id"), Some(requested.into()));
    assert_eq!(text(fallback.spec(), "name"), Some(requested.into()));
    assert_eq!(
        fallback.spec().get("contextWindow"),
        Some(&WireValue::Number(123.0)),
        "route removal precedes date removal in the BFS"
    );
    assert_eq!(
        fallback.spec().get_path(&["identity".into(), "class".into()]),
        Some(&WireValue::String("donor-only".into()))
    );
    live_headers.set("x-retained-donor", "synthetic-only");
    match fallback.headers() {
        HeaderSlot::Source(HeaderSource::Live(headers)) => assert!(headers.delete("x-retained-donor")),
        _ => panic!("fallback lost its donor header source"),
    }
    let display = resolve(&router, "openrouter", "lab/base:free").unwrap();
    assert_eq!(text(display.spec(), "name"), Some("Display name".into()));
    assert!(matches!(display.headers(), HeaderSlot::Null));
    for id in ["lab/base:hi", "lab/base:xhi", "lab/base:ma", "lab/base:auto", "lab/base:inherit", "lab/base:"] {
        assert!(
            resolve(&router, "openrouter", id).is_none(),
            "thinking/empty suffix {id} must not become a route clone"
        );
    }
    assert!(
        resolve(&router, "openrouter", "lab/base:constructor").is_some(),
        "prototype names are not thinking selectors"
    );
    assert!(
        resolve(&router, "openrouter", "lab/base:HIGH").is_some(),
        "native thinking suffix parsing is case-sensitive"
    );
    assert!(
        resolve(&router, "openrouter", " lab/base:free ").is_none(),
        "fallback candidates retain their original leading whitespace"
    );
    let ambiguous_router = ProviderModelReferenceIndex::new(&[dated.clone(), dated, routed_base]).unwrap();
    assert!(
        resolve(&ambiguous_router, "openrouter", requested).is_none(),
        "the first ambiguous fallback cannot fall through to a later winner"
    );
    let literal_max = plain("openrouter", "lab/base:max");
    let literal_router = ProviderModelReferenceIndex::new(&[base, literal_max.clone()]).unwrap();
    same(&literal_router, "openrouter", "lab/base:max", &literal_max);

    // Native registry test's profile ARN, plus strict suffix/partition guards.
    let bedrock_headers = create_live_config_headers(
        &[Some(HeaderSource::Config(HeaderConfigRecord::default()))],
        HeaderResolutionOptions::default(),
    )
    .unwrap();
    let mut template_fields = VariantSpec::from_json(
        &json!({"provider":"AMAZON-BEDROCK","id":"us.anthropic.claude-opus-4-8",
        "name":"Template","api":"bedrock-converse-stream","baseUrl":"https://bedrock-runtime.us-east-1.amazonaws.com",
        "reasoning":true,"transport":null,"guardrailIdentifier":"guard","guardrailVersion":null,"requestMetadata":{"tenant":"unit"},"authHeader":true}),
    );
    template_fields.set_undefined("guardrailVersion");
    let template =
        HostModel::new(Arc::new(template_fields), HeaderSlot::Source(HeaderSource::Live(bedrock_headers.clone())))
            .unwrap();
    let bedrock = ProviderModelReferenceIndex::new(std::slice::from_ref(&template)).unwrap();
    let arn = "arn:aws:bedrock:us-east-2:123456789012:application-inference-profile/company-opus-48";
    let profile = resolve(&bedrock, "Amazon-Bedrock", &format!(" {arn} ")).unwrap();
    assert_eq!(text(profile.spec(), "id"), Some(arn.into()));
    assert_eq!(text(profile.spec(), "provider"), Some("amazon-bedrock".into()));
    assert_eq!(text(profile.spec(), "api"), Some("bedrock-converse-stream".into()));
    assert_eq!(text(profile.spec(), "name"), Some("Bedrock inference profile".into()));
    assert_eq!(profile.spec().get("reasoning"), Some(&WireValue::Bool(false)));
    assert!(profile.spec().get("thinking").is_none());
    assert_eq!(profile.spec().get("contextWindow"), Some(&WireValue::Null));
    assert_eq!(profile.spec().get("transport"), Some(&WireValue::Null));
    assert_eq!(text(profile.spec(), "guardrailIdentifier"), Some("guard".into()));
    assert!(!profile.spec().own_keys().iter().any(|key| key.equals_ascii("guardrailVersion")));
    assert!(profile.spec().get("authHeader").is_none());
    assert!(profile.spec().get_path(&["requestMetadata".into(), "tenant".into()]).is_some());
    bedrock_headers.set("x-retained-profile", "synthetic-only");
    match profile.headers() {
        HeaderSlot::Source(HeaderSource::Live(headers)) => assert!(headers.delete("x-retained-profile")),
        _ => panic!("profile lost its template header source"),
    }
    assert!(resolve(&bedrock, "amazon-bedrock", &format!("{arn}:high")).is_none());
    assert!(resolve(&bedrock, "amazon-bedrock", &format!("{arn}:hi")).is_none());
    assert!(
        resolve(&bedrock, "amazon-bedrock", &format!("{arn}:max")).is_some(),
        "strict profile suffix guard does not opt into max"
    );
    assert!(
        resolve(&bedrock, " amazon-bedrock ", arn).is_none(),
        "the native profile helper does not trim provider input"
    );
    assert!(
        resolve(&bedrock, "amazon-bedrock", "arn:aws-us-gov:bedrock:us-gov-east-1::inference-profile/unit").is_some()
    );
    assert!(resolve(&bedrock, "amazon-bedrock", "arn:aws:bedrock:us-east-1:account:inference-profile/unit").is_none());
    assert!(resolve(&bedrock, "other", arn).is_none());
    let literal_profile = plain("amazon-bedrock", &format!("{arn}:high"));
    let literal_bedrock = ProviderModelReferenceIndex::new(&[template, literal_profile.clone()]).unwrap();
    same(&literal_bedrock, "amazon-bedrock", &format!("{arn}:high"), &literal_profile);
}
