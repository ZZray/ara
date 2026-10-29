//! Coarse admission/lifetime/reentry observations reached by the existing carrier family.
use ara_rpc::value_heap::{
    CallableTarget, CapabilityId, CompiledFunctionForm, CompiledFunctionSpec, NoRuntime, PropertyDescriptor,
    RuntimeReply, RuntimeRequest, ValueError, ValueHandle, ValueHeap, ValueKind, ValueRuntime, ValueSnapshot,
};
use std::sync::{
    Arc, Mutex,
    atomic::{AtomicBool, Ordering},
};

fn captured_get(
    heap: &Arc<ValueHeap>,
    captures: &[ValueHandle],
    receiver: &ValueHandle,
    _: &[ValueHandle],
    runtime: &dyn ValueRuntime,
) -> Result<ValueHandle, ValueError> {
    assert_eq!(heap.kind(receiver)?, ValueKind::Undefined, "arrow lexical this");
    heap.get(&captures[0], &"value".into(), runtime)
}

fn ordinary(
    heap: &Arc<ValueHeap>,
    _: &[ValueHandle],
    receiver: &ValueHandle,
    arguments: &[ValueHandle],
    _: &dyn ValueRuntime,
) -> Result<ValueHandle, ValueError> {
    heap.adopt(arguments.first().unwrap_or(receiver))
}

fn unchecked_reply(
    heap: &Arc<ValueHeap>,
    captures: &[ValueHandle],
    receiver: &ValueHandle,
    arguments: &[ValueHandle],
    runtime: &dyn ValueRuntime,
) -> Result<ValueHandle, ValueError> {
    heap.define_own_data(&captures[0], "touched".into(), heap.boolean(true)?, runtime)?;
    match runtime.resolve(RuntimeRequest::Call { capability: CapabilityId(502), receiver, arguments })? {
        RuntimeReply::Value(value) => Ok(value),
        _ => Err(ValueError::WrongRuntimeReply("controlled body reply")),
    }
}

static CAPTURE: CompiledFunctionSpec = CompiledFunctionSpec {
    source: "carrier-fixture/captured-get",
    name: "",
    length: 1,
    form: CompiledFunctionForm::Arrow,
    capture_count: 1,
    body: captured_get,
};
static NAMED: CompiledFunctionSpec = CompiledFunctionSpec {
    source: "carrier-fixture/ordinary",
    name: "ordinary",
    length: 1,
    form: CompiledFunctionForm::Ordinary,
    capture_count: 0,
    body: ordinary,
};
static COLLISION: CompiledFunctionSpec = CompiledFunctionSpec {
    source: "carrier-fixture/another-ordinary",
    name: "ordinary",
    length: 1,
    form: CompiledFunctionForm::Ordinary,
    capture_count: 0,
    body: ordinary,
};
static RAW: CompiledFunctionSpec = CompiledFunctionSpec {
    source: "carrier-fixture/unchecked-reply",
    name: "",
    length: 1,
    form: CompiledFunctionForm::Arrow,
    capture_count: 1,
    body: unchecked_reply,
};

struct Runtime {
    heap: Arc<ValueHeap>,
    saved: ValueHandle,
    payload: ValueHandle,
    foreign: ValueHandle,
    mode: u8,
    entered: AtomicBool,
    trace: Mutex<Vec<&'static str>>,
}
impl ValueRuntime for Runtime {
    fn resolve(&self, request: RuntimeRequest<'_>) -> Result<RuntimeReply, ValueError> {
        match request {
            RuntimeRequest::Call { capability: CapabilityId(501), .. } => {
                self.trace.lock().unwrap().push("getter");
                if !self.entered.swap(true, Ordering::SeqCst) {
                    let nested = self.heap.call(&self.saved, &self.heap.null()?, &[], self)?;
                    assert!(nested.same_identity(&self.payload));
                }
                Ok(RuntimeReply::Value(self.payload.clone()))
            }
            RuntimeRequest::Call { capability: CapabilityId(502), .. } => match self.mode {
                0 => Ok(RuntimeReply::Value(self.payload.clone())),
                1 => Err(ValueError::Thrown(self.payload.clone())),
                2 => Ok(RuntimeReply::Value(self.foreign.clone())),
                3 => Err(ValueError::Thrown(self.foreign.clone())),
                _ => unreachable!(),
            },
            _ => Err(ValueError::MissingCapability(request.capability())),
        }
    }
}

