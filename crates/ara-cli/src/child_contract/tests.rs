//! Concentrated child contract banks; Native actor/process admission is covered
//! by child_native_contract integration, not inferred from direct body checks.
use super::*;
use ara_rpc::value_heap::{
    CallableTarget, CapabilityId, PropertyDescriptor, RuntimeReply, RuntimeRequest, ValueError,
    ValueHeap, ValueRuntime,
};
use serde_json::json;
use std::sync::atomic::{AtomicUsize, Ordering};
fn owner(
    schema: Option<serde_json::Value>,
    items: Vec<WorkPoolYieldItem>,
) -> Arc<ChildContractOwner> {
    let native = crate::native_factory::PreparedNativeSession::prepare().unwrap();
    ChildContractOwner::new(
        native.values().clone(),
        ChildContractConfig {
            system_prompt: vec!["base".into()],
            output_schema: schema,
            output_schema_mode: "permissive".into(),
            structured: true,
            work_pool_items: items,
        },
    )
    .unwrap()
}
fn body(
    o: &ChildContractOwner,
    args: serde_json::Value,
) -> Result<ara_rpc::value_heap::ValueHandle, ValueError> {
    let v = o.values();
    yield_tool::execute_graph(
        v.heap(),
        &o.graph,
        &v.heap().import_serde(&args)?,
        v.runtime(),
    )
}
fn details(o: &ChildContractOwner, result: &ara_rpc::value_heap::ValueHandle) -> serde_json::Value {
    let v = o.values();
    let h = v.heap();
    let details = schema::get(h, result, "details", v.runtime()).unwrap();
    // This helper checks the JSON projection of the original result envelope.
    // Yield details intentionally contain own undefined data/type fields;
    // JSON.stringify omits them, while the strict serde bridge rejects them.
    let json = h
        .stringify_native(&details, None, None, v.runtime())
        .unwrap()
        .expect("yield details must have a JSON object projection");
    serde_json::from_str(&json.to_utf8().unwrap()).unwrap()
}
#[cfg_attr(test, test)]
pub fn child_schema_yield_bank() {
    closed_section_cases();
    strict_null_cases();
    schema_family_cases();
    result_assembly_cases();
    section_retry_cases();
    let jtd = json!({"properties":{"name":{"type":"string"}},"optionalProperties":{"count":{"type":"uint32"}}});
    assert_eq!(
        schema::jtd_to_json_schema(&jtd),
        json!({"type":"object","properties":{"name":{"type":"string"},"count":{"type":"integer"}},"required":["name"],"additionalProperties":false})
    );
    assert_eq!(
        schema::jtd_to_json_schema(&json!({"properties":{"x":{"minLength":1}},"required":["x"]})),
        json!({"properties":{"x":{"minLength":1}},"required":["x"]})
    );
    let schema = json!({"type":"object","properties":{"x":{"type":"integer"}},"required":["x"],"additionalProperties":false});
    let o = owner(Some(schema), Vec::new());
    for _ in 0..3 {
        assert!(body(&o, json!({"data":{"x":"bad"}})).is_err());
    }
    let overridden = body(&o, json!({"data":{"x":"bad"}})).unwrap();
    assert_eq!(details(&o, &overridden)["schemaOverridden"], true);
    assert!(
        body(&o, json!({"data":{"x":"bad"}})).is_err(),
        "consecutive override budget resets"
    );
    let recovered = body(&o, json!({"data":"{\"x\":3}"})).unwrap();
    assert_eq!(details(&o, &recovered)["data"], json!({"x":3}));
    assert!(body(&o, json!({"type":["unknown"],"data":5})).is_err());
    assert!(
        body(&o, json!({"type":"result"})).is_err(),
        "schema-bound empty terminal is retryable"
    );
    let o = owner(None, Vec::new());
    for _ in 0..3 {
        assert!(body(&o, json!({})).is_err())
    }
    assert_eq!(
        details(&o, &body(&o, json!({})).unwrap())["status"],
        "aborted"
    );
    assert!(body(&o, json!({"data":false,"error":"failure"})).is_err());
    assert_eq!(
        details(&o, &body(&o, json!({"data":false,"error":""})).unwrap())["data"],
        false
    );
}
#[cfg_attr(test, test)]
pub fn child_pool_and_original_graph_bank() {
    turn_and_live_parameter_cases();
    let items = vec![
        WorkPoolYieldItem {
            id: "a".into(),
            index: 1,
        },
        WorkPoolYieldItem {
            id: "b".into(),
            index: 2,
        },
    ];
    let o = owner(None, items.clone());
    assert!(body(&o, json!({"key":"1","data":{}})).is_err());
    assert!(
        !details(&o, &body(&o, json!({"key":1,"data":false})).unwrap())
            .get("complete")
            .is_some_and(|v| v == true)
    );
    assert!(body(&o, json!({"key":1,"data":{}})).is_err());
    assert_eq!(
        details(&o, &body(&o, json!({"key":2,"data":{}})).unwrap())["complete"],
        true
    );
    yield_tool::set_items(o.values(), &o.graph, &items).unwrap();
    assert!(
        body(&o, json!({"key":1,"data":{}})).is_err(),
        "same batch retains submitted identities"
    );
    yield_tool::set_items(
        o.values(),
        &o.graph,
        &[WorkPoolYieldItem {
            id: "new".into(),
            index: 1,
        }],
    )
    .unwrap();
    assert_eq!(
        details(&o, &body(&o, json!({"key":1,"error":"blocked"})).unwrap())["status"],
        "aborted"
    );
    // Submitted data is never exported to serde by the tool callback.
    let v = o.values();
    let h = v.heap();
    let raw = h
        .string(ara_rpc::WireString::from_units(vec![0xd800]))
        .unwrap();
    let args = h.object().unwrap();
    h.define_own_data(&args, "key".into(), h.number(1.0).unwrap(), v.runtime())
        .unwrap();
    h.define_own_data(&args, "data".into(), raw.clone(), v.runtime())
        .unwrap();
    let result = yield_tool::execute_graph(h, &o.graph, &args, v.runtime()).unwrap();
    let d = schema::get(h, &result, "details", v.runtime()).unwrap();
    assert!(
        schema::get(h, &d, "data", v.runtime())
            .unwrap()
            .same_identity(&raw)
    );
}
struct Getter<'a> {
    base: &'a dyn ValueRuntime,
    calls: AtomicUsize,
}
impl ValueRuntime for Getter<'_> {
    fn resolve(&self, request: RuntimeRequest<'_>) -> Result<RuntimeReply, ValueError> {
        if let RuntimeRequest::Call {
            capability: CapabilityId(98001),
            receiver,
            ..
        } = request
        {
            self.calls.fetch_add(1, Ordering::SeqCst);
            return receiver.heap().number(7.0).map(RuntimeReply::Value);
        }
        self.base.resolve(request)
    }
}
#[cfg_attr(test, test)]
pub fn child_schema_borrowed_resolver_bank() {
    let o = owner(None, Vec::new());
    let h = o.values().heap();
    let runtime = Getter {
        base: o.values().runtime(),
        calls: AtomicUsize::new(0),
    };
    let value = h.object().unwrap();
    h.define_own(
        &value,
        "x".into(),
        PropertyDescriptor::Accessor {
            get: Some(CallableTarget::Host(CapabilityId(98001))),
            set: None,
            enumerable: true,
            configurable: true,
        },
        &runtime,
    )
    .unwrap();
    let schema = h
        .import_serde(
            &json!({"type":"object","properties":{"x":{"type":"number"}},"required":["x"]}),
        )
        .unwrap();
    assert!(
        schema::validate(h, &schema, &value, &runtime)
            .unwrap()
            .is_none()
    );
    assert!(runtime.calls.load(Ordering::SeqCst) > 0);
    // Extend the borrowed resolver through the real body, receipt and finish.
    let constrained = owner(
        Some(json!({"type":"object","properties":{"x":{"type":"number"}},"required":["x"]})),
        Vec::new(),
    );
    let values = constrained.values();
    let heap = values.heap();
    let runtime = Getter {
        base: values.runtime(),
        calls: AtomicUsize::new(0),
    };
    let payload = heap.object().unwrap();
    heap.define_own(
        &payload,
        "x".into(),
        PropertyDescriptor::Accessor {
            get: Some(CallableTarget::Host(CapabilityId(98001))),
            set: None,
            enumerable: true,
            configurable: true,
        },
        &runtime,
    )
    .unwrap();
    let args = heap.object().unwrap();
    schema::put(heap, &args, "data", payload.clone(), &runtime).unwrap();
    let result = yield_tool::execute_graph(heap, &constrained.graph, &args, &runtime).unwrap();
    let receipt = yield_tool::read_receipt(heap, &result, &runtime).unwrap();
    assert!(receipt.data.as_ref().unwrap().same_identity(&payload));
    let observer = ChildRunObserver::new(constrained.clone(), 0);
    observer.admit("borrowed".into());
    observer.observe("borrowed".into(), receipt).unwrap();
    let finished = observer
        .finish_with_runtime(
            ChildRunFinish {
                clean_exit: true,
                cancelled: false,
                owned_jobs_settled: true,
                last_assistant_text: None,
            },
            &runtime,
        )
        .unwrap();
    assert!(finished.accepted);
    assert!(finished.data.unwrap().same_identity(&payload));
    assert!(runtime.calls.load(Ordering::SeqCst) >= 2);
}
#[cfg_attr(test, tokio::test)]
pub async fn child_publication_dispatch_bank() {
    overlapping_publication_cases().await;
    let o = owner(None, Vec::new());
    o.initialize_published_prompt(vec!["actual".into()])
        .unwrap();
    assert!(o.initialize_published_prompt(Vec::new()).is_err());
    let ticket = o
        .reserve_items(vec![WorkPoolYieldItem {
            id: "one".into(),
            index: 1,
        }])
        .unwrap();
    assert!(o.try_claim_dispatch(DispatchKind::Ordinary, false).is_err());
    o.publish(ticket, |s| async move {
        Ok(vec![format!("items:{}", s.items.len())])
    })
    .await
    .unwrap();
    let lease = o.try_claim_dispatch(DispatchKind::Pooled, false).unwrap();
    assert!(o.reserve_items(Vec::new()).is_err());
    drop(lease);
    let clear = o.reserve_items(Vec::new()).unwrap();
    let calls = AtomicUsize::new(0);
    assert!(
        o.publish(clear, |s| {
            let n = calls.fetch_add(1, Ordering::SeqCst);
            async move {
                if n == 0 {
                    Err(ChildContractError("actual renderer rejected".into()))
                } else {
                    Ok(s.system_prompt)
                }
            }
        })
        .await
        .is_err()
    );
    assert_eq!(o.published_snapshot().items.len(), 1);
    assert!(o.publication_error().is_none());
    let clear = o.reserve_items(Vec::new()).unwrap();
    o.publish(clear, |_| async { Ok(vec!["clear".into()]) })
        .await
        .unwrap();
    let lease = o.try_claim_dispatch(DispatchKind::Ordinary, false).unwrap();
    drop(lease);
}

