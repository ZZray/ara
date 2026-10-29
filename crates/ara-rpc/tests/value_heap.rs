//! Four coarse foundation families. Fixed OMP 596f2da sources are mapped in
//! SOURCE_MAP.md. These controlled capability observations do not accept
//! actual Host/current Raw/H2/checker/normalizer/hooks/schema/journal consumers.

use ara_rpc::{
    WireString,
    value_heap::{
        CallableTarget, CapabilityId, CaptureOwnControls, CaptureOwnRecords, CapturedOwnScalar, GuardedFacts,
        GuardedPropertyError, GuardedPropertyOp, GuardedPropertyStep, GuardedPropertyValue, Intrinsic, JsonSpace,
        NoRuntime, OpaqueKind, OpaqueOperation, OrderedOwnRecords, OwnScalar, OwnScalarFact, PropertyDescriptor,
        PropertyKey, PropertyPresence, RuntimeReply, RuntimeRequest, StringifyMode, ValueError, ValueHandle, ValueHeap,
        ValueKind, ValueRuntime, ValueSnapshot,
    },
};
use std::sync::{Arc, Mutex};

#[path = "value_heap/compiled.rs"]
mod compiled_observations;
#[path = "value_heap/json_native.rs"]
mod json_native_observations;
#[path = "value_heap/json_replacer.rs"]
mod json_replacer_observations;

fn number(heap: &Arc<ValueHeap>, value: &ValueHandle) -> f64 {
    let ValueSnapshot::Number(value) = heap.snapshot(value).unwrap() else { panic!("expected number") };
    value
}
fn strings(keys: Vec<WireString>) -> Vec<String> {
    keys.into_iter().map(|key| key.to_utf8().unwrap()).collect()
}
fn json(heap: &Arc<ValueHeap>, value: &ValueHandle, mode: StringifyMode, runtime: &dyn ValueRuntime) -> Option<String> {
    heap.stringify(value, mode, None, runtime).unwrap().map(|value| value.to_utf8().unwrap())
}
fn data(value: ValueHandle, enumerable: bool, configurable: bool) -> PropertyDescriptor {
    PropertyDescriptor::Data { value, writable: true, enumerable, configurable }
}
fn accessor(get: Option<u64>, set: Option<u64>) -> PropertyDescriptor {
    PropertyDescriptor::Accessor {
        get: get.map(|id| CallableTarget::Host(CapabilityId(id))),
        set: set.map(|id| CallableTarget::Host(CapabilityId(id))),
        enumerable: true,
        configurable: true,
    }
}

struct Runtime {
    heap: Arc<ValueHeap>,
    shared: ValueHandle,
    thrown: ValueHandle,
    trace: Mutex<Vec<String>>,
}
impl Runtime {
    fn new(heap: &Arc<ValueHeap>) -> Self {
        Self {
            heap: Arc::clone(heap),
            shared: heap.object().unwrap(),
            thrown: heap.string("actual throw".into()).unwrap(),
            trace: Mutex::new(Vec::new()),
        }
    }
    fn record(&self, event: String) {
        self.trace.lock().unwrap().push(event);
    }
}
impl ValueRuntime for Runtime {
    fn resolve(&self, request: RuntimeRequest<'_>) -> Result<RuntimeReply, ValueError> {
        let capability = request.capability();
        match request {
            RuntimeRequest::Call { receiver, arguments, .. } => match capability.0 {
                1 => {
                    self.record("get:reentrant".into());
                    // The resolver trace guard is already released; reenter the same heap/resolver.
                    self.heap.define_own_data(receiver, "a".into(), self.heap.number(9.0)?, self)?;
                    let _ = self.heap.get(receiver, &"a".into(), self)?;
                    Ok(RuntimeReply::Value(self.heap.boolean(true)?))
                }
                2 => {
                    self.record("get:throw".into());
                    self.heap.assign(&self.shared, "touched".into(), self.heap.boolean(true)?, self)?;
                    Err(ValueError::Thrown(self.thrown.clone()))
                }
                3 => {
                    self.record("set:reentrant".into());
                    let value = arguments.first().ok_or(ValueError::WrongRuntimeReply("setter argument"))?;
                    self.heap.define_own_data(receiver, "setter".into(), value.clone(), self)?;
                    Ok(RuntimeReply::Value(self.heap.undefined()?))
                }
                10 => {
                    let key = arguments.first().ok_or(ValueError::WrongRuntimeReply("toJSON key"))?;
                    let ValueSnapshot::String(key) = self.heap.snapshot(key)? else {
                        return Err(ValueError::WrongRuntimeReply("toJSON string key"));
                    };
                    self.record(format!("toJSON:{}:{:?}", key.to_utf8().unwrap(), receiver.node_id()));
                    Ok(RuntimeReply::Value(self.heap.bigint_decimal("9007199254740993123456789")?))
                }
                11 => {
                    if self.heap.kind(receiver)? != ValueKind::BigInt || !arguments.is_empty() {
                        return Err(ValueError::WrongRuntimeReply("BigInt toString receiver/arguments"));
                    }
                    self.record("bigint:toString:replacement".into());
                    Ok(RuntimeReply::Value(self.heap.boolean(true)?))
                }
                12 => {
                    self.record("push:replacement".into());
                    // Reentrant call uses the same arena and resolver, retaining partial native writes.
                    self.heap.array_push(receiver, arguments, self)?;
                    Err(ValueError::Thrown(self.thrown.clone()))
                }
                99 => {
                    self.record("capability:expired".into());
                    Err(ValueError::ExpiredCapability(capability))
                }
                _ => Err(ValueError::MissingCapability(capability)),
            },
            RuntimeRequest::Opaque { operation, .. } if capability.0 == 30 => match operation {
                OpaqueOperation::Get(_) => Ok(RuntimeReply::Value(self.heap.undefined()?)),
                OpaqueOperation::Unbox => Ok(RuntimeReply::Value(self.heap.bigint_decimal("123456789012345678901")?)),
                _ => Err(ValueError::MissingCapability(capability)),
            },
            RuntimeRequest::Opaque { operation: OpaqueOperation::StructuredClone, .. } if capability.0 == 31 => {
                Ok(RuntimeReply::Value(self.heap.opaque(OpaqueKind::Object, CapabilityId(31), 2)?))
            }
            _ => Err(ValueError::MissingCapability(capability)),
        }
    }
}

/// Controlled resolver fault at the existing admission boundary, not a second
/// carrier/factory. Its short trace guard is released before heap reentry.
struct ThrowRuntime {
    heap: Arc<ValueHeap>,
    thrown: ValueHandle,
    shared: ValueHandle,
    clone_only: bool,
    trace: Mutex<Vec<&'static str>>,
}
impl ThrowRuntime {
    fn new(heap: &Arc<ValueHeap>, thrown: ValueHandle, clone_only: bool) -> Self {
        Self {
            heap: Arc::clone(heap),
            thrown,
            shared: heap.object().unwrap(),
            clone_only,
            trace: Mutex::new(Vec::new()),
        }
    }
}
impl ValueRuntime for ThrowRuntime {
    fn resolve(&self, request: RuntimeRequest<'_>) -> Result<RuntimeReply, ValueError> {
        let label = match &request {
            RuntimeRequest::Call { .. } => "Call",
            RuntimeRequest::ConstructArray { .. } => "ConstructArray",
            RuntimeRequest::Opaque { operation, .. } => match operation {
                OpaqueOperation::OwnDescriptor(_) => "OwnDescriptor",
                OpaqueOperation::Prototype => "Prototype",
                OpaqueOperation::OwnKeys => "OwnKeys",
                OpaqueOperation::ArrayLength => "ArrayLength",
                OpaqueOperation::Get(_) => "Get",
                OpaqueOperation::Has(_) => "Has",
                OpaqueOperation::Set { .. } => "Set",
                OpaqueOperation::StructuredClone => "StructuredClone",
                _ => "OtherOpaque",
            },
        };
        let seen = {
            let mut trace = self.trace.lock().unwrap();
            trace.push(label);
            trace.len()
        };
        if self.clone_only {
            match &request {
                RuntimeRequest::Opaque { operation: OpaqueOperation::Prototype, .. } => {
                    return Ok(RuntimeReply::Prototype(Some(self.heap.object_prototype())));
                }
                RuntimeRequest::Opaque { operation: OpaqueOperation::Get(_), .. } => {
                    return Ok(RuntimeReply::Value(self.heap.undefined()?));
                }
                RuntimeRequest::Opaque { operation: OpaqueOperation::OwnKeys, .. } => {
                    return Ok(RuntimeReply::Keys(Vec::new()));
                }
                RuntimeRequest::Opaque { operation: OpaqueOperation::StructuredClone, .. } => {}
                _ => return Err(ValueError::WrongRuntimeReply("clone-only fault request")),
            }
        }
        // This actual same-heap mutation must survive both admitted native
        // throws and rejected foreign throws; the boundary does not roll back.
        self.heap.define_own_data(&self.shared, "touched".into(), self.heap.number(seen as f64)?, &NoRuntime)?;
        Err(ValueError::Thrown(self.thrown.clone()))
    }
}

struct SlotRuntime {
    heap: Arc<ValueHeap>,
    root: ValueHandle,
    member: ValueHandle,
    returned: ValueHandle,
    thrown: Option<ValueHandle>,
    opaque: Option<ValueHandle>,
    detach: bool,
    change_boundary: bool,
    trace: Mutex<Vec<&'static str>>,
}
impl SlotRuntime {
    fn new(heap: &Arc<ValueHeap>, root: &ValueHandle, member: &ValueHandle, returned: &ValueHandle) -> Self {
        Self {
            heap: Arc::clone(heap),
            root: root.clone(),
            member: member.clone(),
            returned: returned.clone(),
            thrown: None,
            opaque: None,
            detach: false,
            change_boundary: false,
            trace: Mutex::new(Vec::new()),
        }
    }
}
impl ValueRuntime for SlotRuntime {
    fn resolve(&self, request: RuntimeRequest<'_>) -> Result<RuntimeReply, ValueError> {
        let (label, receiver, assigned, opaque_set) = match request {
            RuntimeRequest::Call { capability: CapabilityId(70), receiver, arguments: [] } => {
                ("GetCall", receiver, None, false)
            }
            RuntimeRequest::Call { capability: CapabilityId(71), receiver, arguments } if arguments.len() == 1 => {
                ("SetCall", receiver, arguments.first(), false)
            }
            RuntimeRequest::Opaque { capability: CapabilityId(72), target, receiver, operation } => {
                if !self.opaque.as_ref().is_some_and(|opaque| opaque.same_identity(target)) {
                    return Err(ValueError::WrongRuntimeReply("actual opaque target"));
                }
                match operation {
                    OpaqueOperation::Get(key) if key == &PropertyKey::from("arguments") => {
                        ("OpaqueGet", receiver, None, false)
                    }
                    OpaqueOperation::Set { key, value } if key == &PropertyKey::from("arguments") => {
                        ("OpaqueSet", receiver, Some(value), true)
                    }
                    _ => return Err(ValueError::WrongRuntimeReply("opaque source operation")),
                }
            }
            _ => return Err(ValueError::WrongRuntimeReply("selected source callback")),
        };
        if !receiver.same_identity(&self.member) {
            return Err(ValueError::WrongRuntimeReply("actual member receiver"));
        }
        self.trace.lock().unwrap().push(label);
        // Trace guard is released before the same-heap reentry. These effects
        // survive a later callback error or failed current-member admission.
        self.heap.define_own_data(receiver, "side".into(), self.heap.boolean(true)?, &NoRuntime)?;
        if let Some(value) = assigned {
            if !value.same_identity(&self.returned) {
                return Err(ValueError::WrongRuntimeReply("retained assignment value"));
            }
            if !opaque_set {
                self.heap.define_own_data(receiver, "arguments".into(), value.clone(), &NoRuntime)?;
            }
        }
        if self.change_boundary {
            self.heap.assign(&self.root, "stopReason".into(), self.heap.string("error".into())?, &NoRuntime)?;
        }
        if self.detach {
            self.heap.assign(&self.root, "content".into(), self.heap.array(vec![])?, &NoRuntime)?;
        }
        if let Some(thrown) = &self.thrown {
            return Err(ValueError::Thrown(thrown.clone()));
        }
        if opaque_set {
            Ok(RuntimeReply::Unit)
        } else if assigned.is_some() {
            Ok(RuntimeReply::Value(self.heap.undefined()?))
        } else {
            Ok(RuntimeReply::Value(self.returned.clone()))
        }
    }
}

fn slot_graph(
    heap: &Arc<ValueHeap>,
    prototype: Option<&ValueHandle>,
) -> (ValueHandle, ValueHandle, ValueHandle, GuardedFacts) {
    let root = heap.object().unwrap();
    let member = match prototype {
        Some(prototype) => heap.object_with_prototype(Some(prototype)).unwrap(),
        None => heap.object().unwrap(),
    };
    let arguments = heap.object().unwrap();
    heap.define_own_data(&arguments, "self".into(), arguments.clone(), &NoRuntime).unwrap();
    for (key, value) in [
        ("type", heap.string("toolCall".into()).unwrap()),
        ("id", heap.string("call-1".into()).unwrap()),
        ("name", heap.string("tool".into()).unwrap()),
        ("arguments", arguments.clone()),
    ] {
        heap.define_own_data(&member, key.into(), value, &NoRuntime).unwrap();
    }
    for (key, value) in [
        ("role", heap.string("assistant".into()).unwrap()),
        ("stopReason", heap.string("toolUse".into()).unwrap()),
        ("content", heap.array(vec![member.clone()]).unwrap()),
    ] {
        heap.define_own_data(&root, key.into(), value, &NoRuntime).unwrap();
    }
    let facts = GuardedFacts {
        root: vec![
            OwnScalarFact { key: "role".into(), expected: OwnScalar::Text("assistant".into()) },
            OwnScalarFact {
                key: "stopReason".into(),
                expected: OwnScalar::TextOneOf(vec!["toolUse".into(), "stop".into()]),
            },
        ],
        ordered: Some(OrderedOwnRecords {
            array_key: "content".into(),
            discriminator_key: "type".into(),
            discriminator: "toolCall".into(),
            records: vec![vec![
                OwnScalarFact { key: "id".into(), expected: OwnScalar::Text("call-1".into()) },
                OwnScalarFact { key: "name".into(), expected: OwnScalar::Text("tool".into()) },
            ]],
        }),
        array_key: "content".into(),
        member: member.clone(),
    };
    (root, member, arguments, facts)
}