fn data(heap: &Arc<ValueHeap>, target: &ValueHandle, key: &str, value: ValueHandle) {
    heap.define_own_data(target, key.into(), value, &NoRuntime).unwrap();
}

pub(super) fn run() {
    let weak = {
        let heap = ValueHeap::new().unwrap();
        let weak = Arc::downgrade(&heap);
        let capture = heap.object().unwrap();
        data(&heap, &capture, "value", heap.number(1.0).unwrap());
        let first = heap.fresh_compiled_function(&CAPTURE, std::slice::from_ref(&capture)).unwrap();
        let second = heap.fresh_compiled_function(&CAPTURE, std::slice::from_ref(&capture)).unwrap();
        assert!(!first.same_identity(&second));
        assert!(!heap.has_own(&first, &"prototype".into(), &NoRuntime).unwrap());
        for key in ["name", "length"] {
            assert!(matches!(
                heap.own_descriptor(&first, &key.into(), &NoRuntime).unwrap(),
                Some(PropertyDescriptor::Data { writable: false, enumerable: false, configurable: true, .. })
            ));
        }
        let undefined = heap.undefined().unwrap();
        let changed = heap.number(9.0).unwrap();
        data(&heap, &capture, "value", changed.clone());
        assert!(heap.call(&first, &heap.object().unwrap(), &[], &NoRuntime).unwrap().same_identity(&changed));
        assert!(matches!(heap.fresh_compiled_function(&CAPTURE, &[]), Err(ValueError::WrongRuntimeReply(_))));
        assert!(matches!(
            heap.module_compiled_function("fixture", "bad", &CAPTURE),
            Err(ValueError::WrongRuntimeReply(_))
        ));

        let foreign_heap = ValueHeap::new().unwrap();
        let foreign_capture = foreign_heap.object().unwrap();
        assert!(matches!(
            heap.fresh_compiled_function(&CAPTURE, std::slice::from_ref(&foreign_capture)),
            Err(ValueError::ForeignHeap)
        ));
        let foreign_function = foreign_heap.fresh_compiled_function(&NAMED, &[]).unwrap();
        let ValueSnapshot::Function(foreign_target) = foreign_heap.snapshot(&foreign_function).unwrap() else {
            panic!()
        };
        assert!(matches!(foreign_target, CallableTarget::Compiled(_)));
        assert!(matches!(
            heap.define_own(
                &capture,
                "foreign".into(),
                PropertyDescriptor::Accessor {
                    get: Some(foreign_target),
                    set: None,
                    enumerable: true,
                    configurable: true,
                },
                &NoRuntime
            ),
            Err(ValueError::ForeignHeap)
        ));
        assert!(!heap.has_own(&capture, &"foreign".into(), &NoRuntime).unwrap());

        heap.define_own(
            &capture,
            "value".into(),
            PropertyDescriptor::Accessor {
                get: Some(CallableTarget::Host(CapabilityId(501))),
                set: None,
                enumerable: true,
                configurable: true,
            },
            &NoRuntime,
        )
        .unwrap();
        let old = Runtime {
            heap: heap.clone(),
            saved: first.clone(),
            payload: heap.symbol(Some("old".into())).unwrap(),
            foreign: foreign_capture.clone(),
            mode: 0,
            entered: AtomicBool::new(false),
            trace: Mutex::new(Vec::new()),
        };
        assert!(heap.call(&first, &undefined, &[], &old).unwrap().same_identity(&old.payload));
        assert_eq!(old.trace.lock().unwrap().as_slice(), &["getter", "getter"]);
        drop(old);
        let current = Runtime {
            heap: heap.clone(),
            saved: first.clone(),
            payload: heap.symbol(Some("current".into())).unwrap(),
            foreign: foreign_capture.clone(),
            mode: 0,
            entered: AtomicBool::new(false),
            trace: Mutex::new(Vec::new()),
        };
        assert!(heap.call(&first, &undefined, &[], &current).unwrap().same_identity(&current.payload));
        assert_eq!(current.trace.lock().unwrap().as_slice(), &["getter", "getter"]);
        drop(current);

        let raw = heap.fresh_compiled_function(&RAW, std::slice::from_ref(&capture)).unwrap();
        for mode in 0..4 {
            data(&heap, &capture, "touched", heap.boolean(false).unwrap());
            let runtime = Runtime {
                heap: heap.clone(),
                saved: raw.clone(),
                payload: heap.symbol(Some("original throw".into())).unwrap(),
                foreign: foreign_capture.clone(),
                mode,
                entered: AtomicBool::new(false),
                trace: Mutex::new(Vec::new()),
            };
            let result = heap.call(&raw, &undefined, &[], &runtime);
            match mode {
                0 => assert!(result.unwrap().same_identity(&runtime.payload)),
                1 => assert!(matches!(result, Err(ValueError::Thrown(value)) if value.same_identity(&runtime.payload))),
                _ => assert!(matches!(result, Err(ValueError::ForeignHeap))),
            }
            assert!(heap.truthy(&heap.get(&capture, &"touched".into(), &NoRuntime).unwrap()).unwrap());
        }

        let named = heap.module_compiled_function("fixture", "ordinary", &NAMED).unwrap();
        assert!(named.same_identity(&heap.module_compiled_function("fixture", "ordinary", &NAMED).unwrap()));
        assert!(matches!(
            heap.module_compiled_function("fixture", "ordinary", &COLLISION),
            Err(ValueError::SourceIdentityCollision)
        ));
        assert!(matches!(
            heap.own_descriptor(&named, &"prototype".into(), &NoRuntime).unwrap(),
            Some(PropertyDescriptor::Data { writable: true, enumerable: false, configurable: false, .. })
        ));
        let prototype = heap.get(&named, &"prototype".into(), &NoRuntime).unwrap();
        assert!(heap.get(&prototype, &"constructor".into(), &NoRuntime).unwrap().same_identity(&named));
        heap.define_own(
            &named,
            "name".into(),
            PropertyDescriptor::Data {
                value: heap.string("edited".into()).unwrap(),
                writable: false,
                enumerable: false,
                configurable: true,
            },
            &NoRuntime,
        )
        .unwrap();
        let repeated = heap.module_compiled_function("fixture", "ordinary", &NAMED).unwrap();
        assert!(
            matches!(heap.snapshot(&heap.get(&repeated, &"name".into(), &NoRuntime).unwrap()).unwrap(), ValueSnapshot::String(value) if value.equals_ascii("edited"))
        );

        let live_prototype = heap.object().unwrap();
        heap.assign(&named, "prototype".into(), live_prototype.clone(), &NoRuntime).unwrap();
        let primitive = heap.number(4.0).unwrap();
        let created = heap.construct_compiled_function(&named, std::slice::from_ref(&primitive), &NoRuntime).unwrap();
        assert!(heap.prototype(&created, &NoRuntime).unwrap().unwrap().same_identity(&live_prototype));
        assert!(heap.instance_of(&created, &named, &NoRuntime).unwrap());
        let returned = heap.object().unwrap();
        assert!(
            heap.construct_compiled_function(&named, std::slice::from_ref(&returned), &NoRuntime)
                .unwrap()
                .same_identity(&returned)
        );
        assert!(matches!(heap.construct_compiled_function(&first, &[], &NoRuntime), Err(ValueError::TypeError(_))));

        let input = heap.array(vec![heap.string("x".into()).unwrap()]).unwrap();
        let constructor = heap.object().unwrap();
        data(&heap, &input, "constructor", constructor.clone());
        heap.define_own_data(&constructor, heap.species_key(), named.clone(), &NoRuntime).unwrap();
        let sliced = heap.array_slice(&input, 0.0, None, &NoRuntime).unwrap();
        assert_eq!(heap.kind(&sliced).unwrap(), ValueKind::Object);
        assert!(heap.prototype(&sliced, &NoRuntime).unwrap().unwrap().same_identity(&live_prototype));
        assert!(
            matches!(heap.snapshot(&heap.get(&sliced, &"0".into(), &NoRuntime).unwrap()).unwrap(), ValueSnapshot::String(value) if value.equals_ascii("x"))
        );
        heap.assign(&constructor, heap.species_key(), first.clone(), &NoRuntime).unwrap();
        assert!(matches!(heap.array_slice(&input, 0.0, None, &NoRuntime), Err(ValueError::TypeError(_))));
        heap.assign(&named, "prototype".into(), primitive, &NoRuntime).unwrap();
        let fallback = heap.construct_compiled_function(&named, &[], &NoRuntime).unwrap();
        assert!(heap.prototype(&fallback, &NoRuntime).unwrap().unwrap().same_identity(&heap.object_prototype()));
        weak
    };
    assert!(weak.upgrade().is_none(), "compiled captures, named/prototype cycles and registries own no heap lease");
}
