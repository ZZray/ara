//! Trusted bodies for the fixed OMP596f2da wire/validation callback contracts.
//! Copyright (c) 2025 Mario Zechner; Copyright (c) 2025-2026 Can Bölük;
//! Copyright (c) 2026 Stencil Labs, Inc. MIT; see THIRD_PARTY.md.
//! Internal capture edges are NodeIds. No body owns a heap/factory/runtime lease.

use super::*;

pub type CompiledFunctionBody =
    fn(&Arc<ValueHeap>, &[ValueHandle], &ValueHandle, &[ValueHandle], &dyn ValueRuntime) -> ValueResult<ValueHandle>;

/// A derived class does not allocate this before its original super Construct.
/// Its exact active constructor/newTarget are supplied as borrowed handles.
pub type CompiledDerivedConstructorBody = fn(
    &Arc<ValueHeap>,
    &[ValueHandle],
    &ValueHandle,
    &ValueHandle,
    &[ValueHandle],
    &dyn ValueRuntime,
) -> ValueResult<ValueHandle>;

#[derive(Debug, Clone, Copy)]
pub enum CompiledFunctionForm {
    Arrow,
    Ordinary,
    /// Native builtin method: dynamic this, no prototype or construct domain.
    Builtin,
    /// Native function with separate call and construct domains.
    NativeConstructor(CompiledFunctionBody),
    DerivedConstructor(CompiledDerivedConstructorBody),
}
impl PartialEq for CompiledFunctionForm {
    fn eq(&self, other: &Self) -> bool {
        match (self, other) {
            (Self::Arrow, Self::Arrow) | (Self::Ordinary, Self::Ordinary) | (Self::Builtin, Self::Builtin) => true,
            (Self::NativeConstructor(a), Self::NativeConstructor(b)) => std::ptr::fn_addr_eq(*a, *b),
            (Self::DerivedConstructor(a), Self::DerivedConstructor(b)) => std::ptr::fn_addr_eq(*a, *b),
            _ => false,
        }
    }
}
impl Eq for CompiledFunctionForm {}

/// Contributor-authored executable definition, never decoded from RPC data.
/// Address equality checks immutable registration consistency, not JS identity.
#[derive(Debug)]
pub struct CompiledFunctionSpec {
    pub source: &'static str,
    pub name: &'static str,
    pub length: u32,
    pub form: CompiledFunctionForm,
    pub capture_count: usize,
    pub body: CompiledFunctionBody,
}

/// Sealed affiliation prevents copying a foreign target into a same-index node.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CompiledFunctionKey {
    heap_token: u64,
    node: NodeId,
}

#[derive(Debug)]
pub(super) struct StoredCompiledFunction {
    spec: &'static CompiledFunctionSpec,
    captures: Box<[NodeId]>,
}

