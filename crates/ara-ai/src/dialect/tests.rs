//! Coarse original-module adaptations, not new integration-test targets.
//! Native source: fixed 596f2da packages/utils/test/json-parse.test.ts and
//! packages/ai/test/inband-tools.test.ts and dialect-thinking.test.ts.
//! Thinking scanner projections adapt leaked-thinking-stream.test.ts fixtures;
//! strip-unit inputs are from google-thinking-fence-strip.test.ts. These do
//! not replace the actual wrapper/provider consumer observations.
//! The marked additional source cases
//! are author assertions from the exact parser/scanner, awaiting Root oracle.

use std::convert::Infallible;
use std::sync::{Arc, Mutex};

use super::{
    DialectRenderOptions, DialectRenderer, HarmonyInbandScanner, HarmonyInbandUnitsScanner, HarmonyRenderer,
    InbandScanEvent, InbandScanner, InbandScannerUnits, ToolArgumentJson,
};
use crate::relaxed_json::{
    FinalParseError, FinalValueFactory, parse_json_with_repair, parse_json_with_repair_units, parse_relaxed_final,
    parse_relaxed_final_units,
};
use ara_rpc::{WireString, WireValue};

/// Old grammar-only bindings cannot evaluate actual graph dialect operations.
/// Keep their existing eight families and traces intact. The new coarse family
/// below uses GraphValueFactory/ValueHeap for all actual value operations.
macro_rules! unavailable_graph_dialect_ops {
    ($value:ty, $error:ty) => {
        fn get(&self, _: &$value, _: &WireString) -> Result<$value, $error> {
            unreachable!("grammar-only fixture has no actual Get")
        }
        fn string_value(&self, _: &$value) -> Result<Option<WireString>, $error> {
            unreachable!("grammar-only fixture has no actual string view")
        }
        fn is_number_zero(&self, _: &$value) -> Result<bool, $error> {
            unreachable!("grammar-only fixture has no actual strict-zero view")
        }
        fn define_own_data(&self, _: &mut $value, _: WireString, _: $value) -> Result<(), $error> {
            unreachable!("grammar-only fixture has no actual literal wrapper")
        }
        fn stringify_json(
            &self,
            _: &$value,
            _: ara_rpc::value_heap::StringifyMode,
        ) -> Result<Option<WireString>, $error> {
            unreachable!("grammar-only fixture has no actual stringify")
        }
        fn implicit_string(&self, _: &$value) -> Result<WireString, $error> {
            unreachable!("grammar-only fixture has no actual implicit conversion")
        }
    };
}

fn unit_parts(parts: &[&[u16]]) -> WireString {
    WireString::from_units(parts.iter().flat_map(|part| part.iter().copied()).collect())
}
fn source_units(text: &str) -> Vec<u16> {
    text.encode_utf16().collect()
}

/// Controlled grammar fixture only. It records ordinary assignment calls and
/// uses flat WireValue for ordinary keys. It does NOT implement prototype
/// setters/inheritance or constitute Root's production V2 value bridge.
#[derive(Default)]
struct GrammarRecorder {
    assignments: Mutex<Vec<(WireString, String)>>,
}

impl FinalValueFactory for GrammarRecorder {
    type Value = WireValue;
    type Error = Infallible;
    unavailable_graph_dialect_ops!(WireValue, Infallible);
    fn is_null(&self, value: &WireValue) -> Result<bool, Infallible> {
        Ok(matches!(value, WireValue::Null))
    }
    fn import_strict(&self, value: WireValue) -> Result<WireValue, Infallible> {
        Ok(value)
    }
    fn null(&self) -> Result<WireValue, Infallible> {
        Ok(WireValue::Null)
    }
    fn boolean(&self, value: bool) -> Result<WireValue, Infallible> {
        Ok(WireValue::Bool(value))
    }
    fn number(&self, value: f64) -> Result<WireValue, Infallible> {
        Ok(WireValue::Number(value))
    }
    fn string(&self, value: WireString) -> Result<WireValue, Infallible> {
        Ok(WireValue::String(value))
    }
    fn array(&self) -> Result<WireValue, Infallible> {
        Ok(WireValue::Array(Vec::new()))
    }
    fn append(&self, array: &mut WireValue, value: WireValue) -> Result<(), Infallible> {
        let WireValue::Array(values) = array else {
            panic!("parser array constructor contract");
        };
        values.push(value);
        Ok(())
    }
    fn object(&self) -> Result<WireValue, Infallible> {
        Ok(WireValue::Object(Vec::new()))
    }
    fn assign(&self, object: &mut WireValue, key: WireString, value: WireValue) -> Result<(), Infallible> {
        self.assignments.lock().unwrap().push((key.clone(), value.stringify()));
        let WireValue::Object(entries) = object else {
            panic!("parser object constructor contract");
        };
        if let Some((_, previous)) = entries.iter_mut().find(|(candidate, _)| candidate == &key) {
            *previous = value;
        } else {
            entries.push((key, value));
        }
        Ok(())
    }
    fn is_record(&self, value: &WireValue) -> Result<bool, Infallible> {
        Ok(matches!(value, WireValue::Object(_)))
    }
    fn ordinary_prototype_lookup_truthy(&self, key: &WireString) -> Result<bool, Infallible> {
        Ok([
            "constructor",
            "__defineGetter__",
            "__defineSetter__",
            "hasOwnProperty",
            "__lookupGetter__",
            "__lookupSetter__",
            "isPrototypeOf",
            "propertyIsEnumerable",
            "toString",
            "valueOf",
            "__proto__",
            "toLocaleString",
        ]
        .iter()
        .any(|name| key.equals_ascii(name)))
    }
}

/// Controlled failure binding only, confined to this cfg(test) module. Handles
/// alias for observing pre-throw fixture mutations; nested WireValue snapshots
/// do not model Native prototype/accessor/opaque behavior or a production engine.
#[derive(Debug, Clone)]
struct FaultValue(Arc<Mutex<WireValue>>);
impl FaultValue {
    fn new(value: WireValue) -> Self {
        Self(Arc::new(Mutex::new(value)))
    }
    fn snapshot(&self) -> WireValue {
        self.0.lock().unwrap().clone()
    }
}

