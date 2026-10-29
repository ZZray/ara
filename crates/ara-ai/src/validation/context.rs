//! Fixed OMP validation.ts getValidationContext and authored Ark/JSON validateContext
//! branch. This retains the actual context record, rather than a Rust tag or
//! decoded field cache. Authored Ark calls use the Host's actual type binding.
/*
MIT License

Copyright (c) 2025 Mario Zechner
Copyright (c) 2025-2026 Can Bölük
Copyright (c) 2026 Stencil Labs, Inc.

Permission is hereby granted, free of charge, to any person obtaining a copy
of this software and associated documentation files (the "Software"), to deal
in the Software without restriction, including without limitation the rights
to use, copy, modify, merge, publish, distribute, sublicense, and/or sell
copies of the Software, and to permit persons to whom the Software is
furnished to do so, subject to the following conditions:

The above copyright notice and this permission notice shall be included in all
copies or substantial portions of the Software.

THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF ANY KIND, EXPRESS OR
IMPLIED, INCLUDING BUT NOT LIMITED TO THE WARRANTIES OF MERCHANTABILITY,
FITNESS FOR A PARTICULAR PURPOSE AND NONINFRINGEMENT. IN NO EVENT SHALL THE
AUTHORS OR COPYRIGHT HOLDERS BE LIABLE FOR ANY CLAIM, DAMAGES OR OTHER
LIABILITY, WHETHER IN AN ACTION OF CONTRACT, TORT OR OTHERWISE, ARISING FROM,
OUT OF OR IN CONNECTION WITH THE SOFTWARE OR THE USE OR OTHER DEALINGS IN THE
SOFTWARE.
*/

use super::graph::{
    GraphContextValidation, flatten_graph_json_issues, graph_validation_messages, validate_graph_json_context,
};
use crate::graph_values::GraphValueFactory;
use crate::schema_draft::upgrade_graph_json_schema_to_202012;
use crate::schema_stamps;
use crate::schema_validator::validate_graph_json_schema_value;
use crate::schema_wire::{graph_ark_to_wire_schema, is_graph_ark_schema};
use ara_rpc::value_heap::{PropertyKey, ValueError, ValueHandle, ValueKind, ValueSnapshot};

#[derive(Clone, Debug)]
pub struct GraphValidationContext {
    record: ValueHandle,
}

impl GraphValidationContext {
    pub fn record(&self) -> &ValueHandle {
        &self.record
    }
}

fn get(factory: &GraphValueFactory, target: &ValueHandle, name: &str) -> Result<ValueHandle, ValueError> {
    factory.heap().get(target, &PropertyKey::from(name), factory.runtime())
}

pub fn get_graph_validation_context(
    factory: &GraphValueFactory,
    parameters: &ValueHandle,
) -> Result<GraphValidationContext, ValueError> {
    let heap = factory.heap();
    heap.adopt(parameters)?;
    let key = heap.module_symbol("ara-ai.validation", "ai.validationContext")?;
    let record = schema_stamps::stamp(factory, parameters, &key, |parameters| {
        if is_graph_ark_schema(factory, parameters)? {
            let record = heap.object()?;
            heap.define_own_data(&record, "kind".into(), heap.string("arktype".into())?, factory.runtime())?;
            heap.define_own_data(&record, "ark".into(), parameters.clone(), factory.runtime())?;
            let json = graph_ark_to_wire_schema(factory, parameters)?;
            heap.define_own_data(&record, "json".into(), json, factory.runtime())?;
            return Ok(record);
        }
        let json = upgrade_graph_json_schema_to_202012(factory, parameters)?;
        let record = heap.object()?;
        heap.define_own_data(&record, "kind".into(), heap.string("json".into())?, factory.runtime())?;
        heap.define_own_data(&record, "json".into(), json, factory.runtime())?;
        Ok(record)
    })?;
    // Native permits any non-Undefined cache value. A later actual field Get
    // may fail; eagerly decoding/shape-checking here would change its effects.
    Ok(GraphValidationContext { record })
}

pub fn graph_context_normalization_schema(
    factory: &GraphValueFactory,
    context: &GraphValidationContext,
) -> Result<ValueHandle, ValueError> {
    get(factory, context.record(), "json")
}

pub fn validate_graph_context(
    factory: &GraphValueFactory,
    context: &GraphValidationContext,
    value: &ValueHandle,
) -> Result<GraphContextValidation, ValueError> {
    factory.heap().adopt(value)?;
    let kind = get(factory, context.record(), "kind")?;
    if matches!(factory.heap().snapshot(&kind)?, ValueSnapshot::String(kind) if kind.equals_ascii("arktype")) {
        let heap = factory.heap();
        let runtime = factory.runtime();
        let ark = get(factory, context.record(), "ark")?;
        let out = heap.call(&ark, context.record(), std::slice::from_ref(value), runtime)?;
        // This live Get follows the authored call; binding must not snapshot
        // `errors` or suppress the call's effects when integration is missing.
        let errors = get(factory, factory.ark_type()?, "errors")?;
        if !heap.instance_of(&out, &errors, runtime)? {
            let accepted = if heap.kind(value)? == ValueKind::Object && heap.kind(&out)? == ValueKind::Object {
                let merged = heap.spread(value, runtime)?;
                heap.spread_into(&merged, &out, runtime)?;
                merged
            } else {
                out
            };
            return Ok(GraphContextValidation::Accept(accepted));
        }
        let json = get(factory, context.record(), "json")?;
        let result = validate_graph_json_schema_value(factory, &json, value)?;
        let flat_issues =
            if result.success { heap.array(vec![])? } else { flatten_graph_json_issues(factory, result)? };
        let messages = graph_validation_messages(factory, &out)?;
        return Ok(GraphContextValidation::RejectActual { flat_issues, messages });
    }
    // This Get is distinct from the driver's preceding normalization-schema
    // Get. Mutating/accessor contexts stay live at each Native call position.
    let json = get(factory, context.record(), "json")?;
    validate_graph_json_context(factory, &json, value)
}

