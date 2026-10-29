//! Draft observations called by the existing coarse module corpus; no new
//! test entrypoint. Root compiles/executes this candidate after independent PRE.
use ara_ai::{
    graph_values::GraphValueFactory,
    tool_call_loop_guard::{ToolCallLoopGuard, actual},
};
use ara_rpc::{
    WireString, WireValue,
    value_heap::{
        CompiledFunctionForm, CompiledFunctionSpec, NoRuntime, PropertyDescriptor, ValueError, ValueHandle, ValueHeap,
        ValueSnapshot,
    },
};
use std::sync::Arc;

fn factory() -> GraphValueFactory {
    GraphValueFactory::new(ValueHeap::new().unwrap(), Arc::new(NoRuntime)).unwrap()
}
fn get(f: &GraphValueFactory, value: &ValueHandle, key: &str) -> ValueHandle {
    f.heap().get(value, &key.into(), f.runtime()).unwrap()
}
fn own(f: &GraphValueFactory, value: &ValueHandle, key: &str, member: ValueHandle) {
    f.heap().define_own_data(value, key.into(), member, f.runtime()).unwrap();
}
fn string(f: &GraphValueFactory, value: &ValueHandle) -> WireString {
    match f.heap().snapshot(value).unwrap() {
        ValueSnapshot::String(value) => value,
        other => panic!("{other:?}"),
    }
}
fn make_turn(f: &GraphValueFactory, arguments: &str, output: &str) -> ValueHandle {
    let message = f.heap().import_strict(WireValue::parse(&format!(
        r#"{{"role":"assistant","content":[{{"type":"toolCall","id":"id","name":"read","arguments":{arguments}}}]}}"#
    )).unwrap()).unwrap();
    let result = f.heap().object().unwrap();
    own(f, &result, "toolCallId", f.heap().string("id".into()).unwrap());
    let text = f.heap().object().unwrap();
    own(f, &text, "type", f.heap().string("text".into()).unwrap());
    own(f, &text, "text", f.heap().string(output.into()).unwrap());
    own(f, &result, "content", f.heap().array(vec![text]).unwrap());
    let turn = f.heap().object().unwrap();
    own(f, &turn, "message", message);
    own(f, &turn, "toolResults", f.heap().array(vec![result]).unwrap());
    turn
}
fn record(
    f: &GraphValueFactory,
    guard: &mut ToolCallLoopGuard,
    turn: &ValueHandle,
    set: &ValueHandle,
) -> Option<ValueHandle> {
    let prepared = actual::prepare_turn(f, turn, set).unwrap();
    guard.commit_actual_turn(&prepared).map(|count| actual::finish_turn(f, &prepared, count).unwrap())
}
fn accessor(
    f: &GraphValueFactory,
    target: &ValueHandle,
    key: &str,
    spec: &'static CompiledFunctionSpec,
    captures: &[ValueHandle],
) {
    let function = f.heap().fresh_compiled_function(spec, captures).unwrap();
    let ValueSnapshot::Function(function) = f.heap().snapshot(&function).unwrap() else { unreachable!() };
    f.heap()
        .define_own(
            target,
            key.into(),
            PropertyDescriptor::Accessor { get: Some(function), set: None, enumerable: true, configurable: true },
            f.runtime(),
        )
        .unwrap();
}

static GETTER: CompiledFunctionSpec = CompiledFunctionSpec {
    source: "fixture getter records its exact Get then returns its original edge",
    name: "",
    length: 0,
    form: CompiledFunctionForm::Arrow,
    capture_count: 3,
    body: |heap, captures, _, _, runtime| {
        let push = heap.get(&captures[0], &"push".into(), runtime)?;
        heap.call(&push, &captures[0], &[captures[1].clone()], runtime)?;
        Ok(captures[2].clone())
    },
};
static THROW: CompiledFunctionSpec = CompiledFunctionSpec {
    source: "fixture getter throws its original sentinel",
    name: "",
    length: 0,
    form: CompiledFunctionForm::Arrow,
    capture_count: 1,
    body: |_, captures, _, _, _| Err(ValueError::Thrown(captures[0].clone())),
};
static RETURN: CompiledFunctionSpec = CompiledFunctionSpec {
    source: "fixture method returns its original captured edge",
    name: "",
    length: 0,
    form: CompiledFunctionForm::Arrow,
    capture_count: 1,
    body: |_, captures, _, _, _| Ok(captures[0].clone()),
};
static KEEP_ARROW: CompiledFunctionSpec = CompiledFunctionSpec {
    source: "fixture overridden every retains arrow before returning false",
    name: "",
    length: 1,
    form: CompiledFunctionForm::Arrow,
    capture_count: 1,
    body: |heap, captures, _, args, runtime| {
        heap.define_own_data(&captures[0], "callback".into(), args[0].clone(), runtime)?;
        heap.boolean(false)
    },
};

