//! Native instanceof consumed by fixed OMP validation.ts:1710.
//! OMP596f2da7101178214aa27a753529d15e6b7ad91d (MIT; see THIRD_PARTY.md).
//! Actual @@hasInstance Get/Call precedes ordinary prototype inspection.

use super::*;

impl ValueHeap {
    pub fn has_instance_key(&self) -> PropertyKey {
        PropertyKey::Symbol(SymbolKey { heap_token: self.token, node: self.intrinsics.has_instance })
    }

    pub fn instance_of(
        self: &Arc<Self>,
        value: &ValueHandle,
        constructor: &ValueHandle,
        runtime: &dyn ValueRuntime,
    ) -> ValueResult<bool> {
        self.admit(value)?;
        self.admit(constructor)?;
        if !matches!(self.kind(constructor)?, ValueKind::Object | ValueKind::Array | ValueKind::Function) {
            return Err(ValueError::TypeError("Right-hand side of instanceof is not an object".into()));
        }
        let method = self.get(constructor, &self.has_instance_key(), runtime)?;
        if !matches!(self.kind(&method)?, ValueKind::Null | ValueKind::Undefined) {
            if self.kind(&method)? != ValueKind::Function {
                return Err(ValueError::TypeError("Symbol.hasInstance is not callable".into()));
            }
            let result = self.call(&method, constructor, std::slice::from_ref(value), runtime)?;
            return self.truthy(&result);
        }
        if self.kind(constructor)? != ValueKind::Function {
            return Err(ValueError::TypeError("Right-hand side of instanceof is not callable".into()));
        }
        self.ordinary_has_instance(constructor, value, runtime)
    }

    pub(super) fn ordinary_has_instance(
        self: &Arc<Self>,
        constructor: &ValueHandle,
        value: &ValueHandle,
        runtime: &dyn ValueRuntime,
    ) -> ValueResult<bool> {
        self.admit(constructor)?;
        self.admit(value)?;
        if self.kind(constructor)? != ValueKind::Function {
            return Ok(false);
        }
        // Opaque callable internal slots (including bound/proxy behavior) are
        // owned by its runtime. Do not infer them from exposed properties.
        if let ValueSnapshot::Opaque { kind: OpaqueKind::Function, capability, .. } = self.snapshot(constructor)? {
            let result = self.runtime_value(runtime.resolve(RuntimeRequest::Opaque {
                capability,
                target: constructor,
                receiver: constructor,
                operation: OpaqueOperation::OrdinaryHasInstance(value),
            }))?;
            return match self.snapshot(&result)? {
                ValueSnapshot::Boolean(result) => Ok(result),
                _ => Err(ValueError::WrongRuntimeReply("OrdinaryHasInstance must return a Boolean")),
            };
        }
        if !matches!(self.kind(value)?, ValueKind::Object | ValueKind::Array | ValueKind::Function) {
            return Ok(false);
        }
        let prototype = self.get(constructor, &"prototype".into(), runtime)?;
        if !matches!(self.kind(&prototype)?, ValueKind::Object | ValueKind::Array | ValueKind::Function) {
            return Err(ValueError::TypeError("Function has non-object prototype in instanceof".into()));
        }
        let mut current = value.clone();
        loop {
            let Some(next) = self.prototype(&current, runtime)? else { return Ok(false) };
            if next.same_identity(&prototype) {
                return Ok(true);
            }
            current = next;
        }
    }

    /// Fresh, persistent, capture-free arrow body: argument[index][literal].
    /// The arena stores only literal metadata, never a closure or heap lease.
    pub fn argument_property_function(
        self: &Arc<Self>,
        index: usize,
        key: &'static str,
        name: &'static str,
        length: u32,
    ) -> ValueResult<ValueHandle> {
        let function = self.allocate(Node::Function {
            object: ObjectData::new(Some(self.intrinsics.function_proto)),
            target: CallableTarget::ArgumentProperty { index, key },
        })?;
        for (key, value) in [("name", self.string(name.into())?), ("length", self.number(f64::from(length))?)] {
            self.define_own(
                &function,
                key.into(),
                PropertyDescriptor::Data { value, writable: false, enumerable: false, configurable: true },
                &NoRuntime,
            )?;
        }
        Ok(function)
    }
}