#[cfg(test)]
pub(super) mod tests {
    use super::*;
    use ara_rpc::value_heap::{
        CallableTarget, CapabilityId, NoRuntime, PropertyDescriptor, RuntimeReply, RuntimeRequest, ValueHeap,
    };
    use serde_json::json;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::{Arc, Mutex};

    fn freeze(heap: &Arc<ValueHeap>, value: &ValueHandle) {
        for key in heap.own_keys(value, &NoRuntime).unwrap() {
            let descriptor = match heap.own_descriptor(value, &key, &NoRuntime).unwrap().unwrap() {
                PropertyDescriptor::Data { value, enumerable, .. } => {
                    PropertyDescriptor::Data { value, enumerable, writable: false, configurable: false }
                }
                PropertyDescriptor::Accessor { get, set, enumerable, .. } => {
                    PropertyDescriptor::Accessor { get, set, enumerable, configurable: false }
                }
            };
            heap.define_own(value, key, descriptor, &NoRuntime).unwrap();
        }
        heap.set_extensible(value, false, &NoRuntime).unwrap();
    }
    fn accessor(heap: &Arc<ValueHeap>, value: &ValueHandle, key: PropertyKey, id: u64) {
        heap.define_own(
            value,
            key,
            PropertyDescriptor::Accessor {
                get: Some(CallableTarget::Host(CapabilityId(id))),
                set: None,
                enumerable: true,
                configurable: true,
            },
            &NoRuntime,
        )
        .unwrap();
    }

    pub(crate) fn cache_draft_and_live_fields() {
        crate::schema_draft::graph::tests::conversion_identity_and_original_keyword_families();
        let heap = ValueHeap::new().unwrap();
        let factory = GraphValueFactory::new(heap.clone(), Arc::new(NoRuntime)).unwrap();
        let parameters = heap.import_serde(&json!({"type":"object"})).unwrap();
        let context = get_graph_validation_context(&factory, &parameters).unwrap();
        let warm = get_graph_validation_context(&factory, &parameters).unwrap();
        assert!(context.record().same_identity(warm.record()));
        assert!(graph_context_normalization_schema(&factory, &context).unwrap().same_identity(&parameters));
        let other_parameters = heap.import_serde(&json!({"type":"object"})).unwrap();
        assert!(
            !get_graph_validation_context(&factory, &other_parameters)
                .unwrap()
                .record()
                .same_identity(context.record())
        );
        let value = heap.object().unwrap();
        assert!(
            matches!(validate_graph_context(&factory, &context, &value).unwrap(), GraphContextValidation::Accept(accepted) if accepted.same_identity(&value))
        );
        let before = graph_context_normalization_schema(&factory, &context).unwrap();
        heap.assign(context.record(), "json".into(), heap.boolean(false).unwrap(), &NoRuntime).unwrap();
        assert!(before.same_identity(&parameters));
        assert!(matches!(
            validate_graph_context(&factory, &context, &value).unwrap(),
            GraphContextValidation::Reject(_)
        ));
        heap.assign(context.record(), "kind".into(), heap.string("other".into()).unwrap(), &NoRuntime).unwrap();
        assert!(matches!(
            validate_graph_context(&factory, &context, &value).unwrap(),
            GraphContextValidation::Reject(_)
        ));
        heap.assign(context.record(), "kind".into(), heap.string("arktype".into()).unwrap(), &NoRuntime).unwrap();
        assert!(matches!(validate_graph_context(&factory, &context, &value), Err(ValueError::TypeError(_))));

        let key = heap.module_symbol("ara-ai.validation", "ai.validationContext").unwrap();
        assert!(heap.own_descriptor(&parameters, &key, &NoRuntime).unwrap().is_none());
        assert_eq!(heap.export_serde(&parameters).unwrap(), json!({"type":"object"}));
        assert_eq!(
            heap.own_enumerable_string_keys(&parameters, &NoRuntime).unwrap(),
            vec![ara_rpc::WireString::from("type")]
        );
        freeze(&heap, &parameters);
        assert!(get_graph_validation_context(&factory, &parameters).unwrap().record().same_identity(context.record()));

        let fresh_frozen = heap.import_serde(&json!({"type":"object"})).unwrap();
        freeze(&heap, &fresh_frozen);
        let first = get_graph_validation_context(&factory, &fresh_frozen).unwrap();
        let second = get_graph_validation_context(&factory, &fresh_frozen).unwrap();
        assert!(first.record().same_identity(second.record()));
        assert!(!heap.has_own(&fresh_frozen, &key, &NoRuntime).unwrap());

        let sealed_writable = heap.import_serde(&json!({"type":"object"})).unwrap();
        heap.set_extensible(&sealed_writable, false, &NoRuntime).unwrap();
        assert!(get_graph_validation_context(&factory, &sealed_writable).is_ok());
        let stamped_then_frozen = heap.import_serde(&json!({"type":"object"})).unwrap();
        crate::schema_draft::upgrade_graph_json_schema_to_202012(&factory, &stamped_then_frozen).unwrap();
        freeze(&heap, &stamped_then_frozen);
        assert!(crate::schema_draft::upgrade_graph_json_schema_to_202012(&factory, &stamped_then_frozen).is_ok());

        let arbitrary_host = heap.object().unwrap();
        let arbitrary = heap.number(42.0).unwrap();
        heap.define_own_data(&arbitrary_host, key, arbitrary.clone(), &NoRuntime).unwrap();
        assert!(!get_graph_validation_context(&factory, &arbitrary_host).unwrap().record().same_identity(&arbitrary));
    }

