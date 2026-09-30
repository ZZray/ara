//! Fixed OMP rpc-messages.test.ts plus wire/cursor boundary oracles.
use ara_rpc::frame::MAX_RPC_FRAME_BYTES;
use ara_rpc::messages::{DEFAULT_RPC_MESSAGE_PAGE_LIMIT, MAX_RPC_MESSAGE_PAGE_BYTES};
use ara_rpc::{
    RpcMessageSnapshot, RpcMessagesPageError, RpcMessagesPageOptions, WireString, WireValue, encode_rpc_frame,
    page_rpc_messages,
};
use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD};

fn string(value: &str) -> WireValue {
    WireValue::String(value.into())
}

fn message(index: usize, bytes: usize) -> WireValue {
    WireValue::object(vec![
        ("role", string("user")),
        ("content", string(&format!("{index}:{}", "x".repeat(bytes)))),
        ("timestamp", WireValue::Number(index as f64)),
    ])
}

fn snapshot(count: usize) -> RpcMessageSnapshot {
    RpcMessageSnapshot { session_id: "session-1".into(), leaf_id: Some("leaf-1".into()), message_count: count }
}

fn cursor_payload(count: f64, offset: f64) -> WireValue {
    WireValue::object(vec![
        ("version", WireValue::Number(1.0)),
        ("sessionId", string("session-1")),
        ("leafId", string("leaf-1")),
        ("messageCount", WireValue::Number(count)),
        ("offset", WireValue::Number(offset)),
    ])
}

fn cursor(payload: &WireValue) -> WireValue {
    string(&URL_SAFE_NO_PAD.encode(payload.stringify()))
}

fn invalid_cursor(value: &WireValue) -> RpcMessagesPageError {
    let error =
        page_rpc_messages(&[], &snapshot(0), RpcMessagesPageOptions { cursor: Some(value), limit: None }).unwrap_err();
    assert_eq!(error.message, "Invalid RPC message cursor");
    assert_eq!(error.code, None);
    error
}

#[test]
fn fixed_large_history_reconstructs_without_loss_or_overlap_in_v1_safe_pages() {
    let messages: Vec<_> = (0..40).map(|index| message(index, 32 * 1024)).collect();
    let snapshot = snapshot(messages.len());
    let limit = WireValue::Number(256.0);
    let mut next_cursor = None;
    let mut reconstructed = Vec::new();
    let mut pages = 0;
    loop {
        let page = page_rpc_messages(
            &messages,
            &snapshot,
            RpcMessagesPageOptions { cursor: next_cursor.as_ref(), limit: Some(&limit) },
        )
        .unwrap();
        let frame = WireValue::object(vec![
            ("id", string(&format!("page-{pages}"))),
            ("type", string("response")),
            ("command", string("get_messages_page")),
            ("success", WireValue::Bool(true)),
            ("data", page.clone().into_wire()),
        ]);
        let encoded = encode_rpc_frame(&frame, 0, None);
        assert!(encoded.len() <= MAX_RPC_FRAME_BYTES);
        assert_eq!(WireValue::parse(encoded.trim()).unwrap(), frame, "v1 framing did not truncate a page");
        reconstructed.extend(page.messages);
        next_cursor = page.next_cursor.map(WireValue::String);
        pages += 1;
        if next_cursor.is_none() {
            break;
        }
    }
    assert!(pages > 1);
    assert_eq!(reconstructed, messages);
}

#[test]
fn fixed_snapshot_changes_return_exact_stale_message_and_code() {
    let messages: Vec<_> = (0..40).map(|index| message(index, 1024)).collect();
    let original = snapshot(messages.len());
    let limit = WireValue::Number(5.0);
    let first =
        page_rpc_messages(&messages, &original, RpcMessagesPageOptions { cursor: None, limit: Some(&limit) }).unwrap();
    let cursor = WireValue::String(first.next_cursor.unwrap());
    for changed in [
        RpcMessageSnapshot { session_id: "session-2".into(), ..original.clone() },
        RpcMessageSnapshot { leaf_id: Some("leaf-2".into()), ..original.clone() },
        RpcMessageSnapshot { leaf_id: None, ..original.clone() },
        snapshot(39),
    ] {
        let history = &messages[..changed.message_count];
        let error =
            page_rpc_messages(history, &changed, RpcMessagesPageOptions { cursor: Some(&cursor), limit: Some(&limit) })
                .unwrap_err();
        assert_eq!(error.message, "RPC message cursor is stale");
        assert_eq!(error.code, Some("stale_cursor"));
    }
}

