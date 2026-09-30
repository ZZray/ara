//! Golden receipts produced by unchanged fixed OMP modules under Bun 1.4.0.
use ara_rpc::{RpcFrameDecoder, RpcFrameEncoder, WireValue};
use base64::{Engine, engine::general_purpose::STANDARD};
use ring::digest::{Context, SHA256, digest};
use serde_json::{Value, json};
use std::{
    io,
    pin::Pin,
    task::{Context as TaskContext, Poll},
};
use tokio::io::{AsyncRead, ReadBuf};

fn sha(bytes: &[u8]) -> String {
    digest(&SHA256, bytes).as_ref().iter().map(|b| format!("{b:02x}")).collect()
}
fn fingerprint(bytes: &[u8]) -> Value {
    json!({"byteLength":bytes.len(),"sha256":sha(bytes)})
}
fn codec_error_name(message: &str) -> &'static str {
    // RpcError stores a message, while Bun's fatal TextDecoder reports a
    // TypeError. Retain that distinction in the comparison.
    if message == "The encoded data was not valid for encoding utf-8" { "TypeError" } else { "Error" }
}
fn describe(lines: impl Iterator<Item = String>) -> Value {
    let mut all = Context::new(&SHA256);
    let mut physical = Vec::new();
    let mut total = 0;
    let mut first = None;
    let mut last = String::new();
    let mut single = None;
    for line in lines {
        let bytes = line.as_bytes();
        assert!(bytes.len() <= ara_rpc::frame::MAX_RPC_FRAME_BYTES && line.ends_with('\n'));
        all.update(bytes);
        total += bytes.len();
        first.get_or_insert_with(|| STANDARD.encode(&bytes[..bytes.len().min(160)]));
        last = STANDARD.encode(&bytes[bytes.len().saturating_sub(160)..]);
        physical.push(fingerprint(bytes));
        if physical.len() == 1 && bytes.len() <= 4096 {
            single = Some(STANDARD.encode(bytes));
        }
        if physical.len() > 1 {
            single = None;
        }
    }
    let hash: String = all.finish().as_ref().iter().map(|b| format!("{b:02x}")).collect();
    let mut result = json!({"totalBytes":total,"sha256":hash,"physical":physical,
        "firstPrefixBase64":first,"lastSuffixBase64":last});
    if let Some(single) = single {
        result["encodedBase64"] = single.into();
    }
    result
}
fn wire(value: &Value) -> WireValue {
    WireValue::parse(&value.to_string()).unwrap()
}
fn s(text: impl AsRef<str>) -> WireValue {
    WireValue::String(text.as_ref().into())
}
fn num(recipe: &Value, key: &str) -> usize {
    recipe[key].as_u64().unwrap() as usize
}
fn response(payload: String) -> WireValue {
    WireValue::object(vec![
        ("id", s("req")),
        ("type", s("response")),
        ("command", s("get_state")),
        ("success", WireValue::Bool(true)),
        ("data", WireValue::object(vec![("payload", s(payload))])),
    ])
}
fn repeated(recipe: &Value) -> String {
    recipe["prefix"].as_str().unwrap_or("").to_owned() + &recipe["char"].as_str().unwrap().repeat(num(recipe, "count"))
}
fn make_frame(recipe: &Value) -> WireValue {
    match recipe["kind"].as_str().unwrap() {
        "literal" => wire(&recipe["frame"]),
        "literal-json" => WireValue::parse(recipe["json"].as_str().unwrap()).unwrap(),
        "response-repeat" => response(repeated(recipe)),
        "event-repeat" => WireValue::object(vec![
            ("type", s("message_end")),
            (
                "message",
                WireValue::object(vec![
                    ("role", s("assistant")),
                    (
                        "content",
                        WireValue::Array(vec![WireValue::object(vec![
                            ("type", s("text")),
                            ("text", s(repeated(recipe))),
                        ])]),
                    ),
                ]),
            ),
        ]),
        "event-key-fields" => {
            let padding = "x".repeat(num(recipe, "keyPadding"));
            let details = WireValue::Object(
                (0..num(recipe, "fieldCount"))
                    .map(|index| (format!("k{index}-{padding}").into(), WireValue::Number(0.0)))
                    .collect(),
            );
            WireValue::object(vec![("type", s("tool_execution_end")), ("details", details)])
        }
        "event-one-huge-key" => WireValue::object(vec![
            ("type", s("tool_execution_end")),
            (
                "details",
                WireValue::Object(vec![(
                    format!("k{}", "x".repeat(num(recipe, "keyPadding"))).into(),
                    WireValue::Number(0.0),
                )]),
            ),
        ]),
        "event-object-special-keys" => {
            let mut entries =
                vec![("rpcFrameElidedKeys".into(), s("original")), ("__proto__".into(), s("own-json-key"))];
            let item = s("x".repeat(num(recipe, "itemChars")));
            entries.extend((0..num(recipe, "keys")).map(|index| (format!("k{index}").into(), item.clone())));
            WireValue::object(vec![("type", s("message_end")), ("payload", WireValue::Object(entries))])
        }
        "event-array-repeat" => {
            let item = s("x".repeat(num(recipe, "itemChars")));
            WireValue::object(vec![
                ("type", s("message_end")),
                ("payload", WireValue::Array(vec![item; num(recipe, "count")])),
            ])
        }
        other => panic!("unknown frame recipe {other}"),
    }
}
fn chunk(data: &[u8], index: usize, count: usize, length: usize) -> WireValue {
    WireValue::object(vec![
        ("type", s("rpc_chunk")),
        ("chunkId", s("c")),
        ("index", WireValue::Number(index as f64)),
        ("count", WireValue::Number(count as f64)),
        ("byteLength", WireValue::Number(length as f64)),
        ("data", s(STANDARD.encode(data))),
    ])
}
fn encoded_response_chunks(recipe: &Value) -> Vec<WireValue> {
    let mut encoder = RpcFrameEncoder::new();
    encoder.set_protocol_version(2).unwrap();
    encoder.encode_frames(&response(repeated(recipe))).map(|line| WireValue::parse(&line).unwrap()).collect()
}
fn make_chunks(recipe: &Value) -> Vec<WireValue> {
    match recipe["kind"].as_str().unwrap() {
        "literal" => recipe["frames"].as_array().unwrap().iter().map(wire).collect(),
        "encoded-response-repeat" => encoded_response_chunks(recipe),
        "chunk-repeat" => (0..num(recipe, "count"))
            .map(|i| {
                let len = if i + 1 == num(recipe, "count") {
                    recipe["lastPayloadBytes"].as_u64().map(|n| n as usize).unwrap_or(num(recipe, "payloadBytes"))
                } else {
                    num(recipe, "payloadBytes")
                };
                chunk(&vec![num(recipe, "byte") as u8; len], i, num(recipe, "count"), num(recipe, "declaredBytes"))
            })
            .collect(),
        "bytes-repeat" => {
            let mut bytes = vec![num(recipe, "byte") as u8; num(recipe, "count")];
            bytes[0] = num(recipe, "firstByte") as u8;
            bytes
                .chunks(num(recipe, "payloadBytes"))
                .enumerate()
                .map(|(i, b)| chunk(b, i, num(recipe, "chunks"), bytes.len()))
                .collect()
        }
        "chunk-overrides" => {
            let mut frame = chunk(b"a", 0, 2, ara_rpc::frame::MAX_RPC_FRAME_BYTES);
            for (key, value) in recipe["overrides"].as_object().unwrap() {
                frame.insert(key, wire(value));
            }
            vec![frame]
        }
        "chunk-payload-repeat" => vec![chunk(
            &vec![num(recipe, "byte") as u8; num(recipe, "count")],
            0,
            2,
            ara_rpc::frame::MAX_RPC_FRAME_BYTES,
        )],
        "encoded-response-with-fault" => {
            let chunks = encoded_response_chunks(recipe);
            let fault_at = num(recipe, "faultAt");
            let mut corrupted = chunks[fault_at].clone();
            corrupted.insert("data", s(recipe["faultData"].as_str().unwrap()));
            chunks[..fault_at]
                .iter()
                .cloned()
                .chain(std::iter::once(corrupted))
                .chain(chunks[fault_at..].iter().cloned())
                .collect()
        }
        "encoded-response-with-interruption" => {
            let chunks = encoded_response_chunks(recipe);
            let after_index = num(recipe, "afterIndex");
            chunks[..=after_index]
                .iter()
                .cloned()
                .chain(std::iter::once(wire(&recipe["interrupt"])))
                .chain(chunks[after_index + 1..].iter().cloned())
                .collect()
        }
        "chunked-logical-bytes" => {
            let mut logical = recipe["prefix"].as_str().unwrap().as_bytes().to_vec();
            let filler = recipe["filler"].as_str().unwrap().as_bytes();
            assert_eq!(filler.len(), 1);
            logical.resize(num(recipe, "totalBytes"), filler[0]);
            let count = logical.len().div_ceil(ara_rpc::frame::RPC_CHUNK_PAYLOAD_BYTES);
            let mut frames: Vec<_> = logical
                .chunks(ara_rpc::frame::RPC_CHUNK_PAYLOAD_BYTES)
                .enumerate()
                .map(|(index, data)| chunk(data, index, count, logical.len()))
                .collect();
            frames.push(wire(&recipe["after"]));
            frames
        }
        "logical-response-variable-chunks" => {
            let logical = response(repeated(recipe)).stringify().into_bytes();
            let sizes = recipe["sizes"].as_array().unwrap();
            assert_eq!(sizes.iter().map(|value| value.as_u64().unwrap() as usize).sum::<usize>(), logical.len());
            let mut start = 0;
            sizes
                .iter()
                .enumerate()
                .map(|(index, size)| {
                    let end = start + size.as_u64().unwrap() as usize;
                    let frame = chunk(&logical[start..end], index, sizes.len(), logical.len());
                    start = end;
                    frame
                })
                .collect()
        }
        "chunk-repeat-then-ready" => {
            let mut frames: Vec<_> = (0..num(recipe, "count"))
                .map(|index| {
                    let size = if index + 1 == num(recipe, "count") {
                        num(recipe, "lastPayloadBytes")
                    } else {
                        num(recipe, "payloadBytes")
                    };
                    chunk(
                        &vec![num(recipe, "byte") as u8; size],
                        index,
                        num(recipe, "count"),
                        num(recipe, "declaredBytes"),
                    )
                })
                .collect();
            frames.push(WireValue::object(vec![("type", s("ready"))]));
            frames
        }
        "bytes-repeat-then-ready" => {
            let mut logical = vec![num(recipe, "byte") as u8; num(recipe, "count")];
            logical[0] = num(recipe, "firstByte") as u8;
            let mut frames: Vec<_> = logical
                .chunks(num(recipe, "payloadBytes"))
                .enumerate()
                .map(|(index, bytes)| chunk(bytes, index, num(recipe, "chunks"), logical.len()))
                .collect();
            frames.push(WireValue::object(vec![("type", s("ready"))]));
            frames
        }
        other => panic!("unknown decoder recipe {other}"),
    }
}
fn decoded(frame: &WireValue) -> Value {
    let text = frame.stringify();
    let mut result = fingerprint(text.as_bytes());
    if text.len() < 4096 {
        result["json"] = text.into();
    }
    if let WireValue::Number(number) = frame {
        let display = if number.is_nan() {
            "NaN".to_owned()
        } else if *number == f64::INFINITY {
            "Infinity".to_owned()
        } else if *number == f64::NEG_INFINITY {
            "-Infinity".to_owned()
        } else if *number == 0.0 {
            "0".to_owned()
        } else {
            ryu_js::Buffer::new().format_finite(*number).to_owned()
        };
        result["rawNumber"] = json!({
            "negativeZero":*number == 0.0 && number.is_sign_negative(),
            "finite":number.is_finite(),
            "display":display,
        });
    }
    result
}