    struct ArkRuntime {
        heap: Arc<ValueHeap>,
        ark: ValueHandle,
        json: Mutex<ValueHandle>,
        out: Mutex<ValueHandle>,
        errors: ValueHandle,
        constructor: Mutex<ValueHandle>,
        map: ValueHandle,
        override_messages: ValueHandle,
        thrown: ValueHandle,
        mode: AtomicUsize,
        context: Mutex<Option<ValueHandle>>,
        events: Mutex<Vec<&'static str>>,
    }

    impl ArkRuntime {
        fn reply(
            &self,
            capability: CapabilityId,
            receiver: &ValueHandle,
            arguments: &[ValueHandle],
            runtime: &dyn ara_rpc::value_heap::ValueRuntime,
        ) -> Result<RuntimeReply, ValueError> {
            let heap = &self.heap;
            let value = match capability.0 {
                200 | 212 => {
                    self.events.lock().unwrap().push(if capability.0 == 200 { "ark" } else { "replacement-ark" });
                    *self.context.lock().unwrap() = Some(receiver.clone());
                    if self.mode.load(Ordering::SeqCst) == 3 {
                        return Err(ValueError::Thrown(self.thrown.clone()));
                    }
                    let argument = arguments.first().unwrap();
                    if self.mode.load(Ordering::SeqCst) == 4 {
                        let count = heap.get(argument, &"count".into(), runtime)?;
                        if heap.kind(&count)? == ValueKind::Boolean { argument.clone() } else { self.errors.clone() }
                    } else {
                        self.out.lock().unwrap().clone()
                    }
                }
                201 => {
                    self.events.lock().unwrap().push("wire");
                    assert!(receiver.same_identity(&self.ark));
                    let options = arguments.first().unwrap();
                    assert!(matches!(heap.snapshot(&heap.get(options, &"target".into(), runtime)?)?,
                        ValueSnapshot::String(value) if value.equals_ascii("draft-2020-12")));
                    self.json.lock().unwrap().clone()
                }
                203 => {
                    self.events.lock().unwrap().push("errors");
                    if self.mode.load(Ordering::SeqCst) == 5 {
                        return Err(ValueError::Thrown(self.thrown.clone()));
                    }
                    self.constructor.lock().unwrap().clone()
                }
                204 => {
                    self.events.lock().unwrap().push("has-instance");
                    if self.mode.load(Ordering::SeqCst) == 2 {
                        return Err(ValueError::Thrown(self.thrown.clone()));
                    }
                    if self.mode.load(Ordering::SeqCst) == 1 {
                        heap.string("truthy".into())?
                    } else {
                        heap.boolean(arguments.first().unwrap().same_identity(&self.errors))?
                    }
                }
                205 => {
                    self.events.lock().unwrap().push("input-get");
                    heap.number(1.0)?
                }
                206 => {
                    self.events.lock().unwrap().push("out-get");
                    heap.number(2.0)?
                }
                207 => {
                    self.events.lock().unwrap().push("path");
                    heap.array(vec![heap.string("count".into())?])?
                }
                208 => {
                    self.events.lock().unwrap().push("message");
                    heap.string("bad count".into())?
                }
                209 => {
                    self.events.lock().unwrap().push("map-override");
                    self.override_messages.clone()
                }
                211 => {
                    self.events.lock().unwrap().push("json");
                    self.json.lock().unwrap().clone()
                }
                213 => {
                    self.events.lock().unwrap().push("forward-map");
                    heap.call(&self.map, receiver, arguments, runtime)?
                }
                _ => return Err(ValueError::MissingCapability(capability)),
            };
            Ok(RuntimeReply::Value(value))
        }
    }
    impl ara_rpc::value_heap::ValueRuntime for ArkRuntime {
        fn resolve(&self, request: RuntimeRequest<'_>) -> Result<RuntimeReply, ValueError> {
            match request {
                RuntimeRequest::Call { capability, receiver, arguments } => {
                    self.reply(capability, receiver, arguments, self)
                }
                request => Err(ValueError::MissingCapability(request.capability())),
            }
        }
        fn resolve_call(
            &self,
            capability: CapabilityId,
            receiver: &ValueHandle,
            arguments: &[ValueHandle],
            effective_runtime: &dyn ara_rpc::value_heap::ValueRuntime,
        ) -> Result<RuntimeReply, ValueError> {
            self.reply(capability, receiver, arguments, effective_runtime)
        }
    }
    fn ark_own(heap: &Arc<ValueHeap>, target: &ValueHandle, key: &str, value: ValueHandle) {
        heap.define_own_data(target, key.into(), value, &NoRuntime).unwrap();
    }

