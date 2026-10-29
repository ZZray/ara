//! One concrete value factory for fixed OMP final/streaming grammar and scanners.
//! Host supplies the Session lineage heap and a reentrant runtime resolver.
//! This factory performs no typed/serde approximation of graph values.

use crate::relaxed_json::FinalValueFactory;
use ara_rpc::value_heap::{
    PropertyKey, StringifyMode, ValueError, ValueHandle, ValueHeap, ValueKind, ValueRuntime, ValueSnapshot,
};
use ara_rpc::{WireString, WireValue};
use std::sync::Arc;

/// One fixed provider block Symbol shared by producer and Core consumers.
/// AI owns its key identity so portable AI never depends on the Agent crate.
pub fn cursor_exec_resolved_key(heap: &Arc<ValueHeap>) -> Result<PropertyKey, ValueError> {
    heap.module_symbol("ara-ai.block-symbols", "provider.block.cursorExecResolved")
}

/// Shared module-local identity for fixed block-symbols.ts kStreamingPartialJson.
pub fn streaming_partial_json_key(heap: &Arc<ValueHeap>) -> Result<PropertyKey, ValueError> {
    heap.module_symbol("ara-ai.block-symbols", "provider.block.partialJson")
}

/// Clone shares the same value lineage and module lookup receiver. The Host
/// injects one binding into all consumers in that lineage; this type neither
/// constructs a new heap per parse nor stores the resolver in heap nodes.
#[derive(Clone)]
pub struct GraphValueFactory {
    heap: Arc<ValueHeap>,
    runtime: Arc<dyn ValueRuntime>,
    ordinary_lookup_receiver: ValueHandle,
    ark_type: Option<ValueHandle>,
}

/// Operation-borrowed lineage and current invocation runtime. No module or
/// factory reconstruction, ownership roots, or imported Ark binding is stored.
#[derive(Clone, Copy)]
pub struct GraphExecution<'a> {
    heap: &'a Arc<ValueHeap>,
    runtime: &'a dyn ValueRuntime,
}
impl<'a> GraphExecution<'a> {
    pub fn new(heap: &'a Arc<ValueHeap>, runtime: &'a dyn ValueRuntime) -> Self {
        Self { heap, runtime }
    }
    pub fn from_factory(factory: &'a GraphValueFactory) -> Self {
        Self::new(factory.heap(), factory.runtime())
    }
    pub fn heap(self) -> &'a Arc<ValueHeap> {
        self.heap
    }
    pub fn runtime(self) -> &'a dyn ValueRuntime {
        self.runtime
    }
}

impl GraphValueFactory {
    /// The lexer handles its fixed own non-recoverable literal keys directly.
    /// Keep one ordinary receiver for its inherited lookup. Constructing it
    /// here avoids introducing a new allocation at every bareword lookup.
    pub fn new(heap: Arc<ValueHeap>, runtime: Arc<dyn ValueRuntime>) -> Result<Self, ValueError> {
        // This module's literal Sets are prepared once before the factory is
        // published to normalizers or author hooks. Failed initialization is
        // retained by node identity, never silently retried on this heap.
        const IDENTIFIER_KEYS: &[&str] =
            &["path", "paths", "file", "file_path", "filePath", "filepath", "url", "uri", "title", "label"];
        const CONTENT_KEYS: &[&str] = &["content", "input", "body", "text", "command", "code"];
        const VALIDATION_SETS: &[(&str, &[&str])] =
            &[("IDENTIFIER_STRING_KEYS", IDENTIFIER_KEYS), ("CONTENT_CARRYING_KEYS", CONTENT_KEYS)];
        heap.module_regexp(
            "ara-ai.validation",
            "JSON_NUMBER_PATTERN",
            r"^[+-]?(?:0|[1-9]\d*)(?:\.\d+)?(?:[eE][+-]?\d+)?$",
            "",
        )?;
        heap.module_regexp(
            "ara-ai.validation",
            "NUMERIC_STRING_PATTERN",
            r"^[+-]?\d+(?:\.\d+)?(?:[eE][+-]?\d+)?$",
            "",
        )?;
        heap.module_string_array("ara-ai.validation", "BRACKET_CHARS", &["[", "]", "{", "}"])?;
        heap.prepare_module_string_sets("ara-ai.validation", VALIDATION_SETS, runtime.as_ref())?;
        heap.module_regexp("ara-ai.validation", "TRAILING_LINE_TERMINATOR_RE", r"[\r\n]+$", "")?;
        heap.module_regexp("ara-ai.validation", "FLATTENED_IDENT_RE", r"^[A-Za-z_$][A-Za-z0-9_$]*", "")?;
        heap.module_regexp("ara-ai.validation", "FLATTENED_INDEX_RE", r"^\[(\d+)\]", "")?;
        heap.module_regexp("ara-ai.validation", "SPILL_KEY_PATTERN", r"^[\w.$-]{1,128}$", "")?;
        crate::schema_wire::prepare_graph_wire_module(&heap, runtime.as_ref())?;
        let ordinary_lookup_receiver = heap.object()?;
        let factory = Self { heap, runtime, ordinary_lookup_receiver, ark_type: None };
        crate::providers::openai_responses::actual::prepare_graph_responses_module(&factory)?;
        Ok(factory)
    }