fn encode_observations(recipe: &Value, lines: &[String]) -> Option<Value> {
    let kind = recipe["kind"].as_str().unwrap();
    if kind == "event-repeat" && recipe["prefix"] == "A" {
        return Some(json!({"escapedLoneHighSurrogate":lines.concat().contains("\\ud83d")}));
    }
    let decoded: Value = serde_json::from_str(lines.first()?).unwrap();
    match kind {
        "event-key-fields" => {
            let details = decoded["details"].as_object().unwrap();
            Some(json!({
                "retainedKeys":details.len() - 1,
                "elidedKeys":decoded["details"]["rpcFrameElidedKeys"],
            }))
        }
        "event-one-huge-key" => Some(json!({"resultType":decoded["type"]})),
        "event-object-special-keys" => Some(json!({
            "elidedKeys":decoded["payload"]["rpcFrameElidedKeys"],
            "hasOwnProto":decoded["payload"].as_object().unwrap().contains_key("__proto__"),
        })),
        "event-array-repeat" => {
            let payload = decoded["payload"].as_array().unwrap();
            Some(json!({"retained":payload.len(),"terminalElision":payload.last()}))
        }
        _ => None,
    }
}
fn make_input(recipe: &Value) -> Vec<u8> {
    match recipe["kind"].as_str().unwrap() {
        "utf8" | "utf8-split" => recipe["text"].as_str().unwrap().as_bytes().to_vec(),
        "hex" => recipe["chunks"]
            .as_array()
            .unwrap()
            .iter()
            .flat_map(|part| {
                part.as_str()
                    .unwrap()
                    .as_bytes()
                    .chunks(2)
                    .map(|digits| u8::from_str_radix(std::str::from_utf8(digits).unwrap(), 16).unwrap())
                    .collect::<Vec<_>>()
            })
            .collect(),
        "utf8-repeat" => (recipe["prefix"].as_str().unwrap().to_owned()
            + &recipe["char"].as_str().unwrap().repeat(num(recipe, "count"))
            + recipe["suffix"].as_str().unwrap())
        .into_bytes(),
        "nested-array" => {
            let mut text = "[".repeat(num(recipe, "depth"));
            text.push_str(&recipe["leaf"].to_string());
            text.push_str(&"]".repeat(num(recipe, "depth")));
            if recipe["finalNewline"] == true {
                text.push('\n');
            }
            text.into_bytes()
        }
        other => panic!("unknown input recipe {other}"),
    }
}

