//! Module corpus from OMP `packages/ai/test/tool-call-loop-guard.test.ts` at
//! 596f2da7101178214aa27a753529d15e6b7ad91d. Upstream portions are MIT;
//! Copyright (c) 2025 Mario Zechner; Copyright (c) 2025-2026 Can Bölük;
//! Copyright (c) 2026 Stencil Labs, Inc. See `THIRD_PARTY_NOTICES.md`.

use ara_ai::tool_call_loop_guard::{RepeatedToolCallDetection, ToolCallLoopGuard, ToolCallLoopGuardOptions};
use ara_ai::{AssistantBlock, AssistantMessage, ImageContent, StopReason, ToolCall, ToolResultMessage, UserBlock};
use ara_rpc::{WireString, WireValue};
use serde_json::{Value, json};

fn call(id: &str, name: &str, arguments: Value) -> ToolCall {
    ToolCall {
        id: id.into(),
        name: name.into(),
        arguments: arguments.as_object().unwrap().clone(),
        thought_signature: None,
    }
}

fn message(calls: Vec<ToolCall>) -> AssistantMessage {
    let mut message = AssistantMessage::empty("openai-responses", "openai", "test-model");
    message.stop_reason = StopReason::ToolUse;
    message.content = calls.into_iter().map(AssistantBlock::ToolCall).collect();
    message
}

fn result(id: &str, tool: &str, text: &str) -> ToolResultMessage {
    ToolResultMessage {
        tool_call_id: id.into(),
        tool_name: tool.into(),
        content: vec![UserBlock::text(text)],
        details: None,
        is_error: false,
        timestamp: 0,
    }
}

fn guard(threshold: f64, exempt_tools: &[&str]) -> ToolCallLoopGuard {
    ToolCallLoopGuard::new(ToolCallLoopGuardOptions {
        threshold,
        exempt_tools: exempt_tools.iter().map(|name| (*name).into()).collect(),
    })
}

fn batch() -> Vec<ToolCall> {
    vec![call("bash", "bash", json!({"command":"echo a"})), call("read", "read", json!({"path":"a.ts"}))]
}

fn expected(name: &str, count: f64, result: &str, arguments: &str) -> RepeatedToolCallDetection {
    RepeatedToolCallDetection {
        kind: "repeated_tool_call",
        tool_name: name.into(),
        count,
        result_summary: result.into(),
        arguments_summary: arguments.into(),
    }
}