fn guarded_slot_observations() {
    // Ordinary property faults keep their own stage after successful admission.
    // A same-boundary read-only assignment is a Native TypeError; foreign value
    // admission still occurs first and is never relabeled as a property fault.
    {
        let heap = ValueHeap::new().unwrap();
        let (root, member, arguments, facts) = slot_graph(&heap, None);
        let key = PropertyKey::from("arguments");
        heap.define_own(
            &member,
            key.clone(),
            PropertyDescriptor::Data {
                value: arguments.clone(),
                writable: false,
                enumerable: true,
                configurable: true,
            },
            &NoRuntime,
        )
        .unwrap();
        let replacement = heap.number(42.0).unwrap();
        let Err(GuardedPropertyError::Operation(error)) =
            heap.guarded_property(&root, &facts, &key, GuardedPropertyOp::Assign(&replacement))
        else {
            panic!("Expected ordinary property fault after successful admission")
        };
        assert!(matches!(&error, ValueError::TypeError(_)));
        assert!(error.is_native_exception());
        assert!(heap.get(&member, &key, &NoRuntime).unwrap().same_identity(&arguments));
        assert!(matches!(
            heap.guarded_property(&root, &facts, &key, GuardedPropertyOp::Recheck).unwrap(),
            GuardedPropertyStep::Done(GuardedPropertyValue::Rechecked)
        ));
        let foreign = ValueHeap::new().unwrap().number(42.0).unwrap();
        assert!(matches!(
            heap.guarded_property(&root, &facts, &key, GuardedPropertyOp::Assign(&foreign)),
            Err(GuardedPropertyError::Value(ValueError::ForeignHeap))
        ));
    }
    let heap = ValueHeap::new().unwrap();
    let (root, member, arguments, facts) = slot_graph(&heap, None);
    let key = PropertyKey::from("arguments");
    assert!(matches!(heap.guarded_property(&root, &facts, &key, GuardedPropertyOp::Read).unwrap(),
        GuardedPropertyStep::Done(GuardedPropertyValue::Read(value)) if value.same_identity(&arguments)));
    let replacement = heap.bigint_decimal("12345678901234567890").unwrap();
    assert!(matches!(
        heap.guarded_property(&root, &facts, &key, GuardedPropertyOp::Assign(&replacement)).unwrap(),
        GuardedPropertyStep::Done(GuardedPropertyValue::Assigned)
    ));
    assert!(heap.get(&member, &key, &NoRuntime).unwrap().same_identity(&replacement));
    // Changing the Array allocation or moving unrelated content does not turn
    // an index into slot authority; the current member node remains canonical.
    let unrelated = heap.object().unwrap();
    heap.define_own_data(&unrelated, "type".into(), heap.string("text".into()).unwrap(), &NoRuntime).unwrap();
    heap.assign(&root, "content".into(), heap.array(vec![unrelated, member.clone()]).unwrap(), &NoRuntime).unwrap();
    for reason in ["toolUse", "stop"] {
        heap.assign(&root, "stopReason".into(), heap.string(reason.into()).unwrap(), &NoRuntime).unwrap();
        assert!(matches!(
            heap.guarded_property(&root, &facts, &key, GuardedPropertyOp::Recheck).unwrap(),
            GuardedPropertyStep::Done(GuardedPropertyValue::Rechecked)
        ));
    }
    let mut blocked = facts.clone();
    blocked.root[1].expected = OwnScalar::TextOneOf(vec!["length".into(), "error".into(), "aborted".into()]);
    for reason in ["length", "error", "aborted"] {
        heap.assign(&root, "stopReason".into(), heap.string(reason.into()).unwrap(), &NoRuntime).unwrap();
        assert!(heap.guarded_property(&root, &blocked, &key, GuardedPropertyOp::Recheck).is_ok());
        assert!(matches!(
            heap.guarded_property(&root, &facts, &key, GuardedPropertyOp::Read),
            Err(GuardedPropertyError::BoundaryChanged)
        ));
    }
    heap.assign(&root, "stopReason".into(), heap.string("stop".into()).unwrap(), &NoRuntime).unwrap();
    let mut incongruent = facts.clone();
    incongruent.ordered.as_mut().unwrap().array_key = "other-content".into();
    assert!(matches!(
        heap.guarded_property(&root, &incongruent, &key, GuardedPropertyOp::Assign(&arguments)),
        Err(GuardedPropertyError::UnsupportedStructure(_))
    ));
    assert!(heap.get(&member, &key, &NoRuntime).unwrap().same_identity(&replacement));
    let mut empty_set = facts.clone();
    empty_set.root[1].expected = OwnScalar::TextOneOf(vec![]);
    assert!(matches!(
        heap.guarded_property(&root, &empty_set, &key, GuardedPropertyOp::Recheck),
        Err(GuardedPropertyError::UnsupportedStructure(_))
    ));
    let sparse = heap.array_with_length(2).unwrap();
    heap.assign(&sparse, "0".into(), member.clone(), &NoRuntime).unwrap();
    heap.assign(&root, "content".into(), sparse, &NoRuntime).unwrap();
    assert!(matches!(
        heap.guarded_property(&root, &facts, &key, GuardedPropertyOp::Read),
        Err(GuardedPropertyError::UnsupportedStructure(_))
    ));
    let forged = heap.object().unwrap();
    for (key, value) in [("type", "toolCall"), ("id", "call-1"), ("name", "tool")] {
        heap.define_own_data(&forged, key.into(), heap.string(value.into()).unwrap(), &NoRuntime).unwrap();
    }
    heap.assign(&root, "content".into(), heap.array(vec![forged.clone()]).unwrap(), &NoRuntime).unwrap();
    assert!(matches!(
        heap.guarded_property(&root, &facts, &key, GuardedPropertyOp::Recheck),
        Err(GuardedPropertyError::MissingMember)
    ));
    heap.assign(&root, "content".into(), heap.array(vec![member.clone(), forged]).unwrap(), &NoRuntime).unwrap();
    assert!(matches!(
        heap.guarded_property(&root, &facts, &key, GuardedPropertyOp::Recheck),
        Err(GuardedPropertyError::BoundaryChanged)
    ));

    heap.define_own(&root, "role".into(), accessor(Some(70), None), &NoRuntime).unwrap();
    assert!(matches!(
        heap.guarded_property(&root, &facts, &key, GuardedPropertyOp::Read),
        Err(GuardedPropertyError::UnsupportedStructure(_))
    ));
    assert!(heap.get(&member, &key, &NoRuntime).unwrap().same_identity(&replacement));

    // An inherited getter/setter is captured with its actual block receiver;
    // reentry and partial effects are observed in this existing coarse family.
    for setter in [false, true] {
        let heap = ValueHeap::new().unwrap();
        let prototype = heap.object().unwrap();
        heap.define_own(&prototype, key.clone(), accessor(Some(70), Some(71)), &NoRuntime).unwrap();
        let (root, member, arguments, facts) = slot_graph(&heap, Some(&prototype));
        assert!(heap.delete_own(&member, &key, &NoRuntime).unwrap());
        let mut runtime = SlotRuntime::new(&heap, &root, &member, &arguments);
        runtime.detach = setter;
        let op = if setter { GuardedPropertyOp::Assign(&arguments) } else { GuardedPropertyOp::Read };
        let GuardedPropertyStep::Deferred(plan) = heap.guarded_property(&root, &facts, &key, op).unwrap() else {
            panic!("expected captured accessor")
        };
        assert!(runtime.trace.lock().unwrap().is_empty());
        let result = heap.run_deferred(plan, &runtime).unwrap();
        if setter {
            assert!(matches!(result, GuardedPropertyValue::Assigned));
            assert!(heap.get(&member, &key, &NoRuntime).unwrap().same_identity(&arguments));
            assert!(matches!(
                heap.guarded_property(&root, &facts, &key, GuardedPropertyOp::Recheck),
                Err(GuardedPropertyError::MissingMember)
            ));
        } else {
            assert!(matches!(result, GuardedPropertyValue::Read(value) if value.same_identity(&arguments)));
            assert!(heap.guarded_property(&root, &facts, &key, GuardedPropertyOp::Recheck).is_ok());
        }
        assert_eq!(runtime.trace.lock().unwrap().as_slice(), if setter { &["SetCall"] } else { &["GetCall"] });
        assert!(heap.truthy(&heap.get(&member, &"side".into(), &NoRuntime).unwrap()).unwrap());
    }
    for foreign in [false, true] {
        let heap = ValueHeap::new().unwrap();
        let prototype = heap.object().unwrap();
        heap.define_own(&prototype, key.clone(), accessor(Some(70), None), &NoRuntime).unwrap();
        let (root, member, arguments, facts) = slot_graph(&heap, Some(&prototype));
        heap.delete_own(&member, &key, &NoRuntime).unwrap();
        let thrown = if foreign {
            ValueHeap::new().unwrap().string("foreign".into()).unwrap()
        } else {
            heap.string("same".into()).unwrap()
        };
        let mut runtime = SlotRuntime::new(&heap, &root, &member, &arguments);
        runtime.thrown = Some(thrown.clone());
        runtime.change_boundary = true;
        let GuardedPropertyStep::Deferred(plan) =
            heap.guarded_property(&root, &facts, &key, GuardedPropertyOp::Read).unwrap()
        else {
            panic!("expected captured getter")
        };
        let callback_error = heap.run_deferred(plan, &runtime).unwrap_err();
        if foreign {
            assert!(matches!(&callback_error, ValueError::ForeignHeap));
        } else {
            assert!(matches!(&callback_error, ValueError::Thrown(value) if value.same_identity(&thrown)));
        }
        // Carrier keeps callback and admission observations separate. The ARA
        // owner must combine them with admission primary; no Native catch claim.
        assert!(matches!(
            heap.guarded_property(&root, &facts, &key, GuardedPropertyOp::Recheck),
            Err(GuardedPropertyError::BoundaryChanged)
        ));
        assert_eq!(runtime.trace.lock().unwrap().as_slice(), &["GetCall"]);
        assert!(heap.truthy(&heap.get(&member, &"side".into(), &NoRuntime).unwrap()).unwrap());
    }
    for setter in [false, true] {
        let heap = ValueHeap::new().unwrap();
        let opaque = heap.opaque(OpaqueKind::Object, CapabilityId(72), 1).unwrap();
        let (root, member, arguments, facts) = slot_graph(&heap, Some(&opaque));
        heap.delete_own(&member, &key, &NoRuntime).unwrap();
        let mut runtime = SlotRuntime::new(&heap, &root, &member, &arguments);
        runtime.opaque = Some(opaque);
        let op = if setter { GuardedPropertyOp::Assign(&arguments) } else { GuardedPropertyOp::Read };
        let GuardedPropertyStep::Deferred(plan) = heap.guarded_property(&root, &facts, &key, op).unwrap() else {
            panic!("expected direct opaque source operation")
        };
        let result = heap.run_deferred(plan, &runtime).unwrap();
        if setter {
            assert!(matches!(result, GuardedPropertyValue::Assigned));
            assert!(matches!(heap.own_get(&member, &key, &NoRuntime).unwrap(), PropertyPresence::Missing));
        } else {
            assert!(matches!(result, GuardedPropertyValue::Read(value) if value.same_identity(&arguments)));
        }
        assert!(heap.guarded_property(&root, &facts, &key, GuardedPropertyOp::Recheck).is_ok());
        assert_eq!(runtime.trace.lock().unwrap().as_slice(), if setter { &["OpaqueSet"] } else { &["OpaqueGet"] });
    }
}

fn initial_own_capture_observations() {
    let heap = ValueHeap::new().unwrap();
    let (root, member, arguments, _) = slot_graph(&heap, None);
    for (key, value) in [
        ("undefined-control", heap.undefined().unwrap()),
        ("null-control", heap.null().unwrap()),
        ("bool-control", heap.boolean(false).unwrap()),
    ] {
        heap.define_own_data(&root, key.into(), value, &NoRuntime).unwrap();
    }
    // Arguments can remain cyclic or accessor-backed without being observed.
    heap.define_own(&member, "arguments".into(), accessor(Some(70), Some(71)), &NoRuntime).unwrap();
    let request = CaptureOwnControls {
        root_scalar_keys: vec![
            "role".into(),
            "absent-control".into(),
            "undefined-control".into(),
            "null-control".into(),
            "bool-control".into(),
        ],
        selected_records_root_scalar_keys: Vec::new(),
        records: Some(CaptureOwnRecords {
            array_key: "content".into(),
            discriminator_key: "type".into(),
            discriminator: "toolCall".into(),
            scalar_keys: vec!["id".into(), "name".into()],
        }),
    };
    let captured = heap.capture_own_controls(&root, &request).unwrap();
    assert!(captured.root.same_identity(&root));
    assert_eq!(captured.controls.iter().map(|(key, _)| key.clone()).collect::<Vec<_>>(), request.root_scalar_keys);
    assert!(matches!(&captured.controls[0].1, CapturedOwnScalar::Text(text) if text == &WireString::from("assistant")));
    assert!(matches!(&captured.controls[1].1, CapturedOwnScalar::Missing));
    assert!(matches!(&captured.controls[2].1, CapturedOwnScalar::Undefined));
    assert!(matches!(&captured.controls[3].1, CapturedOwnScalar::Null));
    assert!(matches!(&captured.controls[4].1, CapturedOwnScalar::Bool(false)));
    assert_eq!(captured.records.len(), 1);
    assert!(captured.records[0].member.same_identity(&member));
    assert!(matches!(&captured.records[0].controls[0].1,
        CapturedOwnScalar::Text(text) if text == &WireString::from("call-1")));
    assert!(matches!(&captured.records[0].controls[1].1,
        CapturedOwnScalar::Text(text) if text == &WireString::from("tool")));
    assert!(matches!(
        heap.own_descriptor(&member, &"arguments".into(), &NoRuntime).unwrap(),
        Some(PropertyDescriptor::Accessor { .. })
    ));
    assert!(heap.get(&arguments, &"self".into(), &NoRuntime).unwrap().same_identity(&arguments));

    heap.define_own_data(&root, "conditional-control".into(), heap.boolean(true).unwrap(), &NoRuntime).unwrap();
    let mut conditional = request.clone();
    conditional.selected_records_root_scalar_keys = vec!["stopReason".into(), "conditional-control".into()];
    let with_calls = heap.capture_own_controls(&root, &conditional).unwrap();
    let expected_keys: Vec<_> =
        request.root_scalar_keys.iter().chain(&conditional.selected_records_root_scalar_keys).cloned().collect();
    assert_eq!(with_calls.controls.iter().map(|(key, _)| key.clone()).collect::<Vec<_>>(), expected_keys);
    assert!(matches!(&with_calls.controls[request.root_scalar_keys.len()].1,
        CapturedOwnScalar::Text(text) if text == &WireString::from("toolUse")));
    assert!(matches!(&with_calls.controls[request.root_scalar_keys.len() + 1].1, CapturedOwnScalar::Bool(true)));

    let unrelated = heap.object().unwrap();
    heap.define_own_data(&unrelated, "type".into(), heap.string("text".into()).unwrap(), &NoRuntime).unwrap();
    let second = heap.object().unwrap();
    for (key, value) in [("type", "toolCall"), ("id", "call-2"), ("name", "tool-2")] {
        heap.define_own_data(&second, key.into(), heap.string(value.into()).unwrap(), &NoRuntime).unwrap();
    }
    heap.assign(
        &root,
        "content".into(),
        heap.array(vec![unrelated.clone(), second.clone(), member.clone()]).unwrap(),
        &NoRuntime,
    )
    .unwrap();
    let reordered = heap.capture_own_controls(&root, &request).unwrap();
    assert_eq!(reordered.records.len(), 2);
    assert!(reordered.records[0].member.same_identity(&second));
    assert!(reordered.records[1].member.same_identity(&member));

    // No selected calls needs no member placeholder or stopReason observation.
    heap.define_own(&root, "stopReason".into(), accessor(Some(70), None), &NoRuntime).unwrap();
    assert!(matches!(
        heap.capture_own_controls(&root, &conditional),
        Err(GuardedPropertyError::UnsupportedStructure(_))
    ));
    for content in [heap.array(vec![]).unwrap(), heap.array(vec![unrelated]).unwrap()] {
        heap.assign(&root, "content".into(), content, &NoRuntime).unwrap();
        assert!(heap.capture_own_controls(&root, &request).unwrap().records.is_empty());
        let without_calls = heap.capture_own_controls(&root, &conditional).unwrap();
        assert!(without_calls.records.is_empty());
        assert_eq!(
            without_calls.controls.iter().map(|(key, _)| key.clone()).collect::<Vec<_>>(),
            request.root_scalar_keys
        );
    }
    let foreign_heap = ValueHeap::new().unwrap();
    let symbol = foreign_heap.symbol(None).unwrap();
    let mut foreign_conditional = request.clone();
    foreign_conditional.selected_records_root_scalar_keys =
        vec![PropertyKey::Symbol(foreign_heap.symbol_key(&symbol).unwrap())];
    assert!(matches!(
        heap.capture_own_controls(&root, &foreign_conditional),
        Err(GuardedPropertyError::Value(ValueError::ForeignHeap))
    ));
    let mut foreign_key = request.clone();
    foreign_key
        .records
        .as_mut()
        .unwrap()
        .scalar_keys
        .push(PropertyKey::Symbol(foreign_heap.symbol_key(&symbol).unwrap()));
    assert!(matches!(
        heap.capture_own_controls(&root, &foreign_key),
        Err(GuardedPropertyError::Value(ValueError::ForeignHeap))
    ));
    assert!(matches!(
        heap.capture_own_controls(&foreign_heap.object().unwrap(), &request),
        Err(GuardedPropertyError::Value(ValueError::ForeignHeap))
    ));

    // Root-only capture neither reads nor constrains an unrelated content field.
    heap.define_own(&root, "content".into(), accessor(Some(70), None), &NoRuntime).unwrap();
    let root_only = CaptureOwnControls {
        root_scalar_keys: vec!["role".into()],
        selected_records_root_scalar_keys: Vec::new(),
        records: None,
    };
    assert!(heap.capture_own_controls(&root, &root_only).unwrap().records.is_empty());
    let mut no_records_conditional = root_only.clone();
    no_records_conditional.selected_records_root_scalar_keys = vec!["role".into()];
    assert!(matches!(
        heap.capture_own_controls(&root, &no_records_conditional),
        Err(GuardedPropertyError::UnsupportedStructure(_))
    ));
    no_records_conditional.selected_records_root_scalar_keys = foreign_conditional.selected_records_root_scalar_keys;
    assert!(matches!(
        heap.capture_own_controls(&root, &no_records_conditional),
        Err(GuardedPropertyError::Value(ValueError::ForeignHeap))
    ));
    assert!(matches!(heap.capture_own_controls(&root, &request), Err(GuardedPropertyError::UnsupportedStructure(_))));
    let unsupported_scalar = CaptureOwnControls {
        root_scalar_keys: vec!["bad-control".into()],
        selected_records_root_scalar_keys: Vec::new(),
        records: None,
    };
    for value in [heap.number(1.0).unwrap(), heap.object().unwrap()] {
        heap.define_own_data(&root, "bad-control".into(), value, &NoRuntime).unwrap();
        assert!(matches!(
            heap.capture_own_controls(&root, &unsupported_scalar),
            Err(GuardedPropertyError::UnsupportedStructure(_))
        ));
    }
    heap.define_own(&root, "bad-control".into(), accessor(Some(70), None), &NoRuntime).unwrap();
    assert!(matches!(
        heap.capture_own_controls(&root, &unsupported_scalar),
        Err(GuardedPropertyError::UnsupportedStructure(_))
    ));
    assert!(matches!(
        heap.capture_own_controls(&heap.array(vec![]).unwrap(), &root_only),
        Err(GuardedPropertyError::UnsupportedStructure(_))
    ));
    assert!(matches!(
        heap.capture_own_controls(&heap.opaque(OpaqueKind::Object, CapabilityId(72), 1).unwrap(), &root_only),
        Err(GuardedPropertyError::UnsupportedStructure(_))
    ));
    heap.delete_own(&root, &"content".into(), &NoRuntime).unwrap();
    let sparse = heap.array_with_length(2).unwrap();
    heap.assign(&sparse, "0".into(), member.clone(), &NoRuntime).unwrap();
    heap.define_own_data(&root, "content".into(), sparse, &NoRuntime).unwrap();
    assert!(matches!(heap.capture_own_controls(&root, &request), Err(GuardedPropertyError::UnsupportedStructure(_))));
    heap.assign(&root, "content".into(), heap.array(vec![member.clone()]).unwrap(), &NoRuntime).unwrap();
    heap.define_own(&member, "id".into(), accessor(Some(70), None), &NoRuntime).unwrap();
    assert!(matches!(heap.capture_own_controls(&root, &request), Err(GuardedPropertyError::UnsupportedStructure(_))));
}

#[test]
fn graph_identity_prototypes_order_scalars_and_cycle_release() {
    let heap = ValueHeap::new().unwrap();
    let root = heap.object().unwrap();
    let child = heap.object().unwrap();
    heap.assign(&root, "child".into(), child.clone(), &NoRuntime).unwrap();
    assert!(heap.adopt(&child).unwrap().same_identity(&child));
    let edge = heap.capture_deferred(&child).unwrap();
    assert!(heap.restore_deferred(&edge).unwrap().same_identity(&child));
    let foreign = ValueHeap::new().unwrap();
    assert!(matches!(foreign.capture_deferred(&child), Err(ValueError::ForeignHeap)));
    assert!(matches!(foreign.restore_deferred(&edge), Err(ValueError::ForeignHeap)));
    assert!(matches!(ValueHeap::new().unwrap().adopt(&child), Err(ValueError::ForeignHeap)));
    let parsed = heap.parse_strict_units(&WireString::from("{\"__proto__\":{\"x\":1},\"v\":-0}")).unwrap();
    assert!(heap.has_own(&parsed, &"__proto__".into(), &NoRuntime).unwrap());
    assert!(heap.prototype(&parsed, &NoRuntime).unwrap().unwrap().same_identity(&heap.object_prototype()));
    assert!(number(&heap, &heap.get(&parsed, &"v".into(), &NoRuntime).unwrap()).is_sign_negative());
    let second = heap.parse_strict_units(&WireString::from("{\"__proto__\":{\"x\":1},\"v\":-0}")).unwrap();
    assert!(!parsed.same_identity(&second));
    let proto = heap.object().unwrap();
    heap.assign(&proto, "inherited".into(), heap.boolean(true).unwrap(), &NoRuntime).unwrap();
    heap.assign(&root, "__proto__".into(), proto.clone(), &NoRuntime).unwrap();
    assert!(heap.has(&root, &"inherited".into(), &NoRuntime).unwrap());
    assert!(!heap.has_own(&root, &"__proto__".into(), &NoRuntime).unwrap());
    heap.assign(&root, "__proto__".into(), heap.number(7.0).unwrap(), &NoRuntime).unwrap();
    assert!(heap.prototype(&root, &NoRuntime).unwrap().unwrap().same_identity(&proto));
    heap.assign(&root, "__proto__".into(), heap.null().unwrap(), &NoRuntime).unwrap();
    heap.assign(&root, "__proto__".into(), heap.number(8.0).unwrap(), &NoRuntime).unwrap();
    assert!(heap.prototype(&root, &NoRuntime).unwrap().is_none());
    assert_eq!(number(&heap, &heap.get(&root, &"__proto__".into(), &NoRuntime).unwrap()), 8.0);
    let ordered = heap.object().unwrap();
    for key in ["b", "10", "2", "a", "01"] {
        heap.assign(&ordered, key.into(), heap.undefined().unwrap(), &NoRuntime).unwrap();
    }
    let symbol = heap.symbol(Some("s".into())).unwrap();
    let symbol_key = PropertyKey::Symbol(heap.symbol_key(&symbol).unwrap());
    heap.define_own_data(&ordered, symbol_key.clone(), child.clone(), &NoRuntime).unwrap();
    assert_eq!(strings(heap.own_enumerable_string_keys(&ordered, &NoRuntime).unwrap()), ["2", "10", "b", "a", "01"]);
    heap.delete_own(&ordered, &"b".into(), &NoRuntime).unwrap();
    heap.assign(&ordered, "b".into(), heap.undefined().unwrap(), &NoRuntime).unwrap();
    assert_eq!(strings(heap.own_enumerable_string_keys(&ordered, &NoRuntime).unwrap()), ["2", "10", "a", "01", "b"]);
    assert!(
        matches!(heap.own_get(&ordered, &"a".into(), &NoRuntime).unwrap(), PropertyPresence::Present(value) if heap.kind(&value).unwrap() == ValueKind::Undefined)
    );
    assert!(matches!(heap.own_get(&ordered, &"missing".into(), &NoRuntime).unwrap(), PropertyPresence::Missing));
    heap.assign(&proto, "hidden".into(), child.clone(), &NoRuntime).unwrap();
    let inherited = heap.object_with_prototype(Some(&proto)).unwrap();
    heap.define_own(&inherited, "hidden".into(), data(heap.undefined().unwrap(), false, true), &NoRuntime).unwrap();
    assert_eq!(strings(heap.for_in_string_keys(&inherited, &NoRuntime).unwrap()), ["inherited"]);
    let lone = heap.string(WireString::from_units(vec![0xd800])).unwrap();
    assert!(matches!(heap.snapshot(&lone).unwrap(), ValueSnapshot::String(text) if text.units() == [0xd800]));
    assert!(heap.object_is(&heap.number(f64::NAN).unwrap(), &heap.number(f64::NAN).unwrap()).unwrap());
    assert!(!heap.object_is(&heap.number(-0.0).unwrap(), &heap.number(0.0).unwrap()).unwrap());
    let big = heap.bigint_decimal("-000900719925474099312345").unwrap();
    assert!(
        matches!(heap.snapshot(&big).unwrap(), ValueSnapshot::BigInt(value) if value.as_str() == "-900719925474099312345")
    );
    let foreign_heap = ValueHeap::new().unwrap();
    let foreign_symbol = foreign_heap.symbol(None).unwrap();
    let foreign_key = PropertyKey::Symbol(foreign_heap.symbol_key(&foreign_symbol).unwrap());
    assert!(matches!(heap.define_own_data(&ordered, foreign_key, child, &NoRuntime), Err(ValueError::ForeignHeap)));
    let cycle_heap = ValueHeap::new().unwrap();
    let weak = Arc::downgrade(&cycle_heap);
    let cycle = cycle_heap.object().unwrap();
    cycle_heap.assign(&cycle, "self".into(), cycle.clone(), &NoRuntime).unwrap();
    let cycle_edge = cycle_heap.capture_deferred(&cycle).unwrap();
    drop(cycle_heap);
    assert!(weak.upgrade().is_some());
    drop(cycle);
    assert!(weak.upgrade().is_none());
    assert!(matches!(heap.restore_deferred(&cycle_edge), Err(ValueError::ForeignHeap)));
}

