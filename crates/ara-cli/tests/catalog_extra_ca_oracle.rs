//! Fixed original TLS helper replay; this does not establish native TLS trust.
use ara_cli::{catalog_discovery::*, catalog_extra_ca::*, model_collapse::VariantSpec};
use ara_rpc::{WireString, WireValue};
use async_trait::async_trait;
use serde_json::{Value, json};
use std::{
    collections::{BTreeSet, HashMap},
    sync::{
        Arc, Mutex,
        atomic::{AtomicU64, Ordering},
    },
};

#[path = "support/catalog_extra_ca_references.rs"]
mod references;
const ARTIFACT_SHA: &str = "a99292cfbc7806865edfcb730b7dfa15cc51cfe16184b85a0435dfd3763954b9";
fn hash(bytes: &[u8]) -> String {
    ring::digest::digest(&ring::digest::SHA256, bytes).as_ref().iter().map(|byte| format!("{byte:02x}")).collect()
}
fn path(value: &Value) -> Vec<WireString> {
    value.as_array().unwrap().iter().map(|key| key.as_str().unwrap().into()).collect()
}
fn raw_mut<'a>(mut value: &'a mut WireValue, path: &[WireString]) -> &'a mut WireValue {
    for key in path {
        value = match value {
            WireValue::Object(entries) => &mut entries.iter_mut().find(|(name, _)| name == key).unwrap().1,
            WireValue::Array(items) => &mut items[key.to_utf8().unwrap().parse::<usize>().unwrap()],
            _ => panic!("invalid fixture path"),
        };
    }
    value
}
fn decoded(value: &Value) -> Option<VariantSpec> {
    let text = value.get("rawWireJSON")?.as_str()?;
    let mut out = VariantSpec::from_wire(WireValue::parse(text).unwrap());
    for entry in value["undefinedPaths"].as_array().unwrap() {
        let keys = path(entry);
        if keys.is_empty() {
            return None;
        }
        let parent = raw_mut(&mut out.value, &keys[..keys.len() - 1]);
        match parent {
            WireValue::Object(entries) => {
                if let Some((_, value)) = entries.iter_mut().find(|(key, _)| key == keys.last().unwrap()) {
                    *value = WireValue::Null;
                } else {
                    entries.push((keys.last().unwrap().clone(), WireValue::Null));
                }
            }
            WireValue::Array(items) => {
                items[keys.last().unwrap().to_utf8().unwrap().parse::<usize>().unwrap()] = WireValue::Null
            }
            _ => panic!("invalid undefined fixture"),
        }
        out.undefined_paths.push(keys);
    }
    // Match frozen source decode: append undefined slots without reordering.
    Some(out)
}
fn encoded(value: Option<&VariantSpec>) -> Value {
    let Some(value) = value else {
        return json!({"rawWireJSON":null,"undefinedPaths":[[]],"ownKeys":[]});
    };
    let mut undefined = Vec::<Value>::new();
    let mut keys = Vec::<Value>::new();
    fn visit(
        value: &WireValue,
        prefix: Vec<String>,
        spec: &VariantSpec,
        undefined: &mut Vec<Value>,
        keys: &mut Vec<Value>,
    ) {
        let wire_path: Vec<WireString> = prefix.iter().map(|key| key.as_str().into()).collect();
        if spec.undefined_paths.contains(&wire_path) {
            undefined.push(json!(prefix));
            return;
        }
        match value {
            WireValue::Object(_) => {
                let entries = value.entries().unwrap();
                keys.push(json!({"path":prefix,"keys":entries.iter().map(|(key,_)|key.to_utf8().unwrap()).collect::<Vec<_>>()}));
                for (key, item) in entries {
                    let mut next = prefix.clone();
                    next.push(key.to_utf8().unwrap());
                    visit(item, next, spec, undefined, keys);
                }
            }
            WireValue::Array(items) => {
                keys.push(
                    json!({"path":prefix,"keys":(0..items.len()).map(|index|index.to_string()).collect::<Vec<_>>()}),
                );
                for (index, item) in items.iter().enumerate() {
                    let mut next = prefix.clone();
                    next.push(index.to_string());
                    visit(item, next, spec, undefined, keys);
                }
            }
            _ => {}
        }
    }
    visit(&value.value, Vec::new(), value, &mut undefined, &mut keys);
    json!({"rawWireJSON":value.to_wire_json().stringify(),"undefinedPaths":undefined,"ownKeys":keys})
}
#[derive(Default)]
struct HostState {
    env: Option<WireString>,
    files: HashMap<WireString, Value>,
    stats: Vec<String>,
    reads: Vec<String>,
}
struct FixtureHost {
    state: Mutex<HostState>,
    roots: Vec<WireString>,
}
fn io_error(value: &Value) -> ExtraCaIoError {
    ExtraCaIoError {
        code: value["code"].as_str().unwrap().into(),
        error: DiscoveryError::named(value["name"].as_str().unwrap_or("Error"), value["message"].as_str().unwrap()),
    }
}
impl ExtraCaHost for FixtureHost {
    fn node_extra_ca_certs(&self) -> Option<WireString> {
        self.state.lock().unwrap().env.clone()
    }
    fn file_mtime_ms(&self, path: &WireString) -> Result<f64, ExtraCaIoError> {
        let mut state = self.state.lock().unwrap();
        state.stats.push(path.to_utf8().unwrap());
        let entry =
            state.files.get(path).ok_or_else(|| io_error(&json!({"code":"ENOENT","message":"missing stat"})))?;
        if let Some(error) = entry.get("statError") {
            return Err(io_error(error));
        }
        Ok(entry["mtimeMs"].as_f64().unwrap())
    }
    fn read_utf8(&self, path: &WireString) -> Result<WireString, ExtraCaIoError> {
        let mut state = self.state.lock().unwrap();
        state.reads.push(path.to_utf8().unwrap());
        let entry =
            state.files.get(path).ok_or_else(|| io_error(&json!({"code":"ENOENT","message":"missing read"})))?;
        if let Some(error) = entry.get("readError") {
            return Err(io_error(error));
        }
        Ok(entry["contents"].as_str().unwrap().into())
    }
    fn root_certificates(&self) -> Result<Vec<WireString>, DiscoveryError> {
        Ok(self.roots.clone())
    }
}
fn encoded_native(value: Option<&ExtraCaValue>) -> Value {
    match value {
        Some(value) if !value.is_undefined() => encoded(Some(&value.to_variant())),
        _ => encoded(None),
    }
}
#[derive(Default)]
struct Calls {
    original: Option<ExtraCaValue>,
    calls: Vec<Value>,
}
struct Recording {
    name: &'static str,
    id: u64,
    preconnect: AtomicU64,
    calls: Arc<Mutex<Calls>>,
}
#[async_trait]
impl DiscoveryTransport for Recording {
    fn context_id(&self) -> u64 {
        self.id
    }
    fn preconnect_identity(&self) -> Option<u64> {
        let value = self.preconnect.load(Ordering::Relaxed);
        (value != 0).then_some(value)
    }
    async fn fetch(&self, request: DiscoveryRequest) -> Result<DiscoveryReply, DiscoveryError> {
        let mut calls = self.calls.lock().unwrap();
        let original = calls.original.as_ref().unwrap_or(&ExtraCaValue::Undefined);
        let actual = request.native_init.as_ref().unwrap_or(&ExtraCaValue::Undefined);
        let receipt = json!({"fetch":self.name,"input":request.url.to_utf8().unwrap(),"init":encoded_native(Some(actual)),"initSame":actual.strict_same(original),"tlsSame":actual.get("tls").strict_same(&original.get("tls")),"caSame":actual.get("tls").get("ca").strict_same(&original.get("tls").get("ca"))});
        calls.calls.push(receipt);
        Ok(DiscoveryReply { status: 200, headers: Vec::new(), body: b"ok".to_vec(), json_override: None })
    }
}
fn same_fetch(a: &Option<Arc<dyn DiscoveryTransport>>, b: &Option<Arc<dyn DiscoveryTransport>>) -> bool {
    match (a, b) {
        (Some(a), Some(b)) => Arc::ptr_eq(a, b),
        (None, None) => true,
        _ => false,
    }
}
async fn replay(case: &Value, roots: &[WireString]) -> Value {
    let host = Arc::new(FixtureHost { state: Mutex::default(), roots: roots.to_vec() });
    let runtime = Arc::new(ExtraCaRuntime::new(host.clone()));
    let calls = Arc::new(Mutex::new(Calls::default()));
    let base = Arc::new(Recording { name: "base", id: 101, preconnect: AtomicU64::new(0), calls: calls.clone() });
    let mut global: Arc<dyn DiscoveryTransport> =
        Arc::new(Recording { name: "global", id: 102, preconnect: AtomicU64::new(0), calls: calls.clone() });
    let mut fetches: HashMap<String, Arc<dyn DiscoveryTransport>> = HashMap::from([
        ("base".into(), base.clone() as Arc<dyn DiscoveryTransport>),
        ("global".into(), global.clone()),
    ]);
    let mut options: HashMap<String, Option<Arc<ExtraCaFetchOptions>>> = HashMap::new();
    let mut events = Vec::new();
    for step in case["steps"].as_array().unwrap() {
        let op = step["op"].as_str().unwrap();
        match op {
            "env" => host.state.lock().unwrap().env = step.get("value").map(|value| value.as_str().unwrap().into()),
            "file" => {
                host.state.lock().unwrap().files.insert(step["path"].as_str().unwrap().into(), step["entry"].clone());
            }
            "deleteFile" => {
                host.state.lock().unwrap().files.remove(&WireString::from(step["path"].as_str().unwrap()));
            }
            "reset" => runtime.reset_extra_ca_cache(),
            "wrap" => {
                let input = fetches[step["from"].as_str().unwrap()].clone();
                let output = wrap_fetch_for_extra_ca(runtime.clone(), input.clone());
                events.push(json!({"op":op,"same":Arc::ptr_eq(&input,&output)}));
                fetches.insert(step["to"].as_str().unwrap().into(), output);
            }
            "fetch" => {
                let original =
                    decoded(&step["init"]).as_ref().map_or(ExtraCaValue::Undefined, ExtraCaValue::from_variant);
                let native_tls = original.get("tls");
                let tls = (!native_tls.is_undefined()).then(|| native_tls.to_variant());
                calls.lock().unwrap().original = Some(original.clone());
                match fetches[step["from"].as_str().unwrap()].fetch(DiscoveryRequest{url:"https://fixture.test/models".into(),tls,native_init:Some(original),..Default::default()}).await {
                    Ok(_)=>events.push(json!({"op":op,"ok":true})),Err(error)=>events.push(json!({"op":op,"error":{"name":error.name.to_utf8().unwrap(),"message":error.message.to_utf8().unwrap()}}))
                }
            }
            "originalInit" => {
                events.push(json!({"op":op,"value":encoded_native(calls.lock().unwrap().original.as_ref())}))
            }
            "preconnect" => {
                assert_eq!(step["fetch"], "base");
                base.preconnect.store(step["identity"].as_u64().unwrap(), Ordering::Relaxed);
            }
            "observeFetch" => events
                .push(json!({"op":op,"preconnect":fetches[step["fetch"].as_str().unwrap()].preconnect_identity()})),
            "globalFetch" => global = fetches[step["from"].as_str().unwrap()].clone(),
            "options" => {
                let option = decoded(&step["fields"]).map(|fields| {
                    let fields = ExtraCaValue::from_variant(&fields);
                    let fetch = step.get("fetch").map(|name| fetches[name.as_str().unwrap()].clone());
                    if let Some(fetch) = &fetch {
                        fields.as_object().unwrap().set("fetch", ExtraCaValue::Fetch(fetch.clone()));
                    }
                    Arc::new(ExtraCaFetchOptions { fetch, fields })
                });
                options.insert(step["to"].as_str().unwrap().into(), option);
            }
            "withOptions" => {
                let before = options[step["from"].as_str().unwrap()].clone();
                let after = with_extra_ca_fetch(runtime.clone(), before.clone(), global.clone());
                let same = match (&before, &after) {
                    (Some(a), Some(b)) => Arc::ptr_eq(a, b),
                    (None, None) => true,
                    _ => false,
                };
                events.push(json!({"op":op,"same":same,"fetchSame":same_fetch(&after.as_ref().and_then(|options|options.fetch.clone()),&before.as_ref().and_then(|options|options.fetch.clone())),"keys":after.as_ref().map(|options|options.fields.own_entries().iter().map(|(key,_)|key.to_utf8().unwrap()).collect::<Vec<_>>())}));
                options.insert(step["to"].as_str().unwrap().into(), after);
            }
            "optionsFetch" => {
                fetches.insert(
                    step["to"].as_str().unwrap().into(),
                    options[step["from"].as_str().unwrap()].as_ref().unwrap().fetch.as_ref().unwrap().clone(),
                );
            }
            "error" => {
                let cause = decoded(&step["cause"]).as_ref().map(ExtraCaValue::from_variant);
                let error = ExtraCaError::new(step["message"].as_str().unwrap(), cause.clone());
                let cause_same = match (&cause, &error.cause) {
                    (Some(a), Some(b)) => a.strict_same(b),
                    (None, None) => true,
                    _ => false,
                };
                events.push(json!({"op":op,"name":error.name(),"message":error.message.to_utf8().unwrap(),"hasOwnCause":error.cause.is_some(),"cause":encoded_native(error.cause.as_ref()),"causeSame":cause_same}));
            }
            "referenceProbe" => events.push(
                json!({"op":op,"result":references::probe(runtime.clone(),step["kind"].as_str().unwrap()).await}),
            ),
            _ => panic!("unknown source scenario op"),
        }
    }
    let state = host.state.lock().unwrap();
    json!({"calls":calls.lock().unwrap().calls,"events":events,"stats":state.stats,"reads":state.reads})
}

