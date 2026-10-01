//! Replay explicit scenarios from unchanged fixed OMP source, preserving request
//! receipts, UTF-16, undefined own slots and non-JSON numeric values.
use ara_cli::{
    catalog_discovery::{codex::*, gitlab::*, *},
    model_collapse::{SpecRef, VariantSpec},
};
use ara_rpc::{WireString, WireValue as W};
use async_trait::async_trait;
use base64::{Engine, engine::general_purpose::STANDARD};
use std::{
    collections::{HashMap, VecDeque},
    path::PathBuf,
    sync::{Arc, Mutex},
};

#[path = "support/catalog_discovery_descriptors.rs"]
mod descriptor_fixture;
#[path = "support/catalog_discovery_manager.rs"]
mod manager_fixture;
#[path = "support/catalog_discovery_root.rs"]
mod root_fixture;
#[path = "support/catalog_discovery_rpc.rs"]
mod rpc_fixture;

pub fn field<'a>(value: &'a W, key: &str) -> &'a W {
    value.get(key).unwrap_or_else(|| panic!("missing {key}"))
}
pub fn text(value: &W) -> String {
    value.as_string().expect("string").to_utf8().expect("UTF8 metadata")
}
pub fn array(value: &W) -> &[W] {
    value.as_array().expect("array")
}
pub fn string(value: &W, key: &str) -> WireString {
    field(value, key).as_string().expect(key).clone()
}
pub fn s(value: impl Into<WireString>) -> W {
    W::String(value.into())
}
pub fn n(value: usize) -> W {
    W::Number(value as f64)
}
pub fn path(value: &W) -> Vec<WireString> {
    array(value).iter().map(|value| value.as_string().unwrap().clone()).collect()
}
fn at_mut<'a>(mut value: &'a mut W, path: &[WireString]) -> &'a mut W {
    for key in path {
        value = match value {
            W::Object(entries) => {
                let index = entries.iter().position(|(held, _)| held == key).unwrap_or_else(|| {
                    entries.push((key.clone(), W::Null));
                    entries.len() - 1
                });
                &mut entries[index].1
            }
            W::Array(items) => &mut items[key.to_utf8().unwrap().parse::<usize>().unwrap()],
            _ => panic!("invalid native value path"),
        };
    }
    value
}
pub fn decode(encoded: &W) -> VariantSpec {
    let mut value = match field(encoded, "rawWireJSON") {
        W::Null => W::Null,
        value => W::parse(&text(value)).unwrap(),
    };
    let undefined_paths: Vec<_> = array(field(encoded, "undefinedPaths")).iter().map(path).collect();
    for path in &undefined_paths {
        *at_mut(&mut value, path) = W::Null;
    }
    for item in array(field(encoded, "specialNumbers")) {
        *at_mut(&mut value, &path(field(item, "path"))) = W::Number(match text(field(item, "value")).as_str() {
            "NaN" => f64::NAN,
            "Infinity" => f64::INFINITY,
            "-Infinity" => f64::NEG_INFINITY,
            "-0" => -0.0,
            _ => panic!("special number"),
        });
    }
    for item in array(field(encoded, "ownKeys")) {
        let W::Object(entries) = at_mut(&mut value, &path(field(item, "path"))) else {
            continue;
        };
        let mut old = std::mem::take(entries);
        for key in array(field(item, "keys")) {
            let key = key.as_string().unwrap();
            let index = old.iter().position(|(held, _)| held == key).expect("own key restored");
            entries.push(old.remove(index));
        }
        assert!(old.is_empty());
    }
    VariantSpec { value, undefined_paths }
}
pub fn encode(spec: &VariantSpec) -> W {
    fn visit(
        value: &W,
        path: &mut Vec<WireString>,
        spec: &VariantSpec,
        undefined: &mut Vec<W>,
        special: &mut Vec<W>,
        own: &mut Vec<W>,
    ) {
        let encoded_path = || W::Array(path.iter().cloned().map(W::String).collect());
        if spec.undefined_paths.contains(path) {
            undefined.push(encoded_path());
            return;
        }
        if let W::Number(number) = value
            && (!number.is_finite() || (*number == 0.0 && number.is_sign_negative()))
        {
            special.push(W::object(vec![
                ("path", encoded_path()),
                (
                    "value",
                    s(if number.is_nan() {
                        "NaN"
                    } else if *number == f64::INFINITY {
                        "Infinity"
                    } else if *number == f64::NEG_INFINITY {
                        "-Infinity"
                    } else {
                        "-0"
                    }),
                ),
            ]));
            return;
        }
        let entries: Vec<(WireString, &W)> = match value {
            W::Object(_) => value.entries().unwrap().into_iter().map(|(key, value)| (key.clone(), value)).collect(),
            W::Array(items) => {
                items.iter().enumerate().map(|(index, value)| (index.to_string().into(), value)).collect()
            }
            _ => return,
        };
        own.push(W::object(vec![
            ("path", encoded_path()),
            ("keys", W::Array(entries.iter().map(|(key, _)| s(key.clone())).collect())),
        ]));
        for (key, value) in entries {
            path.push(key);
            visit(value, path, spec, undefined, special, own);
            path.pop();
        }
    }
    let (mut undefined, mut special, mut own) = (Vec::new(), Vec::new(), Vec::new());
    visit(&spec.value, &mut Vec::new(), spec, &mut undefined, &mut special, &mut own);
    W::object(vec![
        (
            "rawWireJSON",
            if spec.undefined_paths.contains(&Vec::new()) { W::Null } else { s(spec.to_wire_json().stringify()) },
        ),
        ("undefinedPaths", W::Array(undefined)),
        ("specialNumbers", W::Array(special)),
        ("ownKeys", W::Array(own)),
    ])
}
pub fn specs_value(specs: &[SpecRef]) -> VariantSpec {
    VariantSpec {
        value: W::Array(specs.iter().map(|spec| spec.value.clone()).collect()),
        undefined_paths: specs
            .iter()
            .enumerate()
            .flat_map(|(index, spec)| {
                spec.undefined_paths.iter().map(move |path| {
                    let mut full = vec![index.to_string().into()];
                    full.extend(path.iter().cloned());
                    full
                })
            })
            .collect(),
    }
}
pub fn insert_record(target: &mut VariantSpec, key: &str, value: &VariantSpec) {
    target.set(key, value.value.clone());
    target.undefined_paths.extend(value.undefined_paths.iter().map(|path| {
        let mut full = vec![key.into()];
        full.extend(path.iter().cloned());
        full
    }));
}
pub fn success(value: &VariantSpec) -> W {
    W::object(vec![("status", s("ok")), ("value", encode(value))])
}
pub fn failure(error: DiscoveryError) -> W {
    W::object(vec![("status", s("error")), ("name", s(error.name)), ("message", s(error.message))])
}
pub fn pairs(value: Option<&W>) -> Vec<(WireString, WireString)> {
    match value {
        Some(W::Object(_)) => value
            .unwrap()
            .entries()
            .unwrap()
            .into_iter()
            .map(|(key, value)| (key.clone(), value.as_string().unwrap().clone()))
            .collect(),
        Some(W::Array(items)) => items
            .iter()
            .map(|item| {
                let pair = array(item);
                (pair[0].as_string().unwrap().clone(), pair[1].as_string().unwrap().clone())
            })
            .collect(),
        _ => Vec::new(),
    }
}