fn schema_array_method_and_iterator_observations() {
    let heap = ValueHeap::new().unwrap();
    let array = heap.array_with_length(3).unwrap();
    heap.define_own_data(&array, "1".into(), heap.string("selected".into()).unwrap(), &NoRuntime).unwrap();
    heap.define_own_data(&heap.array_prototype(), "0".into(), heap.number(7.0).unwrap(), &NoRuntime).unwrap();
    let filter = heap.get(&array, &"filter".into(), &NoRuntime).unwrap();
    let mut visits = Vec::new();
    let mut predicate = |_: &ValueHandle, args: &[ValueHandle], _: &dyn ValueRuntime| {
        assert!(args[2].same_identity(&array));
        visits.push(number(&heap, &args[1]));
        if number(&heap, &args[1]) == 0.0 {
            heap.array_push(&array, &[heap.string("not visited".into())?], &NoRuntime)?;
        }
        heap.boolean(heap.kind(&args[0])? == ValueKind::String)
    };
    let filtered = heap.call_with_local_callback(&filter, &array, &mut predicate, &[], &NoRuntime).unwrap();
    assert_eq!(visits, vec![0.0, 1.0], "captured length, inherited index and holes");
    assert_eq!(heap.array_length(&filtered, &NoRuntime).unwrap(), 1);
    assert_eq!(json(&heap, &filtered, StringifyMode::Plain, &NoRuntime), Some("[\"selected\"]".into()));

    let some = heap.get(&array, &"some".into(), &NoRuntime).unwrap();
    let mut count = 0;
    let mut accept = |_: &ValueHandle, _: &[ValueHandle], _: &dyn ValueRuntime| {
        count += 1;
        heap.boolean(true)
    };
    assert!(heap.truthy(&heap.call_with_local_callback(&some, &array, &mut accept, &[], &NoRuntime).unwrap()).unwrap());
    assert_eq!(count, 1, "some does not visit later getters after selection");
    let every = heap.get(&array, &"every".into(), &NoRuntime).unwrap();
    let mut reject = |_: &ValueHandle, _: &[ValueHandle], _: &dyn ValueRuntime| heap.boolean(false);
    assert!(
        !heap.truthy(&heap.call_with_local_callback(&every, &array, &mut reject, &[], &NoRuntime).unwrap()).unwrap()
    );
    let includes = heap.get(&array, &"includes".into(), &NoRuntime).unwrap();
    assert!(heap.truthy(&heap.call(&includes, &array, &[heap.undefined().unwrap()], &NoRuntime).unwrap()).unwrap());

    struct MethodRuntime {
        heap: Arc<ValueHeap>,
        callback: Mutex<Option<ValueHandle>>,
        calls: Mutex<Vec<&'static str>>,
        thrown: ValueHandle,
    }
    impl ValueRuntime for MethodRuntime {
        fn resolve(&self, request: RuntimeRequest<'_>) -> Result<RuntimeReply, ValueError> {
            Err(ValueError::MissingCapability(request.capability()))
        }
        fn resolve_call(
            &self,
            capability: CapabilityId,
            receiver: &ValueHandle,
            args: &[ValueHandle],
            effective: &dyn ValueRuntime,
        ) -> Result<RuntimeReply, ValueError> {
            match capability.0 {
                910 => {
                    self.calls.lock().unwrap().push("override");
                    for (key, expected) in [("length", self.heap.number(1.0)?), ("name", self.heap.string("".into())?)]
                    {
                        let descriptor = self.heap.own_descriptor(&args[0], &key.into(), effective)?;
                        let Some(PropertyDescriptor::Data {
                            value,
                            writable: false,
                            enumerable: false,
                            configurable: true,
                        }) = descriptor
                        else {
                            panic!("Native callback metadata descriptor missing: {key}");
                        };
                        assert!(self.heap.object_is(&value, &expected)?);
                    }
                    *self.callback.lock().unwrap() = Some(args[0].clone());
                    let first = self.heap.get(receiver, &"0".into(), effective)?;
                    self.heap.call(&args[0], &self.heap.undefined()?, &[first], effective).map(RuntimeReply::Value)
                }
                911 => {
                    self.calls.lock().unwrap().push("return");
                    Err(ValueError::Thrown(self.thrown.clone()))
                }
                _ => Err(ValueError::MissingCapability(capability)),
            }
        }
    }
    let runtime = MethodRuntime {
        heap: heap.clone(),
        callback: Mutex::new(None),
        calls: Mutex::new(Vec::new()),
        thrown: heap.object().unwrap(),
    };
    heap.define_own_data(&array, "some".into(), heap.function(CapabilityId(910)).unwrap(), &NoRuntime).unwrap();
    let selected = heap.get(&array, &"some".into(), &runtime).unwrap();
    let mut selected_value = |_: &ValueHandle, args: &[ValueHandle], _: &dyn ValueRuntime| Ok(args[0].clone());
    let result = heap.call_with_local_callback(&selected, &array, &mut selected_value, &[], &runtime).unwrap();
    assert_eq!(number(&heap, &result), 7.0, "actual override invokes the scoped callback");
    assert_eq!(*runtime.calls.lock().unwrap(), vec!["override"]);
    let expired = runtime.callback.lock().unwrap().clone().unwrap();
    assert!(matches!(heap.call(&expired, &array, &[], &runtime), Err(ValueError::UnsupportedOperation(_))));

    let values = heap.get(&array, &"values".into(), &NoRuntime).unwrap();
    assert!(values.same_identity(&heap.get(&array, &heap.iterator_key(), &NoRuntime).unwrap()));
    let mut iterator = heap.get_iterator(&array, &NoRuntime).unwrap();
    assert_eq!(number(&heap, &heap.iterator_next(&mut iterator, &NoRuntime).unwrap().unwrap()), 7.0);
    heap.array_set_length(&array, 1, &NoRuntime).unwrap();
    assert!(heap.iterator_next(&mut iterator, &NoRuntime).unwrap().is_none(), "iterator reads current length");
    heap.array_push(&array, &[heap.number(8.0).unwrap()], &NoRuntime).unwrap();
    assert!(heap.iterator_next(&mut iterator, &NoRuntime).unwrap().is_none(), "done iterator stays done");
    // Primitive boxing is an explicit gap rather than an approximate success.
    let primitive = heap.string("abc".into()).unwrap();
    let mut unused = |_: &ValueHandle, _: &[ValueHandle], _: &dyn ValueRuntime| -> Result<ValueHandle, ValueError> {
        panic!("unsupported primitive callback must not run")
    };
    for method in [&filter, &some, &every] {
        assert!(matches!(
            heap.call_with_local_callback(method, &primitive, &mut unused, &[], &NoRuntime),
            Err(ValueError::UnsupportedOperation(_))
        ));
        assert!(matches!(
            heap.call_with_local_callback(method, &heap.null().unwrap(), &mut unused, &[], &NoRuntime),
            Err(ValueError::TypeError(_))
        ));
    }
    assert!(matches!(heap.call(&includes, &primitive, &[], &NoRuntime), Err(ValueError::UnsupportedOperation(_))));
    assert!(matches!(heap.call(&values, &primitive, &[], &NoRuntime), Err(ValueError::UnsupportedOperation(_))));
    let actual_iterator = heap.call(&values, &array, &[], &NoRuntime).unwrap();
    assert!(heap.get(&actual_iterator, &heap.iterator_key(), &NoRuntime).is_ok());
    heap.define_own_data(&actual_iterator, "return".into(), heap.function(CapabilityId(911)).unwrap(), &NoRuntime)
        .unwrap();
    let custom_method = heap.function(CapabilityId(912)).unwrap();
    struct IteratorRuntime<'a> {
        base: &'a MethodRuntime,
        iterator: ValueHandle,
    }
    impl ValueRuntime for IteratorRuntime<'_> {
        fn resolve(&self, request: RuntimeRequest<'_>) -> Result<RuntimeReply, ValueError> {
            self.base.resolve(request)
        }
        fn resolve_call(
            &self,
            cap: CapabilityId,
            recv: &ValueHandle,
            args: &[ValueHandle],
            effective: &dyn ValueRuntime,
        ) -> Result<RuntimeReply, ValueError> {
            if cap == CapabilityId(912) {
                Ok(RuntimeReply::Value(self.iterator.clone()))
            } else {
                self.base.resolve_call(cap, recv, args, effective)
            }
        }
    }
    heap.define_own_data(&array, heap.iterator_key(), custom_method, &NoRuntime).unwrap();
    let iterator_runtime = IteratorRuntime { base: &runtime, iterator: actual_iterator };
    let mut iterator = heap.get_iterator(&array, &iterator_runtime).unwrap();
    let error = heap.iterator_close(&mut iterator, &iterator_runtime).unwrap_err();
    assert!(matches!(error, ValueError::Thrown(value) if value.same_identity(&runtime.thrown)));
    assert!(heap.iterator_close(&mut iterator, &iterator_runtime).is_ok(), "no close replay");
}

#[test]
fn callbacks_reenter_actual_receiver_and_preserve_partial_error_effects() {
    compiled_observations::run();
    schema_array_method_and_iterator_observations();
    normalization_native_dependency_observations();
    array_at_native_observations();
    array_find_native_observations();
    context_shared_dependency_observations();
    shared_normalizers_runtime_observations();
    ark_wire_shared_dependency_observations();
    regexp_native_dependency_observations();
    let heap = ValueHeap::new().unwrap();
    let runtime = Runtime::new(&heap);
    let root = heap.object().unwrap();
    heap.define_own(&root, "a".into(), accessor(Some(1), None), &runtime).unwrap();
    assert!(matches!(
        heap.snapshot(&heap.get(&root, &"a".into(), &runtime).unwrap()).unwrap(),
        ValueSnapshot::Boolean(true)
    ));
    assert_eq!(number(&heap, &heap.get(&root, &"a".into(), &runtime).unwrap()), 9.0);
    let proto = heap.object().unwrap();
    heap.define_own(&proto, "setter".into(), accessor(None, Some(3)), &runtime).unwrap();
    heap.set_prototype(&root, Some(&proto), &runtime).unwrap();
    heap.assign(&root, "setter".into(), runtime.shared.clone(), &runtime).unwrap();
    assert!(heap.get(&root, &"setter".into(), &runtime).unwrap().same_identity(&runtime.shared));
    heap.define_own(&root, "b".into(), accessor(Some(2), None), &runtime).unwrap();
    assert!(
        matches!(heap.get(&root, &"b".into(), &runtime), Err(ValueError::Thrown(value)) if value.same_identity(&runtime.thrown))
    );
    assert!(heap.truthy(&heap.get(&runtime.shared, &"touched".into(), &runtime).unwrap()).unwrap());
    heap.define_own(&root, "expired".into(), accessor(Some(99), None), &runtime).unwrap();
    assert!(matches!(
        heap.get(&root, &"expired".into(), &runtime),
        Err(ValueError::ExpiredCapability(CapabilityId(99)))
    ));
    assert!(matches!(
        heap.get(&root, &"expired".into(), &NoRuntime),
        Err(ValueError::MissingCapability(CapabilityId(99)))
    ));
    let array = heap.array_with_length(0).unwrap();
    let value = heap.number(3.0).unwrap();
    let push = heap.get(&array, &"push".into(), &runtime).unwrap();
    assert_eq!(number(&heap, &heap.call(&push, &array, std::slice::from_ref(&value), &runtime).unwrap()), 1.0);
    heap.assign(&heap.array_prototype(), "push".into(), heap.function(CapabilityId(12)).unwrap(), &runtime).unwrap();
    let replacement = heap.get(&array, &"push".into(), &runtime).unwrap();
    assert!(matches!(heap.call(&replacement, &array, &[value], &runtime), Err(ValueError::Thrown(_))));
    assert_eq!(heap.array_length(&array, &runtime).unwrap(), 2);
    assert!(runtime.trace.lock().unwrap().iter().any(|event| event == "push:replacement"));
    let tail =
        heap.array(vec![heap.number(0.0).unwrap(), heap.number(1.0).unwrap(), heap.number(2.0).unwrap()]).unwrap();
    heap.define_own(&tail, "1".into(), data(heap.number(1.0).unwrap(), true, false), &runtime).unwrap();
    assert!(matches!(heap.array_set_length(&tail, 0, &runtime), Err(ValueError::TypeError(_))));
    assert_eq!(heap.array_length(&tail, &runtime).unwrap(), 2);
    assert!(matches!(heap.array_slot(&tail, 2, &runtime).unwrap(), PropertyPresence::Missing));
    for foreign in [false, true] {
        let thrown = if foreign {
            ValueHeap::new().unwrap().string("foreign throw".into()).unwrap()
        } else {
            heap.string("same-heap throw".into()).unwrap()
        };
        let fault = ThrowRuntime::new(&heap, thrown.clone(), false);
        let object = heap.opaque(OpaqueKind::Object, CapabilityId(61), u64::from(foreign)).unwrap();
        let array = heap.opaque(OpaqueKind::Array, CapabilityId(62), u64::from(foreign)).unwrap();
        let function = heap.function(CapabilityId(63)).unwrap();
        let key = PropertyKey::from("x");
        let observations = [
            ("OwnDescriptor", heap.own_descriptor(&object, &key, &fault).map(|_| ())),
            ("Prototype", heap.prototype(&object, &fault).map(|_| ())),
            ("OwnKeys", heap.own_keys(&object, &fault).map(|_| ())),
            ("ArrayLength", heap.array_length(&array, &fault).map(|_| ())),
            ("Get", heap.get(&object, &key, &fault).map(|_| ())),
            ("Set", heap.assign(&object, key.clone(), heap.boolean(true).unwrap(), &fault)),
            ("Has", heap.has(&object, &key, &fault).map(|_| ())),
            ("Call", heap.call(&function, &root, &[], &fault).map(|_| ())),
        ];
        for (label, observation) in observations {
            let error = observation.expect_err(label);
            if foreign {
                assert!(matches!(&error, ValueError::ForeignHeap), "{label}: {error:?}");
                assert!(!error.is_native_exception());
            } else {
                assert!(
                    matches!(&error, ValueError::Thrown(value) if value.same_identity(&thrown)),
                    "{label}: {error:?}"
                );
                assert!(error.is_native_exception());
            }
        }
        assert_eq!(
            *fault.trace.lock().unwrap(),
            ["OwnDescriptor", "Prototype", "OwnKeys", "ArrayLength", "Get", "Set", "Has", "Call"]
        );
        assert_eq!(number(&heap, &heap.get(&fault.shared, &"touched".into(), &NoRuntime).unwrap()), 8.0);
    }
    guarded_slot_observations();
    initial_own_capture_observations();
}