struct FaultReader {
    first: Option<Vec<u8>>,
    message: String,
}

impl AsyncRead for FaultReader {
    fn poll_read(mut self: Pin<&mut Self>, _: &mut TaskContext<'_>, buffer: &mut ReadBuf<'_>) -> Poll<io::Result<()>> {
        if let Some(first) = self.first.take() {
            assert!(buffer.remaining() >= first.len());
            buffer.put_slice(&first);
            Poll::Ready(Ok(()))
        } else {
            Poll::Ready(Err(io::Error::other(self.message.clone())))
        }
    }
}

fn stateful_messages(recipe: &Value) -> Vec<WireValue> {
    (0..num(recipe, "messageCount"))
        .map(|index| {
            WireValue::object(vec![
                ("role", s("assistant")),
                (
                    "content",
                    WireValue::Array(vec![WireValue::object(vec![
                        ("type", s("text")),
                        ("text", s(format!("{index}-{}", "x".repeat(num(recipe, "messageChars"))))),
                    ])]),
                ),
            ])
        })
        .collect()
}

fn agent_end(messages: Vec<WireValue>, continuing: bool) -> WireValue {
    let mut frame = WireValue::object(vec![("type", s("agent_end")), ("messages", WireValue::Array(messages))]);
    if continuing {
        frame.insert("willContinue", WireValue::Bool(true));
    }
    frame
}