pub fn observe_original_value_consumer() {
    // Getter/method order, retained original arguments, nested canonicalization
    // and no intent Get. A skipped intent accessor would throw if consumed.
    let f = factory();
    let set = actual::create_exempt_set(&f, &["hub".into()]).unwrap();
    let turn = make_turn(
        &f,
        r#"{"z":2,"a":1,"nested":[null,{"i":"intent","__intent":"legacy","b":3}]}"#,
        " first\t second\nthird ",
    );
    let calls = get(&f, &get(&f, &turn, "message"), "content");
    let call = get(&f, &calls, "0");
    let args = get(&f, &call, "arguments");
    let sentinel = f.heap().object().unwrap();
    accessor(&f, &args, "i", &THROW, &[sentinel.clone()]);
    let log = f.heap().array(vec![]).unwrap();
    for key in ["a", "z"] {
        let value = get(&f, &args, key);
        accessor(&f, &args, key, &GETTER, &[log.clone(), f.heap().string(key.into()).unwrap(), value]);
    }
    let mut detector = super::guard(2.0, &["hub"]);
    assert!(record(&f, &mut detector, &turn, &set).is_none());
    let detection = record(&f, &mut detector, &turn, &set).unwrap();
    assert_eq!(string(&f, &get(&f, &detection, "resultSummary")), "first second third".into());
    assert_eq!(string(&f, &get(&f, &detection, "argumentsSummary")), r#"{"a":1,"nested":[null,{"b":3}],"z":2}"#.into());
    assert_eq!(
        f.heap().stringify(&log, ara_rpc::value_heap::StringifyMode::Plain, None, f.runtime()).unwrap().unwrap(),
        r#"["a","z","a","z","a","z"]"#.into()
    );
    assert!(get(&f, &call, "arguments").same_identity(&args));

    // Ordinary {} assignment invokes inherited __proto__; inherited toJSON
    // changes the hash and argument summary. Typed skip is not this consumer.
    let f = factory();
    let set = actual::create_exempt_set(&f, &[]).unwrap();
    let turn = make_turn(&f, "{}", "");
    let call = get(&f, &get(&f, &get(&f, &turn, "message"), "content"), "0");
    let args = get(&f, &call, "arguments");
    let proto = f.heap().object().unwrap();
    let return_text =
        f.heap().fresh_compiled_function(&RETURN, &[f.heap().string("via-proto".into()).unwrap()]).unwrap();
    own(&f, &proto, "toJSON", return_text);
    own(&f, &args, "__proto__", proto);
    let detection = record(&f, &mut super::guard(1.0, &[]), &turn, &set).unwrap();
    assert_eq!(string(&f, &get(&f, &detection, "argumentsSummary")), r#""via-proto""#.into());

    // Sparse original array map retains holes, then JSON.stringify emits null.
    let sparse = f.heap().array_with_length(2).unwrap();
    own(&f, &sparse, "1", f.heap().number(7.0).unwrap());
    own(&f, &args, "__proto__", f.heap().null().unwrap());
    own(&f, &args, "sparse", sparse);
    let detection = record(&f, &mut super::guard(1.0, &[]), &turn, &set).unwrap();
    assert_eq!(string(&f, &get(&f, &detection, "argumentsSummary")), r#"{"sparse":[null,7]}"#.into());

    // Finish Throw preserves the already committed count; the third same
    // batch does not repeat an exact threshold=2 notice.
    let f = factory();
    let set = actual::create_exempt_set(&f, &[]).unwrap();
    let turn = make_turn(&f, r#"{"path":"x"}"#, "original");
    let mut detector = super::guard(2.0, &[]);
    assert!(record(&f, &mut detector, &turn, &set).is_none());
    let results = get(&f, &turn, "toolResults");
    let result = get(&f, &results, "0");
    let original_content = get(&f, &result, "content");
    let sentinel = f.heap().object().unwrap();
    accessor(&f, &result, "content", &THROW, &[sentinel.clone()]);
    let prepared = actual::prepare_turn(&f, &turn, &set).unwrap();
    let count = detector.commit_actual_turn(&prepared).unwrap();
    match actual::finish_turn(&f, &prepared, count) {
        Err(ValueError::Thrown(value)) => assert!(value.same_identity(&sentinel)),
        _ => panic!("Original summary Throw must retain identity"),
    }
    own(&f, &result, "content", original_content);
    assert!(record(&f, &mut detector, &turn, &set).is_none());

    // Original UTF16 slice can end at a high surrogate, retained in Graph.
    let f = factory();
    let set = actual::create_exempt_set(&f, &[]).unwrap();
    let turn = make_turn(&f, r#"{"path":"x"}"#, &format!("{}🌊z", "a".repeat(199)));
    let detection = record(&f, &mut super::guard(1.0, &[]), &turn, &set).unwrap();
    let summary = string(&f, &get(&f, &detection, "resultSummary"));
    assert_eq!(&summary.units()[199..], &[0xd83c, 0x2026]);

    // Overridden sort returns undefined: upstream lastHash starts undefined,
    // so this commits count=1, with no forced JSON/UTF8 DTO domain check.
    let f = factory();
    let set = actual::create_exempt_set(&f, &[]).unwrap();
    let turn = make_turn(&f, r#""argument""#, "");
    let sort = f.heap().fresh_compiled_function(&RETURN, &[f.heap().undefined().unwrap()]).unwrap();
    own(&f, &f.heap().array_prototype(), "sort", sort);
    assert!(record(&f, &mut super::guard(1.0, &[]), &turn, &set).is_some());

    // Overridden every may retain its arrow. Captures are original Set edges;
    // invoke later after dropping the factory with the current runtime.
    let f = factory();
    let heap = f.heap().clone();
    let set = actual::create_exempt_set(&f, &["read".into()]).unwrap();
    let saved = heap.object().unwrap();
    let every = heap.fresh_compiled_function(&KEEP_ARROW, &[saved.clone()]).unwrap();
    own(&f, &heap.array_prototype(), "every", every);
    let turn = make_turn(&f, "{}", "");
    let prepared = actual::prepare_turn(&f, &turn, &set).unwrap();
    let callback = get(&f, &saved, "callback");
    let call = get(&f, &get(&f, &get(&f, &turn, "message"), "content"), "0");
    drop(prepared);
    drop(f);
    let result = heap.call(&callback, &heap.undefined().unwrap(), &[call], &NoRuntime).unwrap();
    assert!(matches!(heap.snapshot(&result).unwrap(), ValueSnapshot::Boolean(true)));

    // Shared prerequisite bodies: stable default UTF16 order, present
    // undefined before holes, post-sort hole deletion, and pop's final Get.
    let f = factory();
    let values = f.heap().array_with_length(4).unwrap();
    own(&f, &values, "0", f.heap().undefined().unwrap());
    own(&f, &values, "2", f.heap().string("b".into()).unwrap());
    own(&f, &values, "3", f.heap().string("a".into()).unwrap());
    let sort = get(&f, &values, "sort");
    f.heap().call(&sort, &values, &[], f.runtime()).unwrap();
    assert_eq!(string(&f, &get(&f, &values, "0")), "a".into());
    assert_eq!(string(&f, &get(&f, &values, "1")), "b".into());
    assert!(f.heap().own_descriptor(&values, &"2".into(), f.runtime()).unwrap().is_some());
    assert!(f.heap().own_descriptor(&values, &"3".into(), f.runtime()).unwrap().is_none());
    let popped = f.heap().call(&get(&f, &values, "pop"), &values, &[], f.runtime()).unwrap();
    assert!(matches!(f.heap().snapshot(&popped).unwrap(), ValueSnapshot::Undefined));
    assert!(matches!(f.heap().snapshot(&get(&f, &values, "length")).unwrap(), ValueSnapshot::Number(3.0)));

    // A prepare-stage original Throw occurs before commit and leaves the
    // previous hash/count available for the next real identical turn.
    let f = factory();
    let set = actual::create_exempt_set(&f, &[]).unwrap();
    let turn = make_turn(&f, r#"{"a":1}"#, "");
    let call = get(&f, &get(&f, &get(&f, &turn, "message"), "content"), "0");
    let args = get(&f, &call, "arguments");
    let mut detector = super::guard(2.0, &[]);
    assert!(record(&f, &mut detector, &turn, &set).is_none());
    let sentinel = f.heap().object().unwrap();
    accessor(&f, &args, "a", &THROW, &[sentinel.clone()]);
    match actual::prepare_turn(&f, &turn, &set) {
        Err(ValueError::Thrown(value)) => assert!(value.same_identity(&sentinel)),
        _ => panic!("Prepare Throw must retain the original sentinel"),
    }
    own(&f, &args, "a", f.heap().number(1.0).unwrap());
    assert!(record(&f, &mut detector, &turn, &set).is_some());
}