/// One coarse scenario set for the actual shared dependencies of optional-null.
fn array_at_native_observations() {
    let heap = ValueHeap::new().unwrap();
    let array =
        heap.array(vec![heap.number(10.0).unwrap(), heap.number(20.0).unwrap(), heap.number(30.0).unwrap()]).unwrap();
    let at = heap.get(&array, &"at".into(), &NoRuntime).unwrap();
    for (index, expected) in [
        (0.0, Some(10.0)),
        (-1.0, Some(30.0)),
        (-3.0, Some(10.0)),
        (-4.0, None),
        (1.9, Some(20.0)),
        (-1.9, Some(30.0)),
        (-0.5, Some(10.0)),
        (f64::NAN, Some(10.0)),
        (f64::INFINITY, None),
        (f64::NEG_INFINITY, None),
        (3.0, None),
    ] {
        let value = heap.call(&at, &array, &[heap.number(index).unwrap()], &NoRuntime).unwrap();
        match expected {
            Some(expected) => assert_eq!(number(&heap, &value), expected),
            None => assert_eq!(heap.kind(&value).unwrap(), ValueKind::Undefined),
        }
    }
    assert_eq!(number(&heap, &heap.call(&at, &array, &[], &NoRuntime).unwrap()), 10.0);
    assert!(matches!(
        heap.call(&at, &array, &[heap.bigint_decimal("1").unwrap()], &NoRuntime),
        Err(ValueError::TypeError(_))
    ));
    let PropertyDescriptor::Data { writable, enumerable, configurable, .. } =
        heap.own_descriptor(&heap.array_prototype(), &"at".into(), &NoRuntime).unwrap().unwrap()
    else {
        panic!("at must be a real data method")
    };
    assert!(writable && !enumerable && configurable);
    assert_eq!(number(&heap, &heap.get(&at, &"length".into(), &NoRuntime).unwrap()), 1.0);
    assert!(
        matches!(heap.snapshot(&heap.get(&at, &"name".into(), &NoRuntime).unwrap()).unwrap(), ValueSnapshot::String(name) if name.equals_ascii("at"))
    );
    for key in ["name", "length"] {
        assert!(matches!(
            heap.own_descriptor(&at, &key.into(), &NoRuntime).unwrap(),
            Some(PropertyDescriptor::Data { writable: false, enumerable: false, configurable: true, .. })
        ));
    }

    let sparse = heap.array_with_length(2).unwrap();
    let prototype = heap.object().unwrap();
    let inherited = heap.object().unwrap();
    heap.define_own_data(&prototype, "0".into(), inherited.clone(), &NoRuntime).unwrap();
    heap.set_prototype(&sparse, Some(&prototype), &NoRuntime).unwrap();
    assert!(heap.call(&at, &sparse, &[heap.number(0.0).unwrap()], &NoRuntime).unwrap().same_identity(&inherited));
    assert_eq!(
        heap.kind(&heap.call(&at, &sparse, &[heap.number(1.0).unwrap()], &NoRuntime).unwrap()).unwrap(),
        ValueKind::Undefined
    );

    struct AtEffects {
        heap: Arc<ValueHeap>,
        target: ValueHandle,
        selected: ValueHandle,
        trace: Mutex<Vec<u64>>,
    }
    impl ValueRuntime for AtEffects {
        fn resolve(&self, request: RuntimeRequest<'_>) -> Result<RuntimeReply, ValueError> {
            self.trace.lock().unwrap().push(request.capability().0);
            match request.capability().0 {
                410 => Ok(RuntimeReply::Value(self.heap.number(3.0)?)),
                411 => {
                    self.heap.define_own_data(&self.target, "length".into(), self.heap.number(1.0)?, &NoRuntime)?;
                    Ok(RuntimeReply::Value(self.heap.number(-1.9)?))
                }
                412 => Ok(RuntimeReply::Value(self.selected.clone())),
                _ => Err(ValueError::MissingCapability(request.capability())),
            }
        }
    }
    let target = heap.object().unwrap();
    heap.define_own(&target, "length".into(), accessor(Some(410), None), &NoRuntime).unwrap();
    heap.define_own(&target, "2".into(), accessor(Some(412), None), &NoRuntime).unwrap();
    let index = heap.object().unwrap();
    heap.define_own_data(&index, "valueOf".into(), heap.function(CapabilityId(411)).unwrap(), &NoRuntime).unwrap();
    let runtime = AtEffects {
        heap: heap.clone(),
        target: target.clone(),
        selected: heap.object().unwrap(),
        trace: Mutex::new(Vec::new()),
    };
    assert!(heap.call(&at, &target, std::slice::from_ref(&index), &runtime).unwrap().same_identity(&runtime.selected));
    assert_eq!(*runtime.trace.lock().unwrap(), [410, 411, 412], "capture length before index coercion, then live Get");
    assert_eq!(number(&heap, &heap.get(&target, &"length".into(), &NoRuntime).unwrap()), 1.0);
    runtime.trace.lock().unwrap().clear();
    heap.define_own_data(&target, "length".into(), heap.bigint_decimal("3").unwrap(), &NoRuntime).unwrap();
    assert!(matches!(heap.call(&at, &target, std::slice::from_ref(&index), &runtime), Err(ValueError::TypeError(_))));
    assert!(runtime.trace.lock().unwrap().is_empty(), "BigInt length throws before selected index coercion");
    for invalid in [heap.null().unwrap(), heap.undefined().unwrap()] {
        assert!(matches!(
            heap.call(&at, &invalid, std::slice::from_ref(&index), &runtime),
            Err(ValueError::TypeError(_))
        ));
    }
    assert!(runtime.trace.lock().unwrap().is_empty());
}

/// Native agent-loop resolveToolForCall uses two actual tools.find calls.
fn array_find_native_observations() {
    let heap = ValueHeap::new().unwrap();
    let array = heap.array_with_length(3).unwrap();
    let find = heap.get(&array, &"find".into(), &NoRuntime).unwrap();
    assert_eq!(number(&heap, &heap.get(&find, &"length".into(), &NoRuntime).unwrap()), 1.0);
    assert!(matches!(heap.snapshot(&heap.get(&find, &"name".into(), &NoRuntime).unwrap()).unwrap(),
        ValueSnapshot::String(name) if name.equals_ascii("find")));
    let target = heap.object().unwrap();
    let inherited = heap.object().unwrap();
    let selected = heap.object().unwrap();
    let prototype = heap.object().unwrap();
    heap.define_own_data(&prototype, "1".into(), inherited.clone(), &NoRuntime).unwrap();
    heap.set_prototype(&target, Some(&prototype), &NoRuntime).unwrap();
    heap.define_own_data(&target, "length".into(), heap.number(3.0).unwrap(), &NoRuntime).unwrap();
    let this_arg = heap.object().unwrap();
    let mut visited = Vec::new();
    let mut predicate = |receiver: &ValueHandle, args: &[ValueHandle], _: &dyn ValueRuntime| {
        assert!(receiver.same_identity(&this_arg));
        assert!(args[2].same_identity(&target));
        let index = number(&heap, &args[1]) as u64;
        visited.push(index);
        match index {
            0 => {
                assert_eq!(heap.kind(&args[0])?, ValueKind::Undefined);
                heap.define_own_data(&target, "length".into(), heap.number(1.0)?, &NoRuntime)?;
                heap.define_own_data(&target, "2".into(), selected.clone(), &NoRuntime)?;
            }
            1 => assert!(args[0].same_identity(&inherited)),
            2 => assert!(args[0].same_identity(&selected)),
            _ => panic!("captured length must bound traversal"),
        }
        heap.boolean(index == 2)
    };
    let found = heap
        .call_with_local_callback(&find, &target, &mut predicate, std::slice::from_ref(&this_arg), &NoRuntime)
        .unwrap();
    assert!(found.same_identity(&selected));
    assert_eq!(visited, [0, 1, 2], "visit holes, inherited slots and live mutations using captured length");
    let mut calls = 0;
    let mut no_match = |_: &ValueHandle, args: &[ValueHandle], _: &dyn ValueRuntime| {
        calls += 1;
        assert_eq!(heap.kind(&args[0])?, ValueKind::Undefined);
        heap.boolean(false)
    };
    assert_eq!(
        heap.kind(&heap.call_with_local_callback(&find, &array, &mut no_match, &[], &NoRuntime).unwrap()).unwrap(),
        ValueKind::Undefined
    );
    assert_eq!(calls, 3, "find does not skip absent elements");
    let thrown = heap.symbol(None).unwrap();
    let mut throw = |_: &ValueHandle, _: &[ValueHandle], _: &dyn ValueRuntime| Err(ValueError::Thrown(thrown.clone()));
    assert!(matches!(heap.call_with_local_callback(&find, &array, &mut throw, &[], &NoRuntime),
        Err(ValueError::Thrown(value)) if value.same_identity(&thrown)));
    assert!(matches!(heap.call(&find, &array, &[], &NoRuntime), Err(ValueError::TypeError(_))));
    for invalid in [heap.null().unwrap(), heap.undefined().unwrap()] {
        assert!(matches!(heap.call(&find, &invalid, &[], &NoRuntime), Err(ValueError::TypeError(_))));
    }
}

fn normalization_native_dependency_observations() {
    let heap = ValueHeap::new().unwrap();
    // Required by fixed error/rate-limit.ts's ordinary error and Google paths.
    // Exercise the real prototype dispatch in the existing native dependency group.
    {
        let text = heap.string("abaaba".into()).unwrap();
        let method = heap.get(&text, &"lastIndexOf".into(), &NoRuntime).unwrap();
        let search = heap.string("aba".into()).unwrap();
        assert_eq!(number(&heap, &heap.call(&method, &text, &[search.clone()], &NoRuntime).unwrap()), 3.0);
        assert_eq!(
            number(
                &heap,
                &heap.call(&method, &text, &[search.clone(), heap.number(2.9).unwrap()], &NoRuntime).unwrap()
            ),
            0.0
        );
        assert_eq!(
            number(&heap, &heap.call(&method, &text, &[search, heap.number(f64::NAN).unwrap()], &NoRuntime).unwrap()),
            3.0
        );
        let text = heap.string("a😀a😀".into()).unwrap();
        assert_eq!(
            number(&heap, &heap.call(&method, &text, &[heap.string("😀".into()).unwrap()], &NoRuntime).unwrap()),
            4.0
        );
        assert_eq!(
            number(
                &heap,
                &heap
                    .call(&method, &text, &[heap.string("".into()).unwrap(), heap.number(2.0).unwrap()], &NoRuntime)
                    .unwrap()
            ),
            2.0
        );
        assert_eq!(number(&heap, &heap.get(&method, &"length".into(), &NoRuntime).unwrap()), 1.0);
        let text = heap.string(WireString::from_units(vec![0xd800, 0x00df, 0x0069, 0xdf00])).unwrap();
        let method = heap.get(&text, &"toUpperCase".into(), &NoRuntime).unwrap();
        let upper = heap.call(&method, &text, &[], &NoRuntime).unwrap();
        assert!(
            matches!(heap.snapshot(&upper).unwrap(), ValueSnapshot::String(value) if value.units() == [0xd800, 0x0053, 0x0053, 0x0049, 0xdf00])
        );
        assert_eq!(number(&heap, &heap.get(&method, &"length".into(), &NoRuntime).unwrap()), 0.0);
        assert!(matches!(heap.call(&method, &heap.null().unwrap(), &[], &NoRuntime), Err(ValueError::TypeError(_))));
    }
    let object = heap.object().unwrap();
    let distinct = heap.object().unwrap();
    let symbol = heap.symbol(None).unwrap();
    let members = vec![
        heap.number(-0.0).unwrap(),
        heap.number(f64::NAN).unwrap(),
        object.clone(),
        symbol.clone(),
        heap.bigint_decimal("9007199254740993").unwrap(),
        heap.string("same".into()).unwrap(),
    ];
    let set = heap.new_set(&heap.array(members.clone()).unwrap(), &NoRuntime).unwrap();
    let size = PropertyKey::from("size");
    assert_eq!(number(&heap, &heap.get(&set, &size, &NoRuntime).unwrap()), 6.0);
    assert!(heap.own_descriptor(&set, &size, &NoRuntime).unwrap().is_none());
    assert!(matches!(
        heap.own_descriptor(&heap.set_intrinsic_prototype(), &size, &NoRuntime).unwrap(),
        Some(PropertyDescriptor::Accessor {
            get: Some(CallableTarget::Intrinsic(Intrinsic::SetSizeGet)),
            set: None,
            enumerable: false,
            configurable: true,
        })
    ));
    assert!(matches!(heap.get(&heap.set_intrinsic_prototype(), &size, &NoRuntime), Err(ValueError::TypeError(_))));
    assert!(matches!(
        heap.assign(&set, size.clone(), heap.number(99.0).unwrap(), &NoRuntime),
        Err(ValueError::TypeError(_))
    ));
    // This Set contains a Symbol: native Set serialization reaches and rejects it.
    assert!(matches!(heap.structured_clone(&set, &NoRuntime), Err(ValueError::DataCloneError(_))));
    let container = heap.object().unwrap();
    heap.define_own_data(&container, "set".into(), set.clone(), &NoRuntime).unwrap();
    let fallback = heap.structured_clone_json(&container, &NoRuntime).unwrap();
    assert_eq!(json(&heap, &fallback, StringifyMode::Plain, &NoRuntime), Some("{\"set\":{}}".into()));
    let has = heap.get(&set, &"has".into(), &NoRuntime).unwrap();
    let add = heap.get(&set, &"add".into(), &NoRuntime).unwrap();
    for value in members.into_iter().chain([
        heap.number(0.0).unwrap(),
        heap.number(f64::NAN).unwrap(),
        heap.string("same".into()).unwrap(),
    ]) {
        assert!(heap.truthy(&heap.call(&has, &set, &[value], &NoRuntime).unwrap()).unwrap());
    }
    assert!(!heap.truthy(&heap.call(&has, &set, &[distinct], &NoRuntime).unwrap()).unwrap());
    assert!(heap.call(&add, &set, &[], &NoRuntime).unwrap().same_identity(&set));
    assert_eq!(number(&heap, &heap.get(&set, &size, &NoRuntime).unwrap()), 7.0);
    heap.call(&add, &set, &[heap.number(-0.0).unwrap()], &NoRuntime).unwrap();
    assert_eq!(number(&heap, &heap.get(&set, &size, &NoRuntime).unwrap()), 7.0);
    assert!(heap.truthy(&heap.call(&has, &set, &[], &NoRuntime).unwrap()).unwrap());
    assert!(matches!(heap.call(&has, &object, &[], &NoRuntime), Err(ValueError::TypeError(_))));
    assert!(matches!(
        heap.new_set(&ValueHeap::new().unwrap().array(Vec::new()).unwrap(), &NoRuntime),
        Err(ValueError::ForeignHeap)
    ));
    assert_eq!(number(&heap, &heap.get(&has, &"length".into(), &NoRuntime).unwrap()), 1.0);

    struct Effects {
        heap: Arc<ValueHeap>,
        source: ValueHandle,
        returned: ValueHandle,
        iterator: ValueHandle,
        next_result: ValueHandle,
        thrown: ValueHandle,
        trace: Mutex<Vec<u64>>,
        host_close: bool,
    }
    impl ValueRuntime for Effects {
        fn resolve(&self, request: RuntimeRequest<'_>) -> Result<RuntimeReply, ValueError> {
            let RuntimeRequest::Call { capability, .. } = request else {
                return Err(ValueError::MissingCapability(request.capability()));
            };
            self.trace.lock().unwrap().push(capability.0);
            match capability.0 {
                10 => self.heap.function(CapabilityId(14)).map(RuntimeReply::Value),
                11 => Ok(RuntimeReply::Value(self.iterator.clone())),
                12 => Ok(RuntimeReply::Value(self.next_result.clone())),
                14 => Err(ValueError::Thrown(self.thrown.clone())),
                15 if self.host_close => Err(ValueError::MissingCapability(capability)),
                15 => Err(ValueError::Thrown(self.returned.clone())),
                20 => {
                    self.heap.delete_own(&self.source, &"b".into(), &NoRuntime)?;
                    Ok(RuntimeReply::Value(self.returned.clone()))
                }
                _ => Err(ValueError::MissingCapability(capability)),
            }
        }
    }
    for host_close in [false, true] {
        let heap = ValueHeap::new().unwrap();
        let iterator = heap.object().unwrap();
        heap.define_own_data(&iterator, "next".into(), heap.function(CapabilityId(12)).unwrap(), &NoRuntime).unwrap();
        heap.define_own_data(&iterator, "return".into(), heap.function(CapabilityId(15)).unwrap(), &NoRuntime).unwrap();
        let result = heap.object().unwrap();
        heap.define_own_data(&result, "done".into(), heap.boolean(false).unwrap(), &NoRuntime).unwrap();
        heap.define_own_data(&result, "value".into(), heap.number(3.0).unwrap(), &NoRuntime).unwrap();
        let source = heap.array(Vec::new()).unwrap();
        heap.define_own_data(&source, heap.iterator_key(), heap.function(CapabilityId(11)).unwrap(), &NoRuntime)
            .unwrap();
        heap.define_own(&heap.set_intrinsic_prototype(), "add".into(), accessor(Some(10), None), &NoRuntime).unwrap();
        let runtime = Effects {
            heap: heap.clone(),
            source: source.clone(),
            returned: heap.object().unwrap(),
            iterator,
            next_result: result,
            thrown: heap.object().unwrap(),
            trace: Mutex::new(Vec::new()),
            host_close,
        };
        let error = heap.new_set(&source, &runtime).unwrap_err();
        if host_close {
            let ValueError::IteratorClose { original, close } = error else { panic!("missing dual failure") };
            assert!(matches!(*original, ValueError::Thrown(value) if value.same_identity(&runtime.thrown)));
            assert!(matches!(*close, ValueError::MissingCapability(CapabilityId(15))));
        } else {
            assert!(matches!(error, ValueError::Thrown(value) if value.same_identity(&runtime.thrown)));
        }
        assert_eq!(*runtime.trace.lock().unwrap(), [10, 11, 12, 14, 15]);
    }
    let heap = ValueHeap::new().unwrap();
    let source = heap.object().unwrap();
    heap.define_own(&source, "a".into(), accessor(Some(20), None), &NoRuntime).unwrap();
    heap.define_own_data(&source, "b".into(), heap.number(1.0).unwrap(), &NoRuntime).unwrap();
    let runtime = Effects {
        heap: heap.clone(),
        source: source.clone(),
        returned: heap.object().unwrap(),
        iterator: heap.object().unwrap(),
        next_result: heap.object().unwrap(),
        thrown: heap.object().unwrap(),
        trace: Mutex::new(Vec::new()),
        host_close: false,
    };
    let entries = heap.object_entries(&source, &runtime).unwrap();
    assert_eq!(heap.array_length(&entries, &NoRuntime).unwrap(), 1);
    let pair = heap.get(&entries, &"0".into(), &NoRuntime).unwrap();
    assert!(heap.get(&pair, &"1".into(), &NoRuntime).unwrap().same_identity(&runtime.returned));
    assert_eq!(*runtime.trace.lock().unwrap(), [20]);
    assert!(matches!(heap.object_entries(&heap.null().unwrap(), &NoRuntime), Err(ValueError::TypeError(_))));
    let text = heap.string(WireString::from_units(vec![0xfeff, 0x2000, 0xd800, 0xa0, 0x2029])).unwrap();
    let trim = heap.get(&text, &"trim".into(), &NoRuntime).unwrap();
    assert!(matches!(heap.snapshot(&heap.call(&trim, &text, &[], &NoRuntime).unwrap()).unwrap(),
        ValueSnapshot::String(value) if value.units() == [0xd800]));
    for invalid in [heap.null().unwrap(), heap.undefined().unwrap(), heap.symbol(None).unwrap()] {
        assert!(matches!(heap.call(&trim, &invalid, &[], &NoRuntime), Err(ValueError::TypeError(_))));
    }
    assert_eq!(number(&heap, &heap.get(&trim, &"length".into(), &NoRuntime).unwrap()), 0.0);
}