#[test]
fn fixed_tool_call_loop_module_corpus() {
    // Fifth identical call, ignoring IDs and matching the reported call's result.
    let mut repeated = guard(5.0, &["job", "irc"]);
    for index in 0..6 {
        let id = format!("call-{index}");
        let detection = repeated.record_turn(
            &message(vec![call(&id, "bash", json!({"command":"pytest -q", "timeout":120}))]),
            &[result(&id, "bash", "1263 passed, 4 skipped")],
        );
        assert_eq!(
            detection,
            (index == 4).then(|| expected(
                "bash",
                5.0,
                "1263 passed, 4 skipped",
                r#"{"command":"pytest -q","timeout":120}"#
            )),
            "fifth threshold fires once: turn {index}"
        );
    }

    // Native intent field, stable key order, and legacy intent at nested levels.
    let mut canonical = guard(2.0, &[]);
    assert!(
        canonical
            .record_turn(&message(vec![call("first", "read", json!({"path":"a.ts", "i":"first"}))]), &[])
            .is_none()
    );
    assert_eq!(
        canonical.record_turn(&message(vec![call("second", "read", json!({"i":"second", "path":"a.ts"}))]), &[]),
        Some(expected("read", 2.0, "", r#"{"path":"a.ts"}"#))
    );

    // A different call resets the consecutive run rather than accumulating it.
    let mut different = guard(3.0, &[]);
    for (id, name, arguments) in [
        ("first", "bash", json!({"command":"pytest -q"})),
        ("second", "read", json!({"path":"src/index.ts"})),
        ("third", "bash", json!({"command":"pytest -q"})),
    ] {
        assert!(different.record_turn(&message(vec![call(id, name, arguments)]), &[]).is_none());
    }

    // Exempt-only polling resets state; it does not consume a threshold.
    let mut polling = guard(2.0, &["job"]);
    for id in ["first", "second"] {
        assert!(polling.record_turn(&message(vec![call(id, "job", json!({"poll":["abc"]}))]), &[]).is_none());
    }

    let mut batches = guard(3.0, &[]);
    assert!(batches.record_turn(&message(batch()), &[]).is_none());
    assert!(batches.record_turn(&message(batch()), &[]).is_none());
    assert_eq!(batches.record_turn(&message(batch()), &[]), Some(expected("bash", 3.0, "", r#"{"command":"echo a"}"#)));

    // Source no-call and all-exempt reset scenarios use the same initial batch.
    for (exempt, interruption) in [
        (vec![], message(vec![])),
        (
            vec!["read"],
            message(vec![call("x", "read", json!({"path":"x.ts"})), call("y", "read", json!({"path":"y.ts"}))]),
        ),
    ] {
        let mut reset = guard(2.0, &exempt);
        assert!(reset.record_turn(&message(batch()), &[]).is_none());
        assert!(reset.record_turn(&interruption, &[]).is_none());
        assert!(reset.record_turn(&message(batch()), &[]).is_none());
    }

    // Mixed batches count both calls and report the first non-exempt member.
    let mut mixed = guard(2.0, &["read"]);
    let mut mixed_calls = batch();
    mixed_calls.reverse();
    assert!(mixed.record_turn(&message(mixed_calls.clone()), &[]).is_none());
    assert_eq!(
        mixed.record_turn(
            &message(mixed_calls),
            &[result("read", "read", "file contents"), result("bash", "bash", "command output")],
        ),
        Some(expected("bash", 2.0, "command output", r#"{"command":"echo a"}"#))
    );

    let mut reordered = guard(2.0, &[]);
    assert!(reordered.record_turn(&message(batch()), &[]).is_none());
    let mut reversed = batch();
    reversed.reverse();
    assert_eq!(reordered.record_turn(&message(reversed), &[]), Some(expected("read", 2.0, "", r#"{"path":"a.ts"}"#)));

    let mut alternating = guard(2.0, &[]);
    for calls in [
        batch(),
        vec![call("b", "bash", json!({"command":"echo b"}))],
        batch(),
        vec![call("b", "bash", json!({"command":"echo b"}))],
    ] {
        assert!(alternating.record_turn(&message(calls), &[]).is_none());
    }

    // A changed exempt member changes the mixed batch, while results do not.
    let mut mixed_identity = guard(2.0, &["read"]);
    assert!(mixed_identity.record_turn(&message(batch()), &[result("bash", "bash", "old output")]).is_none());
    let changed = || vec![call("r", "read", json!({"path":"b.ts"})), call("b", "bash", json!({"command":"echo a"}))];
    assert!(mixed_identity.record_turn(&message(changed()), &[]).is_none());
    assert_eq!(
        mixed_identity.record_turn(&message(changed()), &[result("b", "bash", "new output")]),
        Some(expected("bash", 2.0, "new output", r#"{"command":"echo a"}"#))
    );

    // One native-formatting corpus: JSON numbers/keys, recursive intent removal,
    // whitespace, first-result selection, non-text blocks, and surrogate slices.
    let args = json!({
        "10":"ten", "2":"two", "01":"not an index", "4294967295":"not an index",
        "\u{e000}":"bmp", "\u{10000}":"astral", "__proto__":{"discarded":true},
        "nested":[{"__intent":"legacy", "i":"native", "z":1.0,"a":true}],
        "numbers":[-0.0,1e-7,1e20,1e21,9007199254740993_u64], "i":"outer", "__intent":"outer legacy"
    });
    let mut native = guard(2.0, &[]);
    assert!(native.record_turn(&message(vec![call("first", "read", args.clone())]), &[]).is_none());
    let mut native_result = result("second", "ignored-name", "\u{feff} first \t second\u{0085}third \u{2003}");
    native_result
        .content
        .push(UserBlock::Image(ImageContent { data: "not text".into(), mime_type: "image/png".into() }));
    native_result.content.push(UserBlock::text(" fourth \n fifth \u{feff}"));
    let detection = native
        .record_turn(
            &message(vec![call("second", "read", args)]),
            &[
                result("different-id", "read", "wrong result"),
                native_result,
                result("second", "read", "later duplicate"),
            ],
        )
        .unwrap();
    assert_eq!(detection.result_summary.to_utf8().unwrap(), "first second\u{0085}third fourth fifth");
    assert_eq!(
        detection.arguments_summary.to_utf8().unwrap(),
        "{\"2\":\"two\",\"10\":\"ten\",\"01\":\"not an index\",\"4294967295\":\"not an index\",\"nested\":[{\"a\":true,\"z\":1}],\"numbers\":[0,1e-7,100000000000000000000,1e+21,9007199254740992],\"\u{10000}\":\"astral\",\"\u{e000}\":\"bmp\"}"
    );
    let emitted = detection.to_wire_value();
    assert_eq!(emitted.get("count").unwrap().as_number(), Some(2.0));
    assert_eq!(emitted.get("resultSummary").unwrap().as_string(), Some(&detection.result_summary));

    // JS's slice preserves a high surrogate at either summary boundary.
    let mut truncated = guard(1.0, &[]);
    let detection = truncated
        .record_turn(
            &message(vec![call("long", "read", json!({"x":format!("{}🌊z", "a".repeat(393))}))]),
            &[result("long", "read", &format!("{}🌊z", "a".repeat(199)))],
        )
        .unwrap();
    let mut result_units = vec![u16::from(b'a'); 199];
    result_units.extend([0xd83c, 0x2026]);
    assert_eq!(detection.result_summary, WireString::from_units(result_units));
    let mut argument_units: Vec<u16> = "{\"x\":\"".encode_utf16().collect();
    argument_units.extend(vec![u16::from(b'a'); 393]);
    argument_units.extend([0xd83c, 0x2026]);
    assert_eq!(detection.arguments_summary, WireString::from_units(argument_units));
    assert!(detection.result_summary.to_utf8().is_err());
    assert!(detection.arguments_summary.to_utf8().is_err());
    let encoded = detection.to_wire_value().stringify();
    assert_eq!(encoded.matches("\\ud83c…").count(), 2);
    assert!(encoded.contains("\"count\":1,"));
    let parsed = WireValue::parse(&encoded).unwrap();
    assert_eq!(parsed.get("resultSummary").unwrap().as_string(), Some(&detection.result_summary));

    // Constructor math: truncation/clamp, NaN never fires, +Infinity never fires.
    for (threshold, hit) in [(2.9, 2), (0.0, 1), (-3.0, 1), (f64::NEG_INFINITY, 1), (f64::NAN, 0), (f64::INFINITY, 0)] {
        let mut configured = guard(threshold, &[]);
        for index in 1..=3 {
            assert_eq!(
                configured.record_turn(&message(batch()), &[]).is_some(),
                index == hit,
                "threshold={threshold}, turn={index}"
            );
        }
    }
}