pub struct FixtureTransport {
    pub responses: Mutex<VecDeque<W>>,
    pub calls: Mutex<Vec<W>>,
}
impl FixtureTransport {
    pub fn new(test: &W) -> Self {
        Self {
            responses: Mutex::new(
                test.get("responses").and_then(W::as_array).unwrap_or_default().iter().cloned().collect(),
            ),
            calls: Mutex::new(Vec::new()),
        }
    }
    pub fn finish(&self, mut result: W) -> W {
        result.insert("fetchCalls", W::Array(self.calls.lock().unwrap().clone()));
        result.insert("logs", W::Array(Vec::new()));
        result
    }
}
#[async_trait]
impl DiscoveryTransport for FixtureTransport {
    fn context_id(&self) -> u64 {
        1
    }
    async fn fetch(&self, request: DiscoveryRequest) -> Result<DiscoveryReply, DiscoveryError> {
        let method = match request.method {
            HttpMethod::Get => "GET",
            HttpMethod::Post => "POST",
            HttpMethod::Put => "PUT",
            HttpMethod::Delete => "DELETE",
            HttpMethod::Head => "HEAD",
        };
        self.calls.lock().unwrap().push(W::object(vec![
            ("url", s(request.url)),
            ("method", s(method)),
            (
                "headers",
                W::Array(
                    request
                        .headers
                        .iter()
                        .map(|(key, value)| W::Array(vec![s(key.clone()), s(value.clone())]))
                        .collect(),
                ),
            ),
            ("bodyBase64", request.body.map(|body| s(STANDARD.encode(body))).unwrap_or(W::Null)),
            (
                "signal",
                W::object(vec![
                    ("present", W::Bool(request.signal.is_some())),
                    ("aborted", W::Bool(request.signal.as_ref().is_some_and(DiscoverySignal::is_aborted))),
                ]),
            ),
        ]));
        let step = self
            .responses
            .lock()
            .unwrap()
            .pop_front()
            .ok_or_else(|| DiscoveryError::new("unexpected fixture fetch"))?;
        if let Some(delay) = step.get("delayMs").and_then(W::as_number) {
            tokio::time::sleep(std::time::Duration::from_millis(delay as u64)).await;
        }
        if let Some(error) = step.get("error") {
            return Err(DiscoveryError::named(
                error.get("name").and_then(W::as_string).cloned().unwrap_or_else(|| "Error".into()),
                string(error, "message"),
            ));
        }
        let status = step.get("status").and_then(W::as_number).unwrap_or(200.0) as u16;
        let headers = DiscoveryHeaders::from_pairs(&pairs(step.get("headers")))?.entries();
        let body = if let Some(value) = step.get("bodyBase64") {
            STANDARD.decode(text(value)).unwrap()
        } else if let Some(value) = step.get("body") {
            text(value).into_bytes()
        } else if let Some(value) = step.get("json") {
            value.stringify().into_bytes()
        } else {
            Vec::new()
        };
        let json_override = if let Some(error) = step.get("jsonError") {
            Some(Err(DiscoveryError::named(
                error.get("name").and_then(W::as_string).cloned().unwrap_or_else(|| "Error".into()),
                string(error, "message"),
            )))
        } else {
            step.get("jsonEncoded")
                .map(|value| Ok(decode(value)))
                .or_else(|| step.get("json").map(|value| Ok(VariantSpec::from_wire(value.clone()))))
        };
        Ok(DiscoveryReply { status, headers, body, json_override })
    }
}
fn optional(options: &VariantSpec, key: &str) -> Option<WireString> {
    options.get(key).and_then(W::as_string).cloned()
}
fn selection_value(value: GitLabNamespaceSelection) -> VariantSpec {
    let mut result = VariantSpec::from_wire(W::object(vec![("rootNamespaceId", s(value.root_namespace_id))]));
    if let Some(path) = value.namespace_path {
        result.set("namespacePath", s(path));
    }
    if let Some(path) = value.project_path {
        result.set("projectPath", s(path));
    }
    result.set("source", s(value.source));
    result
}
async fn replay_owned(test: &W) -> W {
    let transport = Arc::new(FixtureTransport::new(test));
    let context = CatalogContext::new(transport.clone()).unwrap();
    let options = test.get("optionsEncoded").map(decode).unwrap_or_else(|| {
        VariantSpec::from_wire(test.get("options").cloned().unwrap_or_else(|| W::Object(Vec::new())))
    });
    let operation = text(field(test, "op"));
    let result = if text(field(test, "family")) == "codex" {
        let signal = test.get("signal").map(|value| {
            let signal = DiscoverySignal::default();
            if matches!(value.get("aborted"), Some(W::Bool(true))) {
                signal.abort(DiscoveryError::named("AbortError", "This operation was aborted."));
            }
            signal
        });
        let options = CodexModelDiscoveryOptions {
            access_token: optional(&options, "accessToken").unwrap_or_else(|| "".into()),
            account_id: optional(&options, "accountId"),
            base_url: optional(&options, "baseUrl"),
            client_version: optional(&options, "clientVersion"),
            paths: options
                .get("paths")
                .map(|value| array(value).iter().map(|value| value.as_string().unwrap().clone()).collect()),
            headers: pairs(options.get("headers")),
            signal,
        };
        match fetch_codex_models(&context, &options).await {
            Err(error) => failure(error),
            Ok(None) => success(&VariantSpec::from_wire(W::Null)),
            Ok(Some(value)) => {
                let mut result = VariantSpec::from_wire(W::Object(Vec::new()));
                insert_record(&mut result, "models", &specs_value(&value.models));
                if let Some(etag) = value.etag {
                    result.set("etag", s(etag));
                }
                if let Some(status) = value.rejected_status {
                    result.set("rejectedStatus", W::Number(f64::from(status)));
                }
                success(&result)
            }
        }
    } else {
        let directory = tempfile::tempdir().unwrap();
        let case_dir = directory.path();
        for (name, value) in test.get("files").and_then(W::entries).unwrap_or_default() {
            let target = case_dir.join(name.to_utf8().unwrap());
            std::fs::create_dir_all(target.parent().unwrap()).unwrap();
            std::fs::write(target, text(value).replace("$CASE", &case_dir.to_string_lossy())).unwrap();
        }
        std::fs::create_dir_all(case_dir.join("work/sub")).unwrap();
        let env: HashMap<_, _> =
            pairs(test.get("env")).into_iter().map(|(key, value)| (key.to_utf8().unwrap(), value)).collect();
        let host = GitLabHost::new(case_dir.to_owned(), env, Arc::new(NativeGitLabFs));
        let above_root = matches!(test.get("cwdAboveRoot"), Some(W::Bool(true))).then(|| {
            let volume = case_dir.ancestors().last().unwrap();
            PathBuf::from(format!(
                "{}{}{}",
                volume.display(),
                "../".repeat(50),
                case_dir.join("work").strip_prefix(volume).unwrap().display()
            ))
        });
        let config = GitLabDiscoveryConfig {
            api_key: optional(&options, "apiKey").unwrap_or_else(|| "".into()),
            base_url: optional(&options, "baseUrl"),
            namespace_id: optional(&options, "namespaceId"),
            project_id: optional(&options, "projectId"),
            project_path: optional(&options, "projectPath"),
            cwd: above_root.or_else(|| {
                optional(&options, "cwd")
                    .map(|value| PathBuf::from(value.to_utf8().unwrap().replace("$CASE", &case_dir.to_string_lossy())))
            }),
        };
        match operation.as_str() {
            "fetchGitLabDuoWorkflowModels" => match fetch_gitlab_duo_workflow_models(&context, &config, &host).await {
                Err(error) => failure(error),
                Ok(None) => success(&VariantSpec::from_wire(W::Null)),
                Ok(Some(values)) => success(&specs_value(&values)),
            },
            "discoverGitLabDuoWorkflowNamespace" | "discoverGitLabDuoWorkflowRuntimeNamespace" => {
                let result = if operation == "discoverGitLabDuoWorkflowNamespace" {
                    discover_gitlab_duo_workflow_namespace(&context, &config, &host).await
                } else {
                    discover_gitlab_duo_workflow_runtime_namespace(&context, &config, &host).await
                };
                match result {
                    Ok(value) => success(&selection_value(value)),
                    Err(error) => failure(error),
                }
            }
            "buildGitLabDuoWorkflowModelSpec" => {
                let args = array(field(test, "args"));
                let model = GitLabModelRef { name: string(&args[0], "name"), model_ref: string(&args[0], "ref") };
                success(&build_gitlab_duo_workflow_model_spec(
                    &model,
                    args.get(1).and_then(W::as_string),
                    args.get(2).and_then(W::as_string),
                ))
            }
            "buildGitLabDuoWorkflowFallbackModel" => {
                let args = array(field(test, "args"));
                success(&build_gitlab_duo_workflow_fallback_model(
                    args.first().and_then(W::as_string),
                    args.get(1).and_then(W::as_string),
                    args.get(2).and_then(W::as_string),
                ))
            }
            _ => panic!("unhandled owned discovery operation: {operation}"),
        }
    };
    transport.finish(result)
}