#[test]
fn shallow_and_two_path_clone_operations_follow_fixed_source() {
    let heap = ValueHeap::new().unwrap();
    let runtime = Runtime::new(&heap);
    let child = heap.object().unwrap();
    let source = heap.object().unwrap();
    heap.define_own_data(&source, "__proto__".into(), child.clone(), &runtime).unwrap();
    heap.assign(&source, "other".into(), child.clone(), &runtime).unwrap();
    let symbol = heap.symbol(None).unwrap();
    let key = PropertyKey::Symbol(heap.symbol_key(&symbol).unwrap());
    heap.define_own_data(&source, key.clone(), child.clone(), &runtime).unwrap();
    let shallow = heap.spread(&source, &runtime).unwrap();
    assert!(heap.get(&shallow, &"__proto__".into(), &runtime).unwrap().same_identity(&child));
    assert!(heap.has_own(&shallow, &key, &runtime).unwrap());
    assert!(heap.prototype(&shallow, &runtime).unwrap().unwrap().same_identity(&heap.object_prototype()));
    let deep = heap.structured_clone_json(&source, &runtime).unwrap();
    let copied = heap.get(&deep, &"other".into(), &runtime).unwrap();
    assert!(!copied.same_identity(&child));
    assert!(copied.same_identity(&heap.get(&deep, &"__proto__".into(), &runtime).unwrap()));
    assert!(!heap.has_own(&deep, &key, &runtime).unwrap());
    heap.assign(&source, "self".into(), source.clone(), &runtime).unwrap();
    let cyclic = heap.structured_clone_json(&source, &runtime).unwrap();
    assert!(heap.get(&cyclic, &"self".into(), &runtime).unwrap().same_identity(&cyclic));
    let array = heap.array_with_length(3).unwrap();
    heap.assign(&array, "0".into(), heap.undefined().unwrap(), &runtime).unwrap();
    heap.assign(&array, "2".into(), child.clone(), &runtime).unwrap();
    assert!(matches!(heap.array_slot(&array, 1, &runtime).unwrap(), PropertyPresence::Missing));
    let sliced = heap.array_slice(&array, 0.0, None, &runtime).unwrap();
    assert!(matches!(heap.array_slot(&sliced, 1, &runtime).unwrap(), PropertyPresence::Missing));
    assert!(heap.get(&sliced, &"2".into(), &runtime).unwrap().same_identity(&child));
    let removed = heap.array_splice(&sliced, 0.0, Some(1.0), &[], &runtime).unwrap();
    assert!(
        matches!(heap.array_slot(&removed, 0, &runtime).unwrap(), PropertyPresence::Present(value) if heap.kind(&value).unwrap() == ValueKind::Undefined)
    );
    assert_eq!(heap.array_length(&sliced, &runtime).unwrap(), 2);
    assert!(matches!(heap.array_slot(&sliced, 0, &runtime).unwrap(), PropertyPresence::Missing));
    let fallback = heap.object().unwrap();
    heap.assign(&fallback, "function".into(), heap.function(CapabilityId(7)).unwrap(), &runtime).unwrap();
    heap.assign(&fallback, "keep".into(), heap.number(2.0).unwrap(), &runtime).unwrap();
    let fallback = heap.structured_clone_json(&fallback, &runtime).unwrap();
    assert_eq!(json(&heap, &fallback, StringifyMode::Plain, &runtime), Some("{\"keep\":2}".into()));
    let custom = heap.object_with_prototype(None).unwrap();
    heap.assign(&custom, "nan".into(), heap.number(f64::NAN).unwrap(), &runtime).unwrap();
    heap.assign(&custom, "undefined".into(), heap.undefined().unwrap(), &runtime).unwrap();
    assert_eq!(
        json(&heap, &heap.structured_clone_json(&custom, &runtime).unwrap(), StringifyMode::Plain, &runtime),
        Some("{\"nan\":null}".into())
    );
    let throwing = heap.object().unwrap();
    heap.define_own(&throwing, "b".into(), accessor(Some(2), None), &runtime).unwrap();
    assert!(matches!(heap.structured_clone_json(&throwing, &runtime), Err(ValueError::Thrown(_))));
    assert_eq!(runtime.trace.lock().unwrap().iter().filter(|event| event.as_str() == "get:throw").count(), 2);
    let expired = heap.object().unwrap();
    heap.define_own(&expired, "x".into(), accessor(Some(99), None), &runtime).unwrap();
    assert!(matches!(
        heap.structured_clone_json(&expired, &runtime),
        Err(ValueError::ExpiredCapability(CapabilityId(99)))
    ));
    assert_eq!(runtime.trace.lock().unwrap().iter().filter(|event| event.as_str() == "capability:expired").count(), 1);
    assert!(ValueError::Thrown(runtime.thrown.clone()).is_native_exception());
    assert!(ValueError::DataCloneError("function".into()).is_native_exception());
    assert!(!ValueError::ForeignHeap.is_native_exception());
    assert!(!ValueError::MissingCapability(CapabilityId(31)).is_native_exception());
    let opaque = heap.opaque(OpaqueKind::Object, CapabilityId(31), 1).unwrap();
    let aliases = heap.object().unwrap();
    heap.assign(&aliases, "a".into(), opaque.clone(), &runtime).unwrap();
    heap.assign(&aliases, "b".into(), opaque.clone(), &runtime).unwrap();
    let cloned = heap.structured_clone(&aliases, &runtime).unwrap();
    let a = heap.get(&cloned, &"a".into(), &runtime).unwrap();
    let b = heap.get(&cloned, &"b".into(), &runtime).unwrap();
    assert!(a.same_identity(&b));
    assert!(!a.same_identity(&opaque));
    let function = heap.function(CapabilityId(7)).unwrap();
    assert!(heap.structured_clone_json(&function, &runtime).unwrap().same_identity(&function));
    for foreign in [false, true] {
        let thrown = if foreign {
            ValueHeap::new().unwrap().string("foreign clone throw".into()).unwrap()
        } else {
            heap.string("native clone throw".into()).unwrap()
        };
        let fault = ThrowRuntime::new(&heap, thrown, true);
        let object = heap.opaque(OpaqueKind::Object, CapabilityId(64), u64::from(foreign)).unwrap();
        let cloned = heap.structured_clone_json(&object, &fault);
        if foreign {
            assert!(matches!(cloned, Err(ValueError::ForeignHeap)));
            assert_eq!(*fault.trace.lock().unwrap(), ["Prototype", "StructuredClone"]);
        } else {
            assert_eq!(json(&heap, &cloned.unwrap(), StringifyMode::Plain, &NoRuntime), Some("{}".into()));
            assert_eq!(*fault.trace.lock().unwrap(), ["Prototype", "StructuredClone", "Get", "OwnKeys"]);
        }
        assert_eq!(number(&heap, &heap.get(&fault.shared, &"touched".into(), &NoRuntime).unwrap()), 2.0);
    }
}

#[test]
fn json_modes_keyed_callbacks_full_units_and_checked_true_tree_bridge() {
    json_replacer_observations::observe_callable_replacer();
    json_native_observations::observe_native_dispatch();
    let heap = ValueHeap::new().unwrap();
    let runtime = Runtime::new(&heap);
    let value = heap.object().unwrap();
    heap.assign(&value, "toJSON".into(), heap.function(CapabilityId(10)).unwrap(), &runtime).unwrap();
    let wrapper = heap.object().unwrap();
    heap.define_own_data(&wrapper, "arguments".into(), value.clone(), &runtime).unwrap();
    assert!(matches!(heap.stringify(&wrapper, StringifyMode::Plain, None, &runtime), Err(ValueError::TypeError(_))));
    assert_eq!(
        json(&heap, &wrapper, StringifyMode::BigIntReplacer, &runtime),
        Some("{\"arguments\":\"9007199254740993123456789\"}".into())
    );
    assert!(runtime.trace.lock().unwrap().iter().all(|event| event.starts_with("toJSON:arguments:")));
    heap.assign(&heap.bigint_prototype(), "toString".into(), heap.function(CapabilityId(11)).unwrap(), &runtime)
        .unwrap();
    assert_eq!(json(&heap, &wrapper, StringifyMode::BigIntReplacer, &runtime), Some("{\"arguments\":true}".into()));
    assert!(runtime.trace.lock().unwrap().iter().any(|event| event == "bigint:toString:replacement"));
    heap.assign(&heap.bigint_prototype(), "toString".into(), heap.function(CapabilityId(2)).unwrap(), &runtime)
        .unwrap();
    assert!(
        matches!(heap.stringify(&wrapper, StringifyMode::BigIntReplacer, None, &runtime), Err(ValueError::Thrown(value)) if value.same_identity(&runtime.thrown))
    );
    let boxed = heap.opaque(OpaqueKind::BoxedBigInt, CapabilityId(30), 1).unwrap();
    assert!(matches!(
        heap.stringify(&boxed, StringifyMode::BigIntReplacer, None, &runtime),
        Err(ValueError::TypeError(_))
    ));
    let symbol = heap.symbol(None).unwrap();
    let array =
        heap.array(vec![heap.undefined().unwrap(), symbol.clone(), heap.number(f64::INFINITY).unwrap()]).unwrap();
    assert_eq!(json(&heap, &array, StringifyMode::Plain, &runtime), Some("[null,null,null]".into()));
    assert!(json(&heap, &symbol, StringifyMode::Plain, &runtime).is_none());
    assert!(json(&heap, &heap.function(CapabilityId(8)).unwrap(), StringifyMode::Plain, &runtime).is_none());
    let lone = heap.string(WireString::from_units(vec![0xd800])).unwrap();
    assert_eq!(json(&heap, &lone, StringifyMode::Plain, &runtime), Some("\"\\ud800\"".into()));
    let gap = JsonSpace::String(WireString::from_units(vec![0xd800]));
    let text = heap.stringify(&array, StringifyMode::Plain, Some(&gap), &runtime).unwrap().unwrap();
    assert!(text.units().contains(&0xd800));
    assert!(text.to_utf8().is_err());
    let child = heap.object().unwrap();
    let aliases = heap.object().unwrap();
    heap.assign(&aliases, "a".into(), child.clone(), &runtime).unwrap();
    heap.assign(&aliases, "b".into(), child.clone(), &runtime).unwrap();
    assert_eq!(json(&heap, &aliases, StringifyMode::Plain, &runtime), Some("{\"a\":{},\"b\":{}}".into()));
    assert!(matches!(heap.export_serde(&aliases), Err(ValueError::LegacyBridge(_))));
    heap.assign(&child, "self".into(), child.clone(), &runtime).unwrap();
    assert!(matches!(heap.stringify(&child, StringifyMode::Plain, None, &runtime), Err(ValueError::TypeError(_))));
    let tree = heap.import_serde(&serde_json::json!({"a": [1, true, null], "z": "ok"})).unwrap();
    assert_eq!(heap.export_serde(&tree).unwrap(), serde_json::json!({"a": [1, true, null], "z": "ok"}));
    let integer = heap.export_serde(&heap.number(1.0).unwrap()).unwrap();
    assert_eq!(integer.as_u64(), Some(1));
    assert_eq!(serde_json::from_value::<u64>(integer).unwrap(), 1);
    assert_eq!(heap.export_serde(&heap.number(-1.0).unwrap()).unwrap().as_i64(), Some(-1));
    assert_eq!(heap.export_serde(&heap.number(i64::MIN as f64).unwrap()).unwrap().as_i64(), Some(i64::MIN));
    let high = f64::from_bits((u64::MAX as f64).to_bits() - 1);
    assert_eq!(heap.export_serde(&heap.number(high).unwrap()).unwrap().as_u64(), Some(18_446_744_073_709_549_568));
    for value in [
        -0.0,
        0.0,
        1.5,
        i64::MIN as f64,
        9_223_372_036_854_775_808.0,
        high,
        u64::MAX as f64,
        -18_446_744_073_709_551_616.0,
        f64::MIN_POSITIVE,
    ] {
        let projected = heap.export_serde(&heap.number(value).unwrap()).unwrap();
        assert_eq!(projected.as_f64().unwrap().to_bits(), value.to_bits());
        if value == u64::MAX as f64 {
            assert_eq!(projected.as_u64(), None);
            assert_eq!(projected.as_i64(), None);
        }
    }
    assert!(matches!(heap.export_serde(&lone), Err(ValueError::LegacyBridge(_))));
    assert!(matches!(heap.export_serde(&heap.number(f64::INFINITY).unwrap()), Err(ValueError::LegacyBridge(_))));
    assert!(matches!(
        heap.export_serde(&heap.bigint_decimal("9007199254740993").unwrap()),
        Err(ValueError::LegacyBridge(_))
    ));
    assert!(matches!(heap.export_serde(&heap.array_with_length(1).unwrap()), Err(ValueError::LegacyBridge(_))));
    assert!(matches!(heap.export_serde(&heap.object_with_prototype(None).unwrap()), Err(ValueError::LegacyBridge(_))));
    assert!(matches!(heap.import_serde(&serde_json::json!(9007199254740993u64)), Err(ValueError::LegacyBridge(_))));
}

// Existing carrier family: module symbol publication, actual extensibility and
// fixed draft String methods. No new micro-test target.
fn context_shared_dependency_observations() {
    let heap = ValueHeap::new().unwrap();
    // Cold publication: no module-symbol lookup precedes the raced workers.
    let barrier = Arc::new(std::sync::Barrier::new(8));
    let mut workers = Vec::new();
    for _ in 0..8 {
        let heap = Arc::clone(&heap);
        let barrier = Arc::clone(&barrier);
        workers.push(std::thread::spawn(move || {
            barrier.wait();
            heap.module_symbol("schema/stamps", "cache").unwrap()
        }));
    }
    let expected = workers.remove(0).join().unwrap();
    for worker in workers {
        assert_eq!(worker.join().unwrap(), expected);
    }
    assert_eq!(heap.module_symbol("schema/stamps", "cache").unwrap(), expected);
    assert_ne!(heap.module_symbol("other", "cache").unwrap(), expected);
    assert_ne!(heap.module_symbol("schema/stamps", "epoch").unwrap(), expected);
    let foreign = ValueHeap::new().unwrap();
    assert_ne!(foreign.module_symbol("schema/stamps", "cache").unwrap(), expected);
    assert!(matches!(foreign.get(&foreign.object().unwrap(), &expected, &NoRuntime), Err(ValueError::ForeignHeap)));
    let record = heap.object().unwrap();
    heap.define_own_data(&record, expected.clone(), heap.number(3.0).unwrap(), &NoRuntime).unwrap();
    assert_eq!(number(&heap, &heap.get(&record, &expected, &NoRuntime).unwrap()), 3.0);
    assert!(heap.is_extensible(&record, &NoRuntime).unwrap());
    heap.set_extensible(&record, false, &NoRuntime).unwrap();
    assert!(!heap.is_extensible(&record, &NoRuntime).unwrap());
    assert!(!heap.is_extensible(&heap.undefined().unwrap(), &NoRuntime).unwrap());
    assert!(!heap.is_extensible(&heap.symbol(None).unwrap(), &NoRuntime).unwrap());
    assert!(matches!(heap.is_extensible(&foreign.object().unwrap(), &NoRuntime), Err(ValueError::ForeignHeap)));
    struct ExtensibleRuntime {
        heap: Arc<ValueHeap>,
        touched: ValueHandle,
        thrown: ValueHandle,
    }
    impl ValueRuntime for ExtensibleRuntime {
        fn resolve(&self, request: RuntimeRequest<'_>) -> Result<RuntimeReply, ValueError> {
            let RuntimeRequest::Opaque { capability, target, receiver, operation: OpaqueOperation::IsExtensible } =
                request
            else {
                return Err(ValueError::MissingCapability(request.capability()));
            };
            assert!(target.same_identity(receiver));
            self.heap.assign(&self.touched, "queried".into(), self.heap.boolean(true)?, &NoRuntime)?;
            match capability.0 {
                7201 => Ok(RuntimeReply::Boolean(false)),
                7202 => Ok(RuntimeReply::Unit),
                7203 => Err(ValueError::Thrown(self.thrown.clone())),
                _ => Err(ValueError::ExpiredCapability(capability)),
            }
        }
    }
    let runtime =
        ExtensibleRuntime { heap: Arc::clone(&heap), touched: heap.object().unwrap(), thrown: heap.object().unwrap() };
    for (id, expected) in [(7201, 0), (7202, 1), (7203, 2), (7204, 3)] {
        let opaque = heap.opaque(OpaqueKind::Object, CapabilityId(id), id).unwrap();
        match (expected, heap.is_extensible(&opaque, &runtime)) {
            (0, Ok(false)) | (1, Err(ValueError::WrongRuntimeReply("Boolean"))) => {}
            (2, Err(ValueError::Thrown(value))) => assert!(value.same_identity(&runtime.thrown)),
            (3, Err(ValueError::ExpiredCapability(CapabilityId(7204)))) => {}
            _ => panic!("wrong actual extensibility result"),
        }
    }
    assert!(heap.truthy(&heap.get(&runtime.touched, &"queried".into(), &NoRuntime).unwrap()).unwrap());
    let opaque = heap.opaque(OpaqueKind::Object, CapabilityId(7299), 7299).unwrap();
    assert!(matches!(heap.is_extensible(&opaque, &NoRuntime), Err(ValueError::MissingCapability(CapabilityId(7299)))));

    let text = heap.string(WireString::from_units(vec![0x23, 0x2f, 0xd800, 0x61, 0xdc00])).unwrap();
    let starts = heap.get(&text, &"startsWith".into(), &NoRuntime).unwrap();
    let ends = heap.get(&text, &"endsWith".into(), &NoRuntime).unwrap();
    let slice = heap.get(&text, &"slice".into(), &NoRuntime).unwrap();
    assert_eq!(number(&heap, &heap.get(&starts, &"length".into(), &NoRuntime).unwrap()), 1.0);
    assert_eq!(number(&heap, &heap.get(&ends, &"length".into(), &NoRuntime).unwrap()), 1.0);
    assert_eq!(number(&heap, &heap.get(&slice, &"length".into(), &NoRuntime).unwrap()), 2.0);
    for (function, name) in [(&starts, "startsWith"), (&ends, "endsWith"), (&slice, "slice")] {
        assert!(
            matches!(heap.snapshot(&heap.get(function, &"name".into(), &NoRuntime).unwrap()).unwrap(), ValueSnapshot::String(s) if s.equals_ascii(name))
        );
    }
    let search = heap.string(WireString::from_units(vec![0xd800])).unwrap();
    for (position, suffix, expected) in [
        (None, vec![0xdc00], true),
        (Some(3.9), vec![0xd800], true),
        (Some(f64::NAN), vec![], true),
        (Some(f64::NEG_INFINITY), vec![0x23], false),
        (Some(f64::INFINITY), vec![0x61, 0xdc00], true),
    ] {
        let mut args = vec![heap.string(WireString::from_units(suffix)).unwrap()];
        if let Some(position) = position {
            args.push(heap.number(position).unwrap());
        }
        assert_eq!(heap.truthy(&heap.call(&ends, &text, &args, &NoRuntime).unwrap()).unwrap(), expected);
    }
    assert!(
        heap.truthy(
            &heap
                .call(
                    &ends,
                    &text,
                    &[heap.string(WireString::from_units(vec![0xdc00])).unwrap(), heap.undefined().unwrap()],
                    &NoRuntime
                )
                .unwrap()
        )
        .unwrap()
    );
    let end_matcher = heap.object().unwrap();
    heap.define_own_data(&end_matcher, heap.match_key(), heap.boolean(true).unwrap(), &NoRuntime).unwrap();
    assert!(matches!(heap.call(&ends, &text, &[end_matcher], &NoRuntime), Err(ValueError::TypeError(_))));
    assert!(matches!(heap.call(&ends, &heap.null().unwrap(), &[], &NoRuntime), Err(ValueError::TypeError(_))));
    assert!(
        heap.truthy(&heap.call(&starts, &text, &[search, heap.number(2.9).unwrap()], &NoRuntime).unwrap()).unwrap()
    );
    assert!(
        heap.truthy(
            &heap
                .call(
                    &starts,
                    &text,
                    &[heap.string("".into()).unwrap(), heap.number(f64::INFINITY).unwrap()],
                    &NoRuntime
                )
                .unwrap()
        )
        .unwrap()
    );
    assert!(!heap.truthy(&heap.call(&starts, &text, std::slice::from_ref(&record), &NoRuntime).unwrap()).unwrap());
    let matcher = heap.object().unwrap();
    heap.define_own_data(&matcher, heap.match_key(), heap.boolean(true).unwrap(), &NoRuntime).unwrap();
    assert!(matches!(heap.call(&starts, &text, &[matcher], &NoRuntime), Err(ValueError::TypeError(_))));
    assert!(matches!(
        heap.call(&starts, &text, &[heap.symbol(None).unwrap()], &NoRuntime),
        Err(ValueError::TypeError(_))
    ));
    let sliced =
        heap.call(&slice, &text, &[heap.number(2.0).unwrap(), heap.number(-1.0).unwrap()], &NoRuntime).unwrap();
    assert!(matches!(heap.snapshot(&sliced).unwrap(), ValueSnapshot::String(s) if s.units()==[0xd800,0x61]));
    for (start, end, units) in
        [(f64::NEG_INFINITY, 2.0, vec![0x23, 0x2f]), (f64::NAN, 0.0, vec![]), (f64::INFINITY, 0.0, vec![])]
    {
        let value =
            heap.call(&slice, &text, &[heap.number(start).unwrap(), heap.number(end).unwrap()], &NoRuntime).unwrap();
        assert!(matches!(heap.snapshot(&value).unwrap(), ValueSnapshot::String(s) if s.units()==units));
    }
    assert!(matches!(heap.call(&slice, &heap.null().unwrap(), &[], &NoRuntime), Err(ValueError::TypeError(_))));
    assert!(matches!(heap.call(&slice, &heap.symbol(None).unwrap(), &[], &NoRuntime), Err(ValueError::TypeError(_))));
    assert!(matches!(
        heap.call(&slice, &text, &[heap.bigint_decimal("1").unwrap()], &NoRuntime),
        Err(ValueError::TypeError(_))
    ));
    struct StringRuntime {
        heap: Arc<ValueHeap>,
        text: ValueHandle,
        thrown: ValueHandle,
        trace: Mutex<Vec<(u64, bool)>>,
    }
    impl ValueRuntime for StringRuntime {
        fn resolve(&self, request: RuntimeRequest<'_>) -> Result<RuntimeReply, ValueError> {
            let RuntimeRequest::Opaque {
                capability,
                target,
                receiver,
                operation: OpaqueOperation::ToPrimitive { string_hint },
            } = request
            else {
                return Err(ValueError::MissingCapability(request.capability()));
            };
            assert!(target.same_identity(receiver));
            self.trace.lock().unwrap().push((capability.0, string_hint));
            // Reentry proves conversion is outside the arena guard.
            let _ = self.heap.object()?;
            match capability.0 {
                7301 => Ok(RuntimeReply::Value(self.text.clone())),
                7302 => Ok(RuntimeReply::Value(self.heap.number(2.0)?)),
                7303 => Ok(RuntimeReply::Value(self.heap.number(4.0)?)),
                7304 => Err(ValueError::Thrown(self.thrown.clone())),
                _ => Err(ValueError::ExpiredCapability(capability)),
            }
        }
    }
    let string_runtime = StringRuntime {
        heap: Arc::clone(&heap),
        text: text.clone(),
        thrown: heap.object().unwrap(),
        trace: Mutex::new(Vec::new()),
    };
    let receiver = heap.opaque(OpaqueKind::Object, CapabilityId(7301), 7301).unwrap();
    let start = heap.opaque(OpaqueKind::Object, CapabilityId(7302), 7302).unwrap();
    let end = heap.opaque(OpaqueKind::Object, CapabilityId(7303), 7303).unwrap();
    let failure = heap.opaque(OpaqueKind::Object, CapabilityId(7304), 7304).unwrap();
    let converted = heap.call(&slice, &receiver, &[start.clone(), end.clone()], &string_runtime).unwrap();
    assert!(matches!(heap.snapshot(&converted).unwrap(),ValueSnapshot::String(s) if s.units()==[0xd800,0x61]));
    assert_eq!(*string_runtime.trace.lock().unwrap(), [(7301, true), (7302, false), (7303, false)]);
    string_runtime.trace.lock().unwrap().clear();
    assert!(
        matches!(heap.call(&slice,&receiver,&[failure,end],&string_runtime),Err(ValueError::Thrown(v)) if v.same_identity(&string_runtime.thrown))
    );
    assert_eq!(*string_runtime.trace.lock().unwrap(), [(7301, true), (7304, false)]);
    string_runtime.trace.lock().unwrap().clear();
    let search = heap.string(WireString::from_units(vec![0xd800])).unwrap();
    assert!(heap.truthy(&heap.call(&starts, &receiver, &[search, start.clone()], &string_runtime).unwrap()).unwrap());
    assert_eq!(*string_runtime.trace.lock().unwrap(), [(7301, true), (7302, false)]);
    string_runtime.trace.lock().unwrap().clear();
    assert!(!heap.truthy(&heap.call(&starts, &receiver, &[record, start], &string_runtime).unwrap()).unwrap());
    assert_eq!(*string_runtime.trace.lock().unwrap(), [(7301, true), (7302, false)]);
}

