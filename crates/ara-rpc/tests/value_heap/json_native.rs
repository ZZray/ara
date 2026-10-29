//! One fixed-Bun JSON43 observation body shared by existing RPC/CLI coarse
//! entrances. Controlled original operations; zero new test attributes.
use ara_rpc::{
    WireString,
    value_heap::{
        CapabilityId, CompiledFunctionForm, CompiledFunctionSpec, NoRuntime, OpaqueKind, OpaqueOperation,
        PropertyDescriptor, PropertyKey, RuntimeReply, RuntimeRequest, ValueError, ValueHandle, ValueHeap,
        ValueRuntime, ValueSnapshot,
    },
};
use std::{
    collections::BTreeMap,
    sync::{
        Arc, Mutex,
        atomic::{AtomicU64, Ordering},
    },
};
type Result<T> = std::result::Result<T, ValueError>;
static NEXT_CAP: AtomicU64 = AtomicU64::new(8_000_000);
const ORACLE: &str = include_str!("json-native-oracle.json");

enum Action {
    Return { event: &'static str, value: ValueHandle },
    KeyReturn { event: &'static str, value: ValueHandle },
    Throw { event: &'static str, value: ValueHandle },
    Replacer { root: ValueHandle, record: bool, omit: Vec<&'static str>, result: Option<ValueHandle> },
    Box { hint: bool, event: &'static str, primitive: ValueHandle, payload: ValueHandle, throws: bool },
    Array { prefix: &'static str, values: Vec<ValueHandle>, length: f64 },
}
struct FixtureRuntime {
    heap: Arc<ValueHeap>,
    actions: BTreeMap<u64, Action>,
    trace: Mutex<Vec<String>>,
}
impl FixtureRuntime {
    fn record(&self, event: &str) {
        if !event.is_empty() {
            self.trace.lock().unwrap().push(event.into());
        }
    }
    fn key(key: &PropertyKey) -> String {
        match key {
            PropertyKey::String(key) => key.to_utf8().unwrap(),
            _ => panic!("original string key required"),
        }
    }
}
impl ValueRuntime for FixtureRuntime {
    fn resolve(&self, request: RuntimeRequest<'_>) -> Result<RuntimeReply> {
        match request {
            RuntimeRequest::Call { capability, receiver, arguments } => {
                let action = self.actions.get(&capability.0).ok_or(ValueError::MissingCapability(capability))?;
                match action {
                    Action::Return { event, value } => {
                        self.record(event);
                        Ok(RuntimeReply::Value(value.clone()))
                    }
                    Action::KeyReturn { event, value } => {
                        assert_eq!(arguments.len(), 1);
                        let ValueSnapshot::String(key) = self.heap.snapshot(&arguments[0])? else {
                            panic!("toJSON string key")
                        };
                        self.record(&format!("{event}{}", key.to_utf8().unwrap()));
                        Ok(RuntimeReply::Value(value.clone()))
                    }
                    Action::Throw { event, value } => {
                        self.record(event);
                        Err(ValueError::Thrown(value.clone()))
                    }
                    Action::Replacer { root, record, omit, result } => {
                        assert_eq!(arguments.len(), 2);
                        let ValueSnapshot::String(key) = self.heap.snapshot(&arguments[0])? else {
                            panic!("replacer string key")
                        };
                        let key = key.to_utf8().unwrap();
                        if *record {
                            let identity = if key.is_empty() {
                                assert!(!receiver.same_identity(root));
                                assert!(self.heap.get(receiver, &"".into(), &NoRuntime)?.same_identity(root));
                                assert!(arguments[1].same_identity(root));
                                // A nested actual native encoder can allocate/reenter without an outer arena lock.
                                assert_eq!(
                                    self.heap.stringify_native(&self.heap.number(9.0)?, None, None, &NoRuntime)?,
                                    Some("9".into())
                                );
                                "wrapper"
                            } else {
                                assert!(receiver.same_identity(root));
                                "root"
                            };
                            self.record(&format!("replacer:{key}:{identity}"));
                        }
                        Ok(RuntimeReply::Value(if omit.contains(&key.as_str()) {
                            self.heap.undefined()?
                        } else {
                            result.as_ref().unwrap_or(&arguments[1]).clone()
                        }))
                    }
                    _ => Err(ValueError::WrongRuntimeReply("fixture callable")),
                }
            }
            RuntimeRequest::Opaque { capability, target, receiver, operation } => {
                assert!(target.same_identity(receiver));
                let action = self.actions.get(&capability.0).ok_or(ValueError::MissingCapability(capability))?;
                match (action, operation) {
                    (Action::Box { .. }, OpaqueOperation::Get(key)) if Self::key(key) == "toJSON" => {
                        Ok(RuntimeReply::Value(self.heap.undefined()?))
                    }
                    (
                        Action::Box { hint, event, primitive, throws, .. },
                        OpaqueOperation::ToPrimitive { string_hint },
                    ) => {
                        assert_eq!(*hint, string_hint, "original Number/String hint");
                        self.record(event);
                        if *throws {
                            Err(ValueError::Thrown(primitive.clone()))
                        } else {
                            Ok(RuntimeReply::Value(primitive.clone()))
                        }
                    }
                    (Action::Box { payload, .. }, OpaqueOperation::Unbox) => Ok(RuntimeReply::Value(payload.clone())),
                    (Action::Array { prefix, values, length }, OpaqueOperation::Get(key)) => {
                        let key = Self::key(key);
                        self.record(&format!("{prefix}:{key}"));
                        let value = if key == "length" {
                            self.heap.number(*length)?
                        } else if key == "toJSON" {
                            self.heap.undefined()?
                        } else {
                            key.parse::<usize>()
                                .ok()
                                .and_then(|n| values.get(n))
                                .cloned()
                                .unwrap_or(self.heap.undefined()?)
                        };
                        Ok(RuntimeReply::Value(value))
                    }
                    _ => Err(ValueError::UnsupportedOperation("unexpected JSON fixture opaque operation")),
                }
            }
            request => NoRuntime.resolve(request),
        }
    }
}
struct Builder {
    runtime: FixtureRuntime,
}
impl Builder {
    fn new(heap: &Arc<ValueHeap>) -> Self {
        Self { runtime: FixtureRuntime { heap: heap.clone(), actions: BTreeMap::new(), trace: Mutex::new(Vec::new()) } }
    }
    fn heap(&self) -> &Arc<ValueHeap> {
        &self.runtime.heap
    }
    fn num(&self, n: f64) -> ValueHandle {
        self.heap().number(n).unwrap()
    }
    fn text(&self, s: &str) -> ValueHandle {
        self.heap().string(s.into()).unwrap()
    }
    fn obj(&self) -> ValueHandle {
        self.heap().object().unwrap()
    }
    fn undef(&self) -> ValueHandle {
        self.heap().undefined().unwrap()
    }
    fn set(&self, object: &ValueHandle, key: &str, value: ValueHandle) {
        self.heap().define_own_data(object, key.into(), value, &NoRuntime).unwrap();
    }
    fn action(&mut self, action: Action, kind: Option<OpaqueKind>) -> ValueHandle {
        let id = NEXT_CAP.fetch_add(1, Ordering::Relaxed);
        self.runtime.actions.insert(id, action);
        match kind {
            Some(kind) => self.heap().opaque(kind, CapabilityId(id), id).unwrap(),
            None => self.heap().function(CapabilityId(id)).unwrap(),
        }
    }
    fn getter(&self, object: &ValueHandle, key: &str, function: &ValueHandle) {
        let ValueSnapshot::Function(get) = self.heap().snapshot(function).unwrap() else {
            panic!("original getter target")
        };
        self.heap()
            .define_own(
                object,
                key.into(),
                PropertyDescriptor::Accessor { get: Some(get), set: None, enumerable: true, configurable: true },
                &NoRuntime,
            )
            .unwrap();
    }
    fn boxed(
        &mut self,
        kind: OpaqueKind,
        hint: bool,
        event: &'static str,
        primitive: ValueHandle,
        payload: ValueHandle,
        throws: bool,
    ) -> ValueHandle {
        self.action(Action::Box { hint, event, primitive, payload, throws }, Some(kind))
    }
    fn replacer(
        &mut self,
        root: &ValueHandle,
        record: bool,
        omit: Vec<&'static str>,
        result: Option<ValueHandle>,
    ) -> ValueHandle {
        self.action(Action::Replacer { root: root.clone(), record, omit, result }, None)
    }
}
static REPLACER: CompiledFunctionSpec = CompiledFunctionSpec {
    source: "JSON43 original compiled replacer delegate",
    name: "replacer",
    length: 2,
    form: CompiledFunctionForm::Ordinary,
    capture_count: 1,
    body: |heap, captures, receiver, args, runtime| heap.call(&captures[0], receiver, args, runtime),
};
fn specimen(label: &str, b: &mut Builder) -> (Vec<ValueHandle>, Option<ValueHandle>) {
    let mut marker = None;
    let args = match label {
        "missing-value" => vec![],
        "undefined-before-space" => {
            let space = b.boxed(OpaqueKind::BoxedNumber, false, "space:number", b.num(2.0), b.num(2.0), false);
            vec![b.undef(), b.heap().null().unwrap(), space]
        }
        "noncallable-no-get" => {
            let root = b.obj();
            b.set(&root, "a", b.num(1.0));
            let r = b.obj();
            let f = b.action(Action::Throw { event: "BAD:replacer:length", value: b.num(1.0) }, None);
            b.getter(&r, "length", &f);
            let f = b.action(Action::Throw { event: "BAD:replacer:toJSON", value: b.num(1.0) }, None);
            b.getter(&r, "toJSON", &f);
            vec![root, r]
        }
        "callable-order" => {
            let root = b.obj();
            let child = b.obj();
            let boxed = b.boxed(OpaqueKind::BoxedNumber, false, "number:valueOf", b.num(6.0), b.num(5.0), false);
            let to_json = b.action(Action::KeyReturn { event: "toJSON:", value: boxed }, None);
            b.set(&child, "toJSON", to_json);
            let get = b.action(Action::Return { event: "get:a", value: child }, None);
            b.getter(&root, "a", &get);
            let original = b.replacer(&root, true, vec![], None);
            let compiled = b.heap().fresh_compiled_function(&REPLACER, &[original]).unwrap();
            vec![root, compiled]
        }
        "property-list-recursive" => {
            let proto = b.obj();
            b.set(&proto, "inherited", b.text("proto"));
            let root = b.heap().object_with_prototype(Some(&proto)).unwrap();
            b.set(&root, "a", b.num(1.0));
            let nested = b.obj();
            b.set(&nested, "a", b.num(2.0));
            b.set(&nested, "b", b.num(3.0));
            b.set(&root, "nested", nested);
            let first = b.obj();
            b.set(&first, "a", b.num(4.0));
            b.set(&first, "b", b.num(5.0));
            let items = b.heap().array(vec![first, b.num(6.0)]).unwrap();
            b.set(&root, "items", items);
            b.heap()
                .define_own(
                    &root,
                    "hidden".into(),
                    PropertyDescriptor::Data {
                        value: b.num(9.0),
                        writable: false,
                        enumerable: false,
                        configurable: false,
                    },
                    &NoRuntime,
                )
                .unwrap();
            let boxed_s = b.boxed(OpaqueKind::BoxedString, true, "", b.text("a"), b.text("a"), false);
            let boxed_n = b.boxed(OpaqueKind::BoxedNumber, true, "", b.num(0.0), b.num(0.0), false);
            let mut list = vec!["b", "a", "nested", "items", "inherited", "hidden", "a"]
                .into_iter()
                .map(|s| b.text(s))
                .collect::<Vec<_>>();
            list.extend([
                boxed_s,
                boxed_n,
                b.heap().boolean(true).unwrap(),
                b.heap().null().unwrap(),
                b.heap().bigint_decimal("1").unwrap(),
                b.heap().symbol(Some("x".into())).unwrap(),
            ]);
            vec![root, b.heap().array(list).unwrap()]
        }
        "property-list-effect-dedup" => {
            let root = b.obj();
            b.set(&root, "a", b.num(1.0));
            b.set(&root, "b", b.num(2.0));
            let s = b.boxed(OpaqueKind::BoxedString, true, "box:string", b.text("a"), b.text("a"), false);
            let n = b.boxed(OpaqueKind::BoxedNumber, true, "box:number", b.text("a"), b.num(0.0), false);
            let list = b.heap().array(vec![s, n, b.text("a")]).unwrap();
            let get = b.action(Action::Return { event: "get:2", value: b.text("a") }, None);
            b.getter(&list, "2", &get);
            vec![root, list]
        }
        "property-list-numeric-names" => {
            let root = b.obj();
            for (k, n) in [("0", 1.0), ("NaN", 2.0), ("Infinity", 3.0), ("-Infinity", 4.0)] {
                b.set(&root, k, b.num(n));
            }
            vec![
                root,
                b.heap()
                    .array(vec![b.num(-0.0), b.num(f64::NAN), b.num(f64::INFINITY), b.num(f64::NEG_INFINITY)])
                    .unwrap(),
            ]
        }
        "property-list-hole-inherited" => {
            let root = b.obj();
            b.set(&root, "a", b.num(1.0));
            b.set(&root, "b", b.num(2.0));
            let list = b.heap().array_with_length(3).unwrap();
            b.set(&list, "0", b.text("a"));
            b.set(&list, "2", b.text("a"));
            let proto = b.heap().object_with_prototype(Some(&b.heap().array_prototype())).unwrap();
            let f = b.action(Action::Return { event: "inherited:list:1", value: b.text("b") }, None);
            b.getter(&proto, "1", &f);
            b.heap().set_prototype(&list, Some(&proto), &NoRuntime).unwrap();
            vec![root, list]
        }
        "property-list-array-proxy-length" => {
            let root = b.obj();
            b.set(&root, "a", b.num(1.0));
            b.set(&root, "b", b.num(2.0));
            let list = b.action(
                Action::Array { prefix: "list", values: vec![b.text("a"), b.text("b")], length: 2.9 },
                Some(OpaqueKind::Array),
            );
            vec![root, list]
        }
        "array-proxy-length" => vec![b.action(
            Action::Array { prefix: "array", values: vec![b.num(1.0), b.num(2.0), b.num(3.0)], length: 2.9 },
            Some(OpaqueKind::Array),
        )],
        label if label.starts_with("space-") => {
            let root = b.obj();
            let name = &label[6..];
            let value = if [
                "fraction",
                "negative",
                "nan",
                "infinity",
                "negative-infinity",
                "string-long",
                "string-lone",
                "surrogate-cut",
            ]
            .contains(&name)
            {
                b.heap().array(vec![b.num(1.0)]).unwrap()
            } else {
                b.num(1.0)
            };
            b.set(&root, "a", value);
            let mut replacer = b.heap().null().unwrap();
            let space = match name {
                "fraction" => b.num(2.9),
                "negative" => b.num(-2.0),
                "nan" => b.num(f64::NAN),
                "infinity" => b.num(f64::INFINITY),
                "negative-infinity" => b.num(f64::NEG_INFINITY),
                "string-long" => b.text("abcdefghijk"),
                "string-lone" => b.heap().string(WireString::from_units(vec![0xd800])).unwrap(),
                "surrogate-cut" => {
                    let mut u = "123456789".encode_utf16().collect::<Vec<_>>();
                    u.extend([0xd83d, 0xde00]);
                    b.heap().string(WireString::from_units(u)).unwrap()
                }
                "number-box" => {
                    b.boxed(OpaqueKind::BoxedNumber, false, "space:number", b.text("0x2"), b.num(4.0), false)
                }
                "string-box" => {
                    b.boxed(OpaqueKind::BoxedString, true, "space:string", b.text("--"), b.text("x"), false)
                }
                "ignored-boxes" => b.boxed(
                    OpaqueKind::BoxedBoolean,
                    false,
                    "BAD:boolean",
                    b.heap().boolean(false).unwrap(),
                    b.heap().boolean(true).unwrap(),
                    false,
                ),
                "before-value-order" => {
                    let original = root.clone();
                    let f = b.action(Action::Return { event: "value:toJSON", value: original }, None);
                    let outer = b.obj();
                    b.set(&outer, "toJSON", f);
                    let key = b.boxed(OpaqueKind::BoxedString, true, "list:string", b.text("a"), b.text("a"), false);
                    replacer = b.heap().array(vec![key]).unwrap();
                    let space = b.boxed(OpaqueKind::BoxedNumber, false, "space:number", b.num(1.0), b.num(1.0), false);
                    return (vec![outer, replacer, space], None);
                }
                _ => panic!("unknown space case {name}"),
            };
            vec![root, replacer, space]
        }
        "box-number-overridden" => {
            vec![b.boxed(OpaqueKind::BoxedNumber, false, "number:valueOf", b.num(7.0), b.num(1.0), false)]
        }
        "box-string-overridden" => {
            vec![b.boxed(OpaqueKind::BoxedString, true, "string:toString", b.text("y"), b.text("x"), false)]
        }
        "box-boolean-ignores-conversion" => vec![b.boxed(
            OpaqueKind::BoxedBoolean,
            false,
            "BAD:boolean",
            b.heap().boolean(false).unwrap(),
            b.heap().boolean(true).unwrap(),
            false,
        )],
        "box-number-large-hex" => vec![b.boxed(
            OpaqueKind::BoxedNumber,
            false,
            "number:valueOf",
            b.text("0x1000000000000081"),
            b.num(0.0),
            false,
        )],
        "box-number-symbol-error" => vec![b.boxed(
            OpaqueKind::BoxedNumber,
            false,
            "",
            b.heap().symbol(Some("n".into())).unwrap(),
            b.num(0.0),
            false,
        )],
        "box-number-bigint-error" => {
            vec![b.boxed(OpaqueKind::BoxedNumber, false, "", b.heap().bigint_decimal("1").unwrap(), b.num(0.0), false)]
        }
        "box-string-symbol-error" => vec![b.boxed(
            OpaqueKind::BoxedString,
            true,
            "",
            b.heap().symbol(Some("s".into())).unwrap(),
            b.text("x"),
            false,
        )],
        "bigint-direct-error" => vec![b.heap().bigint_decimal("1").unwrap()],
        "bigint-box-error" => vec![b.boxed(
            OpaqueKind::BoxedBigInt,
            false,
            "BAD:bigint",
            b.heap().bigint_decimal("1").unwrap(),
            b.heap().bigint_decimal("1").unwrap(),
            false,
        )],
        "bigint-toJSON-error" => {
            let root = b.obj();
            let f = b.action(Action::Return { event: "", value: b.heap().bigint_decimal("1").unwrap() }, None);
            b.set(&root, "toJSON", f);
            vec![root]
        }
        "bigint-replacer-error" => {
            let root = b.heap().null().unwrap();
            let f = b.replacer(&root, false, vec![], Some(b.heap().bigint_decimal("1").unwrap()));
            vec![root, f]
        }
        "root-omission" => {
            let root = b.num(1.0);
            let f = b.action(
                Action::Replacer { root: root.clone(), record: false, omit: vec![""], result: None },
                Some(OpaqueKind::Function),
            );
            vec![root, f]
        }
        "object-array-omission" => {
            let root = b.obj();
            b.set(&root, "a", b.num(1.0));
            b.set(&root, "items", b.heap().array(vec![b.num(1.0), b.num(2.0)]).unwrap());
            let f = b.replacer(&root, false, vec!["a", "0"], None);
            vec![root, f]
        }
        "cycle-error" => {
            let root = b.obj();
            b.set(&root, "self", root.clone());
            vec![root]
        }
        "cycle-property-list-removed" => {
            let root = b.obj();
            b.set(&root, "a", b.num(1.0));
            b.set(&root, "self", root.clone());
            vec![root, b.heap().array(vec![b.text("a")]).unwrap()]
        }
        "repeated-alias" => {
            let child = b.obj();
            b.set(&child, "a", b.num(1.0));
            let root = b.obj();
            b.set(&root, "x", child.clone());
            b.set(&root, "y", child);
            vec![root]
        }
        label if label.starts_with("original-throw-") => {
            let thrown = b.obj();
            marker = Some(thrown.clone());
            let root = b.obj();
            b.set(&root, "a", b.num(1.0));
            let mut replacer = b.undef();
            let mut space = b.undef();
            match &label[15..] {
                "get" => {
                    let f = b.action(Action::Throw { event: "throw:get", value: thrown }, None);
                    b.getter(&root, "a", &f)
                }
                "toJSON" => {
                    let f = b.action(Action::Throw { event: "throw:toJSON", value: thrown }, None);
                    b.set(&root, "toJSON", f)
                }
                "replacer" => replacer = b.action(Action::Throw { event: "throw:replacer", value: thrown }, None),
                "space" => space = b.boxed(OpaqueKind::BoxedNumber, false, "throw:space", thrown, b.num(1.0), true),
                "list" => {
                    let key = b.boxed(OpaqueKind::BoxedString, true, "throw:list", thrown, b.text("a"), true);
                    replacer = b.heap().array(vec![key]).unwrap()
                }
                _ => panic!("unknown throw case {label}"),
            }
            vec![root, replacer, space]
        }
        _ => panic!("unknown fixed JSON43 case {label}"),
    };
    (args, marker)
}
fn text(heap: &Arc<ValueHeap>, value: &ValueHandle) -> WireString {
    let ValueSnapshot::String(s) = heap.snapshot(value).unwrap() else { panic!("expected string") };
    s
}
/// The same observation body drives the direct Native seam and installed original
/// global. Native-origin prototype identity is additionally checked for the latter.
pub fn observe_with(
    heap: &Arc<ValueHeap>,
    type_prototype: Option<&ValueHandle>,
    mut invoke: impl FnMut(&[ValueHandle], &dyn ValueRuntime) -> Result<ValueHandle>,
) -> Vec<ValueHandle> {
    let oracle: serde_json::Value = serde_json::from_str(ORACLE).unwrap();
    assert_eq!(oracle["fixed_omp"], "596f2da7101178214aa27a753529d15e6b7ad91d");
    assert_eq!(oracle["bun_version"], "1.4.0");
    assert_eq!(oracle["bun_revision"], "34cbb9a40b4bd1bd767d134a7065e66c2432a676");
    let cases = oracle["cases"].as_array().unwrap();
    assert_eq!(cases.len(), 43);
    let mut receipts = Vec::new();
    for row in cases {
        let label = row["label"].as_str().unwrap();
        let mut builder = Builder::new(heap);
        let (args, marker) = specimen(label, &mut builder);
        let result = invoke(&args, &builder.runtime);
        if row["ok"].as_bool().unwrap() {
            let value = result.unwrap_or_else(|e| panic!("JSON43 {label}: {e:?}"));
            if row["undefined"].as_bool().unwrap() {
                assert!(matches!(heap.snapshot(&value).unwrap(), ValueSnapshot::Undefined), "JSON43 {label}");
            } else {
                let expected = row["output"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .map(|v| u16::try_from(v.as_u64().unwrap()).unwrap())
                    .collect::<Vec<_>>();
                assert_eq!(text(heap, &value).units(), expected, "JSON43 {label}");
            }
            receipts.push(value);
        } else {
            let error = result.expect_err("original JSON43 expected throw");
            if row["original_throw"].as_bool().unwrap() {
                assert!(
                    matches!(error,ValueError::Thrown(ref value) if value.same_identity(marker.as_ref().unwrap())),
                    "JSON43 {label} original thrown handle: {error:?}"
                );
            } else {
                let (name, message) = match error {
                    ValueError::Thrown(value) if type_prototype.is_some() => {
                        assert!(
                            heap.prototype(&value, &builder.runtime)
                                .unwrap()
                                .unwrap()
                                .same_identity(type_prototype.unwrap()),
                            "JSON43 {label} original realm TypeError"
                        );
                        let name =
                            text(heap, &heap.get(&value, &"name".into(), &builder.runtime).unwrap()).to_utf8().unwrap();
                        let message = text(heap, &heap.get(&value, &"message".into(), &builder.runtime).unwrap())
                            .to_utf8()
                            .unwrap();
                        receipts.push(value);
                        (name, message)
                    }
                    ValueError::TypeError(message) if type_prototype.is_none() => ("TypeError".into(), message),
                    other => panic!("JSON43 {label} wrong exception carrier {other:?}"),
                };
                assert_eq!(name, row["error"]["name"].as_str().unwrap(), "JSON43 {label}");
                assert_eq!(message, row["error"]["message"].as_str().unwrap(), "JSON43 {label}");
            }
        }
        let expected = row["effects"].as_array().unwrap().iter().map(|v| v.as_str().unwrap()).collect::<Vec<_>>();
        assert_eq!(builder.runtime.trace.lock().unwrap().as_slice(), expected, "JSON43 {label} ordered effects");
    }
    // Foreign arguments are a carrier invariant. Admission precedes any local source effects.
    let foreign = ValueHeap::new().unwrap().string("foreign".into()).unwrap();
    let mut b = Builder::new(heap);
    let root = b.obj();
    let f = b.action(Action::Throw { event: "BAD:foreign:root", value: b.obj() }, None);
    b.getter(&root, "a", &f);
    for args in
        [vec![foreign.clone()], vec![root.clone(), foreign.clone()], vec![root, heap.undefined().unwrap(), foreign]]
    {
        assert!(matches!(invoke(&args, &b.runtime), Err(ValueError::ForeignHeap)));
        assert!(b.runtime.trace.lock().unwrap().is_empty());
    }
    println!(
        "Native JSON.stringify same-Bun observation: 43 reference cases; original throws/prototypes/UTF16/effects and foreign admission"
    );
    receipts
}
pub fn observe_native_dispatch() {
    let heap = ValueHeap::new().unwrap();
    observe_with(&heap, None, |args, runtime| {
        let value = args.first().cloned().unwrap_or(heap.undefined()?);
        match heap.stringify_native(&value, args.get(1), args.get(2), runtime)? {
            Some(output) => heap.string(output),
            None => heap.undefined(),
        }
    });
}