fn stateful_result(recipe: &Value) -> (Value, Option<Value>) {
    let mut encoder = RpcFrameEncoder::new();
    encoder.set_protocol_version(recipe["version"].as_u64().unwrap() as u8).unwrap();
    let start_frame = WireValue::object(vec![("type", s("agent_start"))]);
    let start = describe(encoder.encode_frames(&start_frame));
    match recipe["kind"].as_str().unwrap() {
        "stateful-mutation" | "stateful-prefix" => {
            let mut messages = stateful_messages(recipe);
            let streamed: Vec<_> = messages
                .iter()
                .map(|message| {
                    describe(encoder.encode_frames(&WireValue::object(vec![
                        ("type", s("message_end")),
                        ("message", message.clone()),
                    ])))
                })
                .collect();
            if recipe["kind"] == "stateful-mutation" {
                let content = messages[0].get("content").unwrap().as_array().unwrap()[0]
                    .get("text")
                    .unwrap()
                    .as_string()
                    .unwrap()
                    .to_utf8()
                    .unwrap();
                messages[0].insert(
                    "content",
                    WireValue::Array(vec![WireValue::object(vec![
                        ("type", s("text")),
                        ("text", s(format!("{}{}", recipe["mutation"]["firstTextPrefix"].as_str().unwrap(), content))),
                    ])]),
                );
                let terminal = describe(encoder.encode_frames(&agent_end(messages.clone(), true)));
                let final_frame = describe(encoder.encode_frames(&agent_end(messages, false)));
                (json!({"start":start,"streamed":streamed,"terminal":terminal,"final":final_frame}), None)
            } else {
                let continuing = describe(encoder.encode_frames(&agent_end(messages.clone(), true)));
                let final_frame = describe(encoder.encode_frames(&agent_end(messages.clone(), false)));
                let after_final_reset = describe(encoder.encode_frames(&agent_end(messages.clone(), false)));
                let new_start = describe(encoder.encode_frames(&start_frame));
                let after_start_reset = describe(encoder.encode_frames(&agent_end(messages, false)));
                let observations = json!({
                    "continuingCompact": continuing["totalBytes"].as_u64().unwrap() < 1024,
                    "finalCompact": final_frame["totalBytes"].as_u64().unwrap() < 1024,
                    "afterFinalNotCompact": after_final_reset["totalBytes"].as_u64().unwrap() > 1024,
                    "afterStartNotCompact": after_start_reset["totalBytes"].as_u64().unwrap() > 1024,
                });
                (
                    json!({"start":start,"streamed":streamed,"continuing":continuing,"final":final_frame,
                        "afterFinalReset":after_final_reset,"newStart":new_start,"afterStartReset":after_start_reset}),
                    Some(observations),
                )
            }
        }
        "stateful-shrunk-message" => {
            let message = WireValue::object(vec![
                ("role", s("assistant")),
                (
                    "content",
                    WireValue::Array(vec![WireValue::object(vec![
                        ("type", s("text")),
                        ("text", s(recipe["char"].as_str().unwrap().repeat(num(recipe, "count")))),
                    ])]),
                ),
            ]);
            let streamed = describe(
                encoder
                    .encode_frames(&WireValue::object(vec![("type", s("message_end")), ("message", message.clone())])),
            );
            let terminal = describe(encoder.encode_frames(&agent_end(vec![message], false)));
            let observations = json!({"terminalCompacted":terminal["totalBytes"].as_u64().unwrap() < 1024});
            (json!({"start":start,"streamed":streamed,"terminal":terminal}), Some(observations))
        }
        other => panic!("unknown stateful recipe {other}"),
    }
}

