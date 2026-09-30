use ara_rpc::frame::{MAX_RPC_FRAME_BYTES, MAX_RPC_REASSEMBLED_BYTES};
use ara_rpc::{RpcFrameDecoder, RpcFrameEncoder, WireValue, encode_rpc_frame};

fn s(value: &str) -> WireValue {
    WireValue::String(value.into())
}
fn frame(ty: &str, entries: Vec<(&str, WireValue)>) -> WireValue {
    let mut result = WireValue::object(vec![("type", s(ty))]);
    for (key, value) in entries {
        result.insert(key, value);
    }
    result
}
fn messages() -> Vec<WireValue> {
    (0..20)
        .map(|i| {
            WireValue::object(vec![("role", s("assistant")), ("content", s(&format!("{i}-{}", "x".repeat(65536))))])
        })
        .collect()
}
fn read(line: &str) -> WireValue {
    WireValue::parse(line).unwrap()
}

#[test]
fn source_terminal_prefix_continuation_and_reset() {
    for version in [1, 2] {
        let mut encoder = RpcFrameEncoder::new();
        encoder.set_protocol_version(version).unwrap();
        let messages = messages();
        encoder.encode(&frame("agent_start", vec![]));
        for message in &messages {
            encoder.encode(&frame("message_end", vec![("message", message.clone())]));
        }
        let fitting = frame(
            "agent_end",
            vec![("messages", WireValue::Array(vec![messages[0].clone()])), ("willContinue", WireValue::Bool(true))],
        );
        assert_eq!(encoder.encode(&fitting), fitting.stringify() + "\n", "fitting terminal histories remain complete");
        let continuing = frame(
            "agent_end",
            vec![("messages", WireValue::Array(messages.clone())), ("willContinue", WireValue::Bool(true))],
        );
        let result = read(&encoder.encode(&continuing));
        assert_eq!(result.get("messages").unwrap().as_array().unwrap().len(), 0);
        assert_eq!(result.get("messageCount").unwrap().as_number(), Some(20.0));
        let terminal = frame("agent_end", vec![("messages", WireValue::Array(messages.clone()))]);
        assert_eq!(read(&encoder.encode(&terminal)).get("messages").unwrap().as_array().unwrap().len(), 0);
        let mut decoder = RpcFrameDecoder::new();
        let mut replay = None;
        for line in encoder.encode_frames(&terminal) {
            replay = decoder.push(read(&line)).unwrap();
        }
        assert!(
            !replay.unwrap().get("messages").unwrap().as_array().unwrap().is_empty(),
            "final agent_end reset snapshots"
        );
        for message in &messages {
            encoder.encode(&frame("message_end", vec![("message", message.clone())]));
        }
        encoder.encode(&frame("agent_start", vec![]));
        let mut after = None;
        for line in encoder.encode_frames(&terminal) {
            after = decoder.push(read(&line)).unwrap();
        }
        assert!(!after.unwrap().get("messages").unwrap().as_array().unwrap().is_empty(), "agent_start reset snapshots");
    }
}

#[test]
fn source_snapshot_is_eager_and_v1_keeps_sent_shrunk_shape() {
    let large = WireValue::object(vec![("text", s(&"😀".repeat(300000)))]);
    for version in [1, 2] {
        let mut encoder = RpcFrameEncoder::new();
        encoder.set_protocol_version(version).unwrap();
        let encoded = encoder.encode_frames(&frame("message_end", vec![("message", large.clone())]));
        // Request terminal before consuming iterator: source snapshot bookkeeping
        // already completed; iterator consumption does not alter it.
        let terminal =
            encoder.encode_frames(&frame("agent_end", vec![("messages", WireValue::Array(vec![large.clone()]))]));
        let mut decoder = RpcFrameDecoder::new();
        for line in encoded {
            decoder.push(read(&line)).unwrap();
        }
        let mut result = None;
        for line in terminal {
            result = decoder.push(read(&line)).unwrap();
        }
        let result = result.unwrap();
        let count = result.get("messages").unwrap().as_array().unwrap().len();
        assert_eq!(count, if version == 1 { 1 } else { 0 });
    }
}

#[test]
fn all_overflow_families_and_metadata_caps() {
    let huge_key = "k".repeat(MAX_RPC_FRAME_BYTES);
    let generic = frame("tool_execution_end", vec![(&huge_key, WireValue::Null)]);
    let output = read(&encode_rpc_frame(&generic, 0, None));
    assert_eq!(output.get("type"), Some(&s("rpc_frame_error")));
    assert_eq!(output.get("originalType"), Some(&s("tool_execution_end")));
    let terminal =
        frame("agent_end", vec![("messages", WireValue::Array(vec![s("ok")])), (&huge_key, WireValue::Null)]);
    let output = read(&encode_rpc_frame(&terminal, 0, None));
    assert_eq!(output.get("messageCount").unwrap().as_number(), Some(1.0));
    assert!(output.get("messages").unwrap().as_array().unwrap().is_empty());
    let response =
        frame("response", vec![("id", s(&"😀".repeat(MAX_RPC_FRAME_BYTES / 4))), ("command", WireValue::Null)]);
    let encoded = encode_rpc_frame(&response, 0, None);
    assert!(encoded.len() <= MAX_RPC_FRAME_BYTES);
    let output = read(&encoded);
    assert_eq!(output.get("success"), Some(&WireValue::Bool(false)));
    assert_eq!(output.get("command"), Some(&s("unknown")));
    assert!(output.get("id").unwrap().as_string().unwrap().len() < 1024);
    let mut encoder = RpcFrameEncoder::new();
    encoder.set_protocol_version(2).unwrap();
    let terminal =
        frame("agent_end", vec![("messages", WireValue::Array(vec![s(&"x".repeat(MAX_RPC_REASSEMBLED_BYTES))]))]);
    assert_eq!(read(&encoder.encode(&terminal)).get("messageCount").unwrap().as_number(), Some(1.0));
}

#[test]
fn chunk_counter_advances_on_overflow_and_protocol_switch_preserves_snapshot() {
    let mut encoder = RpcFrameEncoder::new();
    encoder.set_protocol_version(2).unwrap();
    let over = frame("response", vec![("data", s(&"x".repeat(MAX_RPC_REASSEMBLED_BYTES)))]);
    let ignored = encoder.encode_frames(&over);
    drop(ignored);
    let large = frame("response", vec![("data", s(&"x".repeat(MAX_RPC_FRAME_BYTES)))]);
    let first = read(&encoder.encode_frames(&large).next().unwrap());
    assert_eq!(first.get("chunkId"), Some(&s("rpc-2")));
    assert!(encoder.set_protocol_version(0).is_err());
    encoder.set_protocol_version(1).unwrap();
    assert_eq!(read(&encoder.encode(&large)).get("success"), Some(&WireValue::Bool(false)));
}
