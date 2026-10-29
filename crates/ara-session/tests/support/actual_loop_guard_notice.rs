//! Compound draft attached to the existing notice corpus. No new test attrs.
use ara_ai::graph_values::GraphValueFactory;
use ara_prompt::actual::{NativePromptModule, prepare_module};
use ara_rpc::{
    WireString,
    value_heap::{
        CompiledFunctionForm, CompiledFunctionSpec, NoRuntime, PropertyDescriptor, ValueError, ValueHandle, ValueHeap,
        ValueSnapshot,
    },
};
use std::sync::Arc;

fn setup() -> (GraphValueFactory, NativePromptModule) {
    let factory = GraphValueFactory::new(ValueHeap::new().unwrap(), Arc::new(NoRuntime)).unwrap();
    let module = prepare_module(factory.heap(), factory.runtime()).unwrap();
    (factory, module)
}
fn get(f: &GraphValueFactory, root: &ValueHandle, key: &str) -> ValueHandle {
    f.heap().get(root, &key.into(), f.runtime()).unwrap()
}
fn own(f: &GraphValueFactory, root: &ValueHandle, key: &str, value: ValueHandle) {
    f.heap().define_own_data(root, key.into(), value, f.runtime()).unwrap();
}
fn text(f: &GraphValueFactory, root: &ValueHandle) -> WireString {
    match f.heap().snapshot(root).unwrap() {
        ValueSnapshot::String(value) => value,
        other => panic!("{other:?}"),
    }
}
fn make_detection(f: &GraphValueFactory) -> ValueHandle {
    let root = f.heap().object().unwrap();
    own(f, &root, "toolName", f.heap().string("read".into()).unwrap());
    own(f, &root, "count", f.heap().number(2.0).unwrap());
    own(f, &root, "argumentsSummary", f.heap().string(r#"{"path":"x"}"#.into()).unwrap());
    own(f, &root, "resultSummary", f.heap().string("".into()).unwrap());
    root
}
fn call_notice(f: &GraphValueFactory, module: &NativePromptModule, detection: &ValueHandle) -> ValueHandle {
    ara_session::actual_loop_guard_notice::tool_call_loop(f, module, detection).unwrap()
}
fn accessor(
    f: &GraphValueFactory,
    root: &ValueHandle,
    key: &str,
    spec: &'static CompiledFunctionSpec,
    captures: &[ValueHandle],
) {
    let callback = f.heap().fresh_compiled_function(spec, captures).unwrap();
    let ValueSnapshot::Function(callback) = f.heap().snapshot(&callback).unwrap() else { unreachable!() };
    f.heap()
        .define_own(
            root,
            key.into(),
            PropertyDescriptor::Accessor { get: Some(callback), set: None, enumerable: true, configurable: true },
            f.runtime(),
        )
        .unwrap();
}
static LOG_GET: CompiledFunctionSpec = CompiledFunctionSpec {
    source: "fixture getter logs Get and returns original value",
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
static RETURN: CompiledFunctionSpec = CompiledFunctionSpec {
    source: "fixture returns original captured handle",
    name: "",
    length: 0,
    form: CompiledFunctionForm::Arrow,
    capture_count: 1,
    body: |_, captures, _, _, _| Ok(captures[0].clone()),
};
static FUNCTION_VALUE: CompiledFunctionSpec = CompiledFunctionSpec {
    source: "fixture function value observes exact context receiver",
    name: "tool_name",
    length: 0,
    form: CompiledFunctionForm::Ordinary,
    capture_count: 0,
    body: |heap, _, receiver, _, runtime| {
        let count = heap.get(receiver, &"count".into(), runtime)?;
        assert!(matches!(heap.snapshot(&count)?, ValueSnapshot::Number(2.0)));
        heap.string("function-name".into())
    },
};
static DEFAULT_HINT: CompiledFunctionSpec = CompiledFunctionSpec {
    source: "fixture observes original default hint on result +=",
    name: "",
    length: 1,
    form: CompiledFunctionForm::Arrow,
    capture_count: 0,
    body: |heap, _, _, args, _| {
        assert!(matches!(heap.snapshot(&args[0])?, ValueSnapshot::String(value) if value.equals_ascii("default")));
        heap.string("default-name".into())
    },
};
static MUTATE_RENDER: CompiledFunctionSpec = CompiledFunctionSpec {
    source: "fixture original render then mutates detection before details",
    name: "render",
    length: 1,
    form: CompiledFunctionForm::Ordinary,
    capture_count: 2,
    body: |heap, captures, receiver, args, runtime| {
        let content = heap.call(&captures[0], receiver, args, runtime)?;
        heap.define_own_data(
            &captures[1],
            "argumentsSummary".into(),
            heap.string("changed-after-render".into())?,
            runtime,
        )?;
        Ok(content)
    },
};
static NUMBER_HINT_LOG: CompiledFunctionSpec = CompiledFunctionSpec {
    source: "fixture logs each relational number-hint coercion",
    name: "",
    length: 1,
    form: CompiledFunctionForm::Arrow,
    capture_count: 1,
    body: |heap, captures, _, args, runtime| {
        assert!(matches!(heap.snapshot(&args[0])?, ValueSnapshot::String(value) if value.equals_ascii("number")));
        let push = heap.get(&captures[0], &"push".into(), runtime)?;
        heap.call(&push, &captures[0], &[args[0].clone()], runtime)?;
        heap.number(65.0)
    },
};
static STRING_HINT_LOG: CompiledFunctionSpec = CompiledFunctionSpec {
    source: "fixture logs default-hint arithmetic without flattening cursor",
    name: "",
    length: 1,
    form: CompiledFunctionForm::Arrow,
    capture_count: 3,
    body: |heap, captures, _, args, runtime| {
        assert!(matches!(heap.snapshot(&args[0])?, ValueSnapshot::String(value) if value.equals_ascii("default")));
        let push = heap.get(&captures[0], &"push".into(), runtime)?;
        heap.call(&push, &captures[0], &[captures[1].clone()], runtime)?;
        Ok(captures[2].clone())
    },
};
static COMMENT_INDEX: CompiledFunctionSpec = CompiledFunctionSpec {
    source: "fixture original indexOf returns original numeric-like objects",
    name: "indexOf",
    length: 1,
    form: CompiledFunctionForm::Builtin,
    capture_count: 3,
    body: |heap, captures, _, args, runtime| {
        if matches!(heap.snapshot(&args[0])?, ValueSnapshot::String(value) if value.equals_ascii("<!--")) {
            let seen = heap.get(&captures[2], &"seen".into(), runtime)?;
            if heap.truthy(&seen)? {
                return heap.number(-1.0);
            }
            heap.define_own_data(&captures[2], "seen".into(), heap.boolean(true)?, runtime)?;
            Ok(captures[0].clone())
        } else {
            assert!(matches!(heap.snapshot(&args[1])?, ValueSnapshot::String(value) if value.equals_ascii("54")));
            Ok(captures[1].clone())
        }
    },
};
static COMMENT_SLICE: CompiledFunctionSpec = CompiledFunctionSpec {
    source: "fixture original slice observes String cursor and end identities",
    name: "slice",
    length: 2,
    form: CompiledFunctionForm::Builtin,
    capture_count: 2,
    body: |heap, captures, _, args, runtime| {
        if args.first().is_some_and(|value| value.same_identity(&captures[1])) {
            assert!(matches!(heap.snapshot(&args[1])?, ValueSnapshot::String(value) if value.equals_ascii("83")));
        }
        if args.len() == 1 {
            assert!(matches!(heap.snapshot(&args[0])?, ValueSnapshot::String(value) if value.equals_ascii("83")));
        }
        let push = heap.get(&captures[0], &"push".into(), runtime)?;
        heap.call(&push, &captures[0], &[heap.string("slice".into())?], runtime)?;
        heap.string("".into())
    },
};
static PREFIX_THROW: CompiledFunctionSpec = CompiledFunctionSpec {
    source: "fixture first table substitution throws before second expression",
    name: "",
    length: 1,
    form: CompiledFunctionForm::Arrow,
    capture_count: 1,
    body: |heap, captures, _, args, _| {
        assert!(matches!(heap.snapshot(&args[0])?, ValueSnapshot::String(value) if value.equals_ascii("string")));
        Err(ValueError::Thrown(captures[0].clone()))
    },
};
static TABLE_SPLIT_OBSERVE: CompiledFunctionSpec = CompiledFunctionSpec {
    source: "fixture table split observes whether the second expression ran",
    name: "split",
    length: 1,
    form: CompiledFunctionForm::Arrow,
    capture_count: 2,
    body: |heap, captures, _, args, runtime| {
        assert!(matches!(heap.snapshot(&args[0])?, ValueSnapshot::String(value) if value.equals_ascii("|")));
        let push = heap.get(&captures[0], &"push".into(), runtime)?;
        heap.call(&push, &captures[0], &[heap.string("table-split".into())?], runtime)?;
        Ok(captures[1].clone())
    },
};

pub fn observe_native_notice_body() {
    // Executable default render/format/clock, compared with the exact trusted
    // template wording; content and details are Graph values throughout.
    let (f, module) = setup();
    let detection = make_detection(&f);
    let notice = call_notice(&f, &module, &detection);
    let content = text(&f, &get(&f, &notice, "content"));
    assert_eq!(
        content.to_utf8().unwrap(),
        "<system-interrupt reason=\"tool_call_loop_detected\">\nYou called `read` 2 consecutive times with identical arguments:\n`{\"path\":\"x\"}`\n\nLast result (truncated): `(no text result)`\n\nNEVER call `read` with those arguments again this turn. Use different arguments, choose another tool, or summarize findings and yield if complete.\n</system-interrupt>"
    );
    assert!(matches!(f.heap().snapshot(&get(&f, &notice, "display")).unwrap(), ValueSnapshot::Boolean(false)));
    assert!(
        matches!(f.heap().snapshot(&get(&f, &notice, "timestamp")).unwrap(), ValueSnapshot::Number(value) if value > 0.0)
    );
    assert_eq!(text(&f, &get(&f, &get(&f, &notice, "details"), "resultSummary")), "".into());

    // Context fields are evaluated once before render and again for details.
    // Date.now's Get occurs last; original clock result is copied by identity.
    let log = f.heap().array(vec![]).unwrap();
    for key in ["toolName", "count", "argumentsSummary", "resultSummary"] {
        let value = get(&f, &detection, key);
        accessor(&f, &detection, key, &LOG_GET, &[log.clone(), f.heap().string(key.into()).unwrap(), value]);
    }
    let timestamp = f.heap().number(123.0).unwrap();
    let now = f.heap().fresh_compiled_function(&RETURN, &[timestamp.clone()]).unwrap();
    accessor(&f, module.date(), "now", &LOG_GET, &[log.clone(), f.heap().string("Date.now".into()).unwrap(), now]);
    let notice = call_notice(&f, &module, &detection);
    assert!(get(&f, &notice, "timestamp").same_identity(&timestamp));
    assert_eq!(f.heap().stringify(&log, ara_rpc::value_heap::StringifyMode::Plain, None, f.runtime()).unwrap().unwrap(),
        r#"["toolName","count","argumentsSummary","resultSummary","toolName","count","argumentsSummary","resultSummary","Date.now"]"#.into());

    // Original Function.call dispatch for function-valued template properties.
    let (f, module) = setup();
    let detection = make_detection(&f);
    let function = f.heap().fresh_compiled_function(&FUNCTION_VALUE, &[]).unwrap();
    own(&f, &detection, "toolName", function);
    let notice = call_notice(&f, &module, &detection);
    assert!(text(&f, &get(&f, &notice, "content")).to_utf8().unwrap().contains("`function-name`"));

    // Real private brand survives actual rendering; a prototype forgery is
    // instanceof-positive but fails the private-field receiver brand check.
    let (f, module) = setup();
    let detection = make_detection(&f);
    let safe = module.safe_string(f.heap(), &f.heap().string("<unsafe&raw>".into()).unwrap(), f.runtime()).unwrap();
    own(&f, &detection, "toolName", safe.clone());
    let notice = call_notice(&f, &module, &detection);
    assert!(text(&f, &get(&f, &notice, "content")).to_utf8().unwrap().contains("`<unsafe&raw>`"));
    let fake = f.heap().object().unwrap();
    f.heap().set_prototype(&fake, Some(&get(&f, module.safe_string_constructor(), "prototype")), f.runtime()).unwrap();
    own(&f, &detection, "toolName", fake);
    assert!(matches!(
        ara_session::actual_loop_guard_notice::tool_call_loop(&f, &module, &detection),
        Err(ValueError::TypeError(_))
    ));

    // SafeString.toString override returns an object. Source += must perform
    // default ToPrimitive, independently of explicit String coercion.
    let coercible = f.heap().object().unwrap();
    let primitive = f.heap().fresh_compiled_function(&DEFAULT_HINT, &[]).unwrap();
    f.heap().define_own_data(&coercible, f.heap().to_primitive_key(), primitive, f.runtime()).unwrap();
    let returning = f.heap().fresh_compiled_function(&RETURN, &[coercible]).unwrap();
    own(&f, &safe, "toString", returning);
    own(&f, &detection, "toolName", safe);
    let notice = call_notice(&f, &module, &detection);
    assert!(text(&f, &get(&f, &notice, "content")).to_utf8().unwrap().contains("`default-name`"));

    // Render can mutate detection. Details are read after the call; do not
    // snapshot/serialize detection before original render consumes it.
    let (f, module) = setup();
    let detection = make_detection(&f);
    let original = get(&f, module.prompt(), "render");
    let wrapped = f.heap().fresh_compiled_function(&MUTATE_RENDER, &[original, detection.clone()]).unwrap();
    own(&f, module.prompt(), "render", wrapped);
    let notice = call_notice(&f, &module, &detection);
    assert!(text(&f, &get(&f, &notice, "content")).to_utf8().unwrap().contains("`{\"path\":\"x\"}`"));
    assert_eq!(text(&f, &get(&f, &get(&f, &notice, "details"), "argumentsSummary")), "changed-after-render".into());

    // Complex post-render formatter branches can arise inside toolName. A
    // lone surrogate remains an exact UTF16 unit through the whole notice.
    let (f, module) = setup();
    let detection = make_detection(&f);
    let name = "x`\n | a | b | \n | :-- | --: |\n```\n | keep | padding |\n```\n\n\n";
    own(&f, &detection, "toolName", f.heap().string(name.into()).unwrap());
    let notice = call_notice(&f, &module, &detection);
    let content = text(&f, &get(&f, &notice, "content")).to_utf8().unwrap();
    assert!(content.contains(" |a|b|"));
    assert!(content.contains(" |:---|---:|"));
    assert!(content.contains(" | keep | padding |"));
    let units = WireString::from_units(vec![0xd83c, 0x2026]);
    own(&f, &detection, "toolName", f.heap().string("read".into()).unwrap());
    own(&f, &detection, "resultSummary", f.heap().string(units.clone()).unwrap());
    let notice = call_notice(&f, &module, &detection);
    assert_eq!(text(&f, &get(&f, &get(&f, &notice, "details"), "resultSummary")), units);
    assert!(text(&f, &get(&f, &notice, "content")).units().windows(2).any(|units| units == [0xd83c, 0x2026]));

    // The same executable formatter body covers enabled optional branches,
    // comments, inline-code RFC2119 exclusions and pre-render closing blanks.
    let options = f.heap().object().unwrap();
    own(&f, &options, "renderPhase", f.heap().string("pre-render".into()).unwrap());
    own(&f, &options, "replaceAsciiSymbols", f.heap().boolean(true).unwrap());
    own(&f, &options, "normalizeRfc2119", f.heap().boolean(true).unwrap());
    let input = f
        .heap()
        .string("MUST NOT use -> ...\n`SHOULD NOT ->` SHOULD NOT wait\n<!-- ->\n... --> ->\n\n{{/if}}\n".into())
        .unwrap();
    let formatted =
        f.heap().call(&get(&f, module.prompt(), "format"), module.prompt(), &[input, options], f.runtime()).unwrap();
    assert_eq!(text(&f, &formatted), "NEVER use → …\n`SHOULD NOT →` AVOID wait\n<!-- ->\n... --> →\n{{/if}}".into());

    // Overridden split/charCodeAt return original handles. Relational BigInt
    // is accepted, and an object is coerced at all three source comparisons.
    let (f, module) = setup();
    let raw = f.heap().object().unwrap();
    own(&f, &raw, "length", f.heap().number(1.0).unwrap());
    let kept = f.heap().string("kept".into()).unwrap();
    own(&f, &raw, "toString", f.heap().fresh_compiled_function(&RETURN, &[kept]).unwrap());
    own(
        &f,
        &raw,
        "charCodeAt",
        f.heap().fresh_compiled_function(&RETURN, &[f.heap().bigint_decimal("65").unwrap()]).unwrap(),
    );
    let lines = f.heap().array(vec![raw.clone()]).unwrap();
    let input = f.heap().object().unwrap();
    own(&f, &input, "split", f.heap().fresh_compiled_function(&RETURN, &[lines]).unwrap());
    let format = get(&f, module.prompt(), "format");
    let options = f.heap().object().unwrap();
    let result = f.heap().call(&format, module.prompt(), &[input.clone(), options.clone()], f.runtime()).unwrap();
    assert_eq!(text(&f, &result), "kept".into());
    let log = f.heap().array(vec![]).unwrap();
    let code = f.heap().object().unwrap();
    own(
        &f,
        &code,
        "toString",
        f.heap().fresh_compiled_function(&RETURN, &[f.heap().string("65".into()).unwrap()]).unwrap(),
    );
    f.heap()
        .define_own_data(
            &code,
            f.heap().to_primitive_key(),
            f.heap().fresh_compiled_function(&NUMBER_HINT_LOG, &[log.clone()]).unwrap(),
            f.runtime(),
        )
        .unwrap();
    own(&f, &raw, "charCodeAt", f.heap().fresh_compiled_function(&RETURN, &[code]).unwrap());
    f.heap().call(&format, module.prompt(), &[input, options], f.runtime()).unwrap();
    assert!(matches!(f.heap().snapshot(&get(&f, &log, "length")).unwrap(), ValueSnapshot::Number(3.0)));

    // closeIndex + 3 executes separately for slice and cursor assignment.
    // The second source iteration passes String("83") to slice unchanged.
    let (f, module) = setup();
    let raw = f.heap().object().unwrap();
    own(&f, &raw, "length", f.heap().number(100.0).unwrap());
    own(&f, &raw, "charCodeAt", f.heap().fresh_compiled_function(&RETURN, &[f.heap().number(65.0).unwrap()]).unwrap());
    own(&f, &raw, "includes", f.heap().fresh_compiled_function(&RETURN, &[f.heap().boolean(true).unwrap()]).unwrap());
    let log = f.heap().array(vec![]).unwrap();
    let open = f.heap().object().unwrap();
    let close = f.heap().object().unwrap();
    for (root, label, value) in [(&open, "open+4", "5"), (&close, "close+3", "8")] {
        let function = f
            .heap()
            .fresh_compiled_function(
                &STRING_HINT_LOG,
                &[log.clone(), f.heap().string(label.into()).unwrap(), f.heap().string(value.into()).unwrap()],
            )
            .unwrap();
        f.heap().define_own_data(root, f.heap().to_primitive_key(), function, f.runtime()).unwrap();
    }
    own(
        &f,
        &raw,
        "indexOf",
        f.heap().fresh_compiled_function(&COMMENT_INDEX, &[open.clone(), close, f.heap().object().unwrap()]).unwrap(),
    );
    own(&f, &raw, "slice", f.heap().fresh_compiled_function(&COMMENT_SLICE, &[log.clone(), open]).unwrap());
    let input = f.heap().object().unwrap();
    own(&f, &input, "split", f.heap().fresh_compiled_function(&RETURN, &[f.heap().array(vec![raw]).unwrap()]).unwrap());
    let options = f.heap().object().unwrap();
    own(&f, &options, "replaceAsciiSymbols", f.heap().boolean(true).unwrap());
    f.heap().call(&get(&f, module.prompt(), "format"), module.prompt(), &[input, options], f.runtime()).unwrap();
    assert_eq!(
        f.heap().stringify(&log, ara_rpc::value_heap::StringifyMode::Plain, None, f.runtime()).unwrap().unwrap(),
        r#"["slice","open+4","close+3","slice","close+3","slice"]"#.into()
    );

    // LG-F1: each first template substitution is implicitly converted before
    // evaluating the table row/separator expression. A prefix Throw retains
    // its original sentinel and permits no table split callback side effect.
    for table_line in ["|a|", "|---|"] {
        let (f, module) = setup();
        let sentinel = f.heap().object().unwrap();
        let prefix = f.heap().object().unwrap();
        let throwing = f.heap().fresh_compiled_function(&PREFIX_THROW, &[sentinel.clone()]).unwrap();
        f.heap().define_own_data(&prefix, f.heap().to_primitive_key(), throwing, f.runtime()).unwrap();
        let raw = f.heap().object().unwrap();
        own(&f, &raw, "length", f.heap().number(table_line.len() as f64).unwrap());
        own(
            &f,
            &raw,
            "charCodeAt",
            f.heap().fresh_compiled_function(&RETURN, &[f.heap().number(124.0).unwrap()]).unwrap(),
        );
        own(
            &f,
            &raw,
            "toString",
            f.heap().fresh_compiled_function(&RETURN, &[f.heap().string(table_line.into()).unwrap()]).unwrap(),
        );
        own(&f, &raw, "slice", f.heap().fresh_compiled_function(&RETURN, &[prefix]).unwrap());
        let log = f.heap().array(vec![]).unwrap();
        let cells = f
            .heap()
            .array(vec![
                f.heap().string("".into()).unwrap(),
                f.heap().string("a".into()).unwrap(),
                f.heap().string("".into()).unwrap(),
            ])
            .unwrap();
        own(&f, &raw, "split", f.heap().fresh_compiled_function(&TABLE_SPLIT_OBSERVE, &[log.clone(), cells]).unwrap());
        let input = f.heap().object().unwrap();
        own(
            &f,
            &input,
            "split",
            f.heap().fresh_compiled_function(&RETURN, &[f.heap().array(vec![raw]).unwrap()]).unwrap(),
        );
        let result = f.heap().call(
            &get(&f, module.prompt(), "format"),
            module.prompt(),
            &[input, f.heap().object().unwrap()],
            f.runtime(),
        );
        assert!(matches!(result, Err(ValueError::Thrown(value)) if value.same_identity(&sentinel)));
        assert!(matches!(f.heap().snapshot(&get(&f, &log, "length")).unwrap(), ValueSnapshot::Number(0.0)));
    }
}