    pub fn heap(&self) -> &Arc<ValueHeap> {
        &self.heap
    }

    pub fn runtime(&self) -> &dyn ValueRuntime {
        self.runtime.as_ref()
    }

    /// Lease the Host resolver for another same-lineage operation owner. The
    /// heap does not retain it; Host closures must not capture this factory,
    /// SessionValues or sinks and thereby create an ownership cycle.
    pub fn runtime_handle(&self) -> Arc<dyn ValueRuntime> {
        self.runtime.clone()
    }

    /// Bind the actual imported `type` value without reading its live `errors`
    /// member. JSON callers retain the existing unbound constructor.
    pub fn with_ark_type(&self, type_handle: ValueHandle) -> Result<Self, ValueError> {
        self.heap.adopt(&type_handle)?;
        let mut binding = self.clone();
        binding.ark_type = Some(type_handle);
        Ok(binding)
    }

    pub(crate) fn ark_type(&self) -> Result<&ValueHandle, ValueError> {
        self.ark_type
            .as_ref()
            .ok_or(ValueError::UnsupportedOperation("Graph ArkType imported type binding is unavailable"))
    }
}

// An invocation view preserves prepared module/ordinary receiver identity while
// resolving effects in the caller's current admitted scope. No heap/module init.
pub struct GraphValueInvocation<'a> { factory: &'a GraphValueFactory, runtime: &'a dyn ValueRuntime }
trait FactorySource {
    fn factory(&self) -> &GraphValueFactory;
    fn invocation_runtime(&self) -> &dyn ValueRuntime;
}
impl FactorySource for GraphValueFactory {
    fn factory(&self) -> &GraphValueFactory { self }
    fn invocation_runtime(&self) -> &dyn ValueRuntime { self.runtime() }
}
impl FactorySource for GraphValueInvocation<'_> {
    fn factory(&self) -> &GraphValueFactory { self.factory }
    fn invocation_runtime(&self) -> &dyn ValueRuntime { self.runtime }
}
impl GraphValueFactory {
    /// Owned scoped resolver adapter for existing concrete-factory consumers.
    /// Prepared heap, module values, Ark binding and receiver retain identity.
    pub fn with_runtime(&self, runtime: Arc<dyn ValueRuntime>) -> Self {
        Self { heap: self.heap.clone(), runtime,
            ordinary_lookup_receiver: self.ordinary_lookup_receiver.clone(), ark_type: self.ark_type.clone() }
    }
    pub fn invocation<'a>(&'a self, runtime: &'a dyn ValueRuntime) -> GraphValueInvocation<'a> {
        GraphValueInvocation { factory: self, runtime }
    }
}

impl<T: FactorySource> FinalValueFactory for T {
    type Value = ValueHandle;
    type Error = ValueError;

    fn import_strict(&self, value: WireValue) -> Result<ValueHandle, ValueError> {
        self.factory().heap.import_strict(value)
    }

    fn null(&self) -> Result<ValueHandle, ValueError> {
        self.factory().heap.null()
    }

    fn boolean(&self, value: bool) -> Result<ValueHandle, ValueError> {
        self.factory().heap.boolean(value)
    }

    fn number(&self, value: f64) -> Result<ValueHandle, ValueError> {
        self.factory().heap.number(value)
    }

    fn string(&self, value: WireString) -> Result<ValueHandle, ValueError> {
        self.factory().heap.string(value)
    }

    fn array(&self) -> Result<ValueHandle, ValueError> {
        self.factory().heap.array_with_length(0)
    }