// Attach to the existing callback coarse family; no new test entrance.
fn shared_normalizers_runtime_observations() {
    let heap = ValueHeap::new().unwrap();
    let call = |receiver: &ValueHandle, name: &str, args: &[ValueHandle]| {
        let method = heap.get(receiver, &name.into(), &NoRuntime).unwrap();
        heap.call(&method, receiver, args, &NoRuntime).unwrap()
    };
    let text = |value: &ValueHandle| {
        let ValueSnapshot::String(text) = heap.snapshot(value).unwrap() else {
            panic!("expected string");
        };
        text
    };
    for (input, method, expected) in
        [(" \tX \r", "trimStart", "X \r"), (" \tX \r", "trimEnd", " \tX"), ("YeS", "toLowerCase", "yes")]
    {
        assert!(text(&call(&heap.string(input.into()).unwrap(), method, &[])).equals_ascii(expected));
    }
    let units = heap.string(WireString::from_units(vec![0xd800, 0x41, 0xdc00])).unwrap();
    assert_eq!(number(&heap, &call(&units, "charCodeAt", &[heap.number(1.0).unwrap()])), 65.0);
    assert_eq!(text(&call(&units, "toLowerCase", &[])).units(), &[0xd800, 0x61, 0xdc00]);
    let control =
        call(&heap.string(WireString::from_units(vec![0x0b])).unwrap(), "charCodeAt", &[heap.number(0.0).unwrap()]);
    let hex = call(&control, "toString", &[heap.number(16.0).unwrap()]);
    assert!(
        text(&call(&hex, "padStart", &[heap.number(4.0).unwrap(), heap.string("0".into()).unwrap()]))
            .equals_ascii("000b")
    );
    assert!(
        text(&call(&heap.bigint_decimal("11").unwrap(), "toString", &[heap.number(16.0).unwrap()])).equals_ascii("b")
    );
    assert!(text(&call(&heap.number(1.5).unwrap(), "toString", &[heap.number(16.0).unwrap()])).equals_ascii("1.8"));
    assert!(
        text(&heap.add(&heap.string("1".into()).unwrap(), &heap.number(2.0).unwrap(), &NoRuntime).unwrap())
            .equals_ascii("12")
    );
    assert!(
        matches!(heap.snapshot(&heap.add(&heap.bigint_decimal("999").unwrap(),&heap.bigint_decimal("-1000").unwrap(),&NoRuntime).unwrap()).unwrap(),ValueSnapshot::BigInt(v) if v.as_str()=="-1")
    );
    assert!(
        matches!(heap.snapshot(&heap.increment(&heap.bigint_decimal("-1").unwrap(),&NoRuntime).unwrap()).unwrap(),ValueSnapshot::BigInt(v) if v.as_str()=="0")
    );
    assert!(matches!(
        heap.add(&heap.bigint_decimal("1").unwrap(), &heap.number(1.0).unwrap(), &NoRuntime),
        Err(ValueError::TypeError(_))
    ));

    struct MapRuntime {
        heap: Arc<ValueHeap>,
        seen: Mutex<Vec<f64>>,
        throw_on_second: bool,
        thrown: ValueHandle,
    }
    impl ValueRuntime for MapRuntime {
        fn resolve(&self, request: RuntimeRequest<'_>) -> Result<RuntimeReply, ValueError> {
            let RuntimeRequest::Call { capability: CapabilityId(7601), arguments, .. } = request else {
                return Err(ValueError::MissingCapability(request.capability()));
            };
            let index = number(&self.heap, &arguments[1]);
            self.seen.lock().unwrap().push(index);
            // Mutation after captured length must not extend map traversal.
            self.heap.assign(&arguments[2], "3".into(), self.heap.number(99.0)?, &NoRuntime)?;
            if self.throw_on_second && index == 2.0 {
                return Err(ValueError::Thrown(self.thrown.clone()));
            }
            Ok(RuntimeReply::Value(self.heap.number(number(&self.heap, &arguments[0]) * 2.0)?))
        }
    }
    let array = heap.array_with_length(3).unwrap();
    heap.define_own_data(&array, "0".into(), heap.number(3.0).unwrap(), &NoRuntime).unwrap();
    heap.define_own_data(&array, "2".into(), heap.number(4.0).unwrap(), &NoRuntime).unwrap();
    let runtime = MapRuntime {
        heap: heap.clone(),
        seen: Mutex::new(Vec::new()),
        throw_on_second: false,
        thrown: heap.object().unwrap(),
    };
    let map = heap.get(&array, &"map".into(), &NoRuntime).unwrap();
    let result = heap.call(&map, &array, &[heap.function(CapabilityId(7601)).unwrap()], &runtime).unwrap();
    assert_eq!(*runtime.seen.lock().unwrap(), [0.0, 2.0]);
    assert_eq!(number(&heap, &heap.get(&result, &"length".into(), &NoRuntime).unwrap()), 3.0);
    assert!(!heap.has_own(&result, &"1".into(), &NoRuntime).unwrap());
    assert_eq!(number(&heap, &heap.get(&result, &"2".into(), &NoRuntime).unwrap()), 8.0);
    // Set iterator is live through add and stays done after its first exhaustion.
    let set = heap.new_set(&heap.array(vec![heap.string("a".into()).unwrap()]).unwrap(), &NoRuntime).unwrap();
    let mut iterator = heap.get_iterator(&set, &NoRuntime).unwrap();
    assert!(text(&heap.iterator_next(&mut iterator, &NoRuntime).unwrap().unwrap()).equals_ascii("a"));
    call(&set, "add", &[heap.string("b".into()).unwrap()]);
    assert!(text(&heap.iterator_next(&mut iterator, &NoRuntime).unwrap().unwrap()).equals_ascii("b"));
    assert!(heap.iterator_next(&mut iterator, &NoRuntime).unwrap().is_none());
    call(&set, "add", &[heap.string("c".into()).unwrap()]);
    assert!(heap.iterator_next(&mut iterator, &NoRuntime).unwrap().is_none());

    const SETS: &[(&str, &[&str])] = &[("first", &["a"]), ("second", &["b"])];
    let barrier = Arc::new(std::sync::Barrier::new(2));
    let mut threads = Vec::new();
    for _ in 0..2 {
        let heap = heap.clone();
        let barrier = barrier.clone();
        threads.push(std::thread::spawn(move || {
            barrier.wait();
            heap.prepare_module_string_sets("shared_observations", SETS, &NoRuntime).unwrap();
            heap.module_string_set("shared_observations", "first", &["a"]).unwrap()
        }));
    }
    let a = threads.remove(0).join().unwrap();
    let b = threads.remove(0).join().unwrap();
    assert!(a.same_identity(&b));
    assert!(matches!(
        heap.module_string_set("shared_observations", "first", &["other"]),
        Err(ValueError::SourceIdentityCollision)
    ));
    let brackets = heap.module_string_array("shared_observations", "brackets", &["[", "]", "{", "}"]).unwrap();
    assert!(
        brackets.same_identity(
            &heap.module_string_array("shared_observations", "brackets", &["[", "]", "{", "}"]).unwrap()
        )
    );
    // Failed initialization does not replay an effectful adder and never
    // publishes the first half of a literal group.
    struct InitRuntime {
        heap: Arc<ValueHeap>,
        calls: std::sync::atomic::AtomicUsize,
        thrown: ValueHandle,
    }
    impl ValueRuntime for InitRuntime {
        fn resolve(&self, request: RuntimeRequest<'_>) -> Result<RuntimeReply, ValueError> {
            let RuntimeRequest::Call { capability: CapabilityId(7602), receiver, .. } = request else {
                return Err(ValueError::MissingCapability(request.capability()));
            };
            let call = self.calls.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            if call == 0 {
                return Ok(RuntimeReply::Value(receiver.clone()));
            }
            assert!(matches!(
                self.heap.prepare_module_string_sets("failed_observations", SETS, self),
                Err(ValueError::UnsupportedOperation(_))
            ));
            Err(ValueError::Thrown(self.thrown.clone()))
        }
    }
    let failed = ValueHeap::new().unwrap();
    let runtime = InitRuntime {
        heap: failed.clone(),
        calls: std::sync::atomic::AtomicUsize::new(0),
        thrown: failed.object().unwrap(),
    };
    let set = failed.new_set(&failed.undefined().unwrap(), &NoRuntime).unwrap();
    let proto = failed.prototype(&set, &NoRuntime).unwrap().unwrap();
    failed.assign(&proto, "add".into(), failed.function(CapabilityId(7602)).unwrap(), &NoRuntime).unwrap();
    for _ in 0..2 {
        assert!(
            matches!(failed.prepare_module_string_sets("failed_observations",SETS,&runtime),Err(ValueError::Thrown(v)) if v.same_identity(&runtime.thrown))
        );
    }
    assert_eq!(runtime.calls.load(std::sync::atomic::Ordering::SeqCst), 2);
    assert!(
        matches!(failed.module_string_set("failed_observations","first",&["a"]),Err(ValueError::Thrown(v)) if v.same_identity(&runtime.thrown))
    );
    let weak = Arc::downgrade(&failed);
    drop(proto);
    drop(set);
    drop(runtime);
    drop(failed);
    assert!(weak.upgrade().is_none(), "retained module failure must not hold a heap lease");

    struct ConversionRuntime {
        heap: Arc<ValueHeap>,
        thrown: ValueHandle,
        calls: Mutex<Vec<u64>>,
    }
    impl ValueRuntime for ConversionRuntime {
        fn resolve(&self, request: RuntimeRequest<'_>) -> Result<RuntimeReply, ValueError> {
            let RuntimeRequest::Call { capability, arguments, .. } = request else {
                return Err(ValueError::MissingCapability(request.capability()));
            };
            self.calls.lock().unwrap().push(capability.0);
            match capability.0 {
                7603 => Err(ValueError::Thrown(self.thrown.clone())),
                7604 => {
                    assert!(
                        matches!(self.heap.snapshot(&arguments[0])?,ValueSnapshot::String(hint) if hint.equals_ascii("default"))
                    );
                    Ok(RuntimeReply::Value(self.heap.string("x".into())?))
                }
                _ => Err(ValueError::ExpiredCapability(capability)),
            }
        }
    }
    let conversion =
        ConversionRuntime { heap: heap.clone(), thrown: heap.object().unwrap(), calls: Mutex::new(Vec::new()) };
    let separator = heap.object().unwrap();
    heap.define_own_data(&separator, "toString".into(), heap.function(CapabilityId(7603)).unwrap(), &NoRuntime)
        .unwrap();
    let input = heap.string("a/b".into()).unwrap();
    let split = heap.get(&input, &"split".into(), &NoRuntime).unwrap();
    assert!(
        matches!(heap.call(&split,&input,&[separator,heap.number(0.0).unwrap()],&conversion),Err(ValueError::Thrown(v)) if v.same_identity(&conversion.thrown))
    );
    assert_eq!(*conversion.calls.lock().unwrap(), [7603]);
    let left = heap.object().unwrap();
    heap.define_own_data(&left, heap.to_primitive_key(), heap.function(CapabilityId(7604)).unwrap(), &NoRuntime)
        .unwrap();
    assert!(text(&heap.add(&left, &heap.number(4.0).unwrap(), &conversion).unwrap()).equals_ascii("x4"));
    assert_eq!(*conversion.calls.lock().unwrap(), [7603, 7604]);
}