// Source case identities are collected in CASE-MAP.md. These are groups inside
// the existing module banks, not separately compiled targets.
fn graph_value(o: &ChildContractOwner, key: &str) -> serde_json::Value {
    let v = o.values();
    v.heap()
        .export_serde(&schema::get(v.heap(), &o.graph, key, v.runtime()).unwrap())
        .unwrap()
}
fn graph_set(o: &ChildContractOwner, key: &str, value: serde_json::Value) {
    let v = o.values();
    schema::put(
        v.heap(),
        &o.graph,
        key,
        v.heap().import_serde(&value).unwrap(),
        v.runtime(),
    )
    .unwrap();
}
fn closed_section_cases() {
    let closed = json!({"type":"object","properties":{"verdict":{"enum":["clean","blockers"]},"notes":true},"additionalProperties":false});
    for declaration in [
        closed.clone(),
        json!({"$ref":"#/$defs/Closed","$defs":{"Closed":closed}}),
        json!({"allOf":[closed]}),
    ] {
        let o = owner(Some(declaration), Vec::new());
        for _ in 0..5 {
            assert!(body(&o, json!({"type":["findings"],"data":{}})).is_err());
        }
        assert!(body(&o, json!({"type":["findings"]})).is_err());
        assert_eq!(
            graph_value(&o, "schemaFailures"),
            0,
            "unknown closed labels never consume retries"
        );
        let shape_result = body(&o, json!({"type":["verdict"],"data":"approved"}));
        // allOf declares labels but intentionally has no concrete root section validator.
        if graph_value(&o, "labelSchema").get("allOf").is_none() {
            assert!(shape_result.is_err());
            assert_eq!(graph_value(&o, "schemaFailures"), 1);
        } else {
            assert!(shape_result.is_ok());
        }
    }
    let o = owner(
        Some(json!({"type":"object","properties":{"notes":true},"additionalProperties":false})),
        Vec::new(),
    );
    assert_eq!(
        details(
            &o,
            &body(&o, json!({"type":["notes"],"data":"plain text"})).unwrap()
        )["data"],
        "plain text"
    );
    let o = owner(
        Some(
            json!({"type":"object","patternProperties":{"^section_[a-z]+$":{"type":"object"}},"additionalProperties":false}),
        ),
        Vec::new(),
    );
    assert!(body(&o, json!({"type":["section_alpha"],"data":{"ok":true}})).is_ok());
    assert!(body(&o, json!({"type":["findings"],"data":{}})).is_err());
    let tagged = json!({"discriminator":"verdict","mapping":{"clean":{"properties":{"issue_key":{"type":"string"}}},"blockers":{"properties":{"issue_key":{"type":"string"}},"optionalProperties":{"blockers":{"elements":{"properties":{"title":{"type":"string"}}}}}}}});
    let o = owner(Some(tagged), Vec::new());
    assert!(body(&o, json!({"type":["findings"],"data":{}})).is_err());
    assert!(
        body(
            &o,
            json!({"type":["blockers"],"data":{"title":"one variant"}})
        )
        .is_ok()
    );
    assert!(body(&o, json!({"type":["verdict"],"data":"blockers"})).is_ok());
    for schema in [
        json!({"type":"object","additionalProperties":true}),
        json!({"oneOf":[{"type":"object","properties":{"known":{"type":"string"}},"additionalProperties":false},{"type":"object","properties":{"notes":{"type":"string"}}}]}),
    ] {
        let o = owner(Some(schema), Vec::new());
        assert!(
            body(
                &o,
                json!({"type":["scratchpad"],"data":{"anything":"goes"}})
            )
            .is_ok()
        );
    }
}
fn strict_null_cases() {
    let nested = json!({"properties":{"status":{"type":"string"},"steps":{"elements":{"properties":{"name":{"type":"string"}},"optionalProperties":{"blocker":{"type":"string"},"nullable":{"type":"string","nullable":true}}}}},"optionalProperties":{"receipt":{"type":"string"}}});
    let o = owner(Some(nested), Vec::new());
    let got=body(&o,json!({"type":null,"error":null,"data":{"status":"done","steps":[{"name":"one","blocker":null,"nullable":null}],"receipt":null}})).unwrap();
    assert_eq!(
        details(&o, &got)["data"],
        json!({"status":"done","steps":[{"name":"one","nullable":null}]})
    );
    assert!(body(&o, json!({"data":{"status":null,"steps":[{"name":"one"}]}})).is_err());
    let prefix = json!({"type":"object","properties":{"id":{"type":"string"},"note":{"type":"string"}},"required":["id"],"additionalProperties":false});
    let trailing = json!({"type":"object","properties":{"label":{"type":"string"},"note":{"type":"string"}},"required":["label"],"additionalProperties":false});
    let o = owner(
        Some(
            json!({"type":"object","properties":{"tuple":{"type":"array","prefixItems":[prefix],"items":trailing,"minItems":2,"maxItems":2}},"required":["tuple"],"additionalProperties":false}),
        ),
        Vec::new(),
    );
    assert_eq!(graph_value(&o, "schemaStrict"), true);
    assert_eq!(
        details(
            &o,
            &body(
                &o,
                json!({"data":{"tuple":[{"id":"done","note":null},{"label":"next","note":null}]}})
            )
            .unwrap()
        )["data"],
        json!({"tuple":[{"id":"done"},{"label":"next"}]})
    );
    assert!(body(&o, json!({"data":{"tuple":[{"id":null},{"label":"next"}]}})).is_err());
    let o = owner(
        Some(
            json!({"discriminator":"kind","mapping":{"done":{"properties":{"status":{"type":"string"}},"optionalProperties":{"blocker":{"type":"string"}}},"blocked":{"properties":{"reason":{"type":"string"}}}}}),
        ),
        Vec::new(),
    );
    assert_eq!(
        details(
            &o,
            &body(
                &o,
                json!({"data":{"kind":"done","status":"complete","blocker":null}})
            )
            .unwrap()
        )["data"],
        json!({"kind":"done","status":"complete"})
    );
    // Independent review finding: advertised schema and original Graph callback
    // must both retain caller-allowed required object/null values.
    let nullable = json!({"type":["object","null"],"properties":{"x":{"type":"string"}},"required":["x"],"additionalProperties":false});
    let declaration = json!({"type":"object","properties":{"p":nullable},"required":["p"],"additionalProperties":false});
    let built = schema::build(Some(&declaration));
    assert!(built.strict);
    let data = &built.parameters["properties"]["data"];
    let full = data.get("anyOf").and_then(|a| a.get(0)).unwrap_or(data);
    let alternatives = full["properties"]["p"]["anyOf"].as_array().unwrap();
    assert!(alternatives.contains(&json!({"type":"null"})));
    assert!(
        alternatives
            .iter()
            .any(|v| v["type"] == "object" && v["required"] == json!(["x"]))
    );
    let o = owner(Some(declaration), Vec::new());
    assert_eq!(
        details(&o, &body(&o, json!({"data":{"p":null}})).unwrap())["data"],
        json!({"p":null})
    );
}
fn schema_family_cases() {
    // Validator declarations: absent, true, false, malformed, native JSON,
    // mixed JTD, local refs, external refs, and literal $ref data.
    assert!(schema::build(None).normalized.is_none());
    let truth = schema::build(Some(&json!(true)));
    assert_eq!(truth.normalized, Some(json!(true)));
    assert!(truth.json_schema.is_none());
    for invalid in [
        json!(false),
        json!(123),
        json!([]),
        json!({"type":"invalid"}),
        json!({"type":"object","required":"x"}),
        json!("{broken"),
    ] {
        let built = schema::build(Some(&invalid));
        assert!(built.error.is_some());
        assert!(!built.strict);
        assert!(built.json_schema.is_none());
        let o = owner(Some(invalid), Vec::new());
        assert!(body(&o, json!({"data":{"accepted":"loose"}})).is_ok());
    }
    let changed = json!({"properties":{"result":{"type":"string"}}});
    let first = schema::build(Some(&changed));
    assert_eq!(
        first.json_schema.as_ref().unwrap()["properties"]["result"]["type"],
        "string"
    );
    let mut changed = changed;
    changed["properties"]["result"]["type"] = json!("int32");
    let second = schema::build(Some(&changed));
    assert_eq!(
        second.json_schema.as_ref().unwrap()["properties"]["result"]["type"],
        "integer"
    );
    for keyword in [
        "not",
        "if",
        "then",
        "else",
        "items",
        "contains",
        "propertyNames",
        "additionalProperties",
        "unevaluatedProperties",
        "unevaluatedItems",
        "contentSchema",
    ] {
        let mut declaration = json!({});
        declaration[keyword] = json!({"elements":{"type":"int32"}});
        let normalized = schema::jtd_to_json_schema(&declaration);
        assert_eq!(
            normalized[keyword],
            json!({"type":"array","items":{"type":"integer"}})
        );
    }
    for keyword in ["allOf", "anyOf", "oneOf", "prefixItems"] {
        let mut declaration = json!({});
        declaration[keyword] = json!([{"elements":{"type":"int32"}}]);
        assert_eq!(
            schema::jtd_to_json_schema(&declaration)[keyword][0],
            json!({"type":"array","items":{"type":"integer"}})
        );
    }
    for keyword in [
        "properties",
        "patternProperties",
        "$defs",
        "definitions",
        "dependentSchemas",
    ] {
        let mut declaration = json!({"required":[]});
        declaration[keyword] = json!({"elements":{"elements":{"type":"int32"}}});
        assert_eq!(
            schema::jtd_to_json_schema(&declaration)[keyword]["elements"],
            json!({"type":"array","items":{"type":"integer"}})
        );
    }
    let malformed = json!({"type":"object","properties":[],"items":[{"type":"int32"}],"dependencies":{"x":{"elements":{"type":"int32"}}}});
    assert_eq!(schema::jtd_to_json_schema(&malformed), malformed);
    assert!(!schema::valid_schema(&malformed));
    let native = json!({"properties":{"elements":{"type":"string"},"values":{"type":"boolean"},"discriminator":{"type":"string"}},"required":["elements"],"default":{"elements":{"type":"int32"}},"examples":[{"ref":"literal"}]});
    assert_eq!(
        schema::jtd_to_json_schema(&native),
        native,
        "data and property names are not schema positions"
    );
    let mixed = json!({"type":"object","properties":{"results":{"type":"array","elements":{"properties":{"issue":{"type":"int32"}}}}},"required":["results"]});
    let o = owner(Some(mixed), Vec::new());
    assert!(body(&o, json!({"data":{"results":[{"issue":185}]}})).is_ok());
    assert!(body(&o, json!({"data":{"results":[{"issue":"185"}]}})).is_err());
    for (declaration, valid, invalid) in [
        (
            json!({"elements":{"type":"int32"}}),
            json!([1, 2]),
            json!([1, "two"]),
        ),
        (
            json!({"values":{"type":"boolean"}}),
            json!({"a":true}),
            json!({"a":"true"}),
        ),
        (
            json!({"type":"object","properties":{"value":{"$ref":"#/$defs/A"}},"required":["value"],"$defs":{"A":{"type":"string","minLength":3}}}),
            json!({"value":"long"}),
            json!({"value":"x"}),
        ),
    ] {
        let o = owner(Some(declaration), Vec::new());
        assert!(body(&o, json!({"data":valid})).is_ok());
        assert!(body(&o, json!({"data":invalid})).is_err());
    }
    // The exact fixed 20,000-layer programmatic declaration must fall back
    // without recursive clone or teardown; the owner still retains the input.
    let mut deep =
        json!({"type":"object","properties":{"value":{"type":"string"}},"required":["value"]});
    for _ in 0..20_000 {
        let mut properties = serde_json::Map::new();
        properties.insert("next".into(), deep);
        let mut node = serde_json::Map::new();
        node.insert("type".into(), json!("object"));
        node.insert("properties".into(), serde_json::Value::Object(properties));
        node.insert("required".into(), json!(["next"]));
        deep = serde_json::Value::Object(node);
    }
    let built = schema::build(Some(&deep));
    assert!(!built.strict);
    assert!(built.json_schema.is_none());
    assert!(built.error.is_some());
    let o = owner(Some(deep), Vec::new());
    assert!(o.config().output_schema.is_some());
    assert!(body(&o, json!({"data":{"nested":true}})).is_ok());
    drop(o);
    let recursive = json!({"$defs":{"Node":{"type":"object","properties":{"next":{"$ref":"#/$defs/Node"}}}},"$ref":"#/$defs/Node"});
    let rendered = schema::dereference(&recursive);
    assert_eq!(rendered["properties"]["next"], json!({}));
    let o = owner(Some(recursive), Vec::new());
    assert!(body(&o, json!({"data":{"next":{}}})).is_ok());
    // Medium nested declarations retain the leaf constraint: no 128-layer
    // artificial loose fallback is allowed. Check the existing Graph validator.
    let mut medium = json!({"type":"string"});
    let mut good = json!("leaf");
    let mut bad = json!(7);
    for _ in 0..512 {
        let mut props = serde_json::Map::new();
        props.insert("next".into(), medium);
        let mut node = serde_json::Map::new();
        node.insert("type".into(), json!("object"));
        node.insert("properties".into(), serde_json::Value::Object(props));
        node.insert("required".into(), json!(["next"]));
        node.insert("additionalProperties".into(), json!(false));
        medium = serde_json::Value::Object(node);
        let mut map = serde_json::Map::new();
        map.insert("next".into(), good);
        good = serde_json::Value::Object(map);
        let mut map = serde_json::Map::new();
        map.insert("next".into(), bad);
        bad = serde_json::Value::Object(map);
    }
    let built = schema::build(Some(&medium));
    assert!(built.strict);
    assert!(built.error.is_none());
    let native = crate::native_factory::PreparedNativeSession::prepare().unwrap();
    let values = native.values();
    let heap = values.heap();
    let declaration = heap
        .import_serde(built.json_schema.as_ref().unwrap())
        .unwrap();
    let good_graph = heap.import_serde(&good).unwrap();
    let bad_graph = heap.import_serde(&bad).unwrap();
    assert!(
        schema::validate(heap, &declaration, &good_graph, values.runtime())
            .unwrap()
            .is_none()
    );
    assert!(
        schema::validate(heap, &declaration, &bad_graph, values.runtime())
            .unwrap()
            .is_some()
    );
    schema::drop_declaration(medium);
    schema::drop_declaration(good);
    schema::drop_declaration(bad);
    let external = schema::build(Some(&json!({"$ref":"https://example.invalid/schema"})));
    assert!(!external.strict);
    assert!(external.error.is_some());
    let literal = json!({"type":"object","properties":{"x":{"enum":[{"$ref":"literal-data"}]}},"required":["x"]});
    assert!(schema::build(Some(&literal)).error.is_none());
    let o = owner(Some(literal), Vec::new());
    assert!(body(&o, json!({"data":{"x":{"$ref":"literal-data"}}})).is_ok());
    for declaration in [None, Some(json!(null)), Some(json!(true))] {
        let o = owner(declaration, Vec::new());
        for payload in [json!(false), json!(42), json!([1, 2]), json!({"x":1})] {
            assert!(body(&o, json!({"data":payload})).is_ok());
        }
    }
    let loose = schema::build(Some(&json!({"type":"object","additionalProperties":true})));
    assert!(!loose.strict);
    assert_eq!(
        loose.parameters["properties"]["data"]["additionalProperties"],
        true
    );
    let params = schema::build(None).parameters.clone();
    assert_eq!(params["properties"]["type"]["anyOf"][1]["minItems"], 1);
    for key in ["allOf", "anyOf", "oneOf", "enum", "const", "not"] {
        assert!(params.get(key).is_none());
    }
    // String recovery must run only after raw-value validation fails.
    let o = owner(Some(json!({"type":"integer"})), Vec::new());
    assert_eq!(
        details(&o, &body(&o, json!({"data":"42"})).unwrap())["data"],
        42
    );
    let o = owner(Some(json!({"type":"string"})), Vec::new());
    assert_eq!(
        details(&o, &body(&o, json!({"data":"42"})).unwrap())["data"],
        "42"
    );
    let o = owner(Some(json!({"type":"integer"})), Vec::new());
    assert!(
        body(&o, json!({"data":"\"42\""})).is_err(),
        "one JSON recovery only"
    );
    let o = owner(
        Some(
            json!({"properties":{"verdict":{"enum":["correct","incorrect"]},"count":{"type":"number"}}}),
        ),
        Vec::new(),
    );
    assert_eq!(
        details(
            &o,
            &body(&o, json!({"type":["verdict"],"data":"\"correct\""})).unwrap()
        )["data"],
        "correct"
    );
    assert_eq!(
        details(
            &o,
            &body(&o, json!({"type":["count"],"data":"42"})).unwrap()
        )["data"],
        42
    );
    assert!(body(&o, json!({"type":["verdict"],"data":"null"})).is_err());
    let o = owner(
        Some(
            json!({"properties":{"files":{"elements":{"properties":{"path":{"type":"string"},"description":{"type":"string"}}}}}}),
        ),
        Vec::new(),
    );
    let error = body(&o, json!({"data":{"files":[{"ref":"bad","surface":"g"}]}}))
        .unwrap_err()
        .to_string();
    assert!(
        error.contains("files/0/path")
            && error.contains("files/0/description")
            && error.contains("; ")
    );
}
fn observe_body(
    o: &Arc<ChildContractOwner>,
    observer: &ChildRunObserver,
    args: serde_json::Value,
    force: bool,
) {
    let result = body(o, args).unwrap();
    let mut receipt =
        yield_tool::read_receipt(o.values().heap(), &result, o.values().runtime()).unwrap();
    receipt.terminal |= force;
    let id: ara_rpc::WireString = format!("case-{}", observer.receipts().len()).into();
    observer.admit(id.clone());
    observer.observe(id, receipt).unwrap();
}
fn finish(observer: &ChildRunObserver, text: Option<&str>) -> ObservedChildResult {
    observer
        .finish(ChildRunFinish {
            clean_exit: true,
            cancelled: false,
            owned_jobs_settled: true,
            last_assistant_text: text.map(Into::into),
        })
        .unwrap()
}
fn exported(o: &ChildContractOwner, value: &ObservedChildResult) -> serde_json::Value {
    o.values()
        .heap()
        .export_serde(value.data.as_ref().unwrap())
        .unwrap()
}
fn result_assembly_cases() {
    let o = owner(None, Vec::new());
    graph_set(&o, "lastText", json!("current prose"));
    graph_set(&o, "reportText", json!("eligible preceding prose"));
    let incremental = body(&o, json!({"type":["notes"]})).unwrap();
    let ir =
        yield_tool::read_receipt(o.values().heap(), &incremental, o.values().runtime()).unwrap();
    assert!(ir.use_last_turn);
    assert!(!ir.terminal);
    // A fresh terminal sample must not inherit the preceding incremental section.
    let o = owner(None, Vec::new());
    graph_set(&o, "reportText", json!("eligible preceding prose"));
    let terminal = body(&o, json!({"type":"result"})).unwrap();
    let tr = yield_tool::read_receipt(o.values().heap(), &terminal, o.values().runtime()).unwrap();
    assert!(tr.terminal);
    assert!(!tr.use_last_turn);
    assert_eq!(details(&o, &terminal)["data"], "eligible preceding prose");
    let o = owner(None, Vec::new());
    graph_set(&o, "lastText", json!(""));
    graph_set(&o, "reportText", json!(""));
    for _ in 0..3 {
        assert!(body(&o, json!({"type":"result"})).is_err())
    }
    assert_eq!(
        details(&o, &body(&o, json!({"type":"result"})).unwrap())["status"],
        "aborted"
    );
    assert!(body(&o, json!({"data":1})).is_ok());
    assert_eq!(graph_value(&o, "emptyFailures"), 0);
    let loose = owner(None, Vec::new());
    let observer = ChildRunObserver::new(loose.clone(), 0);
    observe_body(
        &loose,
        &observer,
        json!({"type":[" notes ",""],"data":"x"}),
        false,
    );
    observe_body(&loose, &observer, json!({"type":"result"}), false);
    assert_eq!(
        exported(&loose, &finish(&observer, None)),
        json!({"notes":"x"})
    );
    let salvage = ChildRunObserver::new(loose.clone(), 0);
    let prose = finish(&salvage, Some("completed with evidence"));
    assert_eq!(prose.status, ChildResultStatus::Success);
    assert!(!prose.accepted);
    assert_eq!(exported(&loose, &prose), "completed with evidence");
    // Array sections accumulate even once; scalar sections replace; terminal
    // complete object wins and is never wrapped under its string type.
    let declaration = json!({"type":"object","properties":{"findings":{"type":"array","items":{"type":"string"}},"summary":{"type":"string"}},"required":["findings","summary"],"additionalProperties":false});
    let o = owner(Some(declaration.clone()), Vec::new());
    let obs = ChildRunObserver::new(o.clone(), 0);
    observe_body(&o, &obs, json!({"type":["findings"],"data":"one"}), false);
    observe_body(&o, &obs, json!({"type":["summary"],"data":"old"}), false);
    observe_body(&o, &obs, json!({"type":["summary"],"data":"new"}), false);
    let partial = finish(&obs, None);
    assert_eq!(
        exported(&o, &partial),
        json!({"findings":["one"],"summary":"new"})
    );
    assert!(
        !partial.accepted,
        "incremental output alone is not accepted structured delivery"
    );
    observe_body(&o, &obs, json!({"type":"result"}), false);
    let assembled = finish(&obs, None);
    assert!(assembled.accepted);
    assert_eq!(
        exported(&o, &assembled),
        json!({"findings":["one"],"summary":"new"})
    );
    observe_body(
        &o,
        &obs,
        json!({"type":"result","data":{"findings":[],"summary":"full"}}),
        false,
    );
    assert_eq!(
        exported(&o, &finish(&obs, None)),
        json!({"findings":[],"summary":"full"})
    );
    let fallback = ChildRunObserver::new(o.clone(), 0);
    let salvaged = finish(
        &fallback,
        Some("{\"data\":{\"findings\":[],\"summary\":\"fallback\"}}"),
    );
    assert_eq!(salvaged.status, ChildResultStatus::Success);
    assert!(
        !salvaged.accepted,
        "clean JSON salvage is not a real yield receipt"
    );
    // Adoption of new owned work invalidates old delivery; a fresh terminal is
    // required after its receipts, not merely a settled jobs boolean.
    obs.invalidate_for_async_work();
    assert!(!obs.has_terminal_yield());
    assert_eq!(
        finish(&obs, None).status,
        ChildResultStatus::PendingOwnedWork
    );
    observe_body(
        &o,
        &obs,
        json!({"data":{"findings":[],"summary":"after jobs"}}),
        false,
    );
    assert!(finish(&obs, None).accepted);
    for (clean, cancelled, settled, want) in [
        (false, false, true, ChildResultStatus::Failed),
        (true, true, true, ChildResultStatus::Failed),
        (true, false, false, ChildResultStatus::PendingOwnedWork),
    ] {
        let r = obs
            .finish(ChildRunFinish {
                clean_exit: clean,
                cancelled,
                owned_jobs_settled: settled,
                last_assistant_text: None,
            })
            .unwrap();
        assert_eq!(r.status, want);
        assert!(!r.accepted);
    }
    let o = owner(None, Vec::new());
    let obs = ChildRunObserver::new(o.clone(), 0);
    observe_body(&o, &obs, json!({"type":["forced"],"data":false}), true);
    assert!(obs.has_terminal_yield());
    assert!(finish(&obs, None).accepted);
    let o = owner(None, Vec::new());
    let obs = ChildRunObserver::new(o.clone(), 0);
    observe_body(&o, &obs, json!({"error":"blocked"}), false);
    let r = finish(&obs, None);
    assert_eq!(r.status, ChildResultStatus::Aborted);
    assert!(!r.accepted);
    assert_eq!(r.error.as_deref(), Some("blocked"));
    // Unstructured text uses JS trim, including non-ASCII whitespace.
    let native = crate::native_factory::PreparedNativeSession::prepare().unwrap();
    let plain = ChildContractOwner::new(
        native.values().clone(),
        ChildContractConfig {
            system_prompt: vec![],
            output_schema: None,
            output_schema_mode: "permissive".into(),
            structured: false,
            work_pool_items: vec![],
        },
    )
    .unwrap();
    let plain_observer = ChildRunObserver::new(plain, 0);
    for empty in [" ", "\u{00a0}", "\u{feff}", "\u{2003}"] {
        let result = finish(&plain_observer, Some(empty));
        assert_eq!(result.status, ChildResultStatus::Failed);
        assert!(!result.accepted);
    }
    assert!(finish(&plain_observer, Some("plain report")).accepted);
    // No receipt may be imported from an unadmitted or superseded turn.
    let forged = yield_tool::read_receipt(
        o.values().heap(),
        &body(&o, json!({"data":1})).unwrap(),
        o.values().runtime(),
    )
    .unwrap();
    assert!(obs.observe("unadmitted".into(), forged.clone()).is_err());
    graph_set(&o, "turn", json!(1));
    obs.admit("old".into());
    assert!(obs.observe("old".into(), forged).is_err());
}
async fn overlapping_publication_cases() {
    // Deterministic gates, no timing sleeps: reserve a newer generation while
    // the first actual publication is awaiting its external operation.
    for newer_fails in [false, true] {
        let o = owner(None, Vec::new());
        o.initialize_published_prompt(vec!["published ordinary".into()])
            .unwrap();
        let install = o
            .reserve_items(vec![WorkPoolYieldItem {
                id: "rejected".into(),
                index: 1,
            }])
            .unwrap();
        let (entered_tx, entered_rx) = tokio::sync::oneshot::channel();
        let (release_tx, release_rx) = tokio::sync::oneshot::channel();
        let mut entered = Some(entered_tx);
        let mut release = Some(release_rx);
        let first = o.publish(install, |_| {
            let tx = entered.take();
            let rx = release.take();
            async move {
                if let Some(tx) = tx {
                    let _ = tx.send(());
                }
                if let Some(rx) = rx {
                    rx.await.unwrap();
                }
                Err(ChildContractError("first boom".into()))
            }
        });
        let second = async {
            entered_rx.await.unwrap();
            let clear = o.reserve_items(Vec::new()).unwrap();
            assert!(o.try_claim_dispatch(DispatchKind::Ordinary, false).is_err());
            release_tx.send(()).unwrap();
            let mut count = 0;
            let result = o
                .publish(clear, |s| {
                    count += 1;
                    let attempt = count;
                    async move {
                        if newer_fails && attempt == 1 {
                            Err(ChildContractError("second boom".into()))
                        } else {
                            Ok(s.system_prompt)
                        }
                    }
                })
                .await;
            if newer_fails {
                assert!(result.is_err());
            } else {
                assert!(result.is_ok());
            }
        };
        let (first_result, ()) = tokio::join!(first, second);
        assert!(first_result.is_err());
        o.when_settled().await;
        assert!(o.published_snapshot().items.is_empty());
        assert!(o.desired_snapshot().items.is_empty());
        assert_eq!(
            o.published_snapshot().system_prompt,
            vec!["published ordinary"]
        );
        assert!(o.publication_error().is_none());
        drop(o.try_claim_dispatch(DispatchKind::Ordinary, false).unwrap());
    }
    // Failure to repair leaves admission blocked; it never silently declares
    // the unknown external pair settled and successful.
    let o = owner(None, Vec::new());
    let ticket = o
        .reserve_items(vec![WorkPoolYieldItem {
            id: "x".into(),
            index: 1,
        }])
        .unwrap();
    assert!(
        o.publish(ticket, |_| async {
            Err(ChildContractError("renderer and repair fail".into()))
        })
        .await
        .is_err()
    );
    assert!(o.publication_error().is_some());
    assert!(o.try_claim_dispatch(DispatchKind::Ordinary, false).is_err());
    // A stale candidate cannot commit a prompt/Journal pair in a check/commit gap.
    let o = owner(None, Vec::new());
    let old = o
        .reserve_items(vec![WorkPoolYieldItem {
            id: "old".into(),
            index: 1,
        }])
        .unwrap();
    let latest = o.reserve_items(Vec::new()).unwrap();
    let committed = AtomicUsize::new(0);
    assert!(
        o.with_current_publication(&old, || {
            committed.fetch_add(1, Ordering::SeqCst);
            Ok(())
        })
        .is_err()
    );
    assert_eq!(committed.load(Ordering::SeqCst), 0);
    o.publish(old, |_| async {
        panic!("superseded candidate must not execute")
    })
    .await
    .unwrap_err();
    o.publish(latest, |_| async { Ok(vec!["latest".into()]) })
        .await
        .unwrap();
    assert_eq!(o.published_snapshot().system_prompt, vec!["latest"]);
}