#[tokio::test]
async fn fixed_omp_byte_and_error_oracle() {
    let path = std::env::var("ARA_RPC_ORACLE")
        .unwrap_or_else(|_| format!("{}/tests/fixtures/omp-rpc-oracle.json", env!("CARGO_MANIFEST_DIR")));
    let document: Value =
        serde_json::from_slice(&std::fs::read(&path).expect("generate oracle fixture first")).unwrap();
    assert_eq!(document["upstreamCommit"], "596f2da7101178214aa27a753529d15e6b7ad91d");
    for case in document["cases"].as_array().unwrap() {
        let id = case["id"].as_str().unwrap();
        let recipe = &case["recipe"];
        match case["kind"].as_str().unwrap() {
            "encode" => {
                let mut encoder = RpcFrameEncoder::new();
                encoder.set_protocol_version(case["version"].as_u64().unwrap() as u8).unwrap();
                let lines: Vec<_> = encoder.encode_frames(&make_frame(recipe)).collect();
                if !case["observations"].is_null() {
                    assert_eq!(
                        encode_observations(recipe, &lines),
                        Some(case["observations"].clone()),
                        "{id} observations"
                    );
                }
                assert_eq!(describe(lines.into_iter()), case["result"], "{id}");
            }
            "stateful" => {
                let (result, observations) = stateful_result(recipe);
                assert_eq!(result, case["result"], "{id}");
                if let Some(observations) = observations {
                    assert_eq!(observations, case["observations"], "{id} observations");
                }
            }
            "decode" => {
                let mut decoder = RpcFrameDecoder::new();
                let mut outputs = Vec::new();
                let mut failure = None;
                for frame in make_chunks(recipe) {
                    match decoder.push(frame) {
                        Ok(Some(value)) => outputs.push(decoded(&value)),
                        Ok(None) => {}
                        Err(error) => {
                            failure = Some(error.to_string());
                            break;
                        }
                    }
                }
                if let Some(failure) = failure {
                    assert_eq!(codec_error_name(&failure), case["error"]["name"].as_str().unwrap(), "{id} error class");
                    assert_eq!(failure, case["error"]["message"].as_str().unwrap(), "{id}");
                    assert_eq!(json!(outputs), case["outputsBeforeError"], "{id}");
                } else {
                    assert_eq!(json!(outputs), case["result"], "{id}");
                }
            }
            "decode-trace" => {
                let mut decoder = RpcFrameDecoder::new();
                let trace: Vec<_> = make_chunks(recipe)
                    .into_iter()
                    .map(|frame| match decoder.push(frame) {
                        Ok(None) => json!({"event":"pending"}),
                        Ok(Some(value)) => {
                            let mut record = decoded(&value);
                            record["event"] = "frame".into();
                            record
                        }
                        Err(error) => {
                            let message = error.to_string();
                            json!({"event":"error","name":codec_error_name(&message),"message":message})
                        }
                    })
                    .collect();
                let expected = case["result"].as_array().unwrap();
                assert_eq!(trace.len(), expected.len(), "{id} trace length");
                for (index, (actual, source)) in trace.iter().zip(expected).enumerate() {
                    if id == "chunk-malformed-final-json" && source["event"] == "error" {
                        // The fixed Bun engine emits SyntaxError wording; the
                        // Rust parser reports its own byte-position diagnostic.
                        // Compare the parse-error class and subsequent state.
                        assert_eq!(source["name"], "SyntaxError", "{id} source class");
                        assert_eq!(actual["event"], "error", "{id} event {index}");
                        assert!(
                            actual["message"].as_str().unwrap().starts_with("JSON at byte "),
                            "{id} Rust parse diagnostic"
                        );
                    } else {
                        assert_eq!(actual, source, "{id} event {index}");
                    }
                }
            }
            "input" => {
                let bytes = make_input(recipe);
                let mut reader = ara_rpc::input::RpcInputReader::new(bytes.as_slice());
                let mut frames = Vec::new();
                let mut errors = Vec::new();
                while let Some(item) = reader.read_next().await.unwrap() {
                    match item {
                        ara_rpc::input::InputItem::Frame(frame) => frames.push(decoded(&frame)),
                        ara_rpc::input::InputItem::ParseError(error) => errors.push(error),
                    }
                }
                assert_eq!(json!(frames), case["result"]["frames"], "{id}");
                // Syntax wording belongs to the JavaScript engine. Rust provides
                // parser-specific detail while preserving prefix and recovery.
                assert_eq!(errors.len(), case["result"]["errors"].as_array().unwrap().len(), "{id}");
                assert!(errors.iter().all(|e| e.starts_with("Failed to parse command: ")));
            }
            "input-error" => {
                assert_eq!(recipe["kind"], "stream-error", "{id} unknown input-error recipe");
                let stream = FaultReader {
                    first: Some(recipe["priorLine"].as_str().unwrap().as_bytes().to_vec()),
                    message: recipe["message"].as_str().unwrap().to_owned(),
                };
                let mut reader = ara_rpc::input::RpcInputReader::new(stream);
                let mut frames = Vec::new();
                let mut errors = Vec::new();
                let thrown = loop {
                    match reader.read_next().await {
                        Ok(Some(ara_rpc::input::InputItem::Frame(frame))) => frames.push(frame.stringify()),
                        Ok(Some(ara_rpc::input::InputItem::ParseError(error))) => errors.push(error),
                        Ok(None) => panic!("{id}: stream failure became EOF"),
                        Err(error) => break json!({"name":"Error","message":error.to_string()}),
                    }
                };
                assert_eq!(json!({"frames":frames,"errors":errors,"thrown":thrown}), case["result"], "{id}");
            }
            other => panic!("unknown oracle kind {other}"),
        }
    }
}
