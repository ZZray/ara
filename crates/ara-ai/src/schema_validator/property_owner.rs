//! OMP b91db66e json-schema-validator.ts:185-310 (MIT, parent notice).
//! Same-instance property ownership, with positive/negative/predicate visits.
use super::*;
use ara_rpc::value_heap::PropertyKey;
use std::collections::HashMap;

static OWNER_ENUM: CompiledFunctionSpec = CompiledFunctionSpec {
    source: "omp:b91db66e:json-schema-validator.ts:227", name: "", length: 1,
    form: CompiledFunctionForm::Arrow, capture_count: 1, body: enum_callback,
};
static OWNER_VISIT: CompiledFunctionSpec = CompiledFunctionSpec {
    source: "omp:b91db66e:json-schema-validator.ts:260", name: "", length: 1,
    form: CompiledFunctionForm::Arrow, capture_count: 4, body: visit_callback,
};
static OWNER_REQUIRED: CompiledFunctionSpec = CompiledFunctionSpec {
    source: "omp:b91db66e:json-schema-validator.ts:208", name: "", length: 1,
    form: CompiledFunctionForm::Arrow, capture_count: 0, body: required_every_callback,
};
fn enum_callback(heap: &Arc<ValueHeap>, captures: &[ValueHandle], _: &ValueHandle, args: &[ValueHandle], runtime: &dyn ValueRuntime) -> Result<ValueHandle, ValueError> {
    let read = ReadContext::Graph { factory: GraphExecution::new(heap, runtime), runtime };
    let value = ValueRef::Graph(checker_argument(heap, args)?);
    let key = read.string(&ValueRef::Graph(captures[0].clone()))?.expect("captured key");
    heap.boolean(read.is_object(&value)? && read.has_own(&value, &key)?)
}
fn visit_callback(heap: &Arc<ValueHeap>, captures: &[ValueHandle], _: &ValueHandle, args: &[ValueHandle], runtime: &dyn ValueRuntime) -> Result<ValueHandle, ValueError> {
    let read = ReadContext::Graph { factory: GraphExecution::new(heap, runtime), runtime };
    let key = read.string(&ValueRef::Graph(captures[1].clone()))?.expect("captured key");
    let context = read.number(&ValueRef::Graph(captures[2].clone()))?.expect("captured context") as u8;
    let mut owner = Owner { root: ValueRef::Graph(captures[0].clone()), key, read, visited: HashMap::new(), graph_visited: Some(captures[3].clone()) };
    heap.boolean(owner.visit(ValueRef::Graph(checker_argument(heap, args)?), context)?)
}
struct Owner<'a, 'r> {
    root: ValueRef<'a>, key: WireString, read: ReadContext<'r>,
    visited: HashMap<NodeIdentity, u8>, graph_visited: Option<ValueHandle>,
}
impl<'a> Owner<'a, '_> {
    fn resolve_ref(&self, reference: ValueRef<'a>) -> Result<ValueRef<'a>, ValueError> {
        let r = self.read;
        let ReadContext::Graph { factory, runtime } = r else {
            let reference = r.string(&reference)?.ok_or_else(|| ValueError::TypeError("ref.startsWith is not callable".into()))?;
            return resolve_local_ref(self.root.clone(), &reference, &r);
        };
        if r.string(&reference)?.is_some_and(|reference| reference.equals_ascii("#")) { return Ok(self.root.clone()); }
        if !r.truthy(&r.call(&reference, "startsWith", &[ValueRef::String("#/".into())])?)? { return Ok(ValueRef::Undefined); }
        let tail = r.call(&reference, "slice", &[ValueRef::Number(2.0)])?;
        let tokens = r.call(&tail, "split", &[ValueRef::String("/".into())])?;
        let mut current = self.root.clone();
        r.for_of(&tokens, |token| {
            let first = factory.heap().regexp("~1".into(), "g")?;
            let token = r.call(&token, "replace", &[ValueRef::Graph(first), ValueRef::String("/".into())])?;
            let second = factory.heap().regexp("~0".into(), "g")?;
            let token = r.call(&token, "replace", &[ValueRef::Graph(second), ValueRef::String("~".into())])?;
            if !r.is_object(&current)? && !r.is_array(&current)? { current = ValueRef::Undefined; return Ok(LoopControl::Break); }
            let token = factory.heap().to_primitive(&r.graph_handle(&token)?, true, runtime)?;
            let key = if factory.heap().kind(&token)? == ara_rpc::value_heap::ValueKind::Symbol {
                PropertyKey::Symbol(factory.heap().symbol_key(&token)?)
            } else { PropertyKey::String(factory.heap().to_string(&token, runtime)?) };
            current = ValueRef::Graph(factory.heap().get(&r.graph_handle(&current)?, &key, runtime)?);
            Ok(LoopControl::Continue)
        })?;
        Ok(current)
    }
    fn includes(&self, value: &ValueRef<'a>) -> Result<bool, ValueError> {
        Ok(self.read.is_array(value)? && self.read.truthy(&self.read.call(value, "includes", &[ValueRef::String(self.key.clone())])?)?)
    }
    fn values(&self, node: &ValueRef<'a>) -> Result<ValueRef<'a>, ValueError> {
        let mut values = Vec::new();
        for key in self.read.own_keys(node)? { values.push(self.read.property(node, &key)?); }
        self.read.make_array(values)
    }
    fn some_visit(&mut self, array: &ValueRef<'a>, context: u8) -> Result<bool, ValueError> {
        let mut captures = vec![self.root.clone(), ValueRef::String(self.key.clone()), ValueRef::Number(context as f64)];
        if let Some(visited) = &self.graph_visited { captures.push(ValueRef::Graph(visited.clone())); }
        let read = self.read;
        let result = read.predicate(array, "some", &OWNER_VISIT, &captures, |branch, _| self.visit(branch, context))?;
        read.truthy(&result)
    }
    fn mark(&mut self, node: &ValueRef<'a>, context: u8) -> Result<bool, ValueError> {
        let identity = self.read.identity(node)?.expect("record identity");
        if let (ReadContext::Graph { factory, runtime }, Some(table), ValueRef::Graph(node)) = (self.read, &self.graph_visited, node) {
            let key = PropertyKey::String(format!("{:?}", node.node_id()).into());
            let current = factory.heap().get(table, &key, runtime)?;
            let previous = match factory.heap().snapshot(&current)? { ara_rpc::value_heap::ValueSnapshot::Number(n) => n as u8, _ => 0 };
            if previous & context != 0 { return Ok(false); }
            factory.heap().define_own_data(table, key, factory.heap().number((previous | context) as f64)?, runtime)?;
        } else {
            let previous = self.visited.entry(identity).or_default();
            if *previous & context != 0 { return Ok(false); }
            *previous |= context;
        }
        Ok(true)
    }
    fn presence(&self, node: ValueRef<'a>, depth: usize) -> Result<bool, ValueError> {
        let r = &self.read;
        if !r.is_object(&node)? || depth >= MAX_REF_DEPTH { return Ok(false); }
        if r.own_keys(&node)?.iter().any(|key| !["required", "type", "$ref", "description", "title", "$comment"].iter().any(|name| key.equals_ascii(name))) { return Ok(false); }
        if r.has(&node, &"$ref".into())? {
            if r.string(&r.get(&node, "$ref")?)?.is_none() { return Ok(false); }
            if !r.is_undefined(&r.get(&node, "required")?)? || !r.is_undefined(&r.get(&node, "type")?)? { return Ok(false); }
            return self.presence(self.resolve_ref(r.get(&node, "$ref")?)?, depth + 1);
        }
        if !r.is_undefined(&r.get(&node, "type")?)? && !r.string(&r.get(&node, "type")?)?.is_some_and(|kind| kind.equals_ascii("object")) { return Ok(false); }
        if !r.is_array(&r.get(&node, "required")?)? { return Ok(false); }
        Ok(r.truthy(&r.call(&r.get(&node, "required")?, "includes", &[ValueRef::String(self.key.clone())])?)? && r.truthy(&r.predicate(&r.get(&node, "required")?, "every", &OWNER_REQUIRED, &[], |entry, read| Ok(read.string(&entry)?.is_some()))?)?)
    }
    fn valid_property_name(&self, schema: ValueRef<'a>) -> Result<bool, ValueError> {
        let mut issues = Vec::new();
        let mut warn = |message: &str| eprintln!("{message}");
        let mut context = ValidationContext { root: self.root.clone(), seen_pairs: HashSet::new(), ref_depth: 0, warning_seen: &SEEN_UNEVALUATED_WARNING, warn: &mut warn };
        validate_node(schema, ValueRef::String(self.key.clone()), &[], &self.read, &mut context, &mut issues)
    }
    fn visit(&mut self, node: ValueRef<'a>, context: u8) -> Result<bool, ValueError> {
        let r = self.read;
        if !r.is_object(&node)? || !self.mark(&node, context)? { return Ok(false); }
        if r.is_object(&r.get(&node, "const")?)? && r.has_own(&r.get(&node, "const")?, &self.key)? { return Ok(true); }
        if r.is_array(&r.get(&node, "enum")?)? && r.truthy(&r.predicate(&r.get(&node, "enum")?, "some", &OWNER_ENUM, &[ValueRef::String(self.key.clone())], |value, read| Ok(read.is_object(&value)? && read.has_own(&value, &self.key)?))?)? { return Ok(true); }
        let properties = r.get(&node, "properties")?;
        if r.is_object(&properties)? && r.has_own(&properties, &self.key)? {
            return Ok(r.boolean(&r.property(&properties, &self.key)?)? != Some(false) || context != 1);
        }
        if self.includes(&r.get(&node, "required")?)? { return Ok(true); }
        if !r.is_undefined(&r.get(&node, "propertyNames")?)? && r.boolean(&r.get(&node, "additionalProperties")?)? != Some(false) && self.valid_property_name(r.get(&node, "propertyNames")?)? { return Ok(true); }
        let patterns = r.get(&node, "patternProperties")?;
        if r.is_object(&patterns)? {
            // Object.entries eagerly reads all values before entering the loop.
            let entries = r.own_keys(&patterns)?.into_iter().map(|key| Ok((key.clone(), r.property(&patterns, &key)?))).collect::<Result<Vec<_>, ValueError>>()?;
            for (pattern, schema) in entries {
                let matches = match r {
                    ReadContext::Graph { factory, runtime } => {
                        let attempt = (|| { let regex = factory.heap().regexp(pattern, "")?; let method = factory.heap().get(&regex, &"test".into(), runtime)?; factory.heap().truthy(&factory.heap().call(&method, &regex, &[factory.heap().string(self.key.clone())?], runtime)?) })();
                        match attempt { Ok(value) => value, Err(error) if error.is_native_exception() => false, Err(error) => return Err(error) }
                    },
                    ReadContext::Wire => compile_pattern(&pattern).ok().is_some_and(|regex| regex.find_from_ucs2(self.key.units(), 0).next().is_some()),
                };
                if matches && r.boolean(&schema)? != Some(false) { return Ok(true); }
            }
        }
        if r.is_object(&r.get(&node, "unevaluatedProperties")?)? || r.is_object(&r.get(&node, "additionalProperties")?)? { return Ok(true); }
        for keyword in ["anyOf", "oneOf", "allOf"] {
            let branches = r.get(&node, keyword)?;
            if r.is_array(&branches)? && self.some_visit(&branches, context)? { return Ok(true); }
        }
        if self.visit(r.get(&node, "if")?, 4)? { return Ok(true); }
        for keyword in ["then", "else"] { if self.visit(r.get(&node, keyword)?, context)? { return Ok(true); } }
        if (context != 1 || !self.presence(r.get(&node, "not")?, 0)?) && self.visit(r.get(&node, "not")?, if context == 4 { 4 } else if context == 1 { 2 } else { 1 })? { return Ok(true); }
        let schemas = r.get(&node, "dependentSchemas")?;
        if r.is_object(&schemas)? {
            if r.has_own(&schemas, &self.key)? { return Ok(true); }
            if self.some_visit(&self.values(&schemas)?, context)? { return Ok(true); }
        }
        let required = r.get(&node, "dependentRequired")?;
        if r.is_object(&required)? {
            if r.has_own(&required, &self.key)? { return Ok(true); }
            let values = self.values(&required)?; let mut found = false;
            r.for_of(&values, |value| { if self.includes(&value)? { found = true; Ok(LoopControl::Break) } else { Ok(LoopControl::Continue) } })?;
            if found { return Ok(true); }
        }
        let dependencies = r.get(&node, "dependencies")?;
        if r.is_object(&dependencies)? {
            if r.has_own(&dependencies, &self.key)? { return Ok(true); }
            let values = self.values(&dependencies)?; let mut found = false;
            r.for_of(&values, |value| { if self.includes(&value)? || self.visit(value, context)? { found = true; Ok(LoopControl::Break) } else { Ok(LoopControl::Continue) } })?;
            if found { return Ok(true); }
        }
        if r.string(&r.get(&node, "$ref")?)?.is_some() {
            let resolved = self.resolve_ref(r.get(&node, "$ref")?)?;
            if !r.is_undefined(&resolved)? && self.visit(resolved, context)? { return Ok(true); }
        }
        Ok(false)
    }
}
pub fn graph_schema_defines_property_in(heap: &Arc<ValueHeap>, runtime: &dyn ValueRuntime, schema: &ValueHandle, key: &str) -> Result<bool, ValueError> {
    let root = ValueRef::Graph(heap.adopt(schema)?);
    let read = ReadContext::Graph { factory: GraphExecution::new(heap, runtime), runtime };
    let mut owner = Owner { root: root.clone(), key: key.into(), read, visited: HashMap::new(), graph_visited: Some(heap.object_with_prototype(None)?) };
    owner.visit(root, 1)
}
pub fn schema_defines_property(schema: &serde_json::Value, key: &str) -> bool {
    let schema = WireValue::parse(&serde_json::to_string(schema).expect("serde value serializes")).expect("serde JSON parses");
    let root = ValueRef::Wire(&schema);
    Owner { root: root.clone(), key: key.into(), read: ReadContext::Wire, visited: HashMap::new(), graph_visited: None }.visit(root, 1).expect("Wire ownership has no runtime boundary")
}

#[cfg(test)]
pub(crate) fn observe_property_owner_family() {
    use ara_rpc::value_heap::NoRuntime;
    use serde_json::json;
    let cases = [
        (json!({"properties":{"i":{}}}), true),
        (json!({"properties":{"i":false},"required":["i"]}), false),
        (json!({"properties":{"nested":{"properties":{"i":{}}}}}), false),
        (json!({"items":{"properties":{"i":{}}}}), false),
        (json!({"$defs":{"unused":{"properties":{"i":{}}}}}), false),
        (json!({"required":["i"]}), true),
        (json!({"const":{"i":null}}), true),
        (json!({"enum":[null,{}, {"i":false}]}), true),
        (json!({"enum":[{"nested":{"i":0}}]}), false),
        (json!({"propertyNames":{"const":"i"}}), true),
        (json!({"propertyNames":{"const":"x"}}), false),
        (json!({"propertyNames":false}), false),
        (json!({"propertyNames":true}), true),
        (json!({"propertyNames":true,"additionalProperties":false}), false),
        (json!({"propertyNames":{"$ref":"#/$defs/name"},"$defs":{"name":{"const":"i"}}}), true),
        (json!({"propertyNames":{"$ref":"#/$defs/name"},"$defs":{"name":{"const":"x"}}}), false),
        (json!({"patternProperties":{"^i$":{}}}), true),
        (json!({"patternProperties":{"^i$":false}}), false),
        (json!({"patternProperties":{"[":{},"^i$":{}}}), true),
        (json!({"patternProperties":{"^x$":{}}}), false),
        (json!({"additionalProperties":{}}), true),
        (json!({"additionalProperties":true}), false),
        (json!({"unevaluatedProperties":{}}), true),
        (json!({"unevaluatedProperties":true}), false),
        (json!({"anyOf":[false, {"properties":{"i":{}}}]}), true),
        (json!({"oneOf":[{"required":["i"]}]}), true),
        (json!({"allOf":[{"const":{"i":0}}]}), true),
        (json!({"if":{"properties":{"i":false}}}), true),
        (json!({"then":{"properties":{"i":{}}}}), true),
        (json!({"else":{"required":["i"]}}), true),
        (json!({"not":{"required":["i"]}}), false),
        (json!({"not":{"type":"object","required":["i"],"description":"presence"}}), false),
        (json!({"not":{"required":["i",1]}}), true),
        (json!({"not":{"required":["i"],"minProperties":1}}), true),
        (json!({"not":{"properties":{"i":false}}}), true),
        (json!({"not":{"not":{"required":["i"]}}}), true),
        (json!({"if":{"not":{"required":["i"]}}}), true),
        (json!({"not":{"$ref":"#/$defs/p"},"$defs":{"p":{"required":["i"]}}}), false),
        (json!({"not":{"$ref":"#/$defs/p","type":"object"},"$defs":{"p":{"required":["i"]}}}), true),
        (json!({"dependentSchemas":{"i":false}}), true),
        (json!({"dependentSchemas":{"x":{"required":["i"]}}}), true),
        (json!({"dependentRequired":{"i":[]}}), true),
        (json!({"dependentRequired":{"x":["i"]}}), true),
        (json!({"dependentRequired":{"x":{"required":["i"]}}}), false),
        (json!({"dependencies":{"i":false}}), true),
        (json!({"dependencies":{"x":["i"]}}), true),
        (json!({"dependencies":{"x":{"required":["i"]}}}), true),
        (json!({"$ref":"#/$defs/a~1b~0c","$defs":{"a/b~c":{"properties":{"i":{}}}}}), true),
        (json!({"$ref":"https://invalid/ref"}), false),
        (json!({"$ref":"#"}), false),
    ];
    let heap = ValueHeap::new().unwrap();
    for (schema, expected) in cases {
        assert_eq!(schema_defines_property(&schema, "i"), expected, "typed {schema}");
        let graph = heap.import_serde(&schema).unwrap();
        assert_eq!(graph_schema_defines_property_in(&heap, &NoRuntime, &graph, "i").unwrap(), expected, "graph {schema}");
        assert_eq!(heap.export_serde(&graph).unwrap(), schema, "ownership must not mutate source");
    }
    let astral = json!({"patternProperties":{"^.$":{}}});
    assert!(!schema_defines_property(&astral, "😀"));
    assert!(!graph_schema_defines_property_in(&heap, &NoRuntime, &heap.import_serde(&astral).unwrap(), "😀").unwrap());
    // The same identity must be reconsidered under each context bit.
    let shared = heap.import_serde(&json!({"properties":{"i":false}})).unwrap();
    let root = heap.object().unwrap();
    heap.define_own_data(&root, "anyOf".into(), heap.array(vec![shared.clone()]).unwrap(), &NoRuntime).unwrap();
    heap.define_own_data(&root, "if".into(), shared, &NoRuntime).unwrap();
    assert!(graph_schema_defines_property_in(&heap, &NoRuntime, &root, "i").unwrap());
    let cycle = heap.object().unwrap();
    heap.define_own_data(&cycle, "allOf".into(), heap.array(vec![cycle.clone()]).unwrap(), &NoRuntime).unwrap();
    assert!(!graph_schema_defines_property_in(&heap, &NoRuntime, &cycle, "i").unwrap());
    heap.define_own_data(&cycle, "then".into(), heap.import_serde(&json!({"required":["i"]})).unwrap(), &NoRuntime).unwrap();
    assert!(graph_schema_defines_property_in(&heap, &NoRuntime, &cycle, "i").unwrap());
    observe_property_owner_effects();
}

#[cfg(test)]
fn observe_property_owner_effects() {
    use ara_rpc::value_heap::{CallableTarget, CapabilityId, NoRuntime, PropertyDescriptor, RuntimeReply, RuntimeRequest};
    use std::collections::VecDeque;
    use std::sync::{Mutex, Weak};
    use serde_json::json;
    struct Runtime {
        heap: Weak<ValueHeap>, values: Mutex<VecDeque<ValueHandle>>,
        saved: Mutex<Option<ValueHandle>>, entry: ValueHandle, thrown: ValueHandle,
    }
    impl ValueRuntime for Runtime {
        fn resolve(&self, request: RuntimeRequest<'_>) -> Result<RuntimeReply, ValueError> {
            if request.capability() == CapabilityId(9702) { return Err(ValueError::Thrown(self.thrown.clone())); }
            self.values.lock().unwrap().pop_front().map(RuntimeReply::Value).ok_or(ValueError::MissingCapability(request.capability()))
        }
        fn resolve_call(&self, capability: CapabilityId, receiver: &ValueHandle, args: &[ValueHandle], effective: &dyn ValueRuntime) -> Result<RuntimeReply, ValueError> {
            if capability != CapabilityId(9701) { return self.resolve(RuntimeRequest::Call { capability, receiver, arguments: args }); }
            *self.saved.lock().unwrap() = Some(args[0].clone());
            let heap = self.heap.upgrade().unwrap();
            heap.call(&args[0], &heap.undefined()?, std::slice::from_ref(&self.entry), effective).map(RuntimeReply::Value)
        }
    }
    fn getter(heap: &Arc<ValueHeap>, object: &ValueHandle, key: &str, capability: u64) {
        heap.define_own(object, key.into(), PropertyDescriptor::Accessor { get: Some(CallableTarget::Host(CapabilityId(capability))), set: None, enumerable: true, configurable: true }, &NoRuntime).unwrap();
    }
    let weak = {
        let heap = ValueHeap::new().unwrap();
        let entry = heap.import_serde(&json!({"required":["i"]})).unwrap();
        let runtime = Runtime { heap: Arc::downgrade(&heap), values: Mutex::new(VecDeque::new()), saved: Mutex::new(None), entry: entry.clone(), thrown: heap.object().unwrap() };
        // Different values on the two getter reads distinguish source order
        // from prematurely memoizing a caller accessor result.
        for (key, values) in [
            ("const", vec![heap.object().unwrap(), heap.import_serde(&json!({"i":0})).unwrap()]),
            ("enum", vec![heap.array(vec![]).unwrap(), heap.import_serde(&json!([{"i":0}])).unwrap()]),
            ("propertyNames", vec![heap.boolean(false).unwrap(), heap.boolean(true).unwrap()]),
            ("$ref", vec![heap.string("#/missing".into()).unwrap(), heap.string("#/$defs/p".into()).unwrap()]),
        ] {
            let root = heap.import_serde(&json!({"$defs":{"p":{"required":["i"]}}})).unwrap();
            getter(&heap, &root, key, 9700);
            *runtime.values.lock().unwrap() = values.into();
            assert!(graph_schema_defines_property_in(&heap, &runtime, &root, "i").unwrap(), "effectful {key}");
            assert!(runtime.values.lock().unwrap().is_empty());
        }
        let branches = heap.array(vec![]).unwrap();
        heap.define_own_data(&branches, "some".into(), heap.function(CapabilityId(9701)).unwrap(), &NoRuntime).unwrap();
        let root = heap.object().unwrap();
        heap.define_own_data(&root, "anyOf".into(), branches, &NoRuntime).unwrap();
        assert!(graph_schema_defines_property_in(&heap, &runtime, &root, "i").unwrap());
        let saved = runtime.saved.lock().unwrap().take().unwrap();
        // Captured visit state remains real after the public helper returns.
        assert!(!heap.truthy(&heap.call(&saved, &heap.undefined().unwrap(), &[entry], &NoRuntime).unwrap()).unwrap());
        let fresh = heap.import_serde(&json!({"required":["i"]})).unwrap();
        assert!(heap.truthy(&heap.call(&saved, &heap.undefined().unwrap(), &[fresh], &NoRuntime).unwrap()).unwrap());
        let throwing = heap.object().unwrap();
        getter(&heap, &throwing, "const", 9702);
        assert!(matches!(heap.call(&saved, &heap.undefined().unwrap(), &[throwing], &runtime), Err(ValueError::Thrown(value)) if value.same_identity(&runtime.thrown)));
        Arc::downgrade(&heap)
    };
    assert!(weak.upgrade().is_none());
}