#[derive(Default)]
struct FaultRecorder {
    failures: Vec<(&'static str, usize, &'static str)>,
    uncaught_errors: Vec<&'static str>,
    classifications: Mutex<Vec<(&'static str, bool)>>,
    calls: Mutex<Vec<String>>,
    assignments: Mutex<Vec<(WireString, String)>>,
    objects: Mutex<Vec<FaultValue>>,
    arrays: Mutex<Vec<FaultValue>>,
}
impl FaultRecorder {
    fn throwing(failures: &[(&'static str, usize, &'static str)]) -> Self {
        Self { failures: failures.to_vec(), ..Self::default() }
    }
    fn step(&self, operation: &str) -> Result<(), &'static str> {
        let occurrence = {
            let mut calls = self.calls.lock().unwrap();
            calls.push(operation.into());
            calls.iter().filter(|call| call.as_str() == operation).count()
        };
        for &(name, nth, error) in &self.failures {
            if name == operation && nth == occurrence {
                return Err(error);
            }
        }
        Ok(())
    }
    fn trace(&self) -> Vec<String> {
        self.calls.lock().unwrap().clone()
    }
    fn write_own(&self, object: &mut FaultValue, key: WireString, value: FaultValue) -> Result<(), &'static str> {
        let value = value.snapshot();
        let mut target = object.0.lock().unwrap();
        let WireValue::Object(entries) = &mut *target else {
            return Err("controlled object fixture contract");
        };
        if let Some((_, previous)) = entries.iter_mut().find(|(candidate, _)| candidate == &key) {
            *previous = value;
        } else {
            entries.push((key, value));
        }
        Ok(())
    }
}
impl FinalValueFactory for FaultRecorder {
    type Value = FaultValue;
    type Error = &'static str;
    unavailable_graph_dialect_ops!(FaultValue, &'static str);
    fn is_null(&self, value: &FaultValue) -> Result<bool, Self::Error> {
        self.step("is_null")?;
        Ok(matches!(value.snapshot(), WireValue::Null))
    }
    fn should_catch_runtime_error(&self, error: &Self::Error) -> bool {
        let catch = !self.uncaught_errors.contains(error);
        self.classifications.lock().unwrap().push((*error, catch));
        catch
    }
    fn import_strict(&self, value: WireValue) -> Result<FaultValue, Self::Error> {
        self.step("import_strict")?;
        Ok(FaultValue::new(value))
    }
    fn null(&self) -> Result<FaultValue, Self::Error> {
        self.step("null")?;
        Ok(FaultValue::new(WireValue::Null))
    }
    fn boolean(&self, value: bool) -> Result<FaultValue, Self::Error> {
        self.step("boolean")?;
        Ok(FaultValue::new(WireValue::Bool(value)))
    }
    fn number(&self, value: f64) -> Result<FaultValue, Self::Error> {
        self.step("number")?;
        Ok(FaultValue::new(WireValue::Number(value)))
    }
    fn string(&self, value: WireString) -> Result<FaultValue, Self::Error> {
        self.step(&format!("string:{}", value.to_utf8().unwrap()))?;
        Ok(FaultValue::new(WireValue::String(value)))
    }
    fn array(&self) -> Result<FaultValue, Self::Error> {
        self.step("array")?;
        let value = FaultValue::new(WireValue::Array(Vec::new()));
        self.arrays.lock().unwrap().push(value.clone());
        Ok(value)
    }
    fn append(&self, array: &mut FaultValue, value: FaultValue) -> Result<(), Self::Error> {
        self.step("append")?;
        let value = value.snapshot();
        let mut target = array.0.lock().unwrap();
        let WireValue::Array(values) = &mut *target else {
            return Err("controlled array fixture contract");
        };
        values.push(value);
        Ok(())
    }
    fn object(&self) -> Result<FaultValue, Self::Error> {
        self.step("object")?;
        let value = FaultValue::new(WireValue::Object(Vec::new()));
        self.objects.lock().unwrap().push(value.clone());
        Ok(value)
    }
    fn assign(&self, object: &mut FaultValue, key: WireString, value: FaultValue) -> Result<(), Self::Error> {
        self.step(&format!("assign:{}", key.to_utf8().unwrap()))?;
        self.assignments.lock().unwrap().push((key.clone(), value.snapshot().stringify()));
        self.write_own(object, key, value)
    }
    fn is_record(&self, value: &FaultValue) -> Result<bool, Self::Error> {
        self.step("is_record")?;
        Ok(matches!(value.snapshot(), WireValue::Object(_)))
    }
    fn ordinary_prototype_lookup_truthy(&self, key: &WireString) -> Result<bool, Self::Error> {
        self.step(&format!("lookup:{}", key.to_utf8().unwrap()))?;
        match GrammarRecorder::default().ordinary_prototype_lookup_truthy(key) {
            Ok(value) => Ok(value),
            Err(error) => match error {},
        }
    }
}

fn assert_wire(input: &str, expected: &str, factory: &GrammarRecorder) {
    let actual = parse_json_with_repair(input, factory).unwrap_or_else(|error| panic!("{input:?}: {error}"));
    let expected = WireValue::parse(expected).unwrap();
    assert!(actual.deep_equal(&expected), "{input:?}: {} != {}", actual.stringify(), expected.stringify());
}

#[test]
fn final_json_native_module_corpus_and_strict_boundary() {
    let factory = GrammarRecorder::default();
    // Selected original parseJsonWithRepair observations, grouped from its
    // module. repairJson/string-prefix/streaming observations are not this
    // final-parser scope; adaptations/additional probes are marked below.
    for (input, expected) in [
        (
            r#"{"text":"quote: \" unicode: \u2028 slash: \/ newline: \n"}"#,
            r#"{"text":"quote: \" unicode: \u2028 slash: / newline: \n"}"#,
        ),
        ("{'path': 'a.ts'}", r#"{"path":"a.ts"}"#),
        (r#"{path: "a.ts", count: 2}"#, r#"{"path":"a.ts","count":2}"#),
        (r#"{"a":1,}"#, r#"{"a":1}"#),
        ("[1, 2, ]", "[1,2]"),
        (r#"{"ok": True, "no": False, "nil": None}"#, r#"{"ok":true,"no":false,"nil":null}"#),
        ("{'msg': 'it's fine'}", r#"{"msg":"it's fine"}"#),
        ("{\"a\":1 /* c */, \"b\":2 // trailing\n}", r#"{"a":1,"b":2}"#),
        (
            r#"{"paths": packages/coding-agent/src/stt/*, "i": "Listing stt module files"}"#,
            r#"{"paths":"packages/coding-agent/src/stt/*","i":"Listing stt module files"}"#,
        ),
        (r#"{"paths": [src/a/*, src/b/* ], "n": 3}"#, r#"{"paths":["src/a/*","src/b/*"],"n":3}"#),
        (r#"{"i": Listing stt files   , "b": true}"#, r#"{"i":"Listing stt files","b":true}"#),
        (r#"{"url": https://example.com/x?y=1}"#, r#"{"url":"https://example.com/x?y=1"}"#),
        (r#"{"p": C:\Users\x}"#, r#"{"p":"C:\\Users\\x"}"#),
        (r#"{"msg": it's fine, "b": 1}"#, r#"{"msg":"it's fine","b":1}"#),
        (r#"{"value":"a\qb"}"#, r#"{"value":"a\\qb"}"#),
        ("{\"text\":\"a\nb\u{0001}c\"}", r#"{"text":"a\nb\u0001c"}"#),
    ] {
        assert_wire(input, expected, &factory);
    }
    for input in [
        r#"{"a":"x" "b":1}"#,
        r#"{"a": NaN}"#,
        r#"{"a": Infinity}"#,
        r#"{"a":1} then prose"#,
        r#"{"a": packages/foo"#,
        r#"{"a": foo "b": 1}"#,
        "{a: foo b: 1}",
        r#"{"a": foo {"b": 1}}"#,
        r#"{"a": foo [1]}"#,
        r#"{"addr": localhost:8080}"#,
        r#"{"a": undefined}"#,
    ] {
        assert!(parse_json_with_repair(input, &factory).is_err(), "{input:?}");
    }

    // Additional exact-source representation probes. No Native runtime oracle
    // has been run for these; tests prove the carrier boundary only when run.
    let strict = parse_json_with_repair(r#"{"big":1e400,"lone":"\ud800","__proto__":{"p":1}}"#, &factory).unwrap();
    assert_eq!(strict.get("big").and_then(WireValue::as_number), Some(f64::INFINITY));
    assert_eq!(strict.get("lone").and_then(WireValue::as_string).unwrap().units(), [0xd800]);
    assert!(strict.get("__proto__").is_some());
    assert!(parse_json_with_repair("{big:1e400}", &factory).is_err());
    assert_wire(
        "{hex:0x10, binary:0b101, decimal:+.5, tail:1.}",
        r#"{"hex":16,"binary":5,"decimal":0.5,"tail":1}"#,
        &factory,
    );
    for input in ["{n:+0x10}", "{n:-0b11}", "{n:0o10}", "{n:1e}", "{a:constructor}", "{a:toString}"] {
        assert!(parse_json_with_repair(input, &factory).is_err(), "{input:?}");
    }

    // Batch4 direct-source additions, not original Native fixtures. Raw lone
    // source units must use strict own-data import first, including overflow.
    let raw_factory = GrammarRecorder::default();
    let raw_strict = unit_parts(&[
        &source_units("{\"lone\":\""),
        &[0xd800, 0xd83c, 0xdf3f, 0xdc00],
        &source_units("\",\"__proto__\":{\"p\":1},\"big\":1e400}"),
    ]);
    let strict = parse_json_with_repair_units(&raw_strict, &raw_factory).unwrap();
    assert_eq!(strict.get("lone").and_then(WireValue::as_string).unwrap().units(), [0xd800, 0xd83c, 0xdf3f, 0xdc00]);
    assert_eq!(strict.get("big").and_then(WireValue::as_number), Some(f64::INFINITY));
    assert!(strict.get("__proto__").is_some());
    assert!(
        raw_factory.assignments.lock().unwrap().is_empty(),
        "strict source must not use relaxed ordinary assignment"
    );
    let raw_relaxed = unit_parts(&[&source_units("{raw:'"), &[0xdfff, 0xd83c, 0xdf3f], &source_units("'}")]);
    let relaxed = parse_relaxed_final_units(&raw_relaxed, &raw_factory).unwrap();
    assert_eq!(relaxed.get("raw").and_then(WireValue::as_string).unwrap().units(), [0xdfff, 0xd83c, 0xdf3f]);
    let position_source = unit_parts(&[&source_units("{\"raw\":\""), &[0xd800, 0xd83c, 0xdf3f], &source_units("\"}")]);
    let trailing = unit_parts(&[position_source.units(), &source_units(" tail")]);
    assert_eq!(
        parse_relaxed_final_units(&trailing, &raw_factory).unwrap_err().syntax().unwrap().position,
        position_source.len() + 1
    );

    // Fallible binding additions in the original coarse parser family. These
    // controlled faults exercise the seam; actual runtime V2 remains separate.
    let import = FaultRecorder::throwing(&[("import_strict", 1, "strict import throw")]);
    let error = parse_json_with_repair_units(&raw_strict, &import).unwrap_err();
    assert!(matches!(error, FinalParseError::Runtime("strict import throw")));
    assert_eq!(import.trace(), ["import_strict"], "runtime import must never enter relaxed grammar");

    for (source, operation) in [
        ("null", "null"),
        ("true", "boolean"),
        ("7", "number"),
        ("'value'", "string:value"),
        ("{}", "object"),
        ("[true,]", "array"),
    ] {
        let factory = FaultRecorder::throwing(&[(operation, 1, "constructor throw")]);
        assert!(matches!(parse_relaxed_final(source, &factory), Err(FinalParseError::Runtime("constructor throw"))));
        assert_eq!(factory.trace(), [operation], "constructor must run at its source position: {source}");
    }
    let array = FaultRecorder::throwing(&[("append", 2, "append throw")]);
    assert!(matches!(parse_relaxed_final("[1,2,3,]", &array), Err(FinalParseError::Runtime("append throw"))));
    assert_eq!(array.trace(), ["array", "number", "append", "number", "append"]);
    let retained = array.arrays.lock().unwrap()[0].clone();
    assert_eq!(retained.snapshot().stringify(), "[1]", "the first push must remain after the second throws");
}

#[test]
fn relaxed_assignment_delegates_all_proto_writes_without_own_flattening() {
    let factory = GrammarRecorder::default();
    let _ = parse_relaxed_final("{__proto__:null,__proto__:{p:1},a:2}", &factory).unwrap();
    let writes = factory.assignments.lock().unwrap();
    assert_eq!(
        writes.iter().map(|(key, _)| key.to_utf8().unwrap()).collect::<Vec<_>>(),
        ["__proto__", "p", "__proto__", "a"]
    );
    assert_eq!(writes[0].1, "null");
    assert_eq!(writes[2].1, "{\"p\":1}");
    // Deliberately do not assert prototype result/own keys with GrammarRecorder.
    // Actual repeated-setter/null-prototype cases require the unique V2 bridge.

    for (source, operation, trace) in [
        (
            "{first:1,second:2,third:3}",
            "assign:second",
            vec!["object", "number", "assign:first", "number", "assign:second"],
        ),
        (
            "{first:1,second:ordinary,third:3}",
            "lookup:ordinary",
            vec!["object", "number", "assign:first", "lookup:ordinary"],
        ),
    ] {
        let fault = FaultRecorder::throwing(&[(operation, 1, "ordinary operation throw")]);
        assert!(matches!(
            parse_relaxed_final(source, &fault),
            Err(FinalParseError::Runtime("ordinary operation throw"))
        ));
        assert_eq!(fault.trace(), trace);
        let retained = fault.objects.lock().unwrap()[0].clone();
        assert_eq!(retained.snapshot().stringify(), r#"{"first":1}"#);
        assert_eq!(fault.assignments.lock().unwrap().len(), 1);
    }
}

fn make_scanner(factory: Arc<GrammarRecorder>) -> HarmonyInbandScanner<GrammarRecorder> {
    HarmonyInbandScanner::with_id_source(factory, Arc::new(|| "ptc_fixture_1".into()))
}

#[test]
fn harmony_native_call_rawblock_and_every_split_boundary() {
    // Exact original raw read/src/a.ts case, fed as whole text and by char.
    let original =
        "<|start|>assistant<|channel|>commentary to=functions.read<|message|>{\"path\":\"src/a.ts\"}<|call|>";
    for chunks in [vec![original.to_owned()], original.chars().map(|character| character.to_string()).collect()] {
        let mut scanner = make_scanner(Arc::new(GrammarRecorder::default()));
        let mut events = Vec::new();
        for chunk in chunks {
            events.extend(scanner.feed(&chunk));
        }
        events.extend(scanner.flush());
        let end = events
            .iter()
            .find_map(|event| match event {
                InbandScanEvent::ToolEnd { name, arguments, raw_block, .. } => Some((name, arguments, raw_block)),
                _ => None,
            })
            .unwrap();
        assert_eq!(end.0, "read");
        assert_eq!(end.1.get("path").and_then(WireValue::as_string).unwrap().to_utf8().unwrap(), "src/a.ts");
        assert_eq!(end.2.as_deref(), Some(original));
    }
    // Adaptation: write/content payload and every two-chunk split. Uses raw characters,
    // JSON escaping only, and exact rawBlock including the terminal control.
    let raw = "<|start|>assistant<|channel|>commentary to=functions.write<|message|>{\"path\":\"a.ts\",\"content\":\"a & b\\n\"}<|call|>";
    for split in 0..=raw.len() {
        if !raw.is_char_boundary(split) {
            continue;
        }
        let mut scanner = make_scanner(Arc::new(GrammarRecorder::default()));
        let mut events = scanner.feed(&raw[..split]);
        events.extend(scanner.feed(&raw[split..]));
        events.extend(scanner.flush());
        assert_eq!(events.len(), 2, "split={split}");
        assert!(matches!(&events[0], InbandScanEvent::ToolStart { name, .. } if name == "write"));
        let InbandScanEvent::ToolEnd { name, arguments, raw_block, .. } = &events[1] else {
            panic!("split={split}");
        };
        assert_eq!(name, "write");
        assert_eq!(arguments.get("path").and_then(WireValue::as_string).unwrap().to_utf8().unwrap(), "a.ts");
        assert_eq!(arguments.get("content").and_then(WireValue::as_string).unwrap().to_utf8().unwrap(), "a & b\n");
        assert_eq!(raw_block.as_deref(), Some(raw));
    }

    // Batch4 full-source domain adaptation: whole and by-code-unit input,
    // including splits within a surrogate pair. This is not wrapper evidence.
    let name = [0x72, 0xd800, 0x65, 0xdc00];
    let payload = [0xd800, 0xd83c, 0xdf3f, 0xdc00];
    let raw = unit_parts(&[
        &source_units("<|start|>assistant<|channel|>commentary to=functions."),
        &name,
        &source_units("<|message|>{\"raw\":\""),
        &payload,
        &source_units("\"}<|call|>"),
    ]);
    for by_unit in [false, true] {
        let mut scanner = HarmonyInbandUnitsScanner::with_id_source(
            Arc::new(GrammarRecorder::default()),
            Arc::new(|| WireString::from("ptc_units")),
        );
        let mut events = Vec::new();
        if by_unit {
            for &unit in raw.units() {
                events.extend(scanner.feed_units(&WireString::from_units(vec![unit])));
            }
        } else {
            events.extend(scanner.feed_units(&raw));
        }
        events.extend(scanner.flush_units());
        assert_eq!(events.len(), 2);
        assert!(
            matches!(&events[0], InbandScanEvent::ToolStart { id, name: actual } if id.equals_ascii("ptc_units") && actual.units() == name)
        );
        let InbandScanEvent::ToolEnd { name: actual, arguments, raw_block, .. } = &events[1] else {
            panic!("units toolEnd");
        };
        assert_eq!(actual.units(), name);
        assert_eq!(arguments.get("raw").and_then(WireValue::as_string).unwrap().units(), payload);
        assert_eq!(raw_block.as_ref(), Some(&raw));
    }
}

#[test]
fn harmony_fixed_source_header_precedence_control_and_flush_contract() {
    for body in ["", "null", "[1]", "7", "{broken"] {
        let mut scanner = make_scanner(Arc::new(GrammarRecorder::default()));
        let raw = format!("<|start|>assistant<|channel|>analysis to=functions.read<|message|>{body}<|call|>");
        let events = scanner.feed(&raw);
        assert!(matches!(&events[0], InbandScanEvent::ToolStart { name, .. } if name == "read"));
        let InbandScanEvent::ToolEnd { arguments, .. } = &events[1] else {
            panic!("missing tool end");
        };
        assert_eq!(arguments.stringify(), "{}");
    }
    let mut scanner = make_scanner(Arc::new(GrammarRecorder::default()));
    assert!(scanner.feed("<|start|>user<|message|>not assistant<|end|>").is_empty());
    let events = scanner.feed("<|channel|>analysis<|message|>思考<|end|>");
    assert!(matches!(events.first(), Some(InbandScanEvent::ThinkingStart)));
    assert!(matches!(events.last(), Some(InbandScanEvent::ThinkingEnd { thinking }) if thinking == "思考"));
    let raw = "<|start|>assistant to=functions.read<|message|>{\"x\":\"a<|constrain|>b<|message|>c\"}<|return|>";
    let events = scanner.feed(raw);
    let InbandScanEvent::ToolEnd { arguments, raw_block, .. } = events.last().unwrap() else {
        panic!("missing tool end");
    };
    assert_eq!(raw_block.as_deref(), Some(raw));
    assert_eq!(arguments.get("x").and_then(WireValue::as_string).unwrap().to_utf8().unwrap(), "abc");

    // Harmony unterminated cases are NOT in Native dialect-thinking.test.ts.
    // These assertions are direct-source additions awaiting Root Native oracle.
    let events = scanner.feed("<|start|>assistant<|channel|>analysis<|message|>plain");
    assert!(matches!(events.last(), Some(InbandScanEvent::ThinkingDelta { delta }) if delta == "plain"));
    assert!(scanner.flush().is_empty(), "already-empty buffer must not invent an end");
    let mut scanner = make_scanner(Arc::new(GrammarRecorder::default()));
    scanner.feed("<|start|>assistant<|channel|>analysis<|message|>plain<|");
    let events = scanner.flush();
    assert!(matches!(events.last(), Some(InbandScanEvent::ThinkingEnd { thinking }) if thinking == "plain<|"));
    let mut scanner = make_scanner(Arc::new(GrammarRecorder::default()));
    scanner.feed("<|start|>assistant to=functions.read");
    assert!(scanner.flush().is_empty());
    assert_eq!(scanner.feed("after header"), vec![InbandScanEvent::Text { text: "after header".into() }]);

    // Native parse+asRecord share one catch. The second asRecord object failure
    // below is caught; the later fallback object failure is propagated.
    for (body, failures, trace) in [
        ("{}", vec![("import_strict", 1, "strict import throw")], vec!["import_strict", "object"]),
        ("{}", vec![("is_record", 1, "record lookup throw")], vec!["import_strict", "is_record", "object"]),
        ("null", vec![("object", 1, "asRecord object throw")], vec!["import_strict", "is_record", "object", "object"]),
        (
            "{first:1,second:2}",
            vec![("assign:second", 1, "assignment throw")],
            vec!["object", "number", "assign:first", "number", "assign:second", "object"],
        ),
    ] {
        let factory = Arc::new(FaultRecorder::throwing(&failures));
        let mut scanner =
            HarmonyInbandUnitsScanner::with_id_source(factory.clone(), Arc::new(|| WireString::from("ptc_fault")));
        let raw = format!("<|start|>assistant to=functions.read<|message|>{body}<|call|>");
        let events = scanner.try_feed_units(&WireString::from(raw)).unwrap();
        assert_eq!(events.len(), 2);
        let InbandScanEvent::ToolEnd { arguments, .. } = &events[1] else {
            panic!("fallible toolEnd");
        };
        assert_eq!(arguments.snapshot().stringify(), "{}");
        let objects = factory.objects.lock().unwrap();
        assert!(Arc::ptr_eq(&arguments.0, &objects.last().unwrap().0), "move the fallback handle unchanged");
        if body.starts_with("{first") {
            assert_eq!(objects[0].snapshot().stringify(), r#"{"first":1}"#);
        }
        assert_eq!(factory.trace(), trace);
        assert!(scanner.try_flush_units().unwrap().is_empty());
    }

    for (body, failures, trace, expected) in [
        ("", vec![("object", 1, "empty object throw")], vec!["object"], "empty object throw"),
        (
            "{}",
            vec![("import_strict", 1, "strict import throw"), ("object", 1, "fallback throw")],
            vec!["import_strict", "object"],
            "fallback throw",
        ),
        ("{broken", vec![("object", 2, "syntax fallback throw")], vec!["object", "object"], "syntax fallback throw"),
        (
            "null",
            vec![("object", 1, "asRecord object throw"), ("object", 2, "fallback throw")],
            vec!["import_strict", "is_record", "object", "object"],
            "fallback throw",
        ),
    ] {
        let factory = Arc::new(FaultRecorder::throwing(&failures));
        let mut scanner =
            HarmonyInbandUnitsScanner::with_id_source(factory.clone(), Arc::new(|| WireString::from("ptc_fault")));
        let raw = format!("<|start|>assistant to=functions.read<|message|>{body}<|call|>");
        assert!(matches!(scanner.try_feed_units(&WireString::from(raw)), Err(error) if error == expected));
        assert_eq!(factory.trace(), trace, "a failure outside the catch must not allocate another object");
    }

    let factory = Arc::new(FaultRecorder::throwing(&[("object", 1, "flush fallback throw")]));
    let mut scanner =
        HarmonyInbandUnitsScanner::with_id_source(factory.clone(), Arc::new(|| WireString::from("ptc_flush_fault")));
    let start = scanner.try_feed_units(&WireString::from("<|start|>assistant to=functions.read<|message|><|")).unwrap();
    assert!(matches!(&start[..], [InbandScanEvent::ToolStart { .. }]));
    assert!(matches!(scanner.try_flush_units(), Err("flush fallback throw")));
    assert_eq!(factory.trace(), ["object"]);

    // END/CALL drained the token before finishBody threw. No clearBody/reset is
    // invented on Err, and locally collected feed events were not delivered.
    let factory = Arc::new(FaultRecorder::throwing(&[("object", 1, "first empty throw")]));
    let mut scanner =
        HarmonyInbandUnitsScanner::with_id_source(factory.clone(), Arc::new(|| WireString::from("ptc_state_fault")));
    let raw = "<|start|>assistant to=functions.read<|message|><|call|>tail";
    assert!(matches!(scanner.try_feed_units(&WireString::from(format!("prefix{raw}"))), Err("first empty throw")));
    assert!(scanner.try_feed_units(&WireString::from_units(Vec::new())).unwrap().is_empty());
    let events = scanner.try_flush_units().unwrap();
    assert!(matches!(&events[..], [InbandScanEvent::ToolEnd { id, raw_block: Some(block), .. }]
        if id.equals_ascii("ptc_state_fault") && block.equals_ascii(raw)));
    assert_eq!(factory.trace(), ["object", "object"]);

    // Binding classification is additional to Native source parity: unsupported
    // Host/carrier errors must not become a successful empty argument object.
    // These labels are controlled fault classes, not actual ValueError EXEC.
    for native in ["Thrown", "TypeError", "RangeError", "DataCloneError"] {
        let factory = Arc::new(FaultRecorder::throwing(&[("is_record", 1, native)]));
        let mut scanner = HarmonyInbandUnitsScanner::new(factory.clone());
        let events = scanner
            .try_feed_units(&WireString::from("<|start|>assistant to=functions.read<|message|>{}<|call|>"))
            .unwrap();
        let InbandScanEvent::ToolEnd { arguments, .. } = events.last().unwrap() else {
            panic!("caught Native class");
        };
        assert_eq!(arguments.snapshot().stringify(), "{}");
        assert_eq!(factory.trace(), ["import_strict", "is_record", "object"]);
        assert_eq!(*factory.classifications.lock().unwrap(), [(native, true)]);
    }
    for boundary in [
        "ForeignHeap",
        "InvalidNode",
        "HeapTokensExhausted",
        "NodeIdsExhausted",
        "PoisonedHeap",
        "SourceIdentityCollision",
        "Allocation",
        "MissingCapability",
        "ExpiredCapability",
        "WrongRuntimeReply",
        "UnsupportedOperation",
        "LegacyBridge",
        "StrictParse",
    ] {
        for (body, operation, trace) in [
            ("{}", "import_strict", vec!["import_strict"]),
            ("{}", "is_record", vec!["import_strict", "is_record"]),
            ("null", "object", vec!["import_strict", "is_record", "object"]),
        ] {
            let mut fault = FaultRecorder::throwing(&[(operation, 1, boundary)]);
            fault.uncaught_errors = vec![boundary];
            let factory = Arc::new(fault);
            let mut scanner = HarmonyInbandUnitsScanner::new(factory.clone());
            let raw = format!("<|start|>assistant to=functions.read<|message|>{body}<|call|>");
            assert!(matches!(scanner.try_feed_units(&WireString::from(raw)), Err(error) if error == boundary));
            assert_eq!(factory.trace(), trace, "{boundary} at {operation}: must not allocate fallback");
            assert_eq!(*factory.classifications.lock().unwrap(), [(boundary, false)]);
        }
    }

    for (body, operation, boundary, trace) in [
        (
            "{first:1,second:2}",
            "assign:second",
            "ForeignHeap",
            vec!["object", "number", "assign:first", "number", "assign:second"],
        ),
        (
            "{first:1,second:ordinary}",
            "lookup:ordinary",
            "MissingCapability",
            vec!["object", "number", "assign:first", "lookup:ordinary"],
        ),
    ] {
        let mut fault = FaultRecorder::throwing(&[(operation, 1, boundary)]);
        fault.uncaught_errors = vec![boundary];
        let factory = Arc::new(fault);
        let mut scanner = HarmonyInbandUnitsScanner::new(factory.clone());
        let raw = format!("<|start|>assistant to=functions.read<|message|>{body}<|call|>");
        assert!(matches!(scanner.try_feed_units(&WireString::from(raw)), Err(error) if error == boundary));
        assert_eq!(factory.trace(), trace);
        assert_eq!(factory.objects.lock().unwrap()[0].snapshot().stringify(), r#"{"first":1}"#);
        assert_eq!(*factory.classifications.lock().unwrap(), [(boundary, false)]);
    }

    // Syntax still reaches the fixed fallback without runtime classification.
    let factory = Arc::new(FaultRecorder::default());
    let mut scanner = HarmonyInbandUnitsScanner::new(factory.clone());
    assert!(
        scanner
            .try_feed_units(&WireString::from("<|start|>assistant to=functions.read<|message|>{broken<|call|>"))
            .is_ok()
    );
    assert_eq!(factory.trace(), ["object", "object"]);
    assert!(factory.classifications.lock().unwrap().is_empty());

    // Neither empty arguments nor a failed fallback allocate/classify again,
    // whether that uncaught error is Native or an unsupported boundary.
    for error in ["TypeError", "MissingCapability"] {
        let mut empty = FaultRecorder::throwing(&[("object", 1, error)]);
        empty.uncaught_errors = vec!["MissingCapability"];
        let factory = Arc::new(empty);
        let mut scanner = HarmonyInbandUnitsScanner::new(factory.clone());
        assert!(matches!(scanner.try_feed_units(&WireString::from(
            "<|start|>assistant to=functions.read<|message|>   <|call|>"
        )), Err(actual) if actual == error));
        assert_eq!(factory.trace(), ["object"]);
        assert!(factory.classifications.lock().unwrap().is_empty());

        let mut fallback = FaultRecorder::throwing(&[("import_strict", 1, "Thrown"), ("object", 1, error)]);
        fallback.uncaught_errors = vec!["MissingCapability"];
        let factory = Arc::new(fallback);
        let mut scanner = HarmonyInbandUnitsScanner::new(factory.clone());
        assert!(matches!(scanner.try_feed_units(&WireString::from(
            "<|start|>assistant to=functions.read<|message|>{}<|call|>"
        )), Err(actual) if actual == error));
        assert_eq!(factory.trace(), ["import_strict", "object"]);
        assert_eq!(*factory.classifications.lock().unwrap(), [("Thrown", true)]);
    }
    let mut fault = FaultRecorder::throwing(&[("object", 1, "TypeError"), ("object", 2, "Allocation")]);
    fault.uncaught_errors = vec!["Allocation"];
    let factory = Arc::new(fault);
    let mut scanner = HarmonyInbandUnitsScanner::new(factory.clone());
    assert!(matches!(
        scanner.try_feed_units(&WireString::from("<|start|>assistant to=functions.read<|message|>null<|call|>")),
        Err("Allocation")
    ));
    assert_eq!(factory.trace(), ["import_strict", "is_record", "object", "object"]);
    assert_eq!(*factory.classifications.lock().unwrap(), [("TypeError", true)]);
}

/// Explicit controlled serializer receipt. The production actual-carrier
/// serializer is absent; this fixture only proves consumer wiring and errors.
struct ArgumentReceipt;
impl ToolArgumentJson for ArgumentReceipt {
    type Error = &'static str;
    fn stringify_tool_arguments(&self, call: &crate::ToolCall) -> Result<Option<String>, Self::Error> {
        match call.id.as_str() {
            "cycle" => Err("Native stringify cycle receipt"),
            "undefined" => Ok(None),
            _ => Ok(Some(r#"{"path":"a.ts","content":"a & b\n"}"#.into())),
        }
    }
}

fn fixture_image(mime: &str) -> crate::ImageContent {
    crate::ImageContent { data: "fixture-bytes".into(), mime_type: mime.into(), detail: None, compaction_frame: false }
}

#[test]
fn harmony_native_rendering_thinking_and_history_vs_transcript_family() {
    use crate::{
        AssistantBlock, AssistantMessage, DeveloperMessage, Message, ThinkingContent, ToolCall, ToolResultMessage,
        UserBlock, UserContent,
    };
    let renderer = HarmonyRenderer { arguments: ArgumentReceipt };
    let options = DialectRenderOptions::default();
    let call = ToolCall { id: "c1".into(), name: "write".into(), ..ToolCall::default() };
    let rendered_call = renderer.render_tool_call(&call, options).unwrap();
    assert_eq!(
        rendered_call,
        "<|start|>assistant<|channel|>commentary to=functions.write<|message|>{\"path\":\"a.ts\",\"content\":\"a & b\\n\"}<|call|>"
    );

    // Adapted from original every-dialect thinking round-trip family.
    let rendered_thinking = renderer.render_thinking("private chain of thought");
    let mut scanner = make_scanner(Arc::new(GrammarRecorder::default()));
    let mut events = scanner.feed(&rendered_thinking);
    events.extend(scanner.flush());
    assert!(events.iter().any(
        |event| matches!(event, InbandScanEvent::ThinkingEnd { thinking } if thinking == "private chain of thought")
    ));
    assert!(
        !events
            .iter()
            .any(|event| matches!(event, InbandScanEvent::Text { text } if text.contains("private chain of thought")))
    );

    let mut assistant = AssistantMessage::empty("receipt-api", "receipt-provider", "receipt-model");
    assistant.timestamp = 17;
    assistant.response_id = Some("keep-receipt".into());
    assistant.content = vec![
        AssistantBlock::text("left "),
        AssistantBlock::Thinking(ThinkingContent {
            thinking: "reason".into(),
            thinking_signature: Some("keep-only-without-calls".into()),
        }),
        AssistantBlock::Image(fixture_image("image/png")),
        AssistantBlock::text("right\n \t"),
        AssistantBlock::ToolCall(call),
    ];
    let first_result = ToolResultMessage {
        tool_call_id: "c1".into(),
        tool_name: "write".into(),
        timestamp: 23,
        content: vec![UserBlock::text("FILE"), UserBlock::Image(fixture_image("image/png")), UserBlock::text(" A")],
        ..ToolResultMessage::default()
    };
    let second_result = ToolResultMessage {
        tool_call_id: "c2".into(),
        tool_name: "functions.read".into(),
        timestamp: 29,
        content: vec![UserBlock::Image(fixture_image("image/jpeg")), UserBlock::text("FILE B")],
        ..ToolResultMessage::default()
    };
    let history = vec![
        Message::Developer(DeveloperMessage {
            content: UserContent::Text("system receipt".into()),
            provider_payload: None,
            provider_payload_origin: None,
            timestamp: 11,
        }),
        Message::Assistant(assistant.clone()),
        Message::ToolResult(first_result),
        Message::ToolResult(second_result),
    ];
    let encoded = super::history::encode_inband_tool_history(&history, &renderer, options).unwrap();
    assert_eq!(encoded.len(), 3);
    assert_eq!(encoded[0], history[0]);
    let Message::Assistant(encoded_assistant) = &encoded[1] else {
        panic!("assistant role");
    };
    assert_eq!(encoded_assistant.response_id, assistant.response_id);
    assert_eq!(encoded_assistant.timestamp, 17);
    assert_eq!(encoded_assistant.content, vec![AssistantBlock::text(format!("left \nright\n{rendered_call}"))]);
    let Message::User(encoded_result) = &encoded[2] else {
        panic!("encoded user role");
    };
    assert_eq!(encoded_result.timestamp, 23);
    let UserContent::Blocks(blocks) = &encoded_result.content else {
        panic!("retained result image blocks");
    };
    assert_eq!(blocks.len(), 3);
    assert!(
        matches!(&blocks[0], UserBlock::Text(block) if block.text == "<|start|>functions.write to=assistant<|channel|>commentary<|message|>FILE A<|end|><|start|>functions.read to=assistant<|channel|>commentary<|message|>FILE B<|end|>")
    );
    assert!(matches!(&blocks[1], UserBlock::Image(block) if block.mime_type == "image/png"));
    assert!(matches!(&blocks[2], UserBlock::Image(block) if block.mime_type == "image/jpeg"));

    let transcript = renderer.render_transcript(&history, options).unwrap();
    assert!(transcript.starts_with("<|start|>developer<|message|>system receipt<|end|>"));
    assert!(transcript.contains("<|channel|>analysis<|message|>reason<|end|>"));
    assert!(transcript.contains("<|channel|>final<|message|>left right\n \t<|end|>"));
    assert!(transcript.contains("FILE[Image: image/png] A"));
    assert!(transcript.contains("[Image: image/jpeg]FILE B"));
    let empty = AssistantMessage::empty("api", "provider", "model");
    assert_eq!(
        renderer.render_transcript(&[Message::Assistant(empty)], options).unwrap(),
        "<|start|>assistant<|channel|>final<|message|><|end|>"
    );

    let mut no_calls = assistant.clone();
    no_calls.content.retain(|block| !matches!(block, AssistantBlock::ToolCall(_)));
    let unchanged =
        super::history::encode_inband_tool_history(&[Message::Assistant(no_calls.clone())], &renderer, options)
            .unwrap();
    assert_eq!(unchanged, vec![Message::Assistant(no_calls)]);
    let error_call = ToolCall { id: "cycle".into(), name: "read".into(), ..ToolCall::default() };
    let mut error_message = assistant;
    error_message.content = vec![AssistantBlock::ToolCall(error_call)];
    assert_eq!(
        renderer.render_transcript(&[Message::Assistant(error_message.clone())], options),
        Err("Native stringify cycle receipt")
    );
    assert_eq!(
        super::history::encode_inband_tool_history(&[Message::Assistant(error_message)], &renderer, options),
        Err("Native stringify cycle receipt")
    );
    let undefined = ToolCall { id: "undefined".into(), name: "read".into(), ..ToolCall::default() };
    assert!(renderer.render_tool_call(&undefined, options).unwrap().ends_with("<|message|>null<|call|>"));
}

impl super::catalog::ToolCatalogBridge for FaultRecorder {
    fn tool_wire_schema(&self, tool: &crate::Tool) -> Result<FaultValue, Self::Error> {
        self.step("tool_wire_schema")?;
        if tool.name == "schema-error" {
            return Err("Native schema bridge receipt");
        }
        // Controlled resolved schema, deliberately distinct from parameters.
        WireValue::parse(r#"{"type":"object","properties":{"actual":{"type":"string"}}}"#)
            .map(FaultValue::new)
            .map_err(|_| "fixture schema")
    }
    fn define_own_data(&self, object: &mut FaultValue, key: WireString, value: FaultValue) -> Result<(), Self::Error> {
        self.step(&format!("define:{}", key.to_utf8().unwrap()))?;
        self.write_own(object, key, value)
    }
    fn stringify_plain(&self, value: &FaultValue) -> Result<Option<String>, Self::Error> {
        self.step("stringify_plain")?;
        Ok(Some(value.snapshot().stringify()))
    }
}

#[test]
fn catalog_and_prompt_exact_bridge_family() {
    let bridge = FaultRecorder::default();
    let renderer = HarmonyRenderer { arguments: ArgumentReceipt };
    let tool = crate::Tool {
        name: "read".into(),
        description: "read & inspect".into(),
        parameters: serde_json::json!({"wrong": "raw parameters"}),
        ..crate::Tool::default()
    };
    let catalog = super::catalog::render_tool_catalog(std::slice::from_ref(&tool), &bridge).unwrap();
    assert_eq!(
        catalog,
        r#"{"type":"function","function":{"name":"read","description":"read & inspect","parameters":{"type":"object","properties":{"actual":{"type":"string"}}}}}"#
    );
    assert!(
        bridge.assignments.lock().unwrap().is_empty(),
        "catalogue literal definitions must bypass ordinary assignment"
    );
    let prompt = super::catalog::render_inband_tool_prompt(std::slice::from_ref(&tool), &renderer, &bridge).unwrap();
    assert_eq!(
        prompt,
        include_str!("prompt-template.md").replacen("{{TOOLS}}", &catalog, 1).replacen(
            "{{DIALECT}}",
            ara_prompt::js::trim(include_str!("harmony.md")),
            1
        )
    );

    // Direct-source token collision probe awaiting Native oracle: the second
    // first-only replacement can target authored catalogue text.
    let mut collision = tool;
    collision.description = "{{DIALECT}}".into();
    let collided = super::catalog::render_inband_tool_prompt(&[collision], &renderer, &bridge).unwrap();
    assert!(collided.contains("\"description\":\"## Format guide"));
    assert!(collided.ends_with("{{DIALECT}}\n"));
    let bad = crate::Tool { name: "schema-error".into(), ..crate::Tool::default() };
    assert_eq!(super::catalog::render_tool_catalog(&[bad], &bridge), Err("Native schema bridge receipt"));

    let ordered = crate::Tool { name: "read".into(), description: "read & inspect".into(), ..crate::Tool::default() };
    let trace = [
        "object",
        "string:function",
        "define:type",
        "object",
        "string:read",
        "define:name",
        "string:read & inspect",
        "define:description",
        "tool_wire_schema",
        "define:parameters",
        "define:function",
        "stringify_plain",
    ];
    let success = FaultRecorder::default();
    assert_eq!(super::catalog::render_tool_catalog(std::slice::from_ref(&ordered), &success).unwrap(), catalog);
    assert_eq!(success.trace(), trace);
    // One coarse family observes each existing call seam and its early stop.
    for end in 0..trace.len() {
        let operation = trace[end];
        let occurrence = trace[..=end].iter().filter(|&&name| name == operation).count();
        let failed = FaultRecorder::throwing(&[(operation, occurrence, "catalogue throw")]);
        assert_eq!(
            super::catalog::render_tool_catalog(std::slice::from_ref(&ordered), &failed),
            Err("catalogue throw")
        );
        assert_eq!(failed.trace(), trace[..=end]);
        if end >= 3 {
            let objects = failed.objects.lock().unwrap();
            let declaration = objects[0].snapshot();
            assert_eq!(declaration.get("type").and_then(WireValue::as_string).unwrap().to_utf8().unwrap(), "function");
        }
        assert!(failed.assignments.lock().unwrap().is_empty());
    }
}

fn scan_thinking(raw: &str, by_character: bool, implied_open: bool) -> Vec<InbandScanEvent<WireValue>> {
    let mut scanner =
        super::ThinkingInbandScanner::<WireValue>::new(super::ThinkingInbandScannerOptions { implied_open });
    let mut events = Vec::new();
    if by_character {
        for character in raw.chars() {
            events.extend(scanner.feed(&character.to_string()));
        }
    } else {
        events.extend(scanner.feed(raw));
    }
    events.extend(scanner.flush());
    events
}
fn thinking_projection(events: &[InbandScanEvent<WireValue>]) -> (String, String, usize) {
    let mut text = String::new();
    let mut thinking = String::new();
    let mut implied = 0;
    for event in events {
        match event {
            InbandScanEvent::Text { text: delta } => text.push_str(delta),
            InbandScanEvent::ThinkingDelta { delta } => thinking.push_str(delta),
            InbandScanEvent::ImpliedThinkingEnd => implied += 1,
            _ => {}
        }
    }
    (text, thinking, implied)
}

fn scan_thinking_units(raw: &WireString, by_unit: bool) -> Vec<InbandScanEvent<WireValue, WireString>> {
    let mut scanner =
        super::ThinkingInbandUnitsScanner::<WireValue>::new(super::ThinkingInbandScannerOptions { implied_open: true });
    let mut events = Vec::new();
    if by_unit {
        for &unit in raw.units() {
            events.extend(scanner.feed_units(&WireString::from_units(vec![unit])));
        }
    } else {
        events.extend(scanner.feed_units(raw));
    }
    events.extend(scanner.flush_units());
    events
}
fn units_projection(events: &[InbandScanEvent<WireValue, WireString>]) -> (WireString, WireString) {
    let mut text = Vec::new();
    let mut thinking = Vec::new();
    for event in events {
        match event {
            InbandScanEvent::Text { text: delta } => text.extend_from_slice(delta.units()),
            InbandScanEvent::ThinkingDelta { delta } => thinking.extend_from_slice(delta.units()),
            _ => {}
        }
    }
    (WireString::from_units(text), WireString::from_units(thinking))
}

#[test]
fn generic_thinking_native_fixture_projection_and_state_family() {
    // Exact Native leaked wrapper input fixtures; only scanner projections
    // are adapted here. Actual indexed replay/signatures remain Root gates.
    for by_character in [false, true] {
        let events = scan_thinking("Visible before.```thinking\nplan\n```Visible after.", by_character, true);
        assert_eq!(thinking_projection(&events), ("Visible before.Visible after.".into(), "plan\n".into(), 0));
        let raw = "Intro.```thinking\nPlan:\n```rs\nfn main() {}\n```\nThen decide.\n```Visible after";
        let events = scan_thinking(raw, by_character, true);
        assert_eq!(
            thinking_projection(&events),
            ("Intro.Visible after".into(), "Plan:\n```rs\nfn main() {}\n```\nThen decide.\n".into(), 0)
        );
        assert!(events.iter().any(|event| matches!(event, InbandScanEvent::ThinkingEnd { .. })));
        for suffix in ["Visible after", " after", "Done"] {
            let raw = format!("```thinking\nplan\n```{suffix}");
            assert_eq!(
                thinking_projection(&scan_thinking(&raw, by_character, true)),
                (suffix.into(), "plan\n".into(), 0)
            );
        }
        let implied = "Let me think. 2 plus 2.\n</think>\n\nThe answer is 4.";
        assert_eq!(
            thinking_projection(&scan_thinking(implied, by_character, true)),
            ("Let me think. 2 plus 2.\n\n\nThe answer is 4.".into(), String::new(), 1)
        );
        assert_eq!(
            thinking_projection(&scan_thinking(implied, by_character, false)),
            (implied.into(), String::new(), 0)
        );
        let literal = "Qwen ends reasoning with `</think>` on its own line.";
        assert_eq!(
            thinking_projection(&scan_thinking(literal, by_character, true)),
            (literal.into(), String::new(), 0)
        );

        // Additional direct-source shields/flush assertions await Native oracle.
        let shield = "```rs\n<think>literal</think>\n```\n<think>actual</think>🌿";
        assert_eq!(
            thinking_projection(&scan_thinking(shield, by_character, true)),
            ("```rs\n<think>literal</think>\n```\n🌿".into(), "actual".into(), 0)
        );
        let tilde = "```thinking\n~~~rs\nx\n~~~\n```Done";
        assert_eq!(
            thinking_projection(&scan_thinking(tilde, by_character, true)),
            ("Done".into(), "~~~rs\nx\n~~~\n".into(), 0)
        );
        let unclosed = scan_thinking("<think>plain", by_character, true);
        assert_eq!(thinking_projection(&unclosed), (String::new(), "plain".into(), 0));
        assert!(
            !unclosed.iter().any(|event| matches!(event, InbandScanEvent::ThinkingEnd { .. })),
            "Native nonfenced empty buffer must not invent an end"
        );
        let unclosed_fence = scan_thinking("```thinking\nplain", by_character, true);
        assert!(
            unclosed_fence
                .iter()
                .any(|event| matches!(event, InbandScanEvent::ThinkingEnd { thinking } if thinking == "plain")),
            "fenced matcher handles final even with empty outer buffer"
        );
    }

    // Batch4 exact-units additions await Root Native oracle. The same families
    // now admit raw lone units and non-BMP unit splits through one kernel.
    let visible_before = [0xd800, 0xd83c, 0xdf3f];
    let reasoning = [0xdc00, 0xd83c, 0xdf3f, 0xdfff];
    let visible_after = [0xdc01];
    let raw =
        unit_parts(&[&visible_before, &source_units("<think>"), &reasoning, &source_units("</think>"), &visible_after]);
    let visible = unit_parts(&[&visible_before, &visible_after]);
    let fenced = unit_parts(&[
        &source_units("```thinking\n```rs\n"),
        &reasoning,
        &source_units("\n```\nthen\n``` done"),
        &visible_after,
    ]);
    let fenced_thinking = unit_parts(&[&source_units("```rs\n"), &reasoning, &source_units("\n```\nthen\n")]);
    for by_unit in [false, true] {
        let events = scan_thinking_units(&raw, by_unit);
        assert_eq!(units_projection(&events), (visible.clone(), WireString::from_units(reasoning.to_vec())));
        assert!(
            matches!(events.iter().find(|event| matches!(event, InbandScanEvent::ThinkingEnd { .. })), Some(InbandScanEvent::ThinkingEnd { thinking }) if thinking.units() == reasoning)
        );
        let events = scan_thinking_units(&fenced, by_unit);
        assert_eq!(
            units_projection(&events),
            (unit_parts(&[&source_units(" done"), &visible_after]), fenced_thinking.clone())
        );
        // Raw units inside an inline code span stay visible.
        let code = unit_parts(&[&source_units("`<think>"), &reasoning, &source_units("</think>`x")]);
        assert_eq!(units_projection(&scan_thinking_units(&code, by_unit)), (code, WireString::from_units(Vec::new())));
    }
}

#[test]
fn structured_thinking_stripper_original_unit_family() {
    // All six original stripper unit cases grouped here. Native's actual
    // consumeGoogleStream cases are NOT represented by these unit fixtures.
    for (chunks, expected) in [
        (vec!["a\n```thinking\nb\n"], "a\nb\n"),
        (vec!["a\n``````thinking\nb\n"], "a\nb\n"),
        (vec!["```reasoning\nx"], "x"),
        (vec!["intro\n``", "````thin", "king\nrest"], "intro\nrest"),
        (vec!["done\n``````thinking"], "done\n"),
        (vec!["look:\n```rs\nlet x = 1;\n```\ndone\n"], "look:\n```rs\nlet x = 1;\n```\ndone\n"),
        (vec!["I should emit a ```thinking block here.\n"], "I should emit a ```thinking block here.\n"),
        (vec!["    ```thinking\n"], "    ```thinking\n"),
    ] {
        let mut stripper = super::ThinkingFenceStripper::new();
        let mut output = String::new();
        for chunk in chunks {
            output.push_str(&stripper.push(chunk));
        }
        output.push_str(&stripper.flush());
        assert_eq!(output, expected);
    }

    // Batch4 full units adaptation; paired and unpaired content survives, and
    // a raw lone unit disqualifies an opener prefix without replacement.
    let content = [0xd800, 0xd83c, 0xdf3f, 0xdc00];
    let raw =
        unit_parts(&[&content, &source_units("\n```thinking\n```thin"), &[0xdfff], &source_units("king\n"), &content]);
    let expected = unit_parts(&[&content, &source_units("\n```thin"), &[0xdfff], &source_units("king\n"), &content]);
    for by_unit in [false, true] {
        let mut stripper = super::ThinkingFenceUnitsStripper::new();
        let mut output = Vec::new();
        if by_unit {
            for &unit in raw.units() {
                output.extend_from_slice(stripper.push_units(&WireString::from_units(vec![unit])).units());
            }
        } else {
            output.extend_from_slice(stripper.push_units(&raw).units());
        }
        output.extend_from_slice(stripper.flush_units().units());
        assert_eq!(output, expected.units());
    }
    // Native prefix uses toLowerCase, final /i classification remains distinct:
    // Kelvin sign lowers to k in a viable prefix, but final opener is not ASCII.
    let kelvin = WireString::from("```thin\u{212a}ing");
    let mut stripper = super::ThinkingFenceUnitsStripper::new();
    assert!(stripper.push_units(&kelvin).is_empty());
    assert_eq!(stripper.flush_units(), kelvin);
}

// Append to the existing dialect/tests.rs only after Root applies the reviewed
// fallible binding, preserving its existing GrammarRecorder/FaultRecorder.
// One coarse Kimi family; no production parser/carrier implementation lives here.
#[test]
fn kimi_units_native_thinking_states_rawblock_and_fallible_arguments_family() {
    use super::kimi::{
        KIMI_ARG_BEGIN, KIMI_CALL_BEGIN, KIMI_CALL_END, KIMI_SECTION_BEGIN, KIMI_SECTION_END, KimiInbandUnitsScanner,
    };

    use super::units::wire;

    fn scan(raw: &WireString, by_unit: bool, parse_thinking: bool) -> Vec<InbandScanEvent<WireValue, WireString>> {
        let mut scanner =
            KimiInbandUnitsScanner::with_parse_thinking(Arc::new(GrammarRecorder::default()), parse_thinking);
        let mut events = Vec::new();
        if by_unit {
            for &unit in raw.units() {
                events.extend(scanner.try_feed_units(&WireString::from_units(vec![unit])).unwrap());
            }
        } else {
            events.extend(scanner.try_feed_units(raw).unwrap());
        }
        events.extend(scanner.try_flush_units().unwrap());
        events
    }
    fn invocation(header: &str, body: &str) -> String {
        format!("{KIMI_CALL_BEGIN}{header}{KIMI_ARG_BEGIN}{body}{KIMI_CALL_END}")
    }
    fn section(header: &str, body: &str) -> String {
        format!("{KIMI_SECTION_BEGIN}{}{KIMI_SECTION_END}", invocation(header, body))
    }

    // Original dialect-thinking.test.ts:161-196 and 228-243, scanner portions.
    // The renderThinking round-trip input is literal here; renderer remains open.
    for (raw, expected_text, expected_thinking) in [
        ("<think>reasoning</think>The answer.".to_owned(), "The answer.", "reasoning"),
        ("<think>\nreasoning\n</think>".to_owned(), "", "\nreasoning\n"),
        (format!("<think>plan</think>{}", section("functions.foo:0", "{\"x\":1}")), "", "plan"),
        (format!("<think>step one</think>{}", section("functions.foo:0", "{\"x\":1}")), "", "step one"),
        ("<think>partial".to_owned(), "", "partial"),
    ] {
        for by_unit in [false, true] {
            let events = scan(&WireString::from(raw.as_str()), by_unit, true);
            let projection = units_projection(&events);
            assert!(projection.0.equals_ascii(expected_text));
            assert!(projection.1.equals_ascii(expected_thinking));
            assert_eq!(events.iter().filter(|event| matches!(event, InbandScanEvent::ThinkingStart)).count(), 1);
            assert_eq!(events.iter().filter(|event| matches!(event, InbandScanEvent::ThinkingEnd { .. })).count(), 1);
            let calls: Vec<_> = events
                .iter()
                .filter_map(|event| match event {
                    InbandScanEvent::ToolEnd { name, arguments, .. } => Some((name, arguments)),
                    _ => None,
                })
                .collect();
            if raw.contains(KIMI_SECTION_BEGIN) {
                assert_eq!(calls.len(), 1);
                assert!(calls[0].0.equals_ascii("foo"));
                assert_eq!(calls[0].1.get("x").and_then(WireValue::as_number), Some(1.0));
            } else {
                assert!(calls.is_empty());
            }
        }
    }
    let disabled = scan(&WireString::from("<think>x</think>answer"), true, false);
    assert!(units_projection(&disabled).0.equals_ascii("<think>x</think>answer"));
    assert!(
        !disabled
            .iter()
            .any(|event| matches!(event, InbandScanEvent::ThinkingStart | InbandScanEvent::ThinkingEnd { .. }))
    );

    // Direct-source state adaptations: default thinking, empty pending buffer,
    // stray control tokens, section junk/tokens, missing header/args endings.
    let mut scanner = KimiInbandUnitsScanner::new(Arc::new(GrammarRecorder::default()));
    assert!(matches!(
        &scanner.try_feed_units(&WireString::from("<think>")).unwrap()[..],
        [InbandScanEvent::ThinkingStart]
    ));
    assert!(
        matches!(&scanner.try_flush_units().unwrap()[..], [InbandScanEvent::ThinkingEnd { thinking }] if thinking.is_empty())
    );
    assert!(scanner.try_flush_units().unwrap().is_empty());
    for token in [KIMI_SECTION_END, KIMI_CALL_BEGIN, KIMI_CALL_END, KIMI_ARG_BEGIN] {
        assert_eq!(
            scan(&WireString::from(token), true, true),
            vec![InbandScanEvent::Text { text: WireString::from(token) }]
        );
    }
    let mixed = format!(
        "pre{KIMI_SECTION_BEGIN}\t\n\r\u{000b}\u{000c} junk {KIMI_ARG_BEGIN}{KIMI_CALL_END}{}{KIMI_SECTION_END}post",
        invocation(" functions.ns. read :9 ", "{x:1}")
    );
    let mixed_events = scan(&WireString::from(mixed.as_str()), true, true);
    assert!(units_projection(&mixed_events).0.equals_ascii("prepost"));
    let mixed_end = mixed_events
        .iter()
        .find_map(|event| match event {
            InbandScanEvent::ToolEnd { id, name, raw_block, .. } => Some((id, name, raw_block)),
            _ => None,
        })
        .unwrap();
    assert!(mixed_end.0.equals_ascii("functions.ns. read :9"));
    assert!(mixed_end.1.equals_ascii("read"));
    assert!(mixed_end.2.as_ref().unwrap().equals_ascii(&invocation(" functions.ns. read :9 ", "{x:1}")));
    for raw in [
        format!("{KIMI_SECTION_BEGIN}{KIMI_CALL_BEGIN}functions.read:0"),
        format!("{KIMI_SECTION_BEGIN}{KIMI_CALL_BEGIN}functions.read:0{KIMI_ARG_BEGIN}{{\"x\":1}}"),
    ] {
        let mut scanner = KimiInbandUnitsScanner::new(Arc::new(GrammarRecorder::default()));
        let events = scanner.try_feed_units(&WireString::from(raw)).unwrap();
        assert!(!events.iter().any(|event| matches!(event, InbandScanEvent::ToolEnd { .. })));
        assert!(scanner.try_flush_units().unwrap().is_empty());
        assert_eq!(
            scanner.try_feed_units(&WireString::from("after")).unwrap(),
            vec![InbandScanEvent::Text { text: WireString::from("after") }]
        );
    }
    // An empty pending header/args buffer never enters Native's consume loop;
    // flush therefore retains those states, rather than dropping the call.
    let mut scanner = KimiInbandUnitsScanner::new(Arc::new(GrammarRecorder::default()));
    assert!(
        scanner.try_feed_units(&WireString::from(format!("{KIMI_SECTION_BEGIN}{KIMI_CALL_BEGIN}"))).unwrap().is_empty()
    );
    assert!(scanner.try_flush_units().unwrap().is_empty());
    assert!(
        matches!(&scanner.try_feed_units(&WireString::from(format!("functions.read:0{KIMI_ARG_BEGIN}"))).unwrap()[..],
        [InbandScanEvent::ToolStart { id, name }] if id.equals_ascii("functions.read:0") && name.equals_ascii("read"))
    );
    assert!(scanner.try_flush_units().unwrap().is_empty());
    assert!(
        matches!(&scanner.try_feed_units(&WireString::from(format!("{{}}{KIMI_CALL_END}{KIMI_SECTION_END}"))).unwrap()[..],
        [InbandScanEvent::ToolEnd { arguments, .. }] if arguments.stringify() == "{}")
    );

    for body in ["", "null", "[1]", "7", "{broken"] {
        let events = scan(&WireString::from(section("", body)), true, true);
        assert!(matches!(&events[0], InbandScanEvent::ToolStart { id, name } if id.is_empty() && name.is_empty()));
        assert!(matches!(&events[1], InbandScanEvent::ToolEnd { arguments, .. } if arguments.stringify() == "{}"));
    }

    // Full unit-domain adaptation, including lone units in names/strict strings,
    // raw trim preservation and every split inside fixed delimiters/pair units.
    let name = [0x72, 0xd800, 0x65, 0xdc00];
    let payload = [0xd800, 0xd83c, 0xdf3f, 0xdc00];
    let block = unit_parts(&[
        &source_units(KIMI_CALL_BEGIN),
        &source_units("\u{feff} functions.ns."),
        &name,
        &source_units(":3 \u{00a0}"),
        &source_units(KIMI_ARG_BEGIN),
        &source_units(" \n{\"raw\":\""),
        &payload,
        &source_units("\"}\t"),
        &source_units(KIMI_CALL_END),
    ]);
    let raw = unit_parts(&[&source_units(KIMI_SECTION_BEGIN), block.units(), &source_units(KIMI_SECTION_END)]);
    for split in 0..=raw.len() {
        let mut scanner = KimiInbandUnitsScanner::new(Arc::new(GrammarRecorder::default()));
        let mut events = scanner.try_feed_units(&wire(&raw.units()[..split])).unwrap();
        events.extend(scanner.try_feed_units(&wire(&raw.units()[split..])).unwrap());
        events.extend(scanner.try_flush_units().unwrap());
        assert_eq!(events.len(), 2, "unit split={split}");
        let InbandScanEvent::ToolEnd { name: actual, arguments, raw_block, .. } = &events[1] else {
            panic!("unit toolEnd");
        };
        assert_eq!(actual.units(), name);
        assert_eq!(arguments.get("raw").and_then(WireValue::as_string).unwrap().units(), payload);
        assert_eq!(raw_block.as_ref(), Some(&block));
    }
    let raw_thinking = unit_parts(&[&source_units("<think>"), &payload, &source_units("</think>"), &payload]);
    let events = scan(&raw_thinking, true, true);
    let projection = units_projection(&events);
    assert_eq!(projection.0.units(), payload);
    assert_eq!(projection.1.units(), payload);

    // Same existing controlled failure binding as the shared H2 family. It is
    // not a production value engine or actual graph/wrapper acceptance.
    for (body, failures, trace) in [
        ("{}", vec![("import_strict", 1, "strict import throw")], vec!["import_strict", "object"]),
        ("{}", vec![("is_record", 1, "record throw")], vec!["import_strict", "is_record", "object"]),
        ("null", vec![("object", 1, "asRecord object throw")], vec!["import_strict", "is_record", "object", "object"]),
        (
            "{first:1,second:2}",
            vec![("assign:second", 1, "assignment throw")],
            vec!["object", "number", "assign:first", "number", "assign:second", "object"],
        ),
    ] {
        let factory = Arc::new(FaultRecorder::throwing(&failures));
        let mut scanner = KimiInbandUnitsScanner::new(factory.clone());
        let events = scanner.try_feed_units(&WireString::from(section("functions.read:0", body))).unwrap();
        let InbandScanEvent::ToolEnd { arguments, .. } = &events[1] else {
            panic!("fallible Kimi end");
        };
        assert_eq!(arguments.snapshot().stringify(), "{}");
        let objects = factory.objects.lock().unwrap();
        assert!(Arc::ptr_eq(&arguments.0, &objects.last().unwrap().0));
        if body.starts_with("{first") {
            assert_eq!(objects[0].snapshot().stringify(), r#"{"first":1}"#);
        }
        assert_eq!(factory.trace(), trace);
    }
    for (body, failures, trace, expected) in [
        ("", vec![("object", 1, "empty throw")], vec!["object"], "empty throw"),
        (
            "{}",
            vec![("import_strict", 1, "strict throw"), ("object", 1, "fallback throw")],
            vec!["import_strict", "object"],
            "fallback throw",
        ),
        (
            "null",
            vec![("object", 1, "asRecord throw"), ("object", 2, "fallback throw")],
            vec!["import_strict", "is_record", "object", "object"],
            "fallback throw",
        ),
    ] {
        let factory = Arc::new(FaultRecorder::throwing(&failures));
        let mut scanner = KimiInbandUnitsScanner::new(factory.clone());
        assert!(
            matches!(scanner.try_feed_units(&WireString::from(section("functions.read:0", body))), Err(error) if error == expected)
        );
        assert_eq!(factory.trace(), trace);
    }
    // Same classification seam as Harmony: unsupported boundaries propagate
    // without fallback; these labels are controlled faults, not Graph EXEC.
    for (body, operation, boundary, trace) in [
        ("{}", "import_strict", "MissingCapability", vec!["import_strict"]),
        ("{}", "is_record", "ForeignHeap", vec!["import_strict", "is_record"]),
        ("null", "object", "MissingCapability", vec!["import_strict", "is_record", "object"]),
        (
            "{first:1,second:2}",
            "assign:second",
            "ForeignHeap",
            vec!["object", "number", "assign:first", "number", "assign:second"],
        ),
    ] {
        let mut fault = FaultRecorder::throwing(&[(operation, 1, boundary)]);
        fault.uncaught_errors = vec![boundary];
        let factory = Arc::new(fault);
        let mut scanner = KimiInbandUnitsScanner::new(factory.clone());
        assert!(
            matches!(scanner.try_feed_units(&WireString::from(section("functions.read:0", body))), Err(error) if error == boundary)
        );
        assert_eq!(factory.trace(), trace);
        assert_eq!(*factory.classifications.lock().unwrap(), [(boundary, false)]);
        if body.starts_with("{first") {
            assert_eq!(factory.objects.lock().unwrap()[0].snapshot().stringify(), r#"{"first":1}"#);
        }
    }
    let factory = Arc::new(FaultRecorder::throwing(&[("is_record", 1, "TypeError")]));
    let mut scanner = KimiInbandUnitsScanner::new(factory.clone());
    let caught = scanner.try_feed_units(&WireString::from(section("functions.read:0", "{}"))).unwrap();
    assert!(
        matches!(&caught[1], InbandScanEvent::ToolEnd { arguments, .. } if arguments.snapshot().stringify() == "{}")
    );
    assert_eq!(*factory.classifications.lock().unwrap(), [("TypeError", true)]);

    // Kimi differs from Harmony: its call end has NOT been sliced on Err.
    // Explicit later flush observes that retained state; no implicit replay.
    let factory = Arc::new(FaultRecorder::throwing(&[("object", 1, "first empty throw")]));
    let mut scanner = KimiInbandUnitsScanner::new(factory.clone());
    let raw = format!("{}tail", section("functions.read:0", ""));
    assert!(matches!(scanner.try_feed_units(&WireString::from(raw)), Err("first empty throw")));
    assert!(scanner.try_feed_units(&WireString::from("")).unwrap().is_empty());
    let later = scanner.try_flush_units().unwrap();
    assert!(matches!(&later[..], [InbandScanEvent::ToolEnd { id, .. }, InbandScanEvent::Text { text }]
        if id.equals_ascii("functions.read:0") && text.equals_ascii("tail")));
    assert_eq!(factory.trace(), ["object", "object"]);
}

/// One coarse block-1 family. Original eight families/inputs remain unchanged;
/// these fixed-source adaptations exercise the actual heap, not a value mock.
#[test]
fn block1_deepseek_hermes_qwen3_actual_module_family() {
    super::actual_projection::observe_owned_projection_family();
    super::owned_stream::observe_owned_stream_family();
    use super::rendering::{
        TranscriptAssistantBlockUnits as Block, TranscriptContentUnits as Content,
        TranscriptMessageUnits as MessageView, TranscriptUserBlockUnits as UserBlockView,
    };
    use super::types::{InbandEventProjectionError, try_project_actual_events_to_strings};
    use super::{
        ActualInbandScanEvent, DeepSeekInbandUnitsScanner, DeepSeekRenderer, HermesInbandUnitsScanner, HermesRenderer,
        InbandNameValue, Qwen3InbandUnitsScanner, Qwen3Renderer,
    };
    use crate::graph_values::GraphValueFactory;
    use crate::relaxed_json::parse_streaming_json_units;
    use ara_rpc::value_heap::{
        CallableTarget, CapabilityId, NoRuntime, PropertyDescriptor, RuntimeReply, RuntimeRequest, ValueError,
        ValueHandle, ValueHeap, ValueRuntime, ValueSnapshot,
    };

    fn scalar_name(name: &InbandNameValue<ValueHandle>, factory: &GraphValueFactory) -> WireString {
        match name {
            InbandNameValue::Source(name) => name.clone(),
            InbandNameValue::Actual(value) => factory.string_value(value).unwrap().unwrap(),
        }
    }
    fn json(value: &ValueHandle, factory: &GraphValueFactory) -> String {
        factory
            .stringify_json(value, ara_rpc::value_heap::StringifyMode::BigIntReplacer)
            .unwrap()
            .unwrap()
            .to_utf8()
            .unwrap()
    }
    let heap = ValueHeap::new().unwrap();
    let factory = Arc::new(GraphValueFactory::new(heap.clone(), Arc::new(NoRuntime)).unwrap());
    for (input, expected) in [
        ("", "{}"),
        ("  /* comment */", "{}"),
        ("null", "null"),
        ("None", "{}"),
        ("{name:'read', arguments:{x:1, y:tru", r#"{"name":"read","arguments":{"x":1}}"#),
        ("[1,2e", "[1]"),
        ("[1, unknown, 2]", "[1]"),
        ("{name:\"r", r#"{"name":"r"}"#),
        ("{text:\"one \"inner\" end", r#"{"text":"one \"inner\" end"}"#),
        ("{x:1} trailing", r#"{"x":1}"#),
    ] {
        assert_eq!(
            json(&parse_streaming_json_units(&input.into(), factory.as_ref()).unwrap(), factory.as_ref()),
            expected,
            "{input}"
        );
    }

    // Reuse the existing controlled grammar fault binding for allocations;
    // its flat snapshots are not actual Graph/provider acceptance.
    let recovered = FaultRecorder::throwing(&[("object", 1, "coalesce error")]);
    assert_eq!(parse_streaming_json_units(&"NaN".into(), &recovered).unwrap().snapshot().stringify(), "{}");
    assert_eq!(recovered.trace(), ["object", "object"]);
    let fallback_failure =
        FaultRecorder::throwing(&[("object", 1, "coalesce error"), ("object", 2, "catch allocation error")]);
    assert!(matches!(
        parse_streaming_json_units(&"NaN".into(), &fallback_failure),
        Err(FinalParseError::Runtime("catch allocation error"))
    ));
    assert_eq!(fallback_failure.trace(), ["object", "object"]);
    let import_failure = FaultRecorder::throwing(&[("import_strict", 1, "import error")]);
    assert!(matches!(
        parse_streaming_json_units(&"{}".into(), &import_failure),
        Err(FinalParseError::Runtime("import error"))
    ));
    assert_eq!(import_failure.trace(), ["import_strict"]);

    // Whole and one-unit feeds reuse one shared scanner grammar per dialect.
    let ds = format!(
        "{}read{}{{x:1}}{}",
        super::deepseek::DEEPSEEK_TOOL_CALL_BEGIN,
        super::deepseek::DEEPSEEK_TOOL_SEPARATOR,
        super::deepseek::DEEPSEEK_TOOL_CALL_END
    );
    let json_call = "<tool_call>{\"name\":\"read\",\"arguments\":{\"x\":1}}</tool_call>";
    for dialect in 0..3 {
        for split_units in [false, true] {
            let input = WireString::from(if dialect == 0 { ds.as_str() } else { json_call });
            let mut deepseek = DeepSeekInbandUnitsScanner::new(factory.clone()).unwrap();
            let mut hermes = HermesInbandUnitsScanner::new(factory.clone());
            let mut qwen = Qwen3InbandUnitsScanner::new(factory.clone());
            let mut events: Vec<ActualInbandScanEvent<ValueHandle>> = Vec::new();
            let chunks = if split_units {
                input.units().iter().map(|unit| WireString::from_units(vec![*unit])).collect::<Vec<_>>()
            } else {
                vec![input.clone()]
            };
            for chunk in chunks {
                events.extend(
                    match dialect {
                        0 => deepseek.try_feed_units(&chunk),
                        1 => hermes.try_feed_units(&chunk),
                        _ => qwen.try_feed_units(&chunk),
                    }
                    .unwrap(),
                );
            }
            events.extend(
                match dialect {
                    0 => deepseek.try_flush_units(),
                    1 => hermes.try_flush_units(),
                    _ => qwen.try_flush_units(),
                }
                .unwrap(),
            );
            assert_eq!(events.iter().filter(|event| matches!(event, InbandScanEvent::ToolStart { .. })).count(), 1);
            let ends = events
                .iter()
                .filter_map(|event| match event {
                    InbandScanEvent::ToolEnd { name, arguments, raw_block, .. } => Some((name, arguments, raw_block)),
                    _ => None,
                })
                .collect::<Vec<_>>();
            assert_eq!(ends.len(), 1);
            assert!(scalar_name(ends[0].0, factory.as_ref()).equals_ascii("read"));
            assert_eq!(json(ends[0].1, factory.as_ref()), r#"{"x":1}"#);
            assert_eq!(ends[0].2.as_ref(), Some(&input));
        }
    }
    for (left, right) in [("<|DSML|", "</|DSML|"), ("<｜DSML｜", "</｜DSML｜")] {
        let input = format!(
            "{left}tool_calls>{left}invoke name=\"\">{left}parameter name=\"x\" string=\"invalid\" string=\"false\">1{right}parameter>{right}invoke>{right}tool_calls>"
        );
        let events = DeepSeekInbandUnitsScanner::new(factory.clone()).unwrap().try_feed_units(&input.into()).unwrap();
        assert!(events.iter().any(|event| matches!(event, InbandScanEvent::ToolArgDelta { key, delta, .. } if key.equals_ascii("x") && delta.equals_ascii("1"))));
        assert!(events.iter().any(|event| matches!(event, InbandScanEvent::ToolEnd { name, arguments, .. } if scalar_name(name, factory.as_ref()).is_empty() && json(arguments, factory.as_ref()) == r#"{"x":1}"#)));
    }
    let legacy = format!(
        "{}{}function{}read```json\r\n{{x:1}}```{}{}",
        super::deepseek::DEEPSEEK_TOOL_CALLS_BEGIN,
        super::deepseek::DEEPSEEK_TOOL_CALL_BEGIN,
        super::deepseek::DEEPSEEK_TOOL_SEPARATOR,
        super::deepseek::DEEPSEEK_TOOL_CALL_END,
        super::deepseek::DEEPSEEK_TOOL_CALLS_END
    );
    let events = DeepSeekInbandUnitsScanner::new(factory.clone()).unwrap().try_feed_units(&legacy.into()).unwrap();
    assert!(events.iter().any(|event| matches!(event, InbandScanEvent::ToolEnd { arguments, .. } if json(arguments, factory.as_ref()) == r#"{"x":1}"#)));

    let mut hermes = HermesInbandUnitsScanner::with_parse_thinking(factory.clone(), true);
    let mut qwen = Qwen3InbandUnitsScanner::new(factory.clone());
    assert!(
        hermes
            .try_feed_units(&"<think>x".into())
            .unwrap()
            .iter()
            .any(|event| matches!(event, InbandScanEvent::ThinkingDelta { .. }))
    );
    qwen.try_feed_units(&"<think>x".into()).unwrap();
    assert!(hermes.try_flush_units().unwrap().is_empty());
    assert!(matches!(qwen.try_flush_units().unwrap().as_slice(), [InbandScanEvent::ThinkingEnd { .. }]));
    let mut hermes = HermesInbandUnitsScanner::new(factory.clone());
    let failed = hermes.try_feed_units(&"<tool_call>{\"name\":\"read\",\"arguments\":{</tool_call>".into()).unwrap();
    assert!(failed.iter().any(|event| matches!(event, InbandScanEvent::ToolStart { .. })));
    assert!(!failed.iter().any(|event| matches!(event, InbandScanEvent::ToolEnd { .. })));
    let disabled = DeepSeekInbandUnitsScanner::with_parse_thinking(factory.clone(), false)
        .unwrap()
        .try_feed_units(&"<｜Assistant｜> \n<think>x</think>y</|DSML|invoke>".into())
        .unwrap();
    let text = disabled
        .iter()
        .filter_map(|event| match event {
            InbandScanEvent::Text { text } => Some(text.units()),
            _ => None,
        })
        .flatten()
        .copied()
        .collect::<Vec<_>>();
    assert_eq!(text, source_units("xy"));

    // Controlled callback receiver uses the ACTUAL heap/property engine.
    struct GetRuntime {
        heap: Arc<ValueHeap>,
        names: Mutex<usize>,
        trace: Mutex<Vec<&'static str>>,
    }
    impl ValueRuntime for GetRuntime {
        fn resolve(&self, request: RuntimeRequest<'_>) -> Result<RuntimeReply, ValueError> {
            match request {
                RuntimeRequest::Call { capability: CapabilityId(301), .. } => {
                    self.trace.lock().unwrap().push("name");
                    let nth = {
                        let mut names = self.names.lock().unwrap();
                        *names += 1;
                        *names
                    };
                    Ok(RuntimeReply::Value(if nth % 3 == 0 {
                        self.heap.number(17.0)?
                    } else {
                        self.heap.string("read".into())?
                    }))
                }
                RuntimeRequest::Call { capability: CapabilityId(302), .. } => {
                    self.trace.lock().unwrap().push("arguments");
                    Ok(RuntimeReply::Value(self.heap.string("{}".into())?))
                }
                request => Err(ValueError::MissingCapability(request.capability())),
            }
        }
    }
    for dialect in [1, 2] {
        let heap = ValueHeap::new().unwrap();
        let runtime = Arc::new(GetRuntime { heap: heap.clone(), names: Mutex::new(0), trace: Mutex::new(Vec::new()) });
        for (key, capability) in [("name", 301), ("arguments", 302)] {
            heap.define_own(
                &heap.object_prototype(),
                key.into(),
                PropertyDescriptor::Accessor {
                    get: Some(CallableTarget::Host(CapabilityId(capability))),
                    set: None,
                    enumerable: true,
                    configurable: true,
                },
                &NoRuntime,
            )
            .unwrap();
        }
        let factory = Arc::new(GraphValueFactory::new(heap.clone(), runtime.clone()).unwrap());
        let input = WireString::from("<tool_call>{}</tool_call>");
        let events = if dialect == 1 {
            HermesInbandUnitsScanner::new(factory.clone()).try_feed_units(&input)
        } else {
            Qwen3InbandUnitsScanner::new(factory.clone()).try_feed_units(&input)
        }
        .unwrap();
        assert!(
            events
                .iter()
                .any(|event| matches!(event, InbandScanEvent::ToolEnd { name: InbandNameValue::Actual(name), .. }
            if matches!(heap.snapshot(name).unwrap(), ValueSnapshot::Number(17.0))))
        );
        assert_eq!(
            *runtime.trace.lock().unwrap(),
            if dialect == 1 {
                vec!["name", "name", "name", "name", "name", "arguments", "name"]
            } else {
                vec!["name", "name", "arguments", "name"]
            }
        );
        assert!(matches!(
            try_project_actual_events_to_strings(events, factory.as_ref()),
            Err(InbandEventProjectionError::NonStringName)
        ));
    }

    // One wrapper stringify preserves BigInt and argument toJSON's key.
    struct JsonRuntime {
        heap: Arc<ValueHeap>,
        keys: Mutex<Vec<WireString>>,
    }
    impl ValueRuntime for JsonRuntime {
        fn resolve(&self, request: RuntimeRequest<'_>) -> Result<RuntimeReply, ValueError> {
            if let RuntimeRequest::Call { capability: CapabilityId(401), arguments, .. } = request {
                let ValueSnapshot::String(key) = self.heap.snapshot(&arguments[0])? else {
                    return Err(ValueError::WrongRuntimeReply("key"));
                };
                self.keys.lock().unwrap().push(key);
                return Ok(RuntimeReply::Value(self.heap.number(7.0)?));
            }
            Err(ValueError::MissingCapability(request.capability()))
        }
    }
    let heap = ValueHeap::new().unwrap();
    let runtime = Arc::new(JsonRuntime { heap: heap.clone(), keys: Mutex::new(Vec::new()) });
    let factory = Arc::new(GraphValueFactory::new(heap.clone(), runtime.clone()).unwrap());
    let args = heap.object().unwrap();
    heap.define_own_data(&args, "toJSON".into(), heap.function(CapabilityId(401)).unwrap(), &NoRuntime).unwrap();
    let call = heap.object().unwrap();
    heap.define_own_data(&call, "name".into(), heap.string("read".into()).unwrap(), &NoRuntime).unwrap();
    heap.define_own_data(&call, "arguments".into(), args, &NoRuntime).unwrap();
    let h = HermesRenderer::new(factory.clone());
    let q = Qwen3Renderer::new(factory.clone());
    let d = DeepSeekRenderer::new(factory.clone());
    assert_eq!(
        h.render_tool_call_units(&call).unwrap().to_utf8().unwrap(),
        "<tool_call>\n{\"name\":\"read\",\"arguments\":7}\n</tool_call>"
    );
    assert_eq!(h.render_tool_call_units(&call).unwrap(), q.render_tool_call_units(&call).unwrap());
    d.render_tool_call_units(&call).unwrap();
    assert_eq!(
        *runtime.keys.lock().unwrap(),
        [WireString::from("arguments"), "arguments".into(), "arguments".into(), "".into()]
    );
    assert!(h.render_thinking_units(&"<think></think>".into()).equals_ascii("<think>\n\n</think>"));
    let bigint_args = heap.object().unwrap();
    heap.define_own_data(&bigint_args, "n".into(), heap.bigint_decimal("18446744073709551616").unwrap(), &NoRuntime)
        .unwrap();
    heap.define_own_data(&call, "arguments".into(), bigint_args, &NoRuntime).unwrap();
    assert!(h.render_tool_call_units(&call).unwrap().to_utf8().unwrap().contains("\"18446744073709551616\""));
    heap.define_own_data(&call, "name".into(), heap.symbol(Some("x".into())).unwrap(), &NoRuntime).unwrap();
    assert!(matches!(d.render_tool_call_units(&call), Err(ValueError::TypeError(_))));

    // Readonly transcript view preserves image order, joined thinking and
    // contiguous toolResult grouping; Root supplies its actual R2 binding.
    let role = WireString::from("developer");
    let one = WireString::from("A");
    let two = WireString::from("B");
    let mime = WireString::from("image/png");
    let empty = WireString::from("");
    let id = WireString::from("id");
    let user_blocks = [
        UserBlockView::Text(Some(&one)),
        UserBlockView::Image(Some(&mime)),
        UserBlockView::Image(None),
        UserBlockView::Text(Some(&two)),
    ];
    let assistant_blocks = [Block::Thinking(&one), Block::Thinking(&empty), Block::Text(&two)];
    let messages = [
        MessageView::<ValueHandle>::Content { role: &role, content: Content::Blocks(&user_blocks) },
        MessageView::Assistant(&assistant_blocks),
        MessageView::ToolResult { id: &id, name: &one, content: Content::Text(&one), is_error: false },
        MessageView::ToolResult { id: &id, name: &one, content: Content::Text(&two), is_error: true },
    ];
    let ht = h.render_transcript_units(&messages).unwrap().to_utf8().unwrap();
    let qt = q.render_transcript_units(&messages).unwrap().to_utf8().unwrap();
    assert!(ht.contains("<|im_start|>system\nA[Image: image/png][Image]B"));
    assert!(ht.contains("<|im_start|>tool\n"));
    assert!(qt.contains("<|im_start|>user\n<tool_response>"));
    assert_eq!(ht.matches("<|im_start|>tool").count(), 1);
    assert!(
        d.render_transcript_units(&messages).unwrap().to_utf8().unwrap().starts_with(super::deepseek::DEEPSEEK_BOS)
    );
    let raw_units = WireString::from_units(vec![0xd800]);
    assert!(matches!(
        try_project_actual_events_to_strings(vec![InbandScanEvent::Text { text: raw_units }], factory.as_ref()),
        Err(InbandEventProjectionError::InvalidUtf16(_))
    ));
}