// Root integration: append this helper to tests/value_heap.rs and call
// regexp_native_dependency_observations() from the existing normalization
// coarse family. This fragment adds no #[test] target and has not been run.
fn regexp_native_dependency_observations() {
    let heap = ValueHeap::new().unwrap();
    let call = |receiver: &ValueHandle, name: &str, args: &[ValueHandle]| {
        let method = heap.get(receiver, &name.into(), &NoRuntime).unwrap();
        heap.call(&method, receiver, args, &NoRuntime).unwrap()
    };
    let text = |value: &ValueHandle| {
        let ValueSnapshot::String(value) = heap.snapshot(value).unwrap() else { panic!("expected UTF16 string") };
        value
    };
    for (pattern, accepted, rejected) in [
        (r"^[+-]?(?:0|[1-9]\d*)(?:\.\d+)?(?:[eE][+-]?\d+)?$", "-12.5e+3", "01"),
        (r"^[+-]?\d+(?:\.\d+)?(?:[eE][+-]?\d+)?$", "01", "one"),
        (r"^[A-Za-z_$][A-Za-z0-9_$]*", "$field_1", "9bad"),
        (r"^\[(\d+)\]", "[12]", "[x]"),
        (r"^[\w.$-]{1,128}$", "field.$-", "bad field"),
    ] {
        let regex = heap.regexp(pattern.into(), "").unwrap();
        assert!(heap.truthy(&call(&regex, "test", &[heap.string(accepted.into()).unwrap()])).unwrap());
        assert!(!heap.truthy(&call(&regex, "test", &[heap.string(rejected.into()).unwrap()])).unwrap());
    }
    // Bash approval's fixed upstream u flag uses this existing engine; astral
    // input must consume one code point while receipts keep UTF16 indices.
    let astral_units = vec![0xd83d, 0xde00];
    let astral = heap.string(WireString::from_units(astral_units.clone())).unwrap();
    let unicode = heap.regexp("(.)".into(), "gu").unwrap();
    assert!(heap.truthy(&heap.get(&unicode, &"unicode".into(), &NoRuntime).unwrap()).unwrap());
    assert!(text(&heap.get(&unicode, &"flags".into(), &NoRuntime).unwrap()).equals_ascii("gu"));
    let input = heap.string(WireString::from_units(vec![0x61, 0xd83d, 0xde00, 0xd800])).unwrap();
    heap.assign(&unicode, "lastIndex".into(), heap.number(1.0).unwrap(), &NoRuntime).unwrap();
    let found = call(&unicode, "exec", &[input.clone()]);
    assert_eq!(text(&heap.get(&found, &"0".into(), &NoRuntime).unwrap()).units(), &astral_units);
    assert_eq!(text(&heap.get(&found, &"1".into(), &NoRuntime).unwrap()).units(), &astral_units);
    assert_eq!(number(&heap, &heap.get(&found, &"index".into(), &NoRuntime).unwrap()), 1.0);
    assert_eq!(number(&heap, &heap.get(&unicode, &"lastIndex".into(), &NoRuntime).unwrap()), 3.0);
    let lone_found = call(&unicode, "exec", &[input.clone()]);
    assert_eq!(text(&heap.get(&lone_found, &"0".into(), &NoRuntime).unwrap()).units(), &[0xd800]);
    assert_eq!(number(&heap, &heap.get(&unicode, &"lastIndex".into(), &NoRuntime).unwrap()), 4.0);
    assert_eq!(heap.kind(&call(&unicode, "exec", &[input])).unwrap(), ValueKind::Null);
    assert_eq!(number(&heap, &heap.get(&unicode, &"lastIndex".into(), &NoRuntime).unwrap()), 0.0);
    let raw_pattern = heap.regexp(WireString::from_units(astral_units.clone()), "u").unwrap();
    assert!(heap.truthy(&call(&raw_pattern, "test", &[astral.clone()])).unwrap());
    let scalar_pattern = heap.regexp(r"^\u{1F600}$".into(), "u").unwrap();
    assert!(heap.truthy(&call(&scalar_pattern, "test", &[astral.clone()])).unwrap());
    assert!(!heap.truthy(&call(&heap.regexp("^.$".into(), "").unwrap(), "test", &[astral.clone()])).unwrap());
    assert!(!heap.truthy(&heap.get(&heap.regexp(".".into(), "").unwrap(), &"unicode".into(), &NoRuntime).unwrap()).unwrap());
    let parts = call(&astral, "split", &[heap.regexp("(?:)".into(), "u").unwrap()]);
    assert_eq!(number(&heap, &heap.get(&parts, &"length".into(), &NoRuntime).unwrap()), 1.0);
    assert_eq!(text(&heap.get(&parts, &"0".into(), &NoRuntime).unwrap()).units(), &astral_units);
    for flags in ["uu", "uv", "guu"] {
        assert!(matches!(heap.regexp("x".into(), flags), Err(ValueError::SyntaxError(_))));
    }
    for flags in ["d", "m", "v", "y"] {
        assert!(matches!(heap.regexp("x".into(), flags), Err(ValueError::UnsupportedOperation(_))));
    }
    assert!(matches!(heap.regexp(r"\q".into(), "u"), Err(ValueError::SyntaxError(_))));
    let line_end = heap.module_regexp("regex_observations", "line_end", r"[\r\n]+$", "").unwrap();
    assert!(line_end.same_identity(&heap.module_regexp("regex_observations", "line_end", r"[\r\n]+$", "").unwrap()));
    assert!(matches!(
        heap.module_regexp("regex_observations", "line_end", "other", ""),
        Err(ValueError::SourceIdentityCollision)
    ));
    let input = heap.string(WireString::from_units(vec![0xd800, 0x61, 0x0d, 0x0a])).unwrap();
    let trimmed = call(&input, "replace", &[line_end, heap.string("".into()).unwrap()]);
    assert_eq!(text(&trimmed).units(), &[0xd800, 0x61]);
    let pointer = heap.string("a~1b~0c~1d".into()).unwrap();
    let pointer =
        call(&pointer, "replace", &[heap.regexp("~1".into(), "g").unwrap(), heap.string("/".into()).unwrap()]);
    let pointer =
        call(&pointer, "replace", &[heap.regexp("~0".into(), "g").unwrap(), heap.string("~".into()).unwrap()]);
    assert!(text(&pointer).equals_ascii("a/b~c/d"));
    let captures = heap.regexp("(a)(b)?".into(), "").unwrap();
    let substitution = call(
        &heap.string("a".into()).unwrap(),
        "replace",
        &[captures, heap.string("$01/$2/$12/$$/$&".into()).unwrap()],
    );
    assert!(text(&substitution).equals_ascii("a//a2/$/a"));
    let lone = heap.regexp(WireString::from_units(vec![0xd800]), "g").unwrap();
    let found = call(&lone, "exec", &[heap.string(WireString::from_units(vec![0x61, 0xd800, 0x62])).unwrap()]);
    assert_eq!(number(&heap, &heap.get(&found, &"index".into(), &NoRuntime).unwrap()), 1.0);
    assert_eq!(number(&heap, &heap.get(&lone, &"lastIndex".into(), &NoRuntime).unwrap()), 2.0);
    let missing = call(&lone, "exec", &[heap.string("x".into()).unwrap()]);
    assert_eq!(heap.kind(&missing).unwrap(), ValueKind::Null);
    assert_eq!(number(&heap, &heap.get(&lone, &"lastIndex".into(), &NoRuntime).unwrap()), 0.0);
    heap.define_own(
        &lone,
        "lastIndex".into(),
        PropertyDescriptor::Data {
            value: heap.number(0.0).unwrap(),
            writable: false,
            enumerable: false,
            configurable: false,
        },
        &NoRuntime,
    )
    .unwrap();
    let exec = heap.get(&lone, &"exec".into(), &NoRuntime).unwrap();
    assert!(matches!(
        heap.call(&exec, &lone, &[heap.string(WireString::from_units(vec![0xd800])).unwrap()], &NoRuntime),
        Err(ValueError::TypeError(_))
    ));
    assert!(matches!(heap.regexp("[".into(), ""), Err(ValueError::SyntaxError(_))));
    assert!(matches!(heap.regexp("x".into(), "gg"), Err(ValueError::SyntaxError(_))));

    // Generic @@replace obtains every exec result before invoking replacement
    // callbacks. Its global collection Get(0) and output Get(0) both run.
    struct RegexRuntime {
        heap: Arc<ValueHeap>,
        results: Vec<ValueHandle>,
        count: Mutex<usize>,
        trace: Mutex<Vec<&'static str>>,
    }
    impl ValueRuntime for RegexRuntime {
        fn resolve(&self, request: RuntimeRequest<'_>) -> Result<RuntimeReply, ValueError> {
            match request {
                RuntimeRequest::Call { capability: CapabilityId(710), .. } => {
                    self.trace.lock().unwrap().push("get_exec");
                    Ok(RuntimeReply::Value(self.heap.function(CapabilityId(711))?))
                }
                RuntimeRequest::Call { capability: CapabilityId(711), .. } => {
                    self.trace.lock().unwrap().push("call_exec");
                    let index = {
                        let mut count = self.count.lock().unwrap();
                        let index = *count;
                        *count += 1;
                        index
                    };
                    Ok(RuntimeReply::Value(self.results.get(index).cloned().unwrap_or(self.heap.null()?)))
                }
                RuntimeRequest::Call { capability: CapabilityId(712), receiver, .. } => {
                    self.trace.lock().unwrap().push("get_zero");
                    Ok(RuntimeReply::Value(self.heap.get(receiver, &"matched".into(), self)?))
                }
                RuntimeRequest::Call { capability: CapabilityId(713), arguments, .. } => {
                    self.trace.lock().unwrap().push("replace_callback");
                    let ValueSnapshot::String(matched) = self.heap.snapshot(&arguments[0])? else {
                        return Err(ValueError::WrongRuntimeReply("replacement match"));
                    };
                    Ok(RuntimeReply::Value(self.heap.string(if matched.equals_ascii("a") { "A" } else { "B" }.into())?))
                }
                _ => Err(ValueError::MissingCapability(request.capability())),
            }
        }
    }
    let generic = heap.object().unwrap();
    for (name, value) in [
        ("global", heap.boolean(true).unwrap()),
        ("unicode", heap.boolean(false).unwrap()),
        ("unicodeSets", heap.boolean(false).unwrap()),
    ] {
        heap.define_own_data(&generic, name.into(), value, &NoRuntime).unwrap();
    }
    heap.define_own(&generic, "exec".into(), accessor(Some(710), None), &NoRuntime).unwrap();
    let mut results = Vec::new();
    for (index, matched) in ["a", "b"].into_iter().enumerate() {
        let result = heap.object().unwrap();
        for (name, value) in [
            ("length", heap.number(1.0).unwrap()),
            ("index", heap.number(index as f64).unwrap()),
            ("matched", heap.string(matched.into()).unwrap()),
        ] {
            heap.define_own_data(&result, name.into(), value, &NoRuntime).unwrap();
        }
        heap.define_own(&result, "0".into(), accessor(Some(712), None), &NoRuntime).unwrap();
        results.push(result);
    }
    let runtime =
        RegexRuntime { heap: Arc::clone(&heap), results, count: Mutex::new(0), trace: Mutex::new(Vec::new()) };
    let builtin = heap.regexp("x".into(), "").unwrap();
    let replace = heap.get(&builtin, &heap.replace_key(), &NoRuntime).unwrap();
    let output = heap
        .call(
            &replace,
            &generic,
            &[heap.string("ab".into()).unwrap(), heap.function(CapabilityId(713)).unwrap()],
            &runtime,
        )
        .unwrap();
    assert!(text(&output).equals_ascii("AB"));
    assert_eq!(
        runtime.trace.lock().unwrap().as_slice(),
        &[
            "get_exec",
            "call_exec",
            "get_zero",
            "get_exec",
            "call_exec",
            "get_zero",
            "get_exec",
            "call_exec",
            "get_zero",
            "replace_callback",
            "get_zero",
            "replace_callback",
        ]
    );
    regexp_split_and_dot_all_observations();
}

// Real Native split + /is dependencies, grouped into the existing coarse family.
// This helper does not accept general RegExp/realm, custom species Construct or v.
fn regexp_split_and_dot_all_observations() {
    regexp_at_offset_dependency_observations();
    fn text(heap: &Arc<ValueHeap>, value: &ValueHandle) -> WireString {
        let ValueSnapshot::String(text) = heap.snapshot(value).unwrap() else { panic!("expected string") };
        text
    }
    fn parts(heap: &Arc<ValueHeap>, value: &ValueHandle) -> Vec<Option<WireString>> {
        let length = number(heap, &heap.get(value, &"length".into(), &NoRuntime).unwrap()) as usize;
        (0..length)
            .map(|index| {
                let item = heap.get(value, &WireString::from(index.to_string()).into(), &NoRuntime).unwrap();
                match heap.snapshot(&item).unwrap() {
                    ValueSnapshot::String(text) => Some(text),
                    ValueSnapshot::Undefined => None,
                    _ => panic!("expected string or unmatched capture"),
                }
            })
            .collect()
    }
    let heap = ValueHeap::new().unwrap();
    let split = heap.get(&heap.string("".into()).unwrap(), &"split".into(), &NoRuntime).unwrap();
    for (pattern, input, expected) in [
        (r"\r?\n", "a\r\nb\n", vec!["a", "b", ""]),
        ("(,)", "a,,", vec!["a", ",", "", ",", ""]),
        ("(?:)", "ab", vec!["a", "b"]),
        ("(?:)", "", vec![]),
        ("x", "", vec![""]),
        ("$", "ab", vec!["ab"]),
    ] {
        let regex = heap.regexp(pattern.into(), "").unwrap();
        let output = heap.call(&split, &heap.string(input.into()).unwrap(), &[regex], &NoRuntime).unwrap();
        assert_eq!(
            parts(&heap, &output),
            expected.into_iter().map(|part| Some(WireString::from(part))).collect::<Vec<_>>()
        );
    }
    let capture_regex = heap.regexp("(,)|(x)".into(), "").unwrap();
    let output = heap.call(&split, &heap.string("a,b".into()).unwrap(), &[capture_regex], &NoRuntime).unwrap();
    assert_eq!(parts(&heap, &output), vec![Some("a".into()), Some(",".into()), None, Some("b".into())]);
    let units = vec![0xd83d, 0xde00, 0xd800];
    let output = heap
        .call(
            &split,
            &heap.string(WireString::from_units(units.clone())).unwrap(),
            &[heap.regexp("(?:)".into(), "").unwrap()],
            &NoRuntime,
        )
        .unwrap();
    assert_eq!(
        parts(&heap, &output),
        units.into_iter().map(|unit| Some(WireString::from_units(vec![unit]))).collect::<Vec<_>>()
    );
    let regex = heap.regexp(",".into(), "g").unwrap();
    heap.assign(&regex, "lastIndex".into(), heap.number(17.0).unwrap(), &NoRuntime).unwrap();
    heap.define_own_data(&regex, heap.match_key(), heap.boolean(false).unwrap(), &NoRuntime).unwrap();
    for (limit, expected) in
        [(0.0, vec![]), (1.0, vec!["a"]), (-1.0, vec!["a", "b"]), (f64::NAN, vec![]), (f64::INFINITY, vec![])]
    {
        let output = heap
            .call(
                &split,
                &heap.string("a,b".into()).unwrap(),
                &[regex.clone(), heap.number(limit).unwrap()],
                &NoRuntime,
            )
            .unwrap();
        assert_eq!(
            parts(&heap, &output),
            expected.into_iter().map(|part| Some(WireString::from(part))).collect::<Vec<_>>()
        );
        assert_eq!(number(&heap, &heap.get(&regex, &"lastIndex".into(), &NoRuntime).unwrap()), 17.0);
    }
    // An explicit null @@split takes the ordinary String delimiter route.
    let fallback = heap.regexp(",".into(), "").unwrap();
    heap.define_own_data(&fallback, heap.split_key(), heap.null().unwrap(), &NoRuntime).unwrap();
    let output = heap.call(&split, &heap.string("a/,/b".into()).unwrap(), &[fallback], &NoRuntime).unwrap();
    assert_eq!(parts(&heap, &output), vec![Some("a".into()), Some("b".into())]);

    // Fixed error/flags.ts:223 literal is retained byte-for-byte in this trial.
    let nan_pattern = r"floating[ _-]?point nan\b.*\bdetected in generation";
    let nan = heap.module_regexp("native_error_flags", "GENERATION_NAN_PATTERN", nan_pattern, "is").unwrap();
    let test = heap.get(&nan, &"test".into(), &NoRuntime).unwrap();
    for separator in ["\n", "\r", "\u{2028}", "\u{2029}"] {
        let input = heap.string(format!("FLOATING POINT NAN{separator}is detected in generation").into()).unwrap();
        assert!(heap.truthy(&heap.call(&test, &nan, &[input.clone()], &NoRuntime).unwrap()).unwrap());
        let without_s = heap.regexp(nan_pattern.into(), "i").unwrap();
        assert!(!heap.truthy(&heap.call(&test, &without_s, &[input], &NoRuntime).unwrap()).unwrap());
    }
    assert!(heap.truthy(&heap.get(&nan, &"dotAll".into(), &NoRuntime).unwrap()).unwrap());
    assert!(text(&heap, &heap.get(&nan, &"flags".into(), &NoRuntime).unwrap()).equals_ascii("is"));
    let cloned = heap.structured_clone(&nan, &NoRuntime).unwrap();
    assert!(!cloned.same_identity(&nan));
    assert!(text(&heap, &heap.get(&cloned, &"flags".into(), &NoRuntime).unwrap()).equals_ascii("is"));
    let output = heap
        .call(&split, &heap.string("a\nb".into()).unwrap(), &[heap.regexp(".".into(), "s").unwrap()], &NoRuntime)
        .unwrap();
    assert_eq!(parts(&heap, &output), vec![Some("".into()), Some("".into()), Some("".into()), Some("".into())]);

    // Separate heap keeps the prototype exec replacement local to this family.
    let heap = ValueHeap::new().unwrap();
    let original = heap.regexp("(,)".into(), "").unwrap();
    let native_exec = heap.get(&original, &"exec".into(), &NoRuntime).unwrap();
    let prototype = heap.prototype(&original, &NoRuntime).unwrap().unwrap();
    let input = heap.object().unwrap();
    let limit = heap.object().unwrap();
    let constructor = heap.object().unwrap();
    let flags_value = heap.object().unwrap();
    let capture = heap.object().unwrap();
    let thrown = heap.object().unwrap();
    struct SplitRuntime {
        heap: Arc<ValueHeap>,
        original: ValueHandle,
        input: ValueHandle,
        limit: ValueHandle,
        constructor: ValueHandle,
        flags_value: ValueHandle,
        native_exec: ValueHandle,
        capture: ValueHandle,
        thrown: ValueHandle,
        trace: Mutex<Vec<&'static str>>,
    }
    impl ValueRuntime for SplitRuntime {
        fn resolve(&self, request: RuntimeRequest<'_>) -> Result<RuntimeReply, ValueError> {
            let RuntimeRequest::Call { capability, receiver, arguments } = request else {
                return Err(ValueError::MissingCapability(request.capability()));
            };
            let (event, value) = match capability.0 {
                740 => ("input_string", self.heap.string("a,b".into())?),
                741 => ("constructor", self.constructor.clone()),
                742 => ("species", self.heap.null()?),
                743 => ("flags", self.flags_value.clone()),
                744 => ("flags_string", self.heap.string("".into())?),
                745 => ("match", self.heap.boolean(false)?),
                746 => ("limit_number", self.heap.number(-1.0)?),
                747 => ("get_exec", self.heap.function(CapabilityId(748))?),
                748 => {
                    self.trace.lock().unwrap().push("call_exec");
                    assert!(!receiver.same_identity(&self.original));
                    assert!(self.heap.truthy(&self.heap.get(receiver, &"sticky".into(), self)?)?);
                    assert!(text(&self.heap, &self.heap.get(receiver, &"flags".into(), self)?).equals_ascii("y"));
                    return Ok(RuntimeReply::Value(self.heap.call(&self.native_exec, receiver, arguments, self)?));
                }
                749 => ("get_split", self.heap.function(CapabilityId(750))?),
                750 => {
                    self.trace.lock().unwrap().push("call_split");
                    assert!(receiver.same_identity(&self.original));
                    assert!(arguments[0].same_identity(&self.input));
                    assert!(arguments[1].same_identity(&self.limit));
                    return Ok(RuntimeReply::Value(self.capture.clone()));
                }
                751 => {
                    self.trace.lock().unwrap().push("throw");
                    return Err(ValueError::Thrown(self.thrown.clone()));
                }
                752 => ("get_synthetic_exec", self.heap.function(CapabilityId(753))?),
                753 => {
                    let start = number(&self.heap, &self.heap.get(receiver, &"lastIndex".into(), self)?);
                    if start != 0.0 {
                        return Ok(RuntimeReply::Value(self.heap.null()?));
                    }
                    self.heap.assign(receiver, "lastIndex".into(), self.heap.number(1.0)?, self)?;
                    return Ok(RuntimeReply::Value(self.heap.array(vec![
                        self.heap.string("a".into())?,
                        self.capture.clone(),
                        self.heap.undefined()?,
                    ])?));
                }
                _ => return Err(ValueError::MissingCapability(capability)),
            };
            self.trace.lock().unwrap().push(event);
            Ok(RuntimeReply::Value(value))
        }
    }
    for (object, key, id) in [
        (&input, heap.to_primitive_key(), 740),
        (&original, "constructor".into(), 741),
        (&constructor, heap.species_key(), 742),
        (&original, "flags".into(), 743),
        (&flags_value, heap.to_primitive_key(), 744),
        (&original, heap.match_key(), 745),
        (&limit, heap.to_primitive_key(), 746),
    ] {
        if matches!(id, 740 | 744 | 746) {
            heap.define_own_data(object, key, heap.function(CapabilityId(id)).unwrap(), &NoRuntime).unwrap();
        } else {
            heap.define_own(object, key, accessor(Some(id), None), &NoRuntime).unwrap();
        }
    }
    heap.define_own(&original, "exec".into(), accessor(Some(751), None), &NoRuntime).unwrap();
    heap.define_own(&prototype, "exec".into(), accessor(Some(747), None), &NoRuntime).unwrap();
    heap.assign(&original, "lastIndex".into(), heap.number(41.0).unwrap(), &NoRuntime).unwrap();
    let runtime = SplitRuntime {
        heap: Arc::clone(&heap),
        original: original.clone(),
        input: input.clone(),
        limit: limit.clone(),
        constructor: constructor.clone(),
        flags_value,
        native_exec,
        capture: capture.clone(),
        thrown: thrown.clone(),
        trace: Mutex::new(Vec::new()),
    };
    let split = heap.get(&heap.string("".into()).unwrap(), &"split".into(), &NoRuntime).unwrap();
    let output = heap.call(&split, &input, &[original.clone(), limit.clone()], &runtime).unwrap();
    assert_eq!(parts(&heap, &output), vec![Some("a".into()), Some(",".into()), Some("b".into())]);
    assert_eq!(
        runtime.trace.lock().unwrap().as_slice(),
        &[
            "input_string",
            "constructor",
            "species",
            "flags",
            "flags_string",
            "match",
            "limit_number",
            "get_exec",
            "call_exec",
            "get_exec",
            "call_exec",
            "get_exec",
            "call_exec"
        ]
    );
    assert_eq!(number(&heap, &heap.get(&original, &"lastIndex".into(), &NoRuntime).unwrap()), 41.0);
    runtime.trace.lock().unwrap().clear();
    let empty = heap.call(&split, &input, &[original.clone(), heap.number(0.0).unwrap()], &runtime).unwrap();
    assert!(parts(&heap, &empty).is_empty());
    assert_eq!(
        runtime.trace.lock().unwrap().as_slice(),
        &["input_string", "constructor", "species", "flags", "flags_string", "match"]
    );

    heap.define_own(&original, heap.split_key(), accessor(Some(749), None), &NoRuntime).unwrap();
    runtime.trace.lock().unwrap().clear();
    assert!(heap.call(&split, &input, &[original.clone(), limit.clone()], &runtime).unwrap().same_identity(&capture));
    assert_eq!(runtime.trace.lock().unwrap().as_slice(), &["get_split", "call_split"]);
    heap.define_own(&original, heap.split_key(), accessor(Some(751), None), &NoRuntime).unwrap();
    assert!(
        matches!(heap.call(&split, &input, &[original.clone(), limit], &runtime), Err(ValueError::Thrown(value)) if value.same_identity(&thrown))
    );

    // Each original observation propagates the same Throw receipt.
    for key in [PropertyKey::from("constructor"), PropertyKey::from("flags"), heap.match_key()] {
        let regex = heap.regexp(",".into(), "").unwrap();
        heap.define_own(&regex, key, accessor(Some(751), None), &NoRuntime).unwrap();
        assert!(
            matches!(heap.call(&split, &heap.string("a,b".into()).unwrap(), &[regex], &runtime), Err(ValueError::Thrown(value)) if value.same_identity(&thrown))
        );
    }
    let regex = heap.regexp(",".into(), "").unwrap();
    let ctor = heap.object().unwrap();
    heap.define_own(&ctor, heap.species_key(), accessor(Some(751), None), &NoRuntime).unwrap();
    heap.define_own_data(&regex, "constructor".into(), ctor, &NoRuntime).unwrap();
    assert!(
        matches!(heap.call(&split, &heap.string("a,b".into()).unwrap(), &[regex], &runtime), Err(ValueError::Thrown(value)) if value.same_identity(&thrown))
    );

    heap.define_own(&prototype, "exec".into(), accessor(Some(752), None), &NoRuntime).unwrap();
    let output = heap
        .call(&split, &heap.string("ab".into()).unwrap(), &[heap.regexp(",".into(), "").unwrap()], &runtime)
        .unwrap();
    assert!(heap.get(&output, &"1".into(), &NoRuntime).unwrap().same_identity(&capture));
    assert_eq!(heap.kind(&heap.get(&output, &"2".into(), &NoRuntime).unwrap()).unwrap(), ValueKind::Undefined);
    assert!(text(&heap, &heap.get(&output, &"3".into(), &NoRuntime).unwrap()).equals_ascii("b"));
    heap.define_own(&prototype, "exec".into(), accessor(Some(751), None), &NoRuntime).unwrap();
    assert!(
        matches!(heap.call(&split, &heap.string("ab".into()).unwrap(), &[heap.regexp(",".into(), "").unwrap()], &runtime), Err(ValueError::Thrown(value)) if value.same_identity(&thrown))
    );

    // Selected-domain boundaries stay executable and visibly excluded.
    let regex = heap.regexp(",".into(), "").unwrap();
    let ctor = heap.object().unwrap();
    heap.define_own_data(&ctor, heap.species_key(), heap.function(CapabilityId(750)).unwrap(), &NoRuntime).unwrap();
    heap.define_own_data(&regex, "constructor".into(), ctor.clone(), &NoRuntime).unwrap();
    assert!(matches!(
        heap.call(&split, &heap.string("ab".into()).unwrap(), &[regex.clone()], &runtime),
        Err(ValueError::UnsupportedOperation("Custom RegExp split species Construct is not connected"))
    ));
    heap.define_own_data(&ctor, heap.species_key(), heap.number(1.0).unwrap(), &NoRuntime).unwrap();
    assert!(matches!(
        heap.call(&split, &heap.string("ab".into()).unwrap(), &[regex], &runtime),
        Err(ValueError::TypeError(_))
    ));
    for flags in ["v"] {
        let regex = heap.regexp(",".into(), "").unwrap();
        heap.define_own_data(&regex, "flags".into(), heap.string(flags.into()).unwrap(), &NoRuntime).unwrap();
        assert!(matches!(
            heap.call(&split, &heap.string("ab".into()).unwrap(), &[regex], &runtime),
            Err(ValueError::UnsupportedOperation(_))
        ));
    }
}