// This delivery replays one exact retained corpus. An upstream label alone
// cannot authorize altered expected results or a silently reduced inventory.
const FROZEN_ORACLE_SHA256: &str = "3635ce1c92541fff971733e41a977d2ddef392c9fd28362ac49498d1e4e6b045";
const FROZEN_CASE_COUNT: usize = 692;
fn sha256(bytes: &[u8]) -> String {
    ring::digest::digest(&ring::digest::SHA256, bytes).as_ref().iter().map(|byte| format!("{byte:02x}")).collect()
}
fn guard_metadata(value: &W, key: &str) -> Result<String, String> {
    value
        .get(key)
        .and_then(W::as_string)
        .ok_or_else(|| format!("missing string metadata: {key}"))?
        .to_utf8()
        .map_err(|error| format!("invalid UTF8 metadata {key}: {error}"))
}
fn validate_inventory(oracle: &W) -> Result<(), String> {
    use std::collections::{BTreeMap, HashSet};
    let expected: BTreeMap<String, usize> = [
        ("codex", 38usize),
        ("gitlab", 47),
        ("manager", 51),
        ("factory", 219),
        ("cursor", 18),
        ("devin", 31),
        ("openai", 24),
        ("gemini", 18),
        ("antigravity", 14),
        ("gemini-cli", 11),
        ("google-headers", 31),
        ("descriptor", 190),
    ]
    .into_iter()
    .map(|(family, count)| (family.to_owned(), count))
    .collect();
    let cases = oracle.get("cases").and_then(W::as_array).ok_or("missing cases")?;
    if cases.len() != FROZEN_CASE_COUNT {
        return Err("wrong exact case count".into());
    }
    // Case IDs themselves can contain an isolated UTF-16 surrogate. Compare
    // their original code units; only known UTF8 metadata is decoded to String.
    let mut ids = HashSet::<WireString>::new();
    let mut actual = BTreeMap::new();
    for case in cases {
        let id = case.get("id").and_then(W::as_string).ok_or("missing case id")?;
        if !ids.insert(id.clone()) {
            return Err(format!("duplicate case id: {id:?}"));
        }
        let family = guard_metadata(case, "family")?;
        *actual.entry(family).or_insert(0usize) += 1;
        if case.get("expected").is_none() {
            return Err(format!("missing expected: {id:?}"));
        }
    }
    let counts = W::Object(actual.iter().map(|(family, count)| (family.as_str().into(), n(*count))).collect());
    if actual != expected || !oracle.get("counts").is_some_and(|value| counts.deep_equal(value)) {
        return Err("wrong exact family inventory".into());
    }
    let isolation = oracle.get("isolation").ok_or("missing isolation")?;
    if isolation.get("processCount").and_then(W::as_number) != Some(FROZEN_CASE_COUNT as f64)
        || isolation.get("processIds").and_then(W::as_array).map(<[W]>::len) != Some(FROZEN_CASE_COUNT)
    {
        return Err("wrong source process inventory".into());
    }
    Ok(())
}
fn parse_guard_wire(bytes: &[u8]) -> Result<W, String> {
    W::parse(std::str::from_utf8(bytes).map_err(|error| error.to_string())?).map_err(|error| error.to_string())
}
fn validate_frozen_oracle(bytes: &[u8], root: &std::path::Path) -> Result<(), String> {
    if sha256(bytes) != FROZEN_ORACLE_SHA256 {
        return Err("frozen oracle SHA256 mismatch".into());
    }
    let oracle = parse_guard_wire(bytes)?;
    validate_inventory(&oracle)?;
    for inventory in ["sourceManifest", "licenses"] {
        for entry in oracle.get(inventory).and_then(W::as_array).ok_or("missing source inventory")? {
            let relative = guard_metadata(entry, "path")?;
            let path = root.join("upstream").join(&relative);
            let actual = std::fs::read(&path).map_err(|e| format!("source read {relative}: {e}"))?;
            if sha256(&actual) != guard_metadata(entry, "sha256")? {
                return Err(format!("source SHA256 mismatch: {relative}"));
            }
        }
    }
    for entry in oracle.get("adapters").and_then(W::as_array).ok_or("missing fixture inventory")? {
        let path = guard_metadata(entry, "path")?;
        let actual = std::fs::read(root.join(&path)).map_err(|e| format!("fixture read {path}: {e}"))?;
        if sha256(&actual) != guard_metadata(entry, "sha256")? {
            return Err(format!("fixture SHA256 mismatch: {path}"));
        }
    }
    let runner = std::fs::read(root.join("runner.mjs")).map_err(|e| e.to_string())?;
    if sha256(&runner) != guard_metadata(&oracle, "runnerSha256")? {
        return Err("runner SHA256 mismatch".into());
    }
    Ok(())
}
#[test]
#[ignore = "requires retained frozen discovery corpus"]
fn frozen_discovery_oracle_rejects_missing_duplicate_and_tampered_evidence() {
    let input = PathBuf::from(std::env::var_os("ARA_CATALOG_DISCOVERY_ORACLE").expect("frozen oracle"));
    let bytes = std::fs::read(&input).unwrap();
    let root = input.parent().unwrap();
    validate_frozen_oracle(&bytes, root).unwrap();
    let source = parse_guard_wire(&bytes).unwrap();
    let lone_id = array(field(&source, "cases"))
        .iter()
        .find_map(|case| field(case, "id").as_string().filter(|id| id.to_utf8().is_err()))
        .expect("the frozen corpus contains original isolated UTF16 case IDs")
        .clone();
    // WireValue::clone recursively copies values without decoding strings.
    let roundtrip = W::parse(&source.clone().stringify()).unwrap();
    assert!(roundtrip.deep_equal(&source), "guard clone/stringify must preserve raw UTF16 values");
    assert!(array(field(&roundtrip, "cases")).iter().any(|case| field(case, "id").as_string() == Some(&lone_id)));
    let mut results = Vec::new();
    for fault in ["missing-case", "duplicate-id", "changed-source-label", "changed-expected"] {
        let mut changed = source.clone();
        match fault {
            "missing-case" => {
                let W::Array(cases) = at_mut(&mut changed, &["cases".into()]) else { panic!("case array") };
                cases.pop();
            }
            "duplicate-id" => {
                let duplicate = field(&array(field(&changed, "cases"))[0], "id").clone();
                *at_mut(&mut changed, &["cases".into(), "1".into(), "id".into()]) = duplicate;
            }
            "changed-source-label" => {
                *at_mut(&mut changed, &["sourceManifest".into(), "0".into(), "sha256".into()]) = s("0".repeat(64))
            }
            "changed-expected" => {
                *at_mut(&mut changed, &["cases".into(), "0".into(), "expected".into()]) =
                    W::object(vec![("status", s("fabricated"))])
            }
            _ => unreachable!(),
        }
        if matches!(fault, "missing-case" | "duplicate-id") {
            assert!(validate_inventory(&changed).is_err());
        }
        let error = validate_frozen_oracle(changed.stringify().as_bytes(), root).unwrap_err();
        assert_eq!(error, "frozen oracle SHA256 mismatch");
        results.push(W::object(vec![("fault", s(fault)), ("rejected", W::Bool(true)), ("error", s(error))]));
    }
    // Alter the first original source on a disposable tree, without mutating
    // either the retained evidence or the expected artifact/hash.
    let temporary = tempfile::tempdir().unwrap();
    let first = guard_metadata(&array(field(&source, "sourceManifest"))[0], "path").unwrap();
    let changed = temporary.path().join("upstream").join(&first);
    std::fs::create_dir_all(changed.parent().unwrap()).unwrap();
    std::fs::write(changed, b"tampered original source").unwrap();
    let error = validate_frozen_oracle(&bytes, temporary.path()).unwrap_err();
    assert_eq!(error, format!("source SHA256 mismatch: {first}"));
    results.push(W::object(vec![
        ("fault", s("changed-original-source-file")),
        ("rejected", W::Bool(true)),
        ("error", s(error)),
    ]));
    if let Some(output) = std::env::var_os("ARA_CATALOG_DISCOVERY_GUARD_RECEIPT") {
        std::fs::write(
            output,
            W::object(vec![
                ("oracleSha256", s(FROZEN_ORACLE_SHA256)),
                ("caseCount", n(FROZEN_CASE_COUNT)),
                ("rawUtf16Roundtrip", W::Bool(true)),
                ("faults", W::Array(results)),
            ])
            .stringify(),
        )
        .unwrap();
    }
}