    /// One coarse authored-context family, reached through the existing test.
    /// These are controlled Host schemas, not the whole Ark authoring library.
    pub(crate) fn authored_ark_context_family() {
        use super::super::graph::{GraphToolValidationError, validate_graph_tool_arguments};
        let heap = ValueHeap::new().unwrap();
        let ark = heap.function(CapabilityId(200)).unwrap();
        ark_own(&heap, &ark, "toJsonSchema", heap.function(CapabilityId(201)).unwrap());
        ark_own(&heap, &ark, "assert", heap.function(CapabilityId(202)).unwrap());
        let error = heap.object().unwrap();
        accessor(&heap, &error, "path".into(), 207);
        accessor(&heap, &error, "message".into(), 208);
        let errors = heap.array(vec![error.clone()]).unwrap();
        let rhs = heap.object().unwrap();
        heap.define_own_data(&rhs, heap.has_instance_key(), heap.function(CapabilityId(204)).unwrap(), &NoRuntime)
            .unwrap();
        let type_handle = heap.object().unwrap();
        accessor(&heap, &type_handle, "errors".into(), 203);
        let parsed = heap.object().unwrap();
        accessor(&heap, &parsed, "probe".into(), 206);
        let runtime = Arc::new(ArkRuntime {
            heap: heap.clone(),
            ark: ark.clone(),
            json: Mutex::new(
                heap.import_serde(
                    &json!({"type":"object","properties":{"count":{"type":"number"}},"required":["count"]}),
                )
                .unwrap(),
            ),
            out: Mutex::new(parsed.clone()),
            errors: errors.clone(),
            constructor: Mutex::new(rhs.clone()),
            map: heap.get(&heap.array_prototype(), &"map".into(), &NoRuntime).unwrap(),
            override_messages: heap.object().unwrap(),
            thrown: heap.symbol(Some("Ark original throw".into())).unwrap(),
            mode: AtomicUsize::new(0),
            context: Mutex::new(None),
            events: Mutex::new(Vec::new()),
        });
        let unbound = GraphValueFactory::new(heap.clone(), runtime.clone()).unwrap();
        runtime.events.lock().unwrap().clear();
        let factory = unbound.with_ark_type(type_handle.clone()).unwrap();
        assert!(runtime.events.lock().unwrap().is_empty(), "binding must not Get errors");
        let foreign = ValueHeap::new().unwrap();
        assert!(factory.with_ark_type(foreign.object().unwrap()).is_err());
        let context = get_graph_validation_context(&factory, &ark).unwrap();
        assert_eq!(*runtime.events.lock().unwrap(), ["wire"]);
        let warm = get_graph_validation_context(&factory, &ark).unwrap();
        assert!(warm.record().same_identity(context.record()));
        assert_eq!(*runtime.events.lock().unwrap(), ["wire"]);
        assert!(get(&factory, context.record(), "ark").unwrap().same_identity(&ark));

        let input = heap.object().unwrap();
        accessor(&heap, &input, "probe".into(), 205);
        let nested = heap.object().unwrap();
        ark_own(&heap, &input, "unknown", nested.clone());
        let symbol = heap.symbol_key(&heap.symbol(Some("unknown symbol".into())).unwrap()).unwrap();
        let key = PropertyKey::Symbol(symbol);
        heap.define_own_data(&input, key.clone(), heap.number(1.0).unwrap(), &NoRuntime).unwrap();
        heap.define_own_data(&parsed, key.clone(), heap.number(2.0).unwrap(), &NoRuntime).unwrap();
        heap.define_own(
            &input,
            "hidden".into(),
            PropertyDescriptor::Data {
                value: heap.boolean(true).unwrap(),
                enumerable: false,
                writable: true,
                configurable: true,
            },
            &NoRuntime,
        )
        .unwrap();
        runtime.events.lock().unwrap().clear();
        let GraphContextValidation::Accept(merged) = validate_graph_context(&factory, &context, &input).unwrap() else {
            panic!("authored success should merge the actual record values");
        };
        assert!(!merged.same_identity(&input) && !merged.same_identity(&parsed));
        assert!(get(&factory, &merged, "unknown").unwrap().same_identity(&nested));
        assert!(matches!(
            heap.snapshot(&get(&factory, &merged, "probe").unwrap()).unwrap(),
            ValueSnapshot::Number(2.0)
        ));
        assert!(matches!(
            heap.snapshot(&heap.get(&merged, &key, &NoRuntime).unwrap()).unwrap(),
            ValueSnapshot::Number(2.0)
        ));
        assert!(!heap.has_own(&merged, &"hidden".into(), &NoRuntime).unwrap());
        assert_eq!(*runtime.events.lock().unwrap(), ["ark", "errors", "has-instance", "input-get", "out-get"]);
        assert!(runtime.context.lock().unwrap().as_ref().unwrap().same_identity(context.record()));
        for alias in [
            heap.null().unwrap(),
            heap.number(7.0).unwrap(),
            heap.array(vec![]).unwrap(),
            heap.function(CapabilityId(202)).unwrap(),
        ] {
            *runtime.out.lock().unwrap() = alias.clone();
            runtime.events.lock().unwrap().clear();
            assert!(matches!(validate_graph_context(&factory, &context, &input).unwrap(),
                GraphContextValidation::Accept(value) if value.same_identity(&alias)));
            assert_eq!(*runtime.events.lock().unwrap(), ["ark", "errors", "has-instance"]);
        }
        *runtime.out.lock().unwrap() = parsed.clone();
        ark_own(&heap, context.record(), "ark", heap.function(CapabilityId(212)).unwrap());
        runtime.events.lock().unwrap().clear();
        validate_graph_context(&factory, &context, &input).unwrap();
        assert_eq!(runtime.events.lock().unwrap().first(), Some(&"replacement-ark"));
        ark_own(&heap, context.record(), "ark", ark.clone());
        runtime.events.lock().unwrap().clear();
        assert!(matches!(validate_graph_context(&unbound, &context, &input), Err(ValueError::UnsupportedOperation(_))));
        assert_eq!(*runtime.events.lock().unwrap(), ["ark"], "missing type is checked after the actual call");

        *runtime.out.lock().unwrap() = errors.clone();
        *runtime.json.lock().unwrap() = heap.boolean(true).unwrap();
        accessor(&heap, context.record(), "json".into(), 211);
        runtime.events.lock().unwrap().clear();
        let GraphContextValidation::RejectActual { flat_issues: first_flat, messages } =
            validate_graph_context(&factory, &context, &input).unwrap()
        else {
            panic!("Ark cross-field errors cannot become JSON success");
        };
        assert!(matches!(
            heap.snapshot(&heap.get(&first_flat, &"length".into(), &NoRuntime).unwrap()).unwrap(),
            ValueSnapshot::Number(0.0)
        ));
        assert!(matches!(heap.snapshot(&heap.get(&messages, &"0".into(), &NoRuntime).unwrap()).unwrap(),
            ValueSnapshot::String(value) if value.equals_ascii("  - count: bad count")));
        assert_eq!(*runtime.events.lock().unwrap(), ["ark", "errors", "has-instance", "json", "path", "message"]);
        let GraphContextValidation::RejectActual { flat_issues: second_flat, .. } =
            validate_graph_context(&factory, &context, &input).unwrap()
        else {
            unreachable!()
        };
        assert!(!first_flat.same_identity(&second_flat));
        ark_own(&heap, &errors, "map", heap.function(CapabilityId(209)).unwrap());
        let GraphContextValidation::RejectActual { messages, .. } =
            validate_graph_context(&factory, &context, &input).unwrap()
        else {
            unreachable!()
        };
        assert!(messages.same_identity(&runtime.override_messages), "actual map override controls the result");
        ark_own(&heap, &errors, "map", heap.function(CapabilityId(213)).unwrap());
        let GraphContextValidation::RejectActual { messages, .. } =
            validate_graph_context(&factory, &context, &input).unwrap()
        else {
            unreachable!()
        };
        assert!(matches!(heap.snapshot(&heap.get(&messages, &"0".into(), &NoRuntime).unwrap()).unwrap(),
            ValueSnapshot::String(value) if value.equals_ascii("  - count: bad count")));

        let truthy_out = heap.array(vec![error.clone()]).unwrap();
        *runtime.out.lock().unwrap() = truthy_out.clone();
        runtime.mode.store(1, Ordering::SeqCst);
        assert!(matches!(
            validate_graph_context(&factory, &context, &input).unwrap(),
            GraphContextValidation::RejectActual { .. }
        ));
        runtime.mode.store(0, Ordering::SeqCst);
        assert!(matches!(validate_graph_context(&factory, &context, &input).unwrap(),
            GraphContextValidation::Accept(value) if value.same_identity(&truthy_out)));
        *runtime.out.lock().unwrap() = errors.clone();

        // Live constructor replacement uses actual ordinary prototype identity.
        let prototype = heap.object().unwrap();
        let constructor = heap.function(CapabilityId(202)).unwrap();
        ark_own(&heap, &constructor, "prototype", prototype.clone());
        let instance = heap.object_with_prototype(Some(&prototype)).unwrap();
        ark_own(&heap, &instance, "length", heap.number(1.0).unwrap());
        ark_own(&heap, &instance, "0", error);
        ark_own(&heap, &instance, "map", runtime.map.clone());
        *runtime.constructor.lock().unwrap() = constructor.clone();
        *runtime.out.lock().unwrap() = instance;
        assert!(matches!(
            validate_graph_context(&factory, &context, &input).unwrap(),
            GraphContextValidation::RejectActual { .. }
        ));
        let impostor = heap.object().unwrap();
        ark_own(&heap, &impostor, "constructor", constructor);
        *runtime.out.lock().unwrap() = impostor;
        assert!(matches!(
            validate_graph_context(&factory, &context, &input).unwrap(),
            GraphContextValidation::Accept(_)
        ));
        *runtime.constructor.lock().unwrap() = rhs.clone();
        *runtime.out.lock().unwrap() = errors.clone();
        for (mode, expected) in
            [(2, vec!["ark", "errors", "has-instance"]), (3, vec!["ark"]), (5, vec!["ark", "errors"])]
        {
            runtime.mode.store(mode, Ordering::SeqCst);
            runtime.events.lock().unwrap().clear();
            assert!(
                matches!(validate_graph_context(&factory, &context, &input), Err(ValueError::Thrown(value)) if value.same_identity(&runtime.thrown))
            );
            assert_eq!(*runtime.events.lock().unwrap(), expected);
        }
        runtime.mode.store(0, Ordering::SeqCst);
        heap.define_own_data(&rhs, heap.has_instance_key(), heap.number(1.0).unwrap(), &NoRuntime).unwrap();
        assert!(matches!(validate_graph_context(&factory, &context, &input), Err(ValueError::TypeError(_))));
        heap.define_own_data(&rhs, heap.has_instance_key(), heap.function(CapabilityId(204)).unwrap(), &NoRuntime)
            .unwrap();

        let tool = heap.object().unwrap();
        ark_own(&heap, &tool, "name", heap.string("ark".into()).unwrap());
        ark_own(&heap, &tool, "parameters", ark);
        let request = heap.object().unwrap();
        ark_own(&heap, &request, "name", heap.string("ark".into()).unwrap());
        let args = heap.object().unwrap();
        ark_own(&heap, &args, "count", heap.number(3.0).unwrap());
        ark_own(&heap, &request, "arguments", args);
        let Err(GraphToolValidationError::Validation(message)) =
            validate_graph_tool_arguments(&factory, &tool, &request)
        else {
            panic!("driver must retain Ark rejection when JSON succeeds");
        };
        let needle = ara_rpc::WireString::from("bad count");
        assert!(message.units().windows(needle.units().len()).any(|part| part == needle.units()));
        *runtime.json.lock().unwrap() = heap
            .import_serde(&json!({"type":"object","properties":{"count":{"type":"boolean"}},"required":["count"]}))
            .unwrap();
        runtime.mode.store(4, Ordering::SeqCst);
        let args = heap.object().unwrap();
        ark_own(&heap, &args, "count", heap.string("true".into()).unwrap());
        ark_own(&heap, &request, "arguments", args);
        runtime.events.lock().unwrap().clear();
        let accepted = validate_graph_tool_arguments(&factory, &tool, &request).unwrap();
        assert!(matches!(
            heap.snapshot(&get(&factory, &accepted, "count").unwrap()).unwrap(),
            ValueSnapshot::Boolean(true)
        ));
        assert!(runtime.events.lock().unwrap().iter().filter(|event| **event == "ark").count() >= 2);
    }