#[tokio::test]
#[ignore = "requires fixed source artifact ARA_CATALOG_EXTRA_CA_ORACLE"]
async fn all_original_extra_ca_helpers_match_fixed_source() {
    let artifact = std::env::var("ARA_CATALOG_EXTRA_CA_ORACLE").expect("fixed TLS source artifact");
    let bytes = std::fs::read(&artifact).unwrap();
    assert_eq!(hash(&bytes), ARTIFACT_SHA);
    let oracle: Value = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(oracle["upstreamCommit"], "596f2da7101178214aa27a753529d15e6b7ad91d");
    assert_eq!(oracle["bunVersion"], "1.4.0");
    assert_eq!(oracle["originalAssertionsExecuted"], true);
    assert_eq!(oracle["bunSha256"], "627d2e4775c24bdedee2cd7ccc18dcadae061e5345274ab6e3c4c797927bfb8f");
    for (file, pin) in [
        ("packages/utils/src/tls-fetch.ts", "5b19f1b4ac7d0bbf95ce1bfab0fc38bdb0888c0c71c5d6f5bf4255770405e447"),
        ("packages/utils/src/fs-error.ts", "cc119ca2c39dd8c7d84df2580719abe6608bec6d06fc19726c0a6ec4ad264eb8"),
    ] {
        assert!(
            oracle["sourceManifest"].as_array().unwrap().iter().any(|row| row["path"] == file && row["sha256"] == pin)
        );
    }
    let cases = oracle["cases"].as_array().unwrap();
    assert_eq!(cases.len(), 58);
    assert_eq!(cases.iter().map(|case| case["id"].as_str().unwrap()).collect::<BTreeSet<_>>().len(), 58);
    let roots: Vec<WireString> =
        oracle["rootCertificates"].as_array().unwrap().iter().map(|value| value.as_str().unwrap().into()).collect();
    assert_eq!(roots.len(), 121);
    let mut mismatches = Vec::new();
    for case in cases {
        let actual = replay(case, &roots).await;
        if actual != case["expected"] {
            mismatches.push(json!({"id":case["id"],"expected":case["expected"],"actual":actual}));
        }
    }
    if let Ok(output) = std::env::var("ARA_CATALOG_EXTRA_CA_MISMATCHES") {
        std::fs::write(output, serde_json::to_vec_pretty(&mismatches).unwrap()).unwrap();
    }
    assert!(mismatches.is_empty(), "{} fixed-source TLS mismatches", mismatches.len());
}
