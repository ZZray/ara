//! Replays every fixed schema/export and native wire edge against unchanged OMP.
use ara_cli::catalog_protobuf::*;
use ara_rpc::WireString;
use num_bigint::BigInt;
use serde_json::{Value, json};
use std::{
    collections::BTreeSet,
    sync::{Arc, Mutex},
};

const ARTIFACT_SHA: &str = "177d8560a6c24b3ef51adf689efc7e9ecfb0c872b7bfbbeab065fd4b2e0b0068";
const IR_SHA: &str = "a45a3d2691ea1b58d0ca7c623dc26d407424076a56218e8ebb263cddb06e7ca8";
fn hash(bytes: &[u8]) -> String {
    ring::digest::digest(&ring::digest::SHA256, bytes).as_ref().iter().map(|b| format!("{b:02x}")).collect()
}
fn guard(value: &Value) -> Result<(), String> {
    if value["upstreamCommit"] != "596f2da7101178214aa27a753529d15e6b7ad91d"
        || value["bunVersion"] != "1.4.0"
        || value["schemaCount"] != 635
        || value["enumCount"] != 46
        || value["originalAssertionsExecuted"] != true
        || value["irSha256"] != IR_SHA
    {
        return Err("wrong fixed provenance".into());
    }
    let cases = value["cases"].as_array().ok_or("missing cases")?;
    if cases.len() != 3388 {
        return Err("incomplete corpus".into());
    }
    let unique: BTreeSet<_> = cases.iter().map(|c| c["id"].as_str().ok_or("missing id")).collect::<Result<_, _>>()?;
    if unique.len() != 3388 {
        return Err("duplicate or missing input".into());
    }
    for (path, pin) in [
        (
            "packages/catalog/src/discovery/protobuf.ts",
            "9892a0e1e58fa9f34bf63d5c55b0ded5c3be9b502ec213e02c066d8221249470",
        ),
        (
            "packages/catalog/src/discovery/cursor-proto.ts",
            "733e4526bf073c388f6b237038dbde7f8d43e669fceb527360f68d561af2e01b",
        ),
        (
            "packages/catalog/src/discovery/devin-proto.ts",
            "a06e2dc3bac69ea93b50d9ff4b230102fec5f1cb1ac6ef05b6cb04ee212c3034",
        ),
    ] {
        if value["sourceHashes"][path] != pin {
            return Err("wrong source bytes".into());
        }
    }
    if hash(ara_cli::catalog_proto_schemas::IR.as_bytes()) != IR_SHA {
        return Err("generated IR changed".into());
    }
    Ok(())
}
fn decoded(value: &Value) -> ProtoValue {
    match value["kind"].as_str().unwrap() {
        "undefined" => ProtoValue::Undefined,
        "null" => ProtoValue::Null,
        "bool" => ProtoValue::Bool(value["value"].as_bool().unwrap()),
        "number" => {
            ProtoValue::Number(f64::from_bits(u64::from_str_radix(value["bits"].as_str().unwrap(), 16).unwrap()))
        }
        "bigint" => ProtoValue::BigInt(value["decimal"].as_str().unwrap().parse::<BigInt>().unwrap()),
        "string" => ProtoValue::String(WireString::from_units(
            value["units"].as_array().unwrap().iter().map(|n| n.as_u64().unwrap() as u16).collect(),
        )),
        "bytes" => ProtoValue::Bytes(ByteView::new(
            value["data"].as_array().unwrap().iter().map(|n| n.as_u64().unwrap() as u8).collect(),
        )),
        "array" => ProtoValue::array(value["items"].as_array().unwrap().iter().map(decoded).collect()),
        "object" => {
            let out = ProtoMessage::new(value["ordinary"].as_bool().unwrap());
            for row in value["entries"].as_array().unwrap() {
                out.set_own(decoded(&row[0]).as_string().unwrap(), decoded(&row[1]));
            }
            if !value["prototype"].is_null() {
                out.set_prototype(decoded(&value["prototype"]).as_object());
            }
            ProtoValue::Object(out)
        }
        "function" => ProtoValue::OpaqueFunction,
        _ => panic!("unknown native tag"),
    }
}
fn encoded(value: &ProtoValue) -> Value {
    match value {
        ProtoValue::Undefined => json!({"kind":"undefined"}),
        ProtoValue::Null => json!({"kind":"null"}),
        ProtoValue::Bool(v) => json!({"kind":"bool","value":v}),
        ProtoValue::Number(v) => json!({"kind":"number","bits":format!("{:016x}",v.to_bits())}),
        ProtoValue::BigInt(v) => json!({"kind":"bigint","decimal":v.to_string()}),
        ProtoValue::String(v) => json!({"kind":"string","units":v.units()}),
        ProtoValue::Bytes(v) => json!({"kind":"bytes","data":v.bytes()}),
        ProtoValue::Array(v) => {
            json!({"kind":"array","items":v.lock().unwrap().iter().map(encoded).collect::<Vec<_>>()})
        }
        ProtoValue::Object(v) => {
            let mut out = json!({"kind":"object","ordinary":v.is_ordinary(),"entries":v.own_entries().iter().map(|(k,v)|json!([encoded(&ProtoValue::String(k.clone())),encoded(v)])).collect::<Vec<_>>()});
            if let Some(proto) = v.prototype() {
                out["prototype"] = encoded(&ProtoValue::Object(proto));
            }
            out
        }
        ProtoValue::OpaqueFunction => json!({"kind":"function"}),
    }
}
fn scalar(s: &str) -> ScalarKind {
    match s {
        "bool" => ScalarKind::Bool,
        "bytes" => ScalarKind::Bytes,
        "double" => ScalarKind::Double,
        "enum" => ScalarKind::Enum,
        "float" => ScalarKind::Float,
        "int32" => ScalarKind::Int32,
        "int64" => ScalarKind::Int64,
        "string" => ScalarKind::String,
        "uint32" => ScalarKind::Uint32,
        "uint64" => ScalarKind::Uint64,
        _ => panic!("unknown scalar"),
    }
}
fn codec(case: &Value) -> MessageCodec {
    if let Some(module) = case["module"].as_str() {
        all_schemas().into_iter().find(|(m, n, _)| m == module && n == case["schema"].as_str().unwrap()).unwrap().2
    } else {
        let s = case["schema"].as_str().unwrap();
        let repeat = case["operation"] == "packed";
        pb(
            format!("probe.{s}"),
            vec![FieldDesc::Scalar { no: 1, name: "v".into(), kind: scalar(s), optional: false, repeat }],
        )
    }
}
fn record(rows: &[(&str, ProtoValue)]) -> ProtoValue {
    ProtoValue::Object(ProtoMessage::from_entries(rows))
}
fn message_field(no: u32, name: &str, codec: MessageCodec) -> FieldDesc {
    FieldDesc::Message { no, name: name.into(), reference: MessageReference::new(move || codec.clone()), repeat: false }
}
fn api_replay(op: &str) -> ProtoResult<ProtoValue> {
    if op == "apiReferenceCache" {
        struct Reference(u8);
        impl ReferenceCodec for Reference {
            fn encode(&self, _: &ProtoValue) -> ProtoResult<ByteView> {
                Ok(ByteView::new(vec![self.0]))
            }
            fn decode(&self, _: &ByteView) -> ProtoResult<ProtoMessage> {
                Ok(ProtoMessage::from_entries(&[("value", ProtoValue::Number(self.0.into()))]))
            }
            fn to_json(&self, _: &ProtoValue) -> ProtoResult<ProtoValue> {
                Ok(record(&[("value", ProtoValue::Number(self.0.into()))]))
            }
        }
        let factories = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let count = factories.clone();
        let reference = MessageReference::custom(move || {
            Arc::new(Reference((count.fetch_add(1, std::sync::atomic::Ordering::Relaxed) + 1) as u8))
        });
        let fields = vec![
            FieldDesc::Message { no: 1, name: "a".into(), reference: reference.clone(), repeat: false },
            FieldDesc::Message { no: 2, name: "b".into(), reference, repeat: false },
        ];
        let one = pb("one", fields.clone());
        let two = pb("two", fields);
        let input = ProtoMessage::from_entries(&[("a", ProtoValue::Number(1.0)), ("b", ProtoValue::Number(2.0))]);
        let first = one.encode(&input)?;
        let second = two.encode(&input)?;
        return Ok(record(&[
            ("first", ProtoValue::Bytes(first)),
            ("second", ProtoValue::Bytes(second)),
            ("factories", ProtoValue::Number(factories.load(std::sync::atomic::Ordering::Relaxed) as f64)),
        ]));
    }
    if op == "apiDefaultIdentities" {
        let codec = pb(
            "probe.defaults",
            vec![
                FieldDesc::scalar(1, "a", ScalarKind::Bytes),
                FieldDesc::scalar(2, "b", ScalarKind::Bytes),
                FieldDesc::Map { no: 3, name: "map".into(), value: ValueKind::Scalar(ScalarKind::Bytes) },
                FieldDesc::Scalar { no: 4, name: "r".into(), kind: ScalarKind::Int32, optional: false, repeat: true },
                FieldDesc::Scalar {
                    no: 5,
                    name: "optional".into(),
                    kind: ScalarKind::Int32,
                    optional: true,
                    repeat: false,
                },
                FieldDesc::Oneof {
                    name: "choice".into(),
                    variants: vec![VariantDesc { no: 6, name: "v".into(), kind: ValueKind::Scalar(ScalarKind::Int32) }],
                },
            ],
        );
        let first = codec.create(None);
        let second = codec.create(None);
        let d1 = codec.decode(&ByteView::new(vec![26, 0]))?;
        let d2 = codec.decode(&ByteView::new(vec![26, 0]))?;
        let authored = ProtoMessage::from_entries(&[
            ("r", ProtoValue::array(vec![ProtoValue::Number(1.0)])),
            ("map", ProtoValue::object(&[("x", ProtoValue::Bytes(ByteView::new(vec![2])))])),
        ]);
        let created = codec.create(Some(&authored));
        let same_bytes = |a: ProtoValue, b: ProtoValue| matches!((a,b),(ProtoValue::Bytes(a),ProtoValue::Bytes(b))if a.shares_storage(&b));
        let same_array = |a: ProtoValue, b: ProtoValue| matches!((a,b),(ProtoValue::Array(a),ProtoValue::Array(b))if Arc::ptr_eq(&a,&b));
        let same_object = |a: ProtoValue, b: ProtoValue| matches!((a,b),(ProtoValue::Object(a),ProtoValue::Object(b))if a.same_identity(&b));
        return Ok(record(&[
            ("sameDefault", ProtoValue::Bool(same_bytes(first.get("a"), second.get("a")))),
            ("distinctField", ProtoValue::Bool(!same_bytes(first.get("a"), first.get("b")))),
            (
                "mapMissingShared",
                ProtoValue::Bool(same_bytes(
                    d1.get("map").as_object().unwrap().get(""),
                    d2.get("map").as_object().unwrap().get(""),
                )),
            ),
            (
                "mapDifferentField",
                ProtoValue::Bool(!same_bytes(d1.get("map").as_object().unwrap().get(""), first.get("a"))),
            ),
            ("freshMap", ProtoValue::Bool(!same_object(first.get("map"), second.get("map")))),
            ("freshRepeated", ProtoValue::Bool(!same_array(first.get("r"), second.get("r")))),
            ("ownOptional", ProtoValue::Bool(first.has_own("optional"))),
            ("oneofOwnCase", ProtoValue::Bool(first.get("choice").as_object().unwrap().has_own("case"))),
            ("shallowArray", ProtoValue::Bool(same_array(created.get("r"), authored.get("r")))),
            ("shallowMap", ProtoValue::Bool(same_object(created.get("map"), authored.get("map")))),
        ]));
    }
    if matches!(op, "apiMapMissingMessage" | "apiMapMissingMessageJson") {
        let child = pb("probe.child", vec![FieldDesc::scalar(1, "v", ScalarKind::Int32)]);
        let codec = pb(
            "probe.map",
            vec![FieldDesc::Map {
                no: 1,
                name: "map".into(),
                value: ValueKind::Message(MessageReference::new(move || child.clone())),
            }],
        );
        let decoded = codec.decode(&ByteView::new(vec![10, 0]))?;
        if op == "apiMapMissingMessageJson" {
            return codec.to_json(&decoded);
        }
        return Ok(record(&[
            ("decoded", ProtoValue::Object(decoded.clone())),
            ("binary", ProtoValue::Bytes(codec.encode(&decoded)?)),
        ]));
    }
    if op == "apiRecursive" {
        let cell = Arc::new(std::sync::OnceLock::<MessageCodec>::new());
        let factory = cell.clone();
        let codec = pb(
            "probe.recursive",
            vec![
                FieldDesc::scalar(1, "v", ScalarKind::Int32),
                FieldDesc::Message {
                    no: 2,
                    name: "next".into(),
                    reference: MessageReference::new(move || factory.get().unwrap().clone()),
                    repeat: false,
                },
            ],
        );
        cell.set(codec.clone()).unwrap();
        let third = codec.create(Some(&ProtoMessage::from_entries(&[("v", ProtoValue::Number(3.0))])));
        let second = codec.create(Some(&ProtoMessage::from_entries(&[
            ("v", ProtoValue::Number(2.0)),
            ("next", ProtoValue::Object(third)),
        ])));
        let created = codec.create(Some(&ProtoMessage::from_entries(&[
            ("v", ProtoValue::Number(1.0)),
            ("next", ProtoValue::Object(second)),
        ])));
        let binary = codec.encode(&created)?;
        return Ok(record(&[
            ("created", ProtoValue::Object(created.clone())),
            ("binary", ProtoValue::Bytes(binary.clone())),
            ("decoded", ProtoValue::Object(codec.decode(&binary)?)),
            ("json", codec.to_json(&created)?),
        ]));
    }
    if op == "apiCustomReference" {
        struct Reference;
        impl ReferenceCodec for Reference {
            fn encode(&self, value: &ProtoValue) -> ProtoResult<ByteView> {
                Ok(ByteView::new(vec![8, value.as_number().unwrap() as u8]))
            }
            fn decode(&self, bytes: &ByteView) -> ProtoResult<ProtoMessage> {
                Ok(ProtoMessage::from_entries(&[("read", ProtoValue::Number(bytes.get(1).unwrap().into()))]))
            }
            fn to_json(&self, value: &ProtoValue) -> ProtoResult<ProtoValue> {
                Ok(record(&[("authored", value.clone())]))
            }
        }
        let factories = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let factory_count = factories.clone();
        let codec = pb(
            "probe.custom",
            vec![FieldDesc::Message {
                no: 1,
                name: "m".into(),
                reference: MessageReference::custom(move || {
                    factory_count.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                    Arc::new(Reference)
                }),
                repeat: false,
            }],
        );
        let created = codec.create(Some(&ProtoMessage::from_entries(&[("m", ProtoValue::Number(7.0))])));
        let before = factories.load(std::sync::atomic::Ordering::Relaxed);
        let binary = codec.encode(&created)?;
        let decoded = codec.decode(&binary)?;
        let json = codec.to_json(&created)?;
        return Ok(record(&[
            ("before", ProtoValue::Number(before as f64)),
            ("factories", ProtoValue::Number(factories.load(std::sync::atomic::Ordering::Relaxed) as f64)),
            ("binary", ProtoValue::Bytes(binary)),
            ("decoded", ProtoValue::Object(decoded)),
            ("json", json),
        ]));
    }
    if op == "apiDuplicateNumbers" {
        let codec = pb(
            "probe.duplicates",
            vec![FieldDesc::scalar(1, "a", ScalarKind::Int32), FieldDesc::scalar(1, "b", ScalarKind::Int32)],
        );
        let created = codec.create(Some(&ProtoMessage::from_entries(&[
            ("a", ProtoValue::Number(1.0)),
            ("b", ProtoValue::Number(2.0)),
        ])));
        let binary = codec.encode(&created)?;
        return Ok(record(&[
            ("created", ProtoValue::Object(created.clone())),
            ("binary", ProtoValue::Bytes(binary.clone())),
            ("decoded", ProtoValue::Object(codec.decode(&binary)?)),
            ("json", codec.to_json(&created)?),
        ]));
    }
    if op == "apiOneofDuplicate" {
        let codec = pb(
            "probe.oneof",
            vec![FieldDesc::Oneof {
                name: "pick".into(),
                variants: vec![
                    VariantDesc { no: 1, name: "shared".into(), kind: ValueKind::Scalar(ScalarKind::Int32) },
                    VariantDesc { no: 2, name: "shared".into(), kind: ValueKind::Scalar(ScalarKind::String) },
                ],
            }],
        );
        let decoded = codec.decode(&ByteView::new(vec![8, 7]))?;
        return Ok(record(&[
            ("decoded", ProtoValue::Object(decoded.clone())),
            ("binary", ProtoValue::Bytes(codec.encode(&decoded)?)),
        ]));
    }
    if op == "apiProtoString" {
        return pb("probe.prototype", vec![FieldDesc::scalar(1, "__proto__", ScalarKind::String)])
            .encode(&ProtoMessage::new(true))
            .map(ProtoValue::Bytes);
    }
    if op == "apiProtoMap" {
        let codec = pb(
            "probe.prototypeMap",
            vec![FieldDesc::Map { no: 1, name: "__proto__".into(), value: ValueKind::Scalar(ScalarKind::String) }],
        );
        let created = codec.create(None);
        return Ok(record(&[
            ("created", ProtoValue::Object(created.clone())),
            ("binary", ProtoValue::Bytes(codec.encode(&created)?)),
            ("json", codec.to_json(&created)?),
        ]));
    }
    if op == "apiMapInherited" {
        let map = ProtoMessage::from_entries(&[("own", ProtoValue::string("value"))]);
        map.set_prototype(Some(ProtoMessage::from_entries(&[("inherited", ProtoValue::string("yes"))])));
        let codec = pb(
            "probe.inheritedMap",
            vec![FieldDesc::Map { no: 1, name: "map".into(), value: ValueKind::Scalar(ScalarKind::String) }],
        );
        let created = codec.create(Some(&ProtoMessage::from_entries(&[("map", ProtoValue::Object(map))])));
        let binary = codec.encode(&created)?;
        return Ok(record(&[
            ("created", ProtoValue::Object(created.clone())),
            ("binary", ProtoValue::Bytes(binary.clone())),
            ("decoded", ProtoValue::Object(codec.decode(&binary)?)),
            ("json", codec.to_json(&created)?),
        ]));
    }
    if op == "apiJsonInherited" {
        let input = ProtoMessage::from_entries(&[("own", ProtoValue::Number(1.0))]);
        input.set_prototype(Some(ProtoMessage::from_entries(&[("inherited", ProtoValue::Number(2.0))])));
        let binary = encode_json_value(&ProtoValue::Object(input))?;
        return Ok(record(&[("binary", ProtoValue::Bytes(binary.clone())), ("decoded", decode_json_value(&binary)?)]));
    }
    if op == "apiSingularReplacement" {
        let child = pb("probe.child", vec![FieldDesc::scalar(1, "v", ScalarKind::Int32)]);
        let codec = pb("probe.replace", vec![message_field(1, "child", child)]);
        let decoded = codec.decode(&ByteView::new(vec![10, 2, 8, 1, 10, 2, 16, 2]))?;
        return Ok(record(&[
            ("decoded", ProtoValue::Object(decoded.clone())),
            ("binary", ProtoValue::Bytes(codec.encode(&decoded)?)),
        ]));
    }
    if op == "apiMapCrossing" {
        let codec = pb(
            "probe.cross",
            vec![FieldDesc::Map { no: 1, name: "map".into(), value: ValueKind::Scalar(ScalarKind::Int32) }],
        );
        let decoded = codec.decode(&ByteView::new(vec![10, 2, 16, 128, 1]))?;
        return Ok(record(&[
            ("decoded", ProtoValue::Object(decoded.clone())),
            ("binary", ProtoValue::Bytes(codec.encode(&decoded)?)),
            ("json", codec.to_json(&decoded)?),
        ]));
    }
    if op == "apiFacade" {
        let codec = pb("probe.facade", vec![FieldDesc::scalar(1, "v", ScalarKind::Int32)]);
        let created = create(&codec, Some(&ProtoMessage::from_entries(&[("v", ProtoValue::Number(7.0))])));
        let binary = to_binary(&codec, &created)?;
        let CodecOutput::Bytes(call_binary) = codec.invoke(CodecArgument::Message(created.clone()))? else {
            unreachable!()
        };
        let CodecOutput::Message(call_decoded) = codec.invoke(CodecArgument::Bytes(binary.clone()))? else {
            unreachable!()
        };
        return Ok(record(&[
            ("binary", ProtoValue::Bytes(binary.clone())),
            ("json", to_json(&codec, &created)?),
            ("decoded", ProtoValue::Object(from_binary(&codec, &binary)?)),
            ("callBinary", ProtoValue::Bytes(call_binary)),
            ("callDecoded", ProtoValue::Object(call_decoded)),
        ]));
    }
    if op == "apiJsonInvalidUtf8" {
        return decode_json_value(&ByteView::new(vec![26, 1, 128]));
    }
    panic!("unhandled public API probe {op}")
}
fn replay(case: &Value) -> ProtoResult<ProtoValue> {
    let op = case["operation"].as_str().unwrap();
    if op.starts_with("api") {
        return api_replay(op);
    }
    if op == "jsonValue" {
        let binary = encode_json_value(&decoded(&case["input"]))?;
        return Ok(record(&[("binary", ProtoValue::Bytes(binary.clone())), ("decoded", decode_json_value(&binary)?)]));
    }
    if op == "unknownView" {
        let codec = pb("probe.identity", vec![FieldDesc::scalar(1, "v", ScalarKind::Int32)]);
        let raw = ByteView::new(vec![16, 129, 0]);
        let m = codec.decode(&raw)?;
        raw.set(1, 130);
        let data = m.get("$unknown").items()[0].as_object().unwrap().get("data");
        return Ok(record(&[("binary", ProtoValue::Bytes(codec.encode(&m)?)), ("data", data)]));
    }
    if op == "inherited" {
        let codec = pb("probe.identity", vec![FieldDesc::scalar(1, "v", ScalarKind::Int32)]);
        let input = ProtoMessage::from_entries(&[("opaque", ProtoValue::Undefined), ("keep", ProtoValue::Number(1.0))]);
        input.set_prototype(Some(ProtoMessage::from_entries(&[("inherited", ProtoValue::Number(2.0))])));
        return Ok(ProtoValue::Object(codec.create(Some(&input))));
    }
    if op == "lazyFreeze" {
        let fields = Arc::new(Mutex::new(vec![]));
        let codec = pb_shared("probe.lazy", fields.clone());
        fields.lock().unwrap().push(FieldDesc::scalar(1, "v", ScalarKind::Int32));
        codec.create(None);
        fields.lock().unwrap().push(FieldDesc::scalar(2, "later", ScalarKind::String));
        return Ok(record(&[
            ("created", ProtoValue::Object(codec.create(None))),
            (
                "binary",
                ProtoValue::Bytes(codec.encode(&ProtoMessage::from_entries(&[
                    ("v", ProtoValue::Number(7.0)),
                    ("later", ProtoValue::string("ignored")),
                ]))?),
            ),
        ]));
    }
    let codec = codec(case);
    if op == "decode" || op == "packed" {
        let ProtoValue::Bytes(bytes) = decoded(&case["input"]) else { panic!("bytes") };
        let m = codec.decode(&bytes)?;
        return Ok(record(&[
            ("decoded", ProtoValue::Object(m.clone())),
            ("binary", ProtoValue::Bytes(codec.encode(&m)?)),
            ("json", codec.to_json(&m)?),
        ]));
    }
    let input = decoded(&case["input"]).as_object().unwrap();
    let created = codec.create(Some(&input));
    let binary = codec.encode(&created)?;
    let json = codec.to_json(&created)?;
    if op == "scalar" {
        return Ok(record(&[
            ("created", ProtoValue::Object(created)),
            ("binary", ProtoValue::Bytes(binary)),
            ("json", json),
        ]));
    }
    let decoded = codec.decode(&binary)?;
    let reencoded = codec.encode(&decoded)?;
    Ok(record(&[
        ("created", ProtoValue::Object(created)),
        ("binary", ProtoValue::Bytes(binary)),
        ("json", json),
        ("decoded", ProtoValue::Object(decoded)),
        ("reencoded", ProtoValue::Bytes(reencoded)),
    ]))
}
#[test]
#[ignore = "requires retained fixed OMP artifact in ARA_CATALOG_PROTOBUF_ORACLE"]
fn all_fixed_schema_exports_and_native_values_compare() {
    let path = std::env::var_os("ARA_CATALOG_PROTOBUF_ORACLE").expect("capture fixed source first");
    let bytes = std::fs::read(path).unwrap();
    assert_eq!(hash(&bytes), ARTIFACT_SHA, "frozen reviewed corpus");
    let oracle: Value = serde_json::from_slice(&bytes).unwrap();
    guard(&oracle).unwrap();
    assert_eq!(all_schemas().len(), 635);
    let mut differences = vec![];
    for case in oracle["cases"].as_array().unwrap() {
        let actual = match replay(case) {
            Ok(v) => json!({"status":"ok","value":encoded(&v)}),
            Err(e) => json!({"status":"error","name":e.name,"message":e.message}),
        };
        if actual != case["expected"] {
            differences.push(json!({"id":case["id"],"expected":case["expected"],"actual":actual}));
        }
    }
    if let Some(path) = std::env::var_os("ARA_CATALOG_PROTOBUF_MISMATCHES") {
        std::fs::write(path, serde_json::to_vec(&differences).unwrap()).unwrap();
    }
    assert!(
        differences.is_empty(),
        "{} native protobuf differences, first={}",
        differences.len(),
        json!(differences.first())
    );
}
#[test]
#[ignore = "requires retained fixed OMP artifact in ARA_CATALOG_PROTOBUF_ORACLE"]
fn malformed_fixed_protobuf_corpus_is_rejected() {
    let bytes = std::fs::read(std::env::var_os("ARA_CATALOG_PROTOBUF_ORACLE").unwrap()).unwrap();
    let oracle: Value = serde_json::from_slice(&bytes).unwrap();
    guard(&oracle).unwrap();
    let mut changed = oracle.clone();
    changed["cases"].as_array_mut().unwrap().pop();
    assert!(guard(&changed).is_err());
    let mut changed = oracle.clone();
    changed["cases"][1] = changed["cases"][0].clone();
    assert!(guard(&changed).is_err());
    let mut changed = oracle.clone();
    changed["schemaCount"] = json!(634);
    assert!(guard(&changed).is_err());
    let mut changed = oracle.clone();
    changed["sourceHashes"]["packages/catalog/src/discovery/protobuf.ts"] = json!("wrong");
    assert!(guard(&changed).is_err());
}

