use super::*;

struct CaptureReference {
    seen: Mutex<Option<ExtraCaValue>>,
}
struct MutationReference {
    calls: Arc<Mutex<Vec<&'static str>>>,
    name: &'static str,
}
#[async_trait]
impl DiscoveryTransport for MutationReference {
    fn context_id(&self) -> u64 {
        702
    }
    async fn fetch(&self, _request: DiscoveryRequest) -> Result<DiscoveryReply, DiscoveryError> {
        self.calls.lock().unwrap().push(self.name);
        Ok(DiscoveryReply { status: 200, headers: Vec::new(), body: Vec::new(), json_override: None })
    }
}
#[async_trait]
impl DiscoveryTransport for CaptureReference {
    fn context_id(&self) -> u64 {
        701
    }
    async fn fetch(&self, request: DiscoveryRequest) -> Result<DiscoveryReply, DiscoveryError> {
        *self.seen.lock().unwrap() = request.native_init;
        Ok(DiscoveryReply { status: 200, headers: Vec::new(), body: Vec::new(), json_override: None })
    }
}
async fn capture(runtime: Arc<ExtraCaRuntime>, init: &ExtraCaValue) -> ExtraCaValue {
    let recording = Arc::new(CaptureReference { seen: Mutex::new(None) });
    let transport: Arc<dyn DiscoveryTransport> = recording.clone();
    wrap_fetch_for_extra_ca(runtime, transport)
        .fetch(DiscoveryRequest {
            url: "https://fixture.test/models".into(),
            native_init: Some(init.clone()),
            ..Default::default()
        })
        .await
        .unwrap();

    recording.seen.lock().unwrap().as_ref().unwrap().clone()
}
fn number(value: ExtraCaValue) -> Value {
    if let ExtraCaValue::Number(value) = value {
        serde_json::from_str(&WireValue::Number(value).stringify()).unwrap()
    } else {
        panic!("expected number")
    }
}
fn string(value: ExtraCaValue) -> String {
    if let ExtraCaValue::String(value) = value { value.to_utf8().unwrap() } else { panic!("expected string") }
}
fn append(array: &ExtraCaValue, value: ExtraCaValue) {
    if let ExtraCaValue::Array(array) = array { array.lock().unwrap().push(value) } else { panic!("expected array") }
}
fn has_own(value: &ExtraCaValue, key: &str) -> bool {
    value.own_entries().iter().any(|(name, _)| name.equals_ascii(key))
}
pub async fn probe(runtime: Arc<ExtraCaRuntime>, kind: &str) -> Value {
    if kind == "init" {
        let nested = ExtraCaValue::object(&[("v", ExtraCaValue::Number(1.0))]);
        let items = ExtraCaValue::array(vec![nested.clone()]);
        let callback = ExtraCaValue::Opaque(Arc::new("callback"));
        let init = ExtraCaValue::object(&[
            ("headers", nested.clone()),
            (
                "tls",
                ExtraCaValue::object(&[
                    ("ca", ExtraCaValue::array(vec![ExtraCaValue::string("root")])),
                    ("unknown", nested.clone()),
                ]),
            ),
            ("items", items.clone()),
            ("callback", callback.clone()),
        ]);
        let out = capture(runtime, &init).await;
        let mut result = json!({"initSame":out.strict_same(&init),"tlsSame":out.get("tls").strict_same(&init.get("tls")),"caSame":out.get("tls").get("ca").strict_same(&init.get("tls").get("ca")),"headersSame":out.get("headers").strict_same(&nested),"unknownSame":out.get("tls").get("unknown").strict_same(&nested),"itemsSame":out.get("items").strict_same(&items),"callbackSame":out.get("callback").strict_same(&callback),"crossAlias":out.get("headers").strict_same(&out.get("tls").get("unknown"))});
        out.get("headers").as_object().unwrap().set("v", ExtraCaValue::Number(2.0));
        append(&out.get("items"), ExtraCaValue::string("new"));
        result["originalNested"] = json!(number(init.get("tls").get("unknown").get("v")));
        result["originalItems"] = json!(init.get("items").items().len());
        result["originalCa"] = json!(init.get("tls").get("ca").items().len());
        result["outputCa"] = json!(out.get("tls").get("ca").items().len());
        return result;
    }
    if kind == "ca-elements" {
        let element = ExtraCaValue::object(&[("v", ExtraCaValue::Number(1.0))]);
        let callback = ExtraCaValue::Opaque(Arc::new("callback"));
        let ca = ExtraCaValue::array(vec![
            element.clone(),
            callback.clone(),
            ExtraCaValue::Hole,
            ExtraCaValue::string("root"),
        ]);
        let init = ExtraCaValue::object(&[("tls", ExtraCaValue::object(&[("ca", ca.clone())]))]);
        let out = capture(runtime, &init).await;
        let output_ca = out.get("tls").get("ca");
        let mut result = json!({"initSame":out.strict_same(&init),"tlsSame":out.get("tls").strict_same(&init.get("tls")),"caSame":output_ca.strict_same(&ca),"elementSame":output_ca.get("0").strict_same(&element),"callbackSame":output_ca.get("1").strict_same(&callback),"inputHole":has_own(&ca,"2"),"outputHole":has_own(&output_ca,"2"),"holeUndefined":output_ca.get("2").is_undefined()});
        output_ca.get("0").as_object().unwrap().set("v", ExtraCaValue::Number(7.0));
        append(&output_ca, ExtraCaValue::string("new"));
        result["originalElement"] = json!(number(ca.get("0").get("v")));
        result["originalCa"] = json!(ca.items().len());
        result["outputCa"] = json!(output_ca.items().len());
        return result;
    }
    if kind == "passthrough" {
        let nested = ExtraCaValue::object(&[("v", ExtraCaValue::Number(1.0))]);
        let init = ExtraCaValue::object(&[
            ("headers", nested.clone()),
            (
                "tls",
                ExtraCaValue::object(&[
                    ("ca", ExtraCaValue::array(vec![ExtraCaValue::string("root")])),
                    ("unknown", nested.clone()),
                ]),
            ),
        ]);
        let out = capture(runtime, &init).await;
        let mut result = json!({"initSame":out.strict_same(&init),"tlsSame":out.get("tls").strict_same(&init.get("tls")),"caSame":out.get("tls").get("ca").strict_same(&init.get("tls").get("ca")),"headersSame":out.get("headers").strict_same(&nested)});
        out.get("headers").as_object().unwrap().set("v", ExtraCaValue::Number(6.0));
        result["originalNested"] = json!(number(init.get("headers").get("v")));
        return result;
    }
    if kind == "options" || kind == "options-wrapped" {
        let nested = ExtraCaValue::object(&[("v", ExtraCaValue::Number(1.0))]);
        let items = ExtraCaValue::array(vec![nested.clone()]);
        let callback = ExtraCaValue::Opaque(Arc::new("callback"));
        let base: Arc<dyn DiscoveryTransport> = Arc::new(CaptureReference { seen: Mutex::new(None) });
        let fetch = if kind == "options-wrapped" {
            wrap_fetch_for_extra_ca(runtime.clone(), base.clone())
        } else {
            base.clone()
        };
        let fields = ExtraCaValue::object(&[
            ("nested", nested.clone()),
            ("items", items.clone()),
            ("callback", callback.clone()),
            ("fetch", ExtraCaValue::Fetch(fetch.clone())),
        ]);
        let options = Arc::new(ExtraCaFetchOptions { fetch: Some(fetch.clone()), fields });
        let out = with_extra_ca_fetch(runtime, Some(options.clone()), base).unwrap();
        if kind == "options-wrapped" {
            let mut result = json!({"optionsSame":Arc::ptr_eq(&out,&options)&&out.fields.strict_same(&options.fields),"fetchSame":out.fields.get("fetch").strict_same(&ExtraCaValue::Fetch(fetch)),"nestedSame":out.fields.get("nested").strict_same(&nested)});
            out.fields.as_object().unwrap().set("newField", ExtraCaValue::string("visible"));
            result["originalHasNewField"] = json!(string(options.fields.get("newField")) == "visible");
            return result;
        }
        let error = ExtraCaError::new("graph", Some(nested.clone()));
        let mut result = json!({"optionsSame":out.fields.strict_same(&options.fields),"fetchSame":out.fields.get("fetch").strict_same(&ExtraCaValue::Fetch(fetch)),"nestedSame":out.fields.get("nested").strict_same(&nested),"itemsSame":out.fields.get("items").strict_same(&items),"callbackSame":out.fields.get("callback").strict_same(&callback),"causeSame":error.cause.as_ref().unwrap().strict_same(&nested)});
        out.fields.get("nested").as_object().unwrap().set("v", ExtraCaValue::Number(3.0));
        append(&out.fields.get("items"), ExtraCaValue::string("new"));
        result["originalNested"] = json!(number(options.fields.get("nested").get("v")));
        result["originalItems"] = json!(options.fields.get("items").items().len());
        result["causeNested"] = json!(number(error.cause.unwrap().get("v")));
        return result;
    }
    if kind == "options-mutation" {
        let calls = Arc::new(Mutex::new(Vec::new()));
        let base: Arc<dyn DiscoveryTransport> = Arc::new(MutationReference { calls: calls.clone(), name: "base" });
        let replacement: Arc<dyn DiscoveryTransport> =
            Arc::new(MutationReference { calls: calls.clone(), name: "replacement" });
        let global: Arc<dyn DiscoveryTransport> = Arc::new(CaptureReference { seen: Mutex::new(None) });
        let options = Arc::new(ExtraCaFetchOptions {
            fetch: Some(base.clone()),
            fields: ExtraCaValue::object(&[("fetch", ExtraCaValue::Fetch(base.clone()))]),
        });
        let out = with_extra_ca_fetch(runtime.clone(), Some(options.clone()), global.clone()).unwrap();
        out.fields.as_object().unwrap().set("fetch", ExtraCaValue::Fetch(replacement.clone()));
        let again = with_extra_ca_fetch(runtime.clone(), Some(out.clone()), global.clone()).unwrap();
        let ExtraCaValue::Fetch(selected) = again.fields.get("fetch") else { panic!("fetch property") };
        selected
            .fetch(DiscoveryRequest { url: "https://fixture.test/models".into(), ..Default::default() })
            .await
            .unwrap();
        let mut result = json!({
            "firstSame":Arc::ptr_eq(&out,&options),
            "secondSame":Arc::ptr_eq(&again,&out),
            "replacementWrapped":!Arc::ptr_eq(&selected,&replacement),
            "sourceStillReplacement":out.fields.get("fetch").strict_same(&ExtraCaValue::Fetch(replacement.clone())),
            "calls":calls.lock().unwrap().clone(),
        });
        out.fields.as_object().unwrap().set("fetch", ExtraCaValue::Undefined);
        let unset = with_extra_ca_fetch(runtime.clone(), Some(out.clone()), global.clone()).unwrap();
        result["undefinedUsesGlobal"] = json!(
            !unset.fields.get("fetch").strict_same(&ExtraCaValue::Fetch(replacement.clone()))
                && !unset.fields.get("fetch").strict_same(&ExtraCaValue::Fetch(base.clone()))
        );
        out.fields.as_object().unwrap().set("fetch", ExtraCaValue::Null);
        let nulled = with_extra_ca_fetch(runtime, Some(out), global).unwrap();
        result["nullUsesGlobal"] = json!(
            !nulled.fields.get("fetch").strict_same(&ExtraCaValue::Fetch(replacement))
                && !nulled.fields.get("fetch").strict_same(&ExtraCaValue::Fetch(base))
        );
        return result;
    }
    if kind == "options-nullish-mutation" {
        let calls = Arc::new(Mutex::new(Vec::new()));
        let base: Arc<dyn DiscoveryTransport> = Arc::new(MutationReference { calls: calls.clone(), name: "base" });
        let global: Arc<dyn DiscoveryTransport> = Arc::new(MutationReference { calls: calls.clone(), name: "global" });
        let options = Arc::new(ExtraCaFetchOptions {
            fetch: Some(base.clone()),
            fields: ExtraCaValue::object(&[("fetch", ExtraCaValue::Fetch(base.clone()))]),
        });
        let out = with_extra_ca_fetch(runtime.clone(), Some(options.clone()), global.clone()).unwrap();
        out.fields.as_object().unwrap().set("fetch", ExtraCaValue::Undefined);
        let unset = with_extra_ca_fetch(runtime.clone(), Some(out.clone()), global.clone()).unwrap();
        let ExtraCaValue::Fetch(selected) = unset.fields.get("fetch") else { panic!("fetch property") };
        selected
            .fetch(DiscoveryRequest { url: "https://fixture.test/models".into(), ..Default::default() })
            .await
            .unwrap();
        out.fields.as_object().unwrap().set("fetch", ExtraCaValue::Null);
        let nulled = with_extra_ca_fetch(runtime, Some(out.clone()), global).unwrap();
        let ExtraCaValue::Fetch(selected) = nulled.fields.get("fetch") else { panic!("fetch property") };
        selected
            .fetch(DiscoveryRequest { url: "https://fixture.test/models".into(), ..Default::default() })
            .await
            .unwrap();
        let calls = calls.lock().unwrap().clone();
        return json!({
            "calls":calls,
            "originalFetchSame":options.fields.get("fetch").strict_same(&ExtraCaValue::Fetch(base)),
            "sourceFetchNull":matches!(out.fields.get("fetch"),ExtraCaValue::Null),
        });
    }
    if kind == "prototype-cycles" {
        let nested = ExtraCaValue::object(&[("v", ExtraCaValue::Number(1.0))]);
        nested.as_object().unwrap().set("self", nested.clone());
        let tls_proto = ExtraCaValue::object(&[
            ("ca", ExtraCaValue::array(vec![ExtraCaValue::string("custom")])),
            ("inherited", nested.clone()),
        ]);
        let existing_tls = ExtraCaObject::new(tls_proto.as_object());
        existing_tls.set("own", nested.clone());
        existing_tls.define("hidden".into(), nested.clone(), false);
        let existing_tls = ExtraCaValue::Object(existing_tls);
        let init_proto =
            ExtraCaValue::object(&[("tls", existing_tls.clone()), ("inherited", ExtraCaValue::string("dontspread"))]);
        let init = ExtraCaObject::new(init_proto.as_object());
        init.set("unknown", nested.clone());
        init.define("hidden".into(), nested.clone(), false);
        let init = ExtraCaValue::Object(init);
        let out = capture(runtime, &init).await;
        let mut result = json!({"initSame":out.strict_same(&init),"tlsSame":out.get("tls").strict_same(&existing_tls),"caSame":out.get("tls").get("ca").strict_same(&tls_proto.get("ca")),"initInherited":has_own(&out,"inherited"),"tlsInherited":has_own(&out.get("tls"),"inherited"),"initHidden":has_own(&out,"hidden"),"tlsHidden":has_own(&out.get("tls"),"hidden"),"tlsOwn":has_own(&out,"tls"),"caOwn":has_own(&out.get("tls"),"ca"),"unknownSame":out.get("unknown").strict_same(&nested),"cycleRetained":out.get("unknown").get("self").strict_same(&nested),"caFirst":string(out.get("tls").get("ca").get("0")),"caLength":out.get("tls").get("ca").items().len()});
        out.get("tls").get("own").as_object().unwrap().set("v", ExtraCaValue::Number(9.0));
        result["originalNested"] = json!(number(init.get("unknown").get("v")));
        nested.as_object().unwrap().set("self", ExtraCaValue::Undefined);
        return result;
    }
    panic!("unknown native graph probe")
}