#[test]
fn fixed_first_individually_oversized_message_is_returned_losslessly() {
    let messages = vec![message(0, 2 * 1024 * 1024), message(1, 128)];
    let snapshot = snapshot(messages.len());
    let first = page_rpc_messages(&messages, &snapshot, RpcMessagesPageOptions::default()).unwrap();
    assert_eq!(first.messages, messages[..1]);
    let cursor = WireValue::String(first.next_cursor.unwrap());
    let second =
        page_rpc_messages(&messages, &snapshot, RpcMessagesPageOptions { cursor: Some(&cursor), limit: None }).unwrap();
    assert_eq!(second.messages, messages[1..]);
    assert!(second.next_cursor.is_none());
}

#[test]
fn null_limit_defaults_and_wire_types_safe_integer_bounds_and_error_order_are_preserved() {
    let messages: Vec<_> = (0..257).map(|index| message(index, 1)).collect();
    let snapshot = snapshot(messages.len());
    let null = WireValue::Null;
    let default = page_rpc_messages(&messages, &snapshot, RpcMessagesPageOptions::default()).unwrap();
    let nullable =
        page_rpc_messages(&messages, &snapshot, RpcMessagesPageOptions { cursor: None, limit: Some(&null) }).unwrap();
    assert_eq!(default, nullable);
    assert_eq!(default.messages.len(), DEFAULT_RPC_MESSAGE_PAGE_LIMIT);
    for raw in ["0", "-1", "1.5", "257", "9007199254740992", "\"1\"", "true", "{}", "[]", "1e999"] {
        let value = WireValue::parse(raw).unwrap();
        let error =
            page_rpc_messages(&messages, &snapshot, RpcMessagesPageOptions { cursor: None, limit: Some(&value) })
                .unwrap_err();
        assert_eq!(error.message, "RPC message page limit must be between 1 and 256");
        assert_eq!(error.code, None);
    }
    for value in [WireValue::Number(f64::NAN), WireValue::Number(f64::NEG_INFINITY)] {
        assert_eq!(
            page_rpc_messages(&messages, &snapshot, RpcMessagesPageOptions { cursor: None, limit: Some(&value) })
                .unwrap_err()
                .code,
            None
        );
    }
    for (limit, count) in [(1.0, 1), (256.0, 256)] {
        let limit = WireValue::Number(limit);
        assert_eq!(
            page_rpc_messages(&messages, &snapshot, RpcMessagesPageOptions { cursor: None, limit: Some(&limit) })
                .unwrap()
                .messages
                .len(),
            count
        );
    }
    invalid_cursor(&null);
    let bad_limit = WireValue::Number(0.0);
    let mismatch = RpcMessageSnapshot { message_count: 0, ..snapshot };
    assert_eq!(
        page_rpc_messages(
            &messages,
            &mismatch,
            RpcMessagesPageOptions { cursor: Some(&null), limit: Some(&bad_limit) }
        )
        .unwrap_err()
        .message,
        "RPC message snapshot does not match current messages"
    );
}

#[test]
fn cursor_rejects_noncanonical_encoding_invalid_utf8_bad_json_types_and_payload_fields() {
    for raw in ["", "=", "AA=", "AA+", "AA/", "A", " a", "é", "🦀"] {
        invalid_cursor(&string(raw));
    }
    invalid_cursor(&string(&"a".repeat(2049)));
    for value in [WireValue::Null, WireValue::Bool(true), WireValue::Number(1.0), WireValue::Array(vec![])] {
        invalid_cursor(&value);
    }
    for bytes in
        [vec![0xff], b"{bad".to_vec(), b"null".to_vec(), b"[]".to_vec(), b"{\"sessionId\":\"\xed\xa0\x80\"}".to_vec()]
    {
        invalid_cursor(&string(&URL_SAFE_NO_PAD.encode(bytes)));
    }
    let mut raw = cursor_payload(0.0, 0.0).stringify();
    while raw.len().is_multiple_of(3) {
        raw.push(' ');
    }
    let mut encoded = URL_SAFE_NO_PAD.encode(raw).into_bytes();
    let alphabet = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789-_";
    let last = encoded.last_mut().unwrap();
    let index = alphabet.iter().position(|byte| *byte == *last).unwrap();
    *last = alphabet[index + 1];
    invalid_cursor(&string(std::str::from_utf8(&encoded).unwrap()));

    for (field, invalid) in [
        ("version", WireValue::Number(2.0)),
        ("version", WireValue::Bool(true)),
        ("sessionId", string("")),
        ("sessionId", WireValue::Null),
        ("sessionId", string(&"x".repeat(257))),
        ("leafId", string("")),
        ("leafId", WireValue::Bool(false)),
        ("leafId", string(&"🦀".repeat(129))),
        ("messageCount", WireValue::Number(-1.0)),
        ("messageCount", string("0")),
        ("messageCount", WireValue::Number(0.5)),
        ("messageCount", WireValue::Number(9_007_199_254_740_992.0)),
        ("offset", WireValue::Number(-1.0)),
        ("offset", WireValue::Number(0.5)),
        ("offset", WireValue::Number(1.0)),
        ("offset", WireValue::Bool(false)),
    ] {
        let mut payload = cursor_payload(0.0, 0.0);
        payload.insert(field, invalid);
        invalid_cursor(&cursor(&payload));
    }
    let missing = WireValue::object(vec![
        ("version", WireValue::Number(1.0)),
        ("sessionId", string("session-1")),
        ("messageCount", WireValue::Number(0.0)),
        ("offset", WireValue::Number(0.0)),
    ]);
    invalid_cursor(&cursor(&missing));
}