#[tokio::test]
#[ignore = "requires unchanged fixed-source Bun oracle corpus"]
async fn unchanged_discovery_oracle() {
    let input =
        std::env::var_os("ARA_CATALOG_DISCOVERY_ORACLE").expect("generate scripts/catalog_discovery_oracle.py first");
    let input = PathBuf::from(input);
    let bytes = std::fs::read(&input).unwrap();
    validate_frozen_oracle(&bytes, input.parent().unwrap()).unwrap();
    let oracle = W::parse(std::str::from_utf8(&bytes).unwrap()).unwrap();
    assert_eq!(text(field(&oracle, "upstreamCommit")), "596f2da7101178214aa27a753529d15e6b7ad91d");
    let cases = array(field(&oracle, "cases"));
    assert_eq!(cases.len(), FROZEN_CASE_COUNT);
    let mut mismatches = Vec::new();
    for (case_index, test) in cases.iter().enumerate() {
        let family = text(field(test, "family"));
        let actual = match family.as_str() {
            "codex" | "gitlab" => replay_owned(test).await,
            "manager" | "factory" => manager_fixture::replay(test).await,
            "descriptor" => {
                // GitLab cache scope hashes the source process cwd. Reproduce
                // the exact retained input instead of normalizing a hash.
                let source_cwd =
                    PathBuf::from(r"C:\temp\ara-model-discovery-batch\oracle\run-20261001T015903Z-d6d698a7")
                        .join("cases")
                        .join(case_index.to_string());
                descriptor_fixture::replay(test, source_cwd).await
            }
            "cursor" | "devin" => rpc_fixture::replay(test).await,
            "openai" | "gemini" | "antigravity" | "gemini-cli" | "google-headers" | "http-native" => {
                root_fixture::replay(test).await
            }
            _ => panic!("fixture family has no Rust replay adapter: {family}"),
        };
        if !actual.deep_equal(field(test, "expected")) {
            mismatches.push(W::object(vec![
                ("id", field(test, "id").clone()),
                ("expected", field(test, "expected").clone()),
                ("actual", actual),
            ]));
        }
    }
    if let Some(output) = std::env::var_os("ARA_CATALOG_DISCOVERY_MISMATCHES") {
        std::fs::write(output, W::Array(mismatches.clone()).stringify()).unwrap();
    }
    assert!(mismatches.is_empty(), "{} discovery mismatches: {}", mismatches.len(), W::Array(mismatches).stringify());
}