// RG-S1 behavior observations in the existing coarse group. Single-attempt
// complexity is established separately from the engine's actual source path.
fn regexp_at_offset_dependency_observations() {
    let regex = regress::Regex::new(r"\r?\n").unwrap();
    assert!(regex.match_at_ucs2(&vec![u16::from(b'a'); 512], 0).is_none());
    let text = [u16::from(b'a'), u16::from(b'\n')];
    assert!(regex.match_at_ucs2(&text, 0).is_none());
    assert_eq!(regex.find_from_ucs2(&text, 0).next().unwrap().range, 1..2);
    assert_eq!(regex.match_at_ucs2(&text, 1).unwrap().range, 1..2);
    assert!(regex.match_at_ucs2(&text, text.len() + 1).is_none());

    let text = "zab".encode_utf16().collect::<Vec<_>>();
    let found = regress::Regex::new(r"(?<=a)(b)$").unwrap().match_at_ucs2(&text, 2).unwrap();
    assert_eq!(found.range, 2..3);
    assert_eq!(found.captures, vec![Some(2..3)]);
    assert!(regress::Regex::new("^b").unwrap().match_at_ucs2(&text, 2).is_none());
    assert_eq!(regress::Regex::new("$").unwrap().match_at_ucs2(&text, 3).unwrap().range, 3..3);
    let found = regress::Regex::new("(.)").unwrap().match_at_ucs2(&[0xd83d, 0xde00, 0xd800], 1).unwrap();
    assert_eq!(found.range, 1..2);
    assert_eq!(found.captures, vec![Some(1..2)]);
}

// Source-backed dependency observations, reached from the existing callback family.
fn ark_wire_shared_dependency_observations() {
    struct Runtime {
        heap: Arc<ValueHeap>,
        prototype: ValueHandle,
        returned: ValueHandle,
        thrown: ValueHandle,
        custom_rhs: ValueHandle,
        expected_lhs: ValueHandle,
        trace: Mutex<Vec<&'static str>>,
    }
    impl ValueRuntime for Runtime {
        fn resolve(&self, request: RuntimeRequest<'_>) -> Result<RuntimeReply, ValueError> {
            if let RuntimeRequest::Opaque { operation: OpaqueOperation::OrdinaryHasInstance(value), .. } = &request {
                self.trace.lock().unwrap().push("opaque-instance");
                assert!(value.same_identity(&self.expected_lhs));
                return Ok(RuntimeReply::Value(self.heap.boolean(true).unwrap()));
            }
            let RuntimeRequest::Call { capability, receiver, arguments } = request else {
                return Err(ValueError::MissingCapability(request.capability()));
            };
            match capability.0 {
                1 => {
                    self.trace.lock().unwrap().push("prototype");
                    Ok(RuntimeReply::Value(self.prototype.clone()))
                }
                2 => {
                    self.trace.lock().unwrap().push("method");
                    Ok(RuntimeReply::Value(self.heap.function(CapabilityId(3)).unwrap()))
                }
                3 => {
                    self.trace.lock().unwrap().push("instance-call");
                    assert!(receiver.same_identity(&self.custom_rhs));
                    assert!(arguments[0].same_identity(&self.expected_lhs));
                    Ok(RuntimeReply::Value(self.returned.clone()))
                }
                4 => {
                    self.trace.lock().unwrap().push("base");
                    Ok(RuntimeReply::Value(self.returned.clone()))
                }
                5 => {
                    self.trace.lock().unwrap().push("throw");
                    Err(ValueError::Thrown(self.thrown.clone()))
                }
                6 => {
                    self.trace.lock().unwrap().push("opaque-method");
                    Ok(RuntimeReply::Value(self.heap.undefined().unwrap()))
                }
                _ => Err(ValueError::MissingCapability(capability)),
            }
        }
    }
    let heap = ValueHeap::new().unwrap();
    let prototype = heap.object().unwrap();
    let lhs = heap.object().unwrap();
    heap.set_prototype(&lhs, Some(&prototype), &NoRuntime).unwrap();
    let constructor = heap.function(CapabilityId(90)).unwrap();
    heap.define_own(&constructor, "prototype".into(), accessor(Some(1), None), &NoRuntime).unwrap();
    let custom = heap.object().unwrap();
    heap.define_own(&custom, heap.has_instance_key(), accessor(Some(2), None), &NoRuntime).unwrap();
    let mut runtime = Runtime {
        heap: heap.clone(),
        prototype: prototype.clone(),
        returned: heap.string("truthy".into()).unwrap(),
        thrown: heap.symbol(None).unwrap(),
        custom_rhs: custom.clone(),
        expected_lhs: lhs.clone(),
        trace: Mutex::new(Vec::new()),
    };
    let inherited = heap.get(&constructor, &heap.has_instance_key(), &NoRuntime).unwrap();
    assert_eq!(number(&heap, &heap.get(&inherited, &"length".into(), &NoRuntime).unwrap()), 1.0);
    assert_eq!(
        json(&heap, &heap.get(&inherited, &"name".into(), &NoRuntime).unwrap(), StringifyMode::Plain, &NoRuntime),
        Some("\"[Symbol.hasInstance]\"".into())
    );
    let function_proto = heap.prototype(&constructor, &NoRuntime).unwrap().unwrap();
    assert!(matches!(
        heap.own_descriptor(&function_proto, &heap.has_instance_key(), &NoRuntime).unwrap(),
        Some(PropertyDescriptor::Data { writable: false, enumerable: false, configurable: false, .. })
    ));
    assert!(heap.instance_of(&lhs, &constructor, &runtime).unwrap());
    assert_eq!(runtime.trace.lock().unwrap().as_slice(), &["prototype"]);
    runtime.trace.lock().unwrap().clear();
    assert!(!heap.instance_of(&heap.number(1.0).unwrap(), &constructor, &runtime).unwrap());
    assert!(runtime.trace.lock().unwrap().is_empty(), "ordinary primitive LHS must skip prototype Get");
    assert!(!heap.instance_of(&heap.object().unwrap(), &constructor, &runtime).unwrap());
    assert!(heap.instance_of(&lhs, &custom, &runtime).unwrap());
    runtime.returned = heap.boolean(false).unwrap();
    assert!(!heap.instance_of(&lhs, &custom, &runtime).unwrap());
    runtime.returned = heap.string("base-value".into()).unwrap();
    for invalid in [heap.number(1.0).unwrap(), heap.null().unwrap()] {
        assert!(matches!(heap.instance_of(&lhs, &invalid, &runtime), Err(ValueError::TypeError(_))));
    }
    let bad_method = heap.object().unwrap();
    heap.define_own_data(&bad_method, heap.has_instance_key(), heap.number(1.0).unwrap(), &NoRuntime).unwrap();
    assert!(matches!(heap.instance_of(&lhs, &bad_method, &runtime), Err(ValueError::TypeError(_))));
    heap.define_own_data(&constructor, heap.has_instance_key(), heap.null().unwrap(), &NoRuntime).unwrap();
    assert!(heap.instance_of(&lhs, &constructor, &runtime).unwrap(), "nullish own override uses ordinary fallback");
    let invalid_prototype = heap.function(CapabilityId(91)).unwrap();
    heap.define_own_data(&invalid_prototype, "prototype".into(), heap.number(1.0).unwrap(), &NoRuntime).unwrap();
    assert!(matches!(heap.instance_of(&lhs, &invalid_prototype, &runtime), Err(ValueError::TypeError(_))));
    assert!(!heap.instance_of(&heap.null().unwrap(), &invalid_prototype, &runtime).unwrap());
    let throwing = heap.object().unwrap();
    heap.define_own(&throwing, heap.has_instance_key(), accessor(Some(5), None), &NoRuntime).unwrap();
    assert!(
        matches!(heap.instance_of(&lhs, &throwing, &runtime), Err(ValueError::Thrown(value)) if value.same_identity(&runtime.thrown))
    );
    let opaque = heap.opaque(OpaqueKind::Function, CapabilityId(6), 910).unwrap();
    // Its actual method Get is owned by the opaque runtime; supply that reply.
    struct OpaqueRuntime<'a>(&'a Runtime);
    impl ValueRuntime for OpaqueRuntime<'_> {
        fn resolve(&self, request: RuntimeRequest<'_>) -> Result<RuntimeReply, ValueError> {
            match &request {
                RuntimeRequest::Opaque { operation: OpaqueOperation::Get(_), .. } => {
                    Ok(RuntimeReply::Value(self.0.heap.undefined().unwrap()))
                }
                _ => self.0.resolve(request),
            }
        }
    }
    assert!(heap.instance_of(&lhs, &opaque, &OpaqueRuntime(&runtime)).unwrap());
    assert!(matches!(
        heap.instance_of(&ValueHeap::new().unwrap().object().unwrap(), &constructor, &runtime),
        Err(ValueError::ForeignHeap)
    ));

    let fallback = heap.argument_property_function(0, "base", "fallback", 1).unwrap();
    let another = heap.argument_property_function(0, "base", "fallback", 1).unwrap();
    assert!(!fallback.same_identity(&another));
    assert_eq!(number(&heap, &heap.get(&fallback, &"length".into(), &NoRuntime).unwrap()), 1.0);
    assert_eq!(
        json(&heap, &heap.get(&fallback, &"name".into(), &NoRuntime).unwrap(), StringifyMode::Plain, &NoRuntime),
        Some("\"fallback\"".into())
    );
    let options = heap.object().unwrap();
    heap.define_own_data(&options, "fallback".into(), fallback, &NoRuntime).unwrap();
    let retained = heap.get(&options, &"fallback".into(), &NoRuntime).unwrap();
    let context = heap.object().unwrap();
    heap.define_own(&context, "base".into(), accessor(Some(4), None), &NoRuntime).unwrap();
    assert!(
        heap.call(&retained, &heap.null().unwrap(), std::slice::from_ref(&context), &runtime)
            .unwrap()
            .same_identity(&runtime.returned)
    );
    runtime.returned = heap.symbol(None).unwrap();
    assert!(
        heap.call(&retained, &options, std::slice::from_ref(&context), &runtime)
            .unwrap()
            .same_identity(&runtime.returned),
        "later invocation uses current runtime, no captured result or expiry"
    );
    heap.define_own(&context, "base".into(), accessor(Some(5), None), &NoRuntime).unwrap();
    assert!(
        matches!(heap.call(&retained, &options, &[context], &runtime), Err(ValueError::Thrown(value)) if value.same_identity(&runtime.thrown))
    );
    assert!(matches!(heap.call(&retained, &options, &[], &runtime), Err(ValueError::TypeError(_))));

    let source = heap.object().unwrap();
    let key = PropertyKey::Symbol(heap.symbol_key(&heap.symbol(None).unwrap()).unwrap());
    heap.define_own_data(&source, key.clone(), lhs.clone(), &NoRuntime).unwrap();
    heap.define_own_data(&source, "same".into(), heap.number(2.0).unwrap(), &NoRuntime).unwrap();
    let target = heap.object().unwrap();
    heap.define_own_data(&target, "same".into(), heap.number(1.0).unwrap(), &NoRuntime).unwrap();
    heap.spread_into(&target, &source, &NoRuntime).unwrap();
    assert!(heap.get(&target, &key, &NoRuntime).unwrap().same_identity(&lhs));
    assert_eq!(number(&heap, &heap.get(&target, &"same".into(), &NoRuntime).unwrap()), 2.0);

    let child = heap.object().unwrap();
    heap.define_own(&child, "value".into(), accessor(Some(4), None), &NoRuntime).unwrap();
    let set = heap.new_set(&heap.array(vec![child.clone()]).unwrap(), &NoRuntime).unwrap();
    let add = heap.get(&set, &"add".into(), &NoRuntime).unwrap();
    heap.call(&add, &set, std::slice::from_ref(&set), &NoRuntime).unwrap();
    heap.define_own(&set, "expando".into(), accessor(Some(5), None), &NoRuntime).unwrap();
    let source = heap.object().unwrap();
    heap.define_own_data(&source, "set".into(), set.clone(), &NoRuntime).unwrap();
    heap.define_own_data(&source, "child".into(), child.clone(), &NoRuntime).unwrap();
    runtime.returned = heap.number(7.0).unwrap();
    runtime.trace.lock().unwrap().clear();
    let cloned = heap.structured_clone(&source, &runtime).unwrap();
    let cloned_set = heap.get(&cloned, &"set".into(), &NoRuntime).unwrap();
    let cloned_child = heap.get(&cloned, &"child".into(), &NoRuntime).unwrap();
    assert!(!cloned_set.same_identity(&set));
    assert!(!cloned_child.same_identity(&child));
    assert_eq!(runtime.trace.lock().unwrap().as_slice(), &["base"], "Set expando is ignored, shared child getter once");
    let has = heap.get(&cloned_set, &"has".into(), &NoRuntime).unwrap();
    assert!(heap.truthy(&heap.call(&has, &cloned_set, &[cloned_child], &NoRuntime).unwrap()).unwrap());
    assert!(
        heap.truthy(&heap.call(&has, &cloned_set, std::slice::from_ref(&cloned_set), &NoRuntime).unwrap()).unwrap()
    );
    assert!(!heap.has_own(&cloned_set, &"expando".into(), &NoRuntime).unwrap());
    let values = heap.get(&set, &heap.iterator_key(), &NoRuntime).unwrap();
    let iterator = heap.call(&values, &set, &[], &NoRuntime).unwrap();
    assert!(matches!(heap.structured_clone(&iterator, &NoRuntime), Err(ValueError::DataCloneError(_))));
    let regexp = heap.regexp("a".into(), "g").unwrap();
    heap.assign(&regexp, "lastIndex".into(), heap.number(8.0).unwrap(), &NoRuntime).unwrap();
    for name in ["source", "flags", "expando"] {
        heap.define_own(&regexp, name.into(), accessor(Some(5), None), &NoRuntime).unwrap();
    }
    runtime.trace.lock().unwrap().clear();
    let clone = heap.structured_clone(&regexp, &runtime).unwrap();
    assert!(!clone.same_identity(&regexp));
    assert!(runtime.trace.lock().unwrap().is_empty());
    assert_eq!(number(&heap, &heap.get(&clone, &"lastIndex".into(), &NoRuntime).unwrap()), 0.0);
    assert!(heap.truthy(&heap.get(&clone, &"global".into(), &NoRuntime).unwrap()).unwrap());
    assert_eq!(
        json(&heap, &heap.get(&clone, &"source".into(), &NoRuntime).unwrap(), StringifyMode::Plain, &NoRuntime),
        Some("\"a\"".into())
    );
    assert!(!heap.has_own(&clone, &"expando".into(), &NoRuntime).unwrap());
    assert_eq!(number(&heap, &heap.get(&regexp, &"lastIndex".into(), &NoRuntime).unwrap()), 8.0);
    drop(runtime);
    drop(heap);
}