/// Fixed Bun source probes read and mutate the repeated array from the public
/// reference callback: encode uses for-of, toJson uses Array.map.
#[test]
fn repeated_public_codec_callbacks_observe_live_array_without_held_lock() {
    struct Reference {
        rows: Arc<Mutex<Vec<ProtoValue>>>,
        seen: Arc<Mutex<Vec<(u8, usize)>>>,
        mutation: &'static str,
    }
    impl Reference {
        fn visit(&self, value: &ProtoValue) -> u8 {
            let value = value.as_number().unwrap() as u8;
            // Fail promptly on regression instead of hanging the test process.
            let mut rows = self.rows.try_lock().expect("codec callback must be able to read its repeated array");
            self.seen.lock().unwrap().push((value, rows.len()));
            if value == 1 {
                match self.mutation {
                    "push" => rows.push(ProtoValue::Number(3.0)),
                    "replace" => rows[1] = ProtoValue::Number(9.0),
                    _ => {}
                }
            }
            value
        }
    }
    impl ReferenceCodec for Reference {
        fn encode(&self, value: &ProtoValue) -> ProtoResult<ByteView> {
            Ok(ByteView::new(vec![8, self.visit(value)]))
        }
        fn decode(&self, _: &ByteView) -> ProtoResult<ProtoMessage> {
            unreachable!("only encode/toJson source probes")
        }
        fn to_json(&self, value: &ProtoValue) -> ProtoResult<ProtoValue> {
            Ok(record(&[("v", ProtoValue::Number(self.visit(value).into()))]))
        }
    }
    for operation in ["encode", "toJson"] {
        for mutation in ["read", "push", "replace"] {
            let rows = Arc::new(Mutex::new(vec![ProtoValue::Number(1.0), ProtoValue::Number(2.0)]));
            let seen = Arc::new(Mutex::new(Vec::new()));
            let callback = Arc::new(Reference { rows: rows.clone(), seen: seen.clone(), mutation });
            let codec = pb(
                "probe.reentrant",
                vec![FieldDesc::Message {
                    no: 1,
                    name: "rows".into(),
                    repeat: true,
                    reference: MessageReference::custom(move || callback.clone()),
                }],
            );
            let input = ProtoMessage::from_entries(&[("rows", ProtoValue::Array(rows))]);
            let expected = match (operation, mutation) {
                (_, "replace") => vec![1, 9],
                ("encode", "push") => vec![1, 2, 3],
                _ => vec![1, 2],
            };
            if operation == "encode" {
                let expected_bytes: Vec<u8> = expected.iter().flat_map(|value| [10, 2, 8, *value]).collect();
                assert_eq!(codec.encode(&input).unwrap().bytes(), expected_bytes);
            } else {
                let actual = codec.to_json(&input).unwrap().as_object().unwrap().get("rows").items();
                let actual: Vec<u8> =
                    actual.iter().map(|value| value.as_object().unwrap().get("v").as_number().unwrap() as u8).collect();
                assert_eq!(actual, expected);
            }
            let actual: Vec<u8> = seen.lock().unwrap().iter().map(|(value, _)| *value).collect();
            assert_eq!(actual, expected);
            if mutation == "push" {
                assert_eq!(seen.lock().unwrap()[1].1, 3, "map sees the current array while retaining original length");
            }
        }
    }
}
