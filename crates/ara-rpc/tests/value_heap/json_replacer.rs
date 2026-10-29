//! Callable JSON replacer observations attached to the existing JSON coarse family.
//! Controlled runtime only; no Native JSON binding or original logger acceptance.
use super::*;
use ara_rpc::value_heap::{CompiledFunctionForm, CompiledFunctionSpec};

struct ReplacerRuntime {
    heap: Arc<ValueHeap>,
    root: ValueHandle,
    child: ValueHandle,
    to_json: ValueHandle,
    items: ValueHandle,
    boxed: ValueHandle,
    boxed_bigint: ValueHandle,
    thrown: ValueHandle,
    unboxed: f64,
    trace: Mutex<Vec<String>>,
}
impl ReplacerRuntime {
    fn record(&self, event: &str) {
        self.trace.lock().unwrap().push(event.into());
    }
}
impl ValueRuntime for ReplacerRuntime {
    fn resolve(&self, request: RuntimeRequest<'_>) -> Result<RuntimeReply, ValueError> {
        match request {
            RuntimeRequest::Call { capability, receiver, arguments } => match capability.0 {
                9001 => {
                    assert!(receiver.same_identity(&self.root));
                    assert!(arguments.is_empty());
                    self.record("get:value");
                    Ok(RuntimeReply::Value(self.child.clone()))
                }
                9002 => {
                    assert!(receiver.same_identity(&self.child));
                    self.record("get:toJSON");
                    Ok(RuntimeReply::Value(self.to_json.clone()))
                }
                9003 => {
                    assert!(receiver.same_identity(&self.child));
                    assert_eq!(arguments.len(), 1);
                    assert!(
                        matches!(self.heap.snapshot(&arguments[0])?, ValueSnapshot::String(key) if key.equals_ascii("value"))
                    );
                    self.record("call:toJSON");
                    Ok(RuntimeReply::Value(self.boxed.clone()))
                }
                9004 => {
                    assert_eq!(arguments.len(), 2);
                    let ValueSnapshot::String(key) = self.heap.snapshot(&arguments[0])? else { panic!("string key") };
                    let key = key.to_utf8().unwrap();
                    self.record(&format!("replacer:{key}"));
                    let value = &arguments[1];
                    match key.as_str() {
                        "" => {
                            assert!(!receiver.same_identity(&self.root));
                            assert!(value.same_identity(&self.root));
                            assert!(self.heap.get(receiver, &"".into(), &NoRuntime)?.same_identity(value));
                            // Heap allocation and a nested serializer call must not encounter an outer arena lock.
                            assert_eq!(
                                self.heap.stringify(&self.heap.number(9.0)?, StringifyMode::Plain, None, &NoRuntime)?,
                                Some("9".into())
                            );
                        }
                        "value" => {
                            assert!(receiver.same_identity(&self.root));
                            assert!(value.same_identity(&self.boxed), "toJSON result reaches replacer before Unbox");
                        }
                        "items" => {
                            assert!(receiver.same_identity(&self.root));
                            assert!(value.same_identity(&self.items));
                        }
                        "0" | "1" | "2" => {
                            assert!(receiver.same_identity(&self.items));
                            if key == "1" {
                                assert!(matches!(self.heap.snapshot(value)?, ValueSnapshot::BigInt(_)));
                                return Ok(RuntimeReply::Value(self.heap.string("big".into())?));
                            }
                        }
                        "drop" => {
                            assert!(receiver.same_identity(&self.root));
                            return Ok(RuntimeReply::Value(self.heap.undefined()?));
                        }
                        _ => panic!("unexpected key {key}"),
                    }
                    Ok(RuntimeReply::Value(value.clone()))
                }
                9006 => {
                    self.record("get:throw");
                    Err(ValueError::Thrown(self.thrown.clone()))
                }
                9007 => {
                    self.record("call:throw");
                    Err(ValueError::Thrown(self.thrown.clone()))
                }
                9008 => Ok(RuntimeReply::Value(arguments[1].clone())),
                9009 => Ok(RuntimeReply::Value(self.heap.undefined()?)),
                9010 => Ok(RuntimeReply::Value(self.boxed_bigint.clone())),
                9012 => Ok(RuntimeReply::Value(self.heap.bigint_decimal("9007199254740993123456789")?)),
                _ => Err(ValueError::MissingCapability(capability)),
            },
            RuntimeRequest::Opaque { capability, target, receiver, operation: OpaqueOperation::Unbox } => {
                assert!(target.same_identity(receiver));
                match capability.0 {
                    9005 => {
                        assert!(target.same_identity(&self.boxed));
                        self.record("unbox:number");
                        Ok(RuntimeReply::Value(self.heap.number(self.unboxed)?))
                    }
                    9011 => {
                        assert!(target.same_identity(&self.boxed_bigint));
                        self.record("unbox:bigint");
                        Ok(RuntimeReply::Value(self.heap.bigint_decimal("9007199254740993123456789")?))
                    }
                    _ => Err(ValueError::MissingCapability(capability)),
                }
            }
            request => Err(ValueError::MissingCapability(request.capability())),
        }
    }
}
static REPLACER: CompiledFunctionSpec = CompiledFunctionSpec {
    source: "ara-rpc:test:original-json-replacer-call-contract",
    name: "replacer",
    length: 2,
    form: CompiledFunctionForm::Ordinary,
    capture_count: 1,
    body: invoke_replacer,
};
fn invoke_replacer(
    heap: &Arc<ValueHeap>,
    captures: &[ValueHandle],
    receiver: &ValueHandle,
    arguments: &[ValueHandle],
    runtime: &dyn ValueRuntime,
) -> Result<ValueHandle, ValueError> {
    heap.call(&captures[0], receiver, arguments, runtime)
}
fn output(
    heap: &Arc<ValueHeap>,
    value: &ValueHandle,
    replacer: &ValueHandle,
    runtime: &dyn ValueRuntime,
) -> Option<String> {
    heap.stringify_with_replacer(value, replacer, None, runtime).unwrap().map(|text| text.to_utf8().unwrap())
}
pub(super) fn observe_callable_replacer() {
    let heap = ValueHeap::new().unwrap();
    let root = heap.object().unwrap();
    let child = heap.object().unwrap();
    let items = heap
        .array(vec![
            heap.undefined().unwrap(),
            heap.bigint_decimal("9007199254740993123456789").unwrap(),
            heap.symbol(None).unwrap(),
        ])
        .unwrap();
    let mut runtime = ReplacerRuntime {
        heap: heap.clone(),
        root: root.clone(),
        child: child.clone(),
        to_json: heap.opaque(OpaqueKind::Function, CapabilityId(9003), 9003).unwrap(),
        items: items.clone(),
        boxed: heap.opaque(OpaqueKind::BoxedNumber, CapabilityId(9005), 9005).unwrap(),
        boxed_bigint: heap.opaque(OpaqueKind::BoxedBigInt, CapabilityId(9011), 9011).unwrap(),
        thrown: heap.object().unwrap(),
        unboxed: 5.0,
        trace: Mutex::new(Vec::new()),
    };
    heap.define_own(&root, "value".into(), accessor(Some(9001), None), &NoRuntime).unwrap();
    heap.define_own(&child, "toJSON".into(), accessor(Some(9002), None), &NoRuntime).unwrap();
    heap.define_own_data(&root, "items".into(), items, &NoRuntime).unwrap();
    heap.define_own_data(&root, "drop".into(), heap.boolean(true).unwrap(), &NoRuntime).unwrap();
    let function = heap.function(CapabilityId(9004)).unwrap();
    let compiled = heap.fresh_compiled_function(&REPLACER, &[function]).unwrap();
    assert_eq!(output(&heap, &root, &compiled, &runtime), Some("{\"value\":5,\"items\":[null,\"big\",null]}".into()));
    assert_eq!(
        runtime.trace.lock().unwrap().as_slice(),
        &[
            "replacer:",
            "get:value",
            "get:toJSON",
            "call:toJSON",
            "replacer:value",
            "unbox:number",
            "replacer:items",
            "replacer:0",
            "replacer:1",
            "replacer:2",
            "replacer:drop"
        ]
    );
    runtime.unboxed = 6.0;
    runtime.trace.lock().unwrap().clear();
    assert_eq!(
        output(&heap, &root, &compiled, &runtime),
        Some("{\"value\":6,\"items\":[null,\"big\",null]}".into()),
        "retained compiled replacer uses current effective runtime"
    );
    let opaque = heap.opaque(OpaqueKind::Function, CapabilityId(9004), 9004).unwrap();
    assert_eq!(
        output(&heap, &root, &opaque, &runtime),
        Some("{\"value\":6,\"items\":[null,\"big\",null]}".into()),
        "Opaque(Function) is callable too"
    );
    let identity = heap.function(CapabilityId(9008)).unwrap();
    let bigint = heap.bigint_decimal("9007199254740993123456789").unwrap();
    assert!(matches!(heap.stringify_with_replacer(&bigint, &identity, None, &runtime), Err(ValueError::TypeError(_))));
    let scalar = heap.null().unwrap();
    for id in [9010, 9012] {
        runtime.trace.lock().unwrap().clear();
        let replace = heap.function(CapabilityId(id)).unwrap();
        assert!(matches!(
            heap.stringify_with_replacer(&scalar, &replace, None, &runtime),
            Err(ValueError::TypeError(_))
        ));
        if id == 9010 {
            assert_eq!(runtime.trace.lock().unwrap().as_slice(), &["unbox:bigint"]);
        }
    }
    assert!(
        output(&heap, &scalar, &heap.function(CapabilityId(9009)).unwrap(), &runtime).is_none(),
        "root omission returns undefined result"
    );
    let bad = heap.object().unwrap();
    heap.define_own(&bad, "value".into(), accessor(Some(9006), None), &NoRuntime).unwrap();
    for (target, expected) in [(&identity, "get:throw"), (&heap.function(CapabilityId(9007)).unwrap(), "call:throw")] {
        runtime.trace.lock().unwrap().clear();
        assert!(
            matches!(heap.stringify_with_replacer(&bad, target, None, &runtime), Err(ValueError::Thrown(value)) if value.same_identity(&runtime.thrown))
        );
        assert_eq!(runtime.trace.lock().unwrap().as_slice(), &[expected]);
    }
    runtime.trace.lock().unwrap().clear();
    let foreign = ValueHeap::new().unwrap().function(CapabilityId(9004)).unwrap();
    assert!(matches!(heap.stringify_with_replacer(&bad, &foreign, None, &runtime), Err(ValueError::ForeignHeap)));
    assert!(matches!(
        heap.stringify_with_replacer(&bad, &heap.object().unwrap(), None, &runtime),
        Err(ValueError::TypeError(_))
    ));
    assert!(runtime.trace.lock().unwrap().is_empty(), "admission must precede source getters");
}