    struct Effects {
        heap: Arc<ValueHeap>,
        thrown: ValueHandle,
        mode: AtomicUsize,
        json_reads: AtomicUsize,
        next_reads: AtomicUsize,
        log: Mutex<Vec<u64>>,
        tuple: Mutex<Option<ValueHandle>>,
        iterator: Mutex<Option<ValueHandle>>,
        entry: Mutex<Option<ValueHandle>>,
        partial: Mutex<Option<ValueHandle>>,
    }
    impl Effects {
        fn reply(
            &self,
            capability: CapabilityId,
            receiver: &ValueHandle,
            arguments: &[ValueHandle],
            runtime: &dyn ara_rpc::value_heap::ValueRuntime,
        ) -> Result<RuntimeReply, ValueError> {
            self.log.lock().unwrap().push(capability.0);
            let value = match capability.0 {
                11 => match self.mode.load(Ordering::SeqCst) {
                    0 => self.heap.undefined()?,
                    2 => return Err(ValueError::Thrown(self.thrown.clone())),
                    _ => self.heap.function(CapabilityId(70))?,
                },
                12 => {
                    if self.mode.load(Ordering::SeqCst) == 3 {
                        return Err(ValueError::Thrown(self.thrown.clone()));
                    }
                    self.heap.function(CapabilityId(71))?
                }
                21 => self.heap.boolean(self.json_reads.fetch_add(1, Ordering::SeqCst).is_multiple_of(2))?,
                31 => {
                    self.heap.define_own_data(receiver, "side".into(), self.heap.boolean(true)?, runtime)?;
                    self.thrown.clone()
                }
                41 => self.heap.boolean(false)?,
                42 => {
                    let callback =
                        arguments.first().ok_or(ValueError::TypeError("missing predicate callback".into()))?;
                    let entry = self.heap.get(receiver, &"0".into(), runtime)?;
                    self.heap.call(callback, &self.heap.undefined()?, &[entry], runtime)?
                }
                43 => {
                    self.heap.array_push(receiver, arguments, runtime)?;
                    *self.partial.lock().unwrap() = Some(receiver.clone());
                    return Err(ValueError::Thrown(self.thrown.clone()));
                }
                44 => self.iterator.lock().unwrap().as_ref().unwrap().clone(),
                45 => self.heap.function(CapabilityId(43))?,
                46 => self.heap.object()?,
                47 => {
                    let done = self.next_reads.fetch_add(1, Ordering::SeqCst) != 0;
                    let result = self.heap.object()?;
                    self.heap.define_own_data(&result, "done".into(), self.heap.boolean(done)?, runtime)?;
                    if !done {
                        self.heap.define_own_data(
                            &result,
                            "value".into(),
                            self.entry.lock().unwrap().as_ref().unwrap().clone(),
                            runtime,
                        )?;
                    }
                    result
                }
                48 => self.heap.boolean(true)?,
                51 => self.tuple.lock().unwrap().as_ref().unwrap().clone(),
                52..=54 => self.heap.boolean(false)?,
                70 => return Err(ValueError::Thrown(self.thrown.clone())),
                _ => return Err(ValueError::MissingCapability(capability)),
            };
            Ok(RuntimeReply::Value(value))
        }
    }
    impl ara_rpc::value_heap::ValueRuntime for Effects {
        fn resolve(&self, request: RuntimeRequest<'_>) -> Result<RuntimeReply, ValueError> {
            match request {
                RuntimeRequest::Call { capability, receiver, arguments } => {
                    self.reply(capability, receiver, arguments, self)
                }
                request => Err(ValueError::MissingCapability(request.capability())),
            }
        }
        fn resolve_call(
            &self,
            capability: CapabilityId,
            receiver: &ValueHandle,
            arguments: &[ValueHandle],
            effective_runtime: &dyn ara_rpc::value_heap::ValueRuntime,
        ) -> Result<RuntimeReply, ValueError> {
            self.reply(capability, receiver, arguments, effective_runtime)
        }
    }