impl ValueHeap {
    fn compiled_definition_locked<'a>(
        &self,
        arena: &'a Arena,
        key: CompiledFunctionKey,
    ) -> ValueResult<&'a StoredCompiledFunction> {
        if key.heap_token != self.token {
            return Err(ValueError::ForeignHeap);
        }
        if !matches!(arena.get(key.node)?, Node::Function { target: CallableTarget::Compiled(actual), .. } if *actual == key)
        {
            return Err(ValueError::InvalidNode);
        }
        arena.compiled_functions.get(&key.node).ok_or(ValueError::InvalidNode)
    }

    pub(super) fn admit_compiled_key(&self, key: CompiledFunctionKey) -> ValueResult<()> {
        let arena = self.lock()?;
        self.compiled_definition_locked(&arena, key)?;
        Ok(())
    }

    fn allocate_compiled_locked(
        &self,
        arena: &mut Arena,
        spec: &'static CompiledFunctionSpec,
        captures: Box<[NodeId]>,
    ) -> ValueResult<NodeId> {
        // No external operation occurs during construction/publication.
        arena.compiled_functions.try_reserve(1).map_err(|_| ValueError::Allocation)?;
        let name = arena.allocate(Node::String(spec.name.into()))?;
        let length = arena.allocate(Node::Number(f64::from(spec.length)))?;
        let mut object = ObjectData::new(Some(self.intrinsics.function_proto));
        object.properties.try_reserve(3).map_err(|_| ValueError::Allocation)?;
        for (key, value) in [("name", name), ("length", length)] {
            object.properties.push(PropertyEntry {
                key: key.into(),
                descriptor: StoredDescriptor::Data { value, writable: false, enumerable: false, configurable: true },
            });
        }
        let prototype = if matches!(
            spec.form,
            CompiledFunctionForm::Ordinary
                | CompiledFunctionForm::NativeConstructor(_)
                | CompiledFunctionForm::DerivedConstructor(_)
        ) {
            let mut prototype = ObjectData::new(Some(self.intrinsics.object_proto));
            prototype.properties.try_reserve(1).map_err(|_| ValueError::Allocation)?;
            Some(arena.allocate(Node::Object(prototype))?)
        } else {
            None
        };
        if let Some(prototype) = prototype {
            object.properties.push(PropertyEntry {
                key: "prototype".into(),
                descriptor: StoredDescriptor::Data {
                    value: prototype,
                    writable: spec.form == CompiledFunctionForm::Ordinary,
                    enumerable: false,
                    configurable: false,
                },
            });
        }
        let node = NodeId(arena.nodes.len());
        let key = CompiledFunctionKey { heap_token: self.token, node };
        let allocated = arena.allocate(Node::Function { object, target: CallableTarget::Compiled(key) })?;
        debug_assert_eq!(allocated, node);
        if let Some(prototype) = prototype {
            arena.get_mut(prototype)?.object_mut().ok_or(ValueError::InvalidNode)?.properties.push(PropertyEntry {
                key: "constructor".into(),
                descriptor: StoredDescriptor::Data {
                    value: node,
                    writable: true,
                    enumerable: false,
                    configurable: true,
                },
            });
        }
        arena.compiled_functions.insert(node, StoredCompiledFunction { spec, captures });
        Ok(node)
    }

    pub fn fresh_compiled_function(
        self: &Arc<Self>,
        spec: &'static CompiledFunctionSpec,
        captures: &[ValueHandle],
    ) -> ValueResult<ValueHandle> {
        if captures.len() != spec.capture_count {
            return Err(ValueError::WrongRuntimeReply("Compiled function capture count"));
        }
        let mut nodes = Vec::new();
        nodes.try_reserve(captures.len()).map_err(|_| ValueError::Allocation)?;
        for capture in captures {
            nodes.push(self.admit(capture)?);
        }
        let mut arena = self.lock()?;
        for node in &nodes {
            arena.get(*node)?;
        }
        let node = self.allocate_compiled_locked(&mut arena, spec, nodes.into_boxed_slice())?;
        Ok(self.handle(node))
    }

    /// Trusted Native constructors whose original intrinsic prototype already
    /// exists (Set/Map). Replace the unexposed allocation's prototype edge
    /// while holding its initial construction guard, before publishing it.
    pub fn fresh_native_constructor_with_prototype(
        self: &Arc<Self>,
        spec: &'static CompiledFunctionSpec,
        captures: &[ValueHandle],
        prototype: &ValueHandle,
    ) -> ValueResult<ValueHandle> {
        if !matches!(
            spec.form,
            CompiledFunctionForm::NativeConstructor(_) | CompiledFunctionForm::DerivedConstructor(_)
        ) || captures.len() != spec.capture_count
        {
            return Err(ValueError::WrongRuntimeReply("Native constructor definition/prototype"));
        }
        let prototype = self.object_id(prototype)?;
        let mut nodes = Vec::new();
        nodes.try_reserve(captures.len()).map_err(|_| ValueError::Allocation)?;
        for capture in captures {
            nodes.push(self.admit(capture)?);
        }
        let mut arena = self.lock()?;
        arena.get(prototype)?;
        for node in &nodes {
            arena.get(*node)?;
        }
        let node = self.allocate_compiled_locked(&mut arena, spec, nodes.into_boxed_slice())?;
        let object = arena.get_mut(node)?.object_mut().ok_or(ValueError::InvalidNode)?;
        let entry = object
            .properties
            .iter_mut()
            .find(|entry| entry.key == PropertyKey::from("prototype"))
            .ok_or(ValueError::InvalidNode)?;
        entry.descriptor =
            StoredDescriptor::Data { value: prototype, writable: false, enumerable: false, configurable: false };
        Ok(self.handle(node))
    }

    pub fn module_compiled_function(
        self: &Arc<Self>,
        module: &'static str,
        binding: &'static str,
        spec: &'static CompiledFunctionSpec,
    ) -> ValueResult<ValueHandle> {
        if spec.capture_count != 0 {
            return Err(ValueError::WrongRuntimeReply("Module function cannot retain local captures"));
        }
        let mut arena = self.lock()?;
        if let Some(node) = arena.module_compiled_functions.get(&(module, binding)) {
            let key = CompiledFunctionKey { heap_token: self.token, node: *node };
            let prior = self.compiled_definition_locked(&arena, key)?;
            return if std::ptr::eq(prior.spec, spec) {
                Ok(self.handle(*node))
            } else {
                Err(ValueError::SourceIdentityCollision)
            };
        }
        arena.module_compiled_functions.try_reserve(1).map_err(|_| ValueError::Allocation)?;
        let node = self.allocate_compiled_locked(&mut arena, spec, Box::new([]))?;
        // Only the fully built function becomes a module binding.
        arena.module_compiled_functions.insert((module, binding), node);
        Ok(self.handle(node))
    }

    pub(super) fn invoke_compiled(
        self: &Arc<Self>,
        key: CompiledFunctionKey,
        receiver: &ValueHandle,
        arguments: &[ValueHandle],
        runtime: &dyn ValueRuntime,
    ) -> ValueResult<ValueHandle> {
        let (spec, captures) = {
            let arena = self.lock()?;
            let stored = self.compiled_definition_locked(&arena, key)?;
            let mut captures = Vec::new();
            captures.try_reserve(stored.captures.len()).map_err(|_| ValueError::Allocation)?;
            captures.extend(stored.captures.iter().map(|node| self.handle(*node)));
            (stored.spec, captures)
        };
        // The guard above has dropped before any body or Host reentry.
        let lexical_receiver;
        let receiver = if spec.form == CompiledFunctionForm::Arrow {
            lexical_receiver = self.undefined()?;
            &lexical_receiver
        } else {
            receiver
        };
        self.runtime_value((spec.body)(self, &captures, receiver, arguments, runtime).map(RuntimeReply::Value))
    }

    pub(super) fn compiled_is_constructor(&self, key: CompiledFunctionKey) -> ValueResult<bool> {
        let arena = self.lock()?;
        Ok(matches!(
            self.compiled_definition_locked(&arena, key)?.spec.form,
            CompiledFunctionForm::Ordinary
                | CompiledFunctionForm::NativeConstructor(_)
                | CompiledFunctionForm::DerivedConstructor(_)
        ))
    }

    pub fn construct_compiled_function(
        self: &Arc<Self>,
        function: &ValueHandle,
        arguments: &[ValueHandle],
        runtime: &dyn ValueRuntime,
    ) -> ValueResult<ValueHandle> {
        self.construct_compiled_with_new_target(function, arguments, function, runtime)
    }
    pub(super) fn construct_compiled_with_new_target(
        self: &Arc<Self>,
        function: &ValueHandle,
        arguments: &[ValueHandle],
        new_target: &ValueHandle,
        runtime: &dyn ValueRuntime,
    ) -> ValueResult<ValueHandle> {
        self.admit(function)?;
        self.admit(new_target)?;
        for argument in arguments {
            self.admit(argument)?;
        }
        let ValueSnapshot::Function(CallableTarget::Compiled(key)) = self.snapshot(function)? else {
            return Err(ValueError::TypeError("Value is not a compiled constructor".into()));
        };
        let form = {
            let arena = self.lock()?;
            self.compiled_definition_locked(&arena, key)?.spec.form
        };
        if !matches!(
            form,
            CompiledFunctionForm::Ordinary
                | CompiledFunctionForm::NativeConstructor(_)
                | CompiledFunctionForm::DerivedConstructor(_)
        ) {
            let message = if form == CompiledFunctionForm::Arrow {
                "Arrow function is not a constructor"
            } else {
                "Native builtin function is not a constructor"
            };
            return Err(ValueError::TypeError(message.into()));
        }
        if let CompiledFunctionForm::DerivedConstructor(body) = form {
            let captures = {
                let arena = self.lock()?;
                self.compiled_definition_locked(&arena, key)?
                    .captures
                    .iter()
                    .map(|node| self.handle(*node))
                    .collect::<Vec<_>>()
            };
            let value = self.runtime_value(
                body(self, &captures, function, new_target, arguments, runtime).map(RuntimeReply::Value),
            )?;
            if !matches!(self.kind(&value)?, ValueKind::Object | ValueKind::Array | ValueKind::Function) {
                return Err(ValueError::TypeError("Derived constructor returned a non-object".into()));
            }
            return Ok(value);
        }
        let prototype = self.get(new_target, &"prototype".into(), runtime)?;
        let prototype = if matches!(self.kind(&prototype)?, ValueKind::Object | ValueKind::Array | ValueKind::Function)
        {
            prototype
        } else if matches!(form, CompiledFunctionForm::NativeConstructor(_)) {
            // Native constructors carry their existing intrinsic default
            // prototype in an immutable data descriptor. Do not add another
            // observable Get after a primitive newTarget.prototype result.
            match self.own_descriptor(function, &"prototype".into(), runtime)? {
                Some(PropertyDescriptor::Data { value, .. }) => value,
                _ => self.object_prototype(),
            }
        } else {
            self.object_prototype()
        };
        let receiver = self.object_with_prototype(Some(&prototype))?;
        let result = if let CompiledFunctionForm::NativeConstructor(body) = form {
            let captures = {
                let arena = self.lock()?;
                let stored = self.compiled_definition_locked(&arena, key)?;
                let mut captures = Vec::new();
                captures.try_reserve(stored.captures.len()).map_err(|_| ValueError::Allocation)?;
                captures.extend(stored.captures.iter().map(|node| self.handle(*node)));
                captures
            };
            // The prototype Get and receiver allocation retain the existing
            // order. Invoke outside the heap mutex and admit the returned
            // handle through the same boundary as a normal compiled call.
            self.runtime_value(body(self, &captures, &receiver, arguments, runtime).map(RuntimeReply::Value))?
        } else {
            self.call(function, &receiver, arguments, runtime)?
        };
        if matches!(self.kind(&result)?, ValueKind::Object | ValueKind::Array | ValueKind::Function) {
            Ok(result)
        } else {
            Ok(receiver)
        }
    }
}