    fn append(&self, array: &mut ValueHandle, value: ValueHandle) -> Result<(), ValueError> {
        // Fixed json-parse.ts uses out.push(value). Resolve the actual method
        // each time; a changed prototype/getter/method must retain its effects
        // and throw behavior instead of bypassing it with an intrinsic call.
        let push = self.factory().heap.get(array, &PropertyKey::from("push"), self.invocation_runtime())?;
        self.factory().heap.call(&push, array, &[value], self.invocation_runtime())?;
        Ok(())
    }

    fn object(&self) -> Result<ValueHandle, ValueError> {
        self.factory().heap.object()
    }

    fn assign(&self, object: &mut ValueHandle, key: WireString, value: ValueHandle) -> Result<(), ValueError> {
        self.factory().heap.assign(object, PropertyKey::String(key), value, self.invocation_runtime())
    }

    fn is_record(&self, value: &ValueHandle) -> Result<bool, ValueError> {
        // Native typeof value === "object", value !== null, !Array.isArray.
        // Boxed/opaque objects have Object kind; functions remain Function.
        Ok(self.factory().heap.kind(value)? == ValueKind::Object)
    }

    fn ordinary_prototype_lookup_truthy(&self, key: &WireString) -> Result<bool, ValueError> {
        let value = self.factory().heap.get(&self.factory().ordinary_lookup_receiver, &PropertyKey::String(key.clone()), self.invocation_runtime())?;
        self.factory().heap.truthy(&value)
    }

    fn should_catch_runtime_error(&self, error: &ValueError) -> bool {
        // Missing Host capability or a rejected checked boundary is an
        // integration failure, not a Native exception eligible for `{}` repair.
        error.is_native_exception()
    }