fn turn_and_live_parameter_cases() {
    let native = crate::native_factory::PreparedNativeSession::prepare().unwrap();
    let o=ChildContractOwner::new(native.values().clone(),ChildContractConfig{system_prompt:vec!["base".into()],output_schema:Some(json!({"type":"object","properties":{"x":{"type":"string"}},"required":["x"],"additionalProperties":false})),output_schema_mode:"permissive".into(),structured:true,work_pool_items:vec![WorkPoolYieldItem{id:"one".into(),index:1}]}).unwrap();
    let bundle = NativeYieldBundle::prepare(o.clone(), native.realm()).unwrap();
    let h = o.values().heap();
    let r = o.values().runtime();
    assert_eq!(
        h.export_serde(&schema::get(h, &bundle.original, "strict", r).unwrap())
            .unwrap(),
        false
    );
    let params = schema::get(h, &bundle.original, "parameters", r).unwrap();
    assert_eq!(
        h.export_serde(&params).unwrap()["properties"]["key"]["enum"],
        json!([1])
    );
    let first = bundle.begin_turn(1).unwrap();
    assert_eq!(first.turn_id, 1);
    assert!(body(&o, json!({"key":1,"data":{"item":"done"}})).is_ok());
    graph_set(&o, "schemaFailures", json!(2));
    graph_set(&o, "emptyFailures", json!(2));
    graph_set(&o, "hasSections", json!(true));
    let second = bundle.begin_turn(2).unwrap();
    assert_eq!(second.turn_id, 2);
    assert_eq!(graph_value(&o, "schemaFailures"), 0);
    assert_eq!(graph_value(&o, "emptyFailures"), 0);
    assert_eq!(graph_value(&o, "hasSections"), false);
    assert!(!second.has_yields());
    assert!(
        body(&o, json!({"key":1,"data":1})).is_err(),
        "new logical turn must retain same-batch submitted items"
    );
    yield_tool::set_items(
        o.values(),
        &o.graph,
        &[WorkPoolYieldItem {
            id: "two".into(),
            index: 2,
        }],
    )
    .unwrap();
    let params = schema::get(h, &bundle.original, "parameters", r).unwrap();
    assert_eq!(
        h.export_serde(&params).unwrap()["properties"]["key"]["enum"],
        json!([2])
    );
    assert!(body(&o, json!({"key":2,"data":1})).is_ok());
    yield_tool::set_items(o.values(), &o.graph, &[]).unwrap();
    assert_eq!(
        h.export_serde(&schema::get(h, &bundle.original, "strict", r).unwrap())
            .unwrap(),
        true
    );
    let params = h
        .export_serde(&schema::get(h, &bundle.original, "parameters", r).unwrap())
        .unwrap();
    assert!(params["properties"].get("key").is_none());
}