#[test]
fn cursor_preserves_utf16_ids_accepts_bom_extra_fields_safe_max_and_end_offsets() {
    let surrogate = WireValue::parse(r#""\ud800""#).unwrap().as_string().unwrap().clone();
    let leaf: WireString = "🦀".repeat(128).into();
    let snapshot_info =
        RpcMessageSnapshot { session_id: surrogate.clone(), leaf_id: Some(leaf.clone()), message_count: 0 };
    let mut payload = cursor_payload(0.0, -0.0);
    payload.insert("sessionId", WireValue::String(surrogate));
    payload.insert("leafId", WireValue::String(leaf));
    payload.insert("ignored", WireValue::Bool(true));
    let with_bom = string(&URL_SAFE_NO_PAD.encode(format!("\u{feff}{}", payload.stringify())));
    let page = page_rpc_messages(&[], &snapshot_info, RpcMessagesPageOptions { cursor: Some(&with_bom), limit: None })
        .unwrap();
    assert_eq!(page.into_wire(), WireValue::parse(r#"{"messages":[],"totalMessages":0}"#).unwrap());
    let safe_max = cursor(&cursor_payload(9_007_199_254_740_991.0, 9_007_199_254_740_991.0));
    assert_eq!(
        page_rpc_messages(&[], &snapshot(0), RpcMessagesPageOptions { cursor: Some(&safe_max), limit: None })
            .unwrap_err()
            .code,
        Some("stale_cursor")
    );
    let none = RpcMessageSnapshot { leaf_id: None, ..snapshot(0) };
    let mut end = cursor_payload(0.0, 0.0);
    end.insert("leafId", WireValue::Null);
    let cursor = cursor(&end);
    assert!(
        page_rpc_messages(&[], &none, RpcMessagesPageOptions { cursor: Some(&cursor), limit: None })
            .unwrap()
            .next_cursor
            .is_none()
    );
}

#[test]
fn page_byte_budget_counts_json_utf8_array_brackets_commas_and_allows_exact_limit() {
    let first = string(&"x".repeat(MAX_RPC_MESSAGE_PAGE_BYTES - 7));
    let second = string("");
    // [first,second]: 2 brackets + (budget - 5) first JSON bytes + comma + 2 second JSON bytes.
    let exact = vec![first.clone(), second.clone()];
    let page = page_rpc_messages(&exact, &snapshot(2), RpcMessagesPageOptions::default()).unwrap();
    assert_eq!(WireValue::Array(page.messages.clone()).stringify().len(), MAX_RPC_MESSAGE_PAGE_BYTES);
    assert_eq!(page.messages, exact);
    assert!(page.next_cursor.is_none());
    let over = vec![first, string("x")];
    let page = page_rpc_messages(&over, &snapshot(2), RpcMessagesPageOptions::default()).unwrap();
    assert_eq!(page.messages.len(), 1);
    assert!(page.next_cursor.is_some());
    let unicode = vec![string(&"🦀".repeat(100_000)), string(&"🦀".repeat(100_000))];
    let page = page_rpc_messages(&unicode, &snapshot(2), RpcMessagesPageOptions::default()).unwrap();
    assert_eq!(page.messages.len(), 1, "UTF-8 byte length, not UTF-16 or scalar count, defines the page budget");
}