    fn get(&self, value: &ValueHandle, key: &WireString) -> Result<ValueHandle, ValueError> {
        self.factory().heap.get(value, &PropertyKey::String(key.clone()), self.invocation_runtime())
    }
    fn string_value(&self, value: &ValueHandle) -> Result<Option<WireString>, ValueError> {
        Ok(match self.factory().heap.snapshot(value)? {
            ValueSnapshot::String(value) => Some(value),
            _ => None,
        })
    }
    fn is_number_zero(&self, value: &ValueHandle) -> Result<bool, ValueError> {
        Ok(matches!(self.factory().heap.snapshot(value)?, ValueSnapshot::Number(value) if value == 0.0))
    }
    fn is_null(&self, value: &ValueHandle) -> Result<bool, ValueError> {
        Ok(self.factory().heap.kind(value)? == ValueKind::Null)
    }
    fn define_own_data(&self, object: &mut ValueHandle, key: WireString, value: ValueHandle) -> Result<(), ValueError> {
        self.factory().heap.define_own_data(object, PropertyKey::String(key), value, self.invocation_runtime())
    }
    fn stringify_json(&self, value: &ValueHandle, mode: StringifyMode) -> Result<Option<WireString>, ValueError> {
        self.factory().heap.stringify(value, mode, None, self.invocation_runtime())
    }
    fn implicit_string(&self, value: &ValueHandle) -> Result<WireString, ValueError> {
        if self.factory().heap.kind(value)? == ValueKind::Symbol {
            return Err(ValueError::TypeError("Cannot implicitly convert Symbol to string".into()));
        }
        self.factory().heap.to_string(value, self.invocation_runtime())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dialect::{HarmonyInbandUnitsScanner, InbandScanEvent};
    use crate::relaxed_json::{FinalParseError, parse_json_with_repair_units, parse_relaxed_final_units};
    use ara_rpc::value_heap::{
        CallableTarget, CapabilityId, NoRuntime, PropertyDescriptor, RuntimeReply, RuntimeRequest, ValueSnapshot,
    };
    use std::sync::Mutex;
    use std::sync::atomic::{AtomicUsize, Ordering};

    struct PushRuntime {
        heap: Arc<ValueHeap>,
        calls: AtomicUsize,
        first_array: Mutex<Option<ValueHandle>>,
    }
    impl ValueRuntime for PushRuntime {
        fn resolve(&self, request: RuntimeRequest<'_>) -> Result<RuntimeReply, ValueError> {
            if let RuntimeRequest::Call { capability: CapabilityId(77), receiver, arguments } = request {
                if self.calls.fetch_add(1, Ordering::SeqCst) == 0 {
                    // Reenter the actual heap while the factory's method call is
                    // pending; a held heap/resolver guard would deadlock here.
                    let length = self.heap.array_push(receiver, arguments, &NoRuntime)?;
                    *self.first_array.lock().unwrap() = Some(receiver.clone());
                    return Ok(RuntimeReply::Value(self.heap.number(f64::from(length))?));
                }
                return Err(ValueError::TypeError("replacement push failed".into()));
            }
            Err(ValueError::MissingCapability(request.capability()))
        }
    }

    struct ThrowRuntime {
        heap: Arc<ValueHeap>,
        thrown: ValueHandle,
        calls: AtomicUsize,
        receiver: Mutex<Option<ValueHandle>>,
    }
    impl ValueRuntime for ThrowRuntime {
        fn resolve(&self, request: RuntimeRequest<'_>) -> Result<RuntimeReply, ValueError> {
            if let RuntimeRequest::Call { capability: CapabilityId(99), receiver, .. } = request {
                self.calls.fetch_add(1, Ordering::SeqCst);
                self.heap.define_own_data(receiver, "side".into(), self.heap.string("kept".into())?, &NoRuntime)?;
                *self.receiver.lock().unwrap() = Some(receiver.clone());
                return Err(ValueError::Thrown(self.thrown.clone()));
            }
            Err(ValueError::MissingCapability(request.capability()))
        }
    }

    fn number(heap: &Arc<ValueHeap>, value: &ValueHandle) -> f64 {
        match heap.snapshot(value).unwrap() {
            ValueSnapshot::Number(value) => value,
            other => panic!("expected number, got {other:?}"),
        }
    }

    #[test]
    fn actual_graph_factory_parser_and_harmony_family() {
        let heap = ValueHeap::new().unwrap();
        let runtime =
            Arc::new(PushRuntime { heap: heap.clone(), calls: AtomicUsize::new(0), first_array: Mutex::new(None) });
        let factory = Arc::new(GraphValueFactory::new(heap.clone(), runtime.clone()).unwrap());

        // Strict input uses the same graph heap and retains its full scalar
        // domain. Strict __proto__ remains own data rather than a setter call.
        let strict = parse_json_with_repair_units(
            &WireString::from(r#"{"__proto__":{"x":1},"a":"\ud800","z":-0,"n":1e999}"#),
            factory.as_ref(),
        )
        .unwrap();
        assert!(Arc::ptr_eq(strict.heap(), &heap));
        assert!(heap.has_own(&strict, &"__proto__".into(), factory.runtime()).unwrap());
        assert!(heap.prototype(&strict, factory.runtime()).unwrap().unwrap().same_identity(&heap.object_prototype()));
        let a = heap.get(&strict, &"a".into(), factory.runtime()).unwrap();
        assert!(matches!(heap.snapshot(&a).unwrap(), ValueSnapshot::String(value) if value.units() == [0xd800]));
        assert_eq!(
            number(&heap, &heap.get(&strict, &"z".into(), factory.runtime()).unwrap()).to_bits(),
            (-0.0f64).to_bits()
        );
        assert_eq!(number(&heap, &heap.get(&strict, &"n".into(), factory.runtime()).unwrap()), f64::INFINITY);
        assert!(strict.clone().same_identity(&strict));

        // Relaxed input uses ordinary assignment. It changes the actual graph
        // prototype and its inherited reads without a JSON/map roundtrip.
        let relaxed =
            parse_relaxed_final_units(&WireString::from("{__proto__: {x: 1}, y: 2}"), factory.as_ref()).unwrap();
        assert!(!heap.has_own(&relaxed, &"__proto__".into(), factory.runtime()).unwrap());
        assert_eq!(number(&heap, &heap.get(&relaxed, &"x".into(), factory.runtime()).unwrap()), 1.0);
        let units = WireString::from_units(vec![0x7b, 0x61, 0x3a, 0x27, 0xd800, 0x27, 0x7d]);
        let unit_value = parse_relaxed_final_units(&units, factory.as_ref()).unwrap();
        assert!(
            matches!(heap.snapshot(&heap.get(&unit_value, &"a".into(), factory.runtime()).unwrap()).unwrap(), ValueSnapshot::String(value) if value.units() == [0xd800])
        );
        assert!(matches!(
            parse_relaxed_final_units(&WireString::from("{a: constructor}"), factory.as_ref()),
            Err(FinalParseError::Syntax(_))
        ));

        // The actual Harmony kernel emits this same lineage's argument handle.
        let mut scanner = HarmonyInbandUnitsScanner::with_id_source(factory.clone(), Arc::new(|| "fixed-id".into()));
        let mut events = scanner
            .try_feed_units(&WireString::from(
                "<|start|>assistant to=functions.read<|channel|>commentary<|message|>{a: 1}<|call|>",
            ))
            .unwrap();
        events.extend(scanner.try_flush_units().unwrap());
        let arguments = events
            .into_iter()
            .find_map(|event| match event {
                InbandScanEvent::ToolEnd { arguments, .. } => Some(arguments),
                _ => None,
            })
            .expect("actual ToolEnd event");
        assert!(Arc::ptr_eq(arguments.heap(), &heap));
        assert_eq!(number(&heap, &heap.get(&arguments, &"a".into(), factory.runtime()).unwrap()), 1.0);

        // Resolve the actual changed push method. First append reenters the
        // heap successfully; second throws. Earlier writes remain observable.
        let replacement = heap.function(CapabilityId(77)).unwrap();
        heap.define_own_data(&heap.array_prototype(), "push".into(), replacement, factory.runtime()).unwrap();
        assert!(
            matches!(parse_relaxed_final_units(&WireString::from("[1,2,]"), factory.as_ref()), Err(FinalParseError::Runtime(ValueError::TypeError(message))) if message == "replacement push failed")
        );
        assert_eq!(runtime.calls.load(Ordering::SeqCst), 2);
        let partially_appended = runtime.first_array.lock().unwrap().clone().unwrap();
        assert_eq!(heap.array_length(&partially_appended, factory.runtime()).unwrap(), 1);
        assert_eq!(number(&heap, &heap.get(&partially_appended, &"0".into(), factory.runtime()).unwrap()), 1.0);

        // A missing Host capability cannot be hidden as successful Harmony
        // fallback arguments. Real Native exceptions still use its catch.
        heap.define_own(
            &heap.object_prototype(),
            "constructor".into(),
            PropertyDescriptor::Accessor {
                get: Some(CallableTarget::Host(CapabilityId(99))),
                set: None,
                enumerable: false,
                configurable: true,
            },
            factory.runtime(),
        )
        .unwrap();
        let mut missing = HarmonyInbandUnitsScanner::new(factory);
        assert!(matches!(
            missing.try_feed_units(&WireString::from(
                "<|start|>assistant to=functions.read<|message|>{a: constructor}<|call|>",
            )),
            Err(ValueError::MissingCapability(CapabilityId(99)))
        ));

        // Same-heap thrown values use Native catch; foreign thrown handles
        // fail at the actual runtime boundary before Harmony can hide them.
        // The callback's earlier mutation survives either outcome, once only.
        for foreign in [false, true] {
            let throw_heap = if foreign { ValueHeap::new().unwrap() } else { heap.clone() };
            let thrown = throw_heap.object().unwrap();
            let runtime = Arc::new(ThrowRuntime {
                heap: heap.clone(),
                thrown,
                calls: AtomicUsize::new(0),
                receiver: Mutex::new(None),
            });
            let binding = Arc::new(GraphValueFactory::new(heap.clone(), runtime.clone()).unwrap());
            let mut scanner = HarmonyInbandUnitsScanner::new(binding);
            let result = scanner.try_feed_units(&WireString::from(
                "<|start|>assistant to=functions.read<|message|>{a: constructor}<|call|>",
            ));
            if foreign {
                let error = result.unwrap_err();
                assert!(matches!(error, ValueError::ForeignHeap));
                assert!(!error.is_native_exception());
            } else {
                let events = result.unwrap();
                let arguments = events
                    .into_iter()
                    .find_map(|event| match event {
                        InbandScanEvent::ToolEnd { arguments, .. } => Some(arguments),
                        _ => None,
                    })
                    .unwrap();
                assert_eq!(
                    heap.stringify(&arguments, ara_rpc::value_heap::StringifyMode::Plain, None, &NoRuntime)
                        .unwrap()
                        .unwrap()
                        .to_utf8()
                        .unwrap(),
                    "{}"
                );
            }
            assert_eq!(runtime.calls.load(Ordering::SeqCst), 1);
            let receiver = runtime.receiver.lock().unwrap().clone().unwrap();
            let side = heap.get(&receiver, &"side".into(), &NoRuntime).unwrap();
            assert!(
                matches!(heap.snapshot(&side).unwrap(), ValueSnapshot::String(value) if value.equals_ascii("kept"))
            );
        }
    }
}