fn section_retry_cases() {
    let empty = owner(None, Vec::new());
    graph_set(&empty, "lastText", json!(""));
    assert!(body(&empty, json!({"type":["notes"]})).is_err());
    let rooted = owner(
        Some(
            json!({"$ref":"#/$defs/Closed","$defs":{"Closed":{"type":"object","properties":{"blockers":{"type":"array","items":{"type":"string"}}},"additionalProperties":false}}}),
        ),
        Vec::new(),
    );
    let rooted_observer = ChildRunObserver::new(rooted.clone(), 0);
    observe_body(
        &rooted,
        &rooted_observer,
        json!({"type":["blockers"],"data":"one"}),
        false,
    );
    observe_body(&rooted, &rooted_observer, json!({"type":"result"}), false);
    assert_eq!(
        exported(&rooted, &finish(&rooted_observer, None)),
        json!({"blockers":["one"]})
    );
    let nested = owner(
        Some(json!({"properties":{"groups":{"elements":{"elements":{"type":"string"}}}}})),
        Vec::new(),
    );
    assert!(body(&nested, json!({"data":{"groups":["flattened"]}})).is_err());
    assert!(body(&nested, json!({"data":{"groups":[["nested"]]}})).is_ok());

    let o = owner(
        Some(
            json!({"properties":{"verdict":{"enum":["clean","blockers"]}},"optionalProperties":{"findings":{"elements":{"properties":{"title":{"type":"string"}}}}}}),
        ),
        Vec::new(),
    );
    for _ in 0..3 {
        assert!(body(&o, json!({"type":["verdict"],"data":"bad"})).is_err());
    }
    let overridden = body(&o, json!({"type":["verdict"],"data":"bad"})).unwrap();
    assert_eq!(details(&o, &overridden)["schemaOverridden"], true);
    assert_eq!(graph_value(&o, "schemaFailures"), 0);
    assert!(
        body(
            &o,
            json!({"type":["findings"],"data":[{"title":"whole array is not one item"}]})
        )
        .is_err()
    );
    assert_eq!(graph_value(&o, "schemaFailures"), 1);
    assert!(
        body(
            &o,
            json!({"type":["findings"],"data":{"title":"valid element"}})
        )
        .is_ok()
    );
    assert_eq!(graph_value(&o, "schemaFailures"), 0);
    // Structural argument errors never become successful just because a schema
    // retry override was previously used.
    for _ in 0..5 {
        assert!(body(&o, json!({"type":42,"data":{}})).is_err());
        assert!(body(&o, json!({"type":[],"data":{}})).is_err());
        assert!(body(&o, json!({"data":{},"error":"both"})).is_err());
    }
    let pool = owner(
        None,
        vec![
            WorkPoolYieldItem {
                id: "a".into(),
                index: 1,
            },
            WorkPoolYieldItem {
                id: "b".into(),
                index: 2,
            },
        ],
    );
    let observer = ChildRunObserver::new(pool.clone(), 0);
    observe_body(&pool, &observer, json!({"key":1,"data":false}), false);
    assert!(!observer.has_terminal_yield());
    observe_body(
        &pool,
        &observer,
        json!({"key":2,"data":{"done":true}}),
        false,
    );
    assert!(observer.has_terminal_yield());
    let out = finish(&observer, None);
    assert!(out.accepted);
    assert_eq!(exported(&pool, &out), json!({"a":false,"b":{"done":true}}));
}