    pub(crate) fn stamps_methods_and_failures() {
        let heap = ValueHeap::new().unwrap();
        let runtime = Arc::new(Effects {
            heap: heap.clone(),
            thrown: heap.object().unwrap(),
            mode: AtomicUsize::new(0),
            json_reads: AtomicUsize::new(0),
            next_reads: AtomicUsize::new(0),
            log: Mutex::new(Vec::new()),
            tuple: Mutex::new(None),
            iterator: Mutex::new(None),
            entry: Mutex::new(None),
            partial: Mutex::new(None),
        });
        let factory = GraphValueFactory::new(heap.clone(), runtime.clone()).unwrap();
        let key = heap.module_symbol("ara-ai.validation", "ai.validationContext").unwrap();
        let callable = heap.function(CapabilityId(90)).unwrap();
        accessor(&heap, &callable, "toJsonSchema".into(), 11);
        accessor(&heap, &callable, "assert".into(), 12);
        let non_ark = get_graph_validation_context(&factory, &callable).unwrap();
        assert!(graph_context_normalization_schema(&factory, &non_ark).unwrap().same_identity(&callable));
        assert_eq!(*runtime.log.lock().unwrap(), vec![11]);
        runtime.mode.store(1, Ordering::SeqCst);
        get_graph_validation_context(&factory, &callable).unwrap();
        assert_eq!(*runtime.log.lock().unwrap(), vec![11], "warm cache bypasses Ark Get and detection");
        heap.delete_own(&callable, &key, factory.runtime()).unwrap();
        runtime.log.lock().unwrap().clear();
        let after_delete = get_graph_validation_context(&factory, &callable).unwrap();
        assert!(after_delete.record().same_identity(non_ark.record()));
        assert!(runtime.log.lock().unwrap().is_empty(), "own-property deletion cannot invalidate the weak memo");
        // A fresh source identity reaches the authored Ark failure paths.
        // Throws do not publish a memo, so this same fresh identity may retry.
        let failing_callable = heap.function(CapabilityId(90)).unwrap();
        accessor(&heap, &failing_callable, "toJsonSchema".into(), 11);
        accessor(&heap, &failing_callable, "assert".into(), 12);
        assert!(matches!(
            get_graph_validation_context(&factory, &failing_callable),
            Err(ValueError::Thrown(value)) if value.same_identity(&runtime.thrown)
        ));
        assert_eq!(*runtime.log.lock().unwrap(), vec![11, 12, 11, 70]);
        assert!(!heap.has_own(&callable, &key, factory.runtime()).unwrap());
        assert!(!heap.has_own(&failing_callable, &key, factory.runtime()).unwrap());
        for (mode, expected) in [(2, vec![11]), (3, vec![11, 12])] {
            runtime.mode.store(mode, Ordering::SeqCst);
            runtime.log.lock().unwrap().clear();
            assert!(
                matches!(get_graph_validation_context(&factory, &failing_callable), Err(ValueError::Thrown(value)) if value.same_identity(&runtime.thrown))
            );
            assert_eq!(*runtime.log.lock().unwrap(), expected);
        }

        let parameters = heap.object().unwrap();
        accessor(&heap, &parameters, key, 31);
        runtime.log.lock().unwrap().clear();
        let cached = get_graph_validation_context(&factory, &parameters).unwrap();
        assert!(!cached.record().same_identity(&runtime.thrown));
        assert!(graph_context_normalization_schema(&factory, &cached).unwrap().same_identity(&parameters));
        assert!(!heap.has_own(&parameters, &"side".into(), factory.runtime()).unwrap());
        assert!(runtime.log.lock().unwrap().is_empty(), "host Symbol accessor must not be read by weak memo lookup");
        assert!(get_graph_validation_context(&factory, &parameters).unwrap().record().same_identity(cached.record()));
        assert!(runtime.log.lock().unwrap().is_empty());

        let record = heap.object().unwrap();
        heap.define_own_data(&record, "kind".into(), heap.string("json".into()).unwrap(), &NoRuntime).unwrap();
        accessor(&heap, &record, "json".into(), 21);
        let context = GraphValidationContext { record };
        let first = graph_context_normalization_schema(&factory, &context).unwrap();
        assert!(matches!(heap.snapshot(&first).unwrap(), ValueSnapshot::Boolean(true)));
        assert!(matches!(
            validate_graph_context(&factory, &context, &heap.object().unwrap()).unwrap(),
            GraphContextValidation::Reject(_)
        ));
        assert_eq!(runtime.json_reads.load(Ordering::SeqCst), 2);

        // A replaced some method controls the Native precheck. The second
        // version calls the operation-scoped predicate with its effective runtime.
        let array = heap.import_serde(&json!([{"nullable":false}])).unwrap();
        heap.define_own_data(&array, "some".into(), heap.function(CapabilityId(41)).unwrap(), &NoRuntime).unwrap();
        runtime.log.lock().unwrap().clear();
        assert!(
            crate::schema_draft::upgrade_graph_json_schema_to_202012(&factory, &array).unwrap().same_identity(&array)
        );
        assert_eq!(*runtime.log.lock().unwrap(), vec![41]);
        heap.assign(&array, "some".into(), heap.function(CapabilityId(42)).unwrap(), &NoRuntime).unwrap();
        runtime.log.lock().unwrap().clear();
        let upgraded = crate::schema_draft::upgrade_graph_json_schema_to_202012(&factory, &array).unwrap();
        assert!(!upgraded.same_identity(&array));
        assert_eq!(*runtime.log.lock().unwrap(), vec![42]);

        let tuple_source = heap.object().unwrap();
        *runtime.tuple.lock().unwrap() = Some(heap.array(vec![heap.boolean(true).unwrap()]).unwrap());
        accessor(&heap, &tuple_source, "items".into(), 51);
        accessor(&heap, &tuple_source, "additionalItems".into(), 52);
        accessor(&heap, &tuple_source, "nullable".into(), 53);
        runtime.log.lock().unwrap().clear();
        crate::schema_draft::upgrade_graph_json_schema_to_202012(&factory, &tuple_source).unwrap();
        let log = runtime.log.lock().unwrap().clone();
        assert_eq!(log.iter().filter(|&&id| id == 51).count(), 4);
        assert_eq!(log.iter().filter(|&&id| id == 52).count(), 4);
        assert_eq!(log.iter().filter(|&&id| id == 53).count(), 2);

        let kinds = heap.array(vec![heap.string("string".into()).unwrap()]).unwrap();
        heap.define_own_data(&kinds, "includes".into(), heap.function(CapabilityId(48)).unwrap(), &NoRuntime).unwrap();
        let nullable = heap.object().unwrap();
        heap.define_own_data(&nullable, "type".into(), kinds.clone(), &NoRuntime).unwrap();
        heap.define_own_data(&nullable, "nullable".into(), heap.boolean(true).unwrap(), &NoRuntime).unwrap();
        runtime.log.lock().unwrap().clear();
        let upgraded = crate::schema_draft::upgrade_graph_json_schema_to_202012(&factory, &nullable).unwrap();
        assert!(get(&factory, &upgraded, "type").unwrap().same_identity(&kinds));
        assert_eq!(*runtime.log.lock().unwrap(), vec![48]);

        // Body failure after the actual result.push Get/Call retains its
        // mutation and closes the input iterator with the original throw.
        let array = heap.import_serde(&json!([{"nullable":false}])).unwrap();
        let iterator = heap.object().unwrap();
        for (name, id) in [("next", 47), ("return", 46)] {
            heap.define_own_data(&iterator, name.into(), heap.function(CapabilityId(id)).unwrap(), &NoRuntime).unwrap();
        }
        let entry = get(&factory, &array, "0").unwrap();
        accessor(&heap, &entry, "nullable".into(), 54);
        *runtime.entry.lock().unwrap() = Some(entry);
        *runtime.iterator.lock().unwrap() = Some(iterator);
        heap.define_own_data(&array, heap.iterator_key(), heap.function(CapabilityId(44)).unwrap(), &NoRuntime)
            .unwrap();
        accessor(&heap, &heap.array_prototype(), "push".into(), 45);
        runtime.log.lock().unwrap().clear();
        assert!(
            matches!(crate::schema_draft::upgrade_graph_json_schema_to_202012(&factory, &array), Err(ValueError::Thrown(value)) if value.same_identity(&runtime.thrown))
        );
        // Precheck child Get first; in conversion push Get(45) precedes
        // both child nullable Gets(54), then push Call(43) and close(46).
        assert_eq!(*runtime.log.lock().unwrap(), vec![54, 44, 47, 45, 54, 54, 43, 46]);
        let partial = runtime.partial.lock().unwrap().as_ref().unwrap().clone();
        assert!(matches!(
            heap.snapshot(&get(&factory, &partial, "length").unwrap()).unwrap(),
            ValueSnapshot::Number(1.0)
        ));
    }
}
