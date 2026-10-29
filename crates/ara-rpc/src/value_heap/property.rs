//! Ordinary property operations needed by the pinned relaxed parser,
//! Object.keys/for-in/spread, validator assignment and JSON serialization.
//! Host capabilities are operation-local; no store guard crosses resolution.

use super::*;

pub(super) enum OwnDescriptorPlan {
    Ordinary(Option<StoredDescriptor>),
    Opaque(CapabilityId),
}

pub(super) enum PrototypePlan {
    Ordinary(Option<NodeId>),
    Opaque(CapabilityId),
}

pub(super) enum PropertyPlan {
    Read(NodeId),
    Assigned,
    Deferred(DeferredSelection),
}

pub(super) enum DeferredSelection {
    GetCall(CallableTarget),
    SetCall { callable: CallableTarget, value: NodeId },
    OpaqueGet { capability: CapabilityId, target: NodeId },
    OpaqueSet { capability: CapabilityId, target: NodeId, value: NodeId },
    // Normal Array assignment may need ToNumber callbacks. Guarded structural
    // members are ordinary Object nodes, so this is not a guarded slot writer.
    ArrayLength { value: NodeId, writable: bool },
}

impl ValueHeap {
    pub(super) fn own_descriptor_locked(
        &self,
        arena: &mut Arena,
        node: NodeId,
        key: &PropertyKey,
    ) -> ValueResult<OwnDescriptorPlan> {
        // Keep the existing virtual String/Array own descriptors in this one
        // engine; allocation also occurs under the caller's existing guard.
        let virtual_data = match arena.get(node)? {
            Node::Opaque { capability, .. } => return Ok(OwnDescriptorPlan::Opaque(*capability)),
            Node::Array { length, length_writable, .. } if is_length(key) => {
                Some((Node::Number(f64::from(*length)), *length_writable, false))
            }
            Node::String(text) => {
                if is_length(key) {
                    Some((Node::Number(text.len() as f64), false, false))
                } else if let Some(index) = array_index(key).filter(|index| (*index as usize) < text.len()) {
                    Some((Node::String(WireString::from_units(vec![text.units()[index as usize]])), false, true))
                } else {
                    return Ok(OwnDescriptorPlan::Ordinary(None));
                }
            }
            Node::Null | Node::Undefined => {
                return Err(ValueError::TypeError("Cannot inspect null or undefined properties".into()));
            }
            node => {
                return Ok(OwnDescriptorPlan::Ordinary(
                    node.object()
                        .and_then(|object| object.properties.iter().find(|entry| &entry.key == key))
                        .map(|entry| entry.descriptor.clone()),
                ));
            }
        };
        let (value, writable, enumerable) = virtual_data.ok_or(ValueError::InvalidNode)?;
        Ok(OwnDescriptorPlan::Ordinary(Some(StoredDescriptor::Data {
            value: arena.allocate(value)?,
            writable,
            enumerable,
            configurable: false,
        })))
    }

    fn prototype_locked(&self, arena: &Arena, node: NodeId) -> ValueResult<PrototypePlan> {
        Ok(PrototypePlan::Ordinary(match arena.get(node)? {
            Node::Opaque { capability, .. } => return Ok(PrototypePlan::Opaque(*capability)),
            Node::Undefined | Node::Null => {
                return Err(ValueError::TypeError("Cannot get null or undefined prototype".into()));
            }
            Node::String(_) => Some(self.intrinsics.string_proto),
            Node::Number(_) => Some(self.intrinsics.number_proto),
            Node::Boolean(_) => Some(self.intrinsics.boolean_proto),
            Node::BigInt(_) => Some(self.intrinsics.bigint_proto),
            Node::Symbol { .. } => {
                return Err(ValueError::UnsupportedOperation("Symbol boxing prototype belongs to Host"));
            }
            node => node.object().and_then(|object| object.prototype),
        }))
    }

    pub(super) fn get_property_locked(
        &self,
        arena: &mut Arena,
        receiver: NodeId,
        key: &PropertyKey,
    ) -> ValueResult<PropertyPlan> {
        let mut current = Some(receiver);
        let mut visited = HashSet::new();
        while let Some(node) = current {
            if !visited.insert(node) {
                return Err(ValueError::TypeError("Cyclic prototype lookup".into()));
            }
            match self.own_descriptor_locked(arena, node, key)? {
                OwnDescriptorPlan::Opaque(capability) => {
                    return Ok(PropertyPlan::Deferred(DeferredSelection::OpaqueGet { capability, target: node }));
                }
                OwnDescriptorPlan::Ordinary(Some(StoredDescriptor::Data { value, .. })) => {
                    arena.get(value)?;
                    return Ok(PropertyPlan::Read(value));
                }
                OwnDescriptorPlan::Ordinary(Some(StoredDescriptor::Accessor { get: Some(callable), .. })) => {
                    return Ok(PropertyPlan::Deferred(DeferredSelection::GetCall(callable)));
                }
                OwnDescriptorPlan::Ordinary(Some(StoredDescriptor::Accessor { get: None, .. })) => {
                    return Ok(PropertyPlan::Read(self.intrinsics.undefined));
                }
                OwnDescriptorPlan::Ordinary(None) => {
                    let PrototypePlan::Ordinary(prototype) = self.prototype_locked(arena, node)? else {
                        return Err(ValueError::InvalidNode);
                    };
                    current = prototype;
                }
            }
        }
        Ok(PropertyPlan::Read(self.intrinsics.undefined))
    }

    pub(super) fn assign_property_locked(
        &self,
        arena: &mut Arena,
        receiver: NodeId,
        key: &PropertyKey,
        value: NodeId,
    ) -> ValueResult<PropertyPlan> {
        if !matches!(arena.get(receiver)?.kind(), ValueKind::Object | ValueKind::Array | ValueKind::Function) {
            return Err(ValueError::TypeError("Value is not an object".into()));
        }
        arena.get(value)?;
        let mut current = Some(receiver);
        let mut visited = HashSet::new();
        while let Some(node) = current {
            if !visited.insert(node) {
                return Err(ValueError::TypeError("Cyclic prototype assignment".into()));
            }
            match self.own_descriptor_locked(arena, node, key)? {
                OwnDescriptorPlan::Opaque(capability) => {
                    return Ok(PropertyPlan::Deferred(DeferredSelection::OpaqueSet {
                        capability,
                        target: node,
                        value,
                    }));
                }
                OwnDescriptorPlan::Ordinary(Some(StoredDescriptor::Accessor { set: Some(callable), .. })) => {
                    return Ok(PropertyPlan::Deferred(DeferredSelection::SetCall { callable, value }));
                }
                OwnDescriptorPlan::Ordinary(Some(StoredDescriptor::Accessor { set: None, .. })) => {
                    return Err(ValueError::TypeError("Property has no setter".into()));
                }
                OwnDescriptorPlan::Ordinary(Some(StoredDescriptor::Data { writable: false, .. })) => {
                    return Err(ValueError::TypeError("Property is read-only".into()));
                }
                OwnDescriptorPlan::Ordinary(Some(StoredDescriptor::Data { .. })) => break,
                OwnDescriptorPlan::Ordinary(None) => {
                    let PrototypePlan::Ordinary(prototype) = self.prototype_locked(arena, node)? else {
                        return Err(ValueError::InvalidNode);
                    };
                    current = prototype;
                }
            }
        }
        let (enumerable, configurable) = match self.own_descriptor_locked(arena, receiver, key)? {
            OwnDescriptorPlan::Ordinary(Some(StoredDescriptor::Accessor { .. })) => {
                return Err(ValueError::TypeError("Receiver property is an accessor".into()));
            }
            OwnDescriptorPlan::Ordinary(Some(StoredDescriptor::Data { writable: false, .. })) => {
                return Err(ValueError::TypeError("Receiver property is read-only".into()));
            }
            OwnDescriptorPlan::Ordinary(Some(StoredDescriptor::Data { enumerable, configurable, .. })) => {
                (enumerable, configurable)
            }
            OwnDescriptorPlan::Ordinary(None) => (true, true),
            OwnDescriptorPlan::Opaque(_) => return Err(ValueError::InvalidNode),
        };
        if matches!(arena.get(receiver)?, Node::Array { .. }) && is_length(key) {
            return Ok(PropertyPlan::Deferred(DeferredSelection::ArrayLength { value, writable: true }));
        }
        self.define_ordinary_locked(
            arena,
            receiver,
            key,
            StoredDescriptor::Data { value, writable: true, enumerable, configurable },
        )?;
        Ok(PropertyPlan::Assigned)
    }

    fn define_ordinary_locked(
        &self,
        arena: &mut Arena,
        node: NodeId,
        key: &PropertyKey,
        stored: StoredDescriptor,
    ) -> ValueResult<()> {
        let target = arena.get(node)?;
        let object = target.object().ok_or(ValueError::TypeError("Value is not an ordinary object".into()))?;
        let previous = object.properties.iter().position(|entry| &entry.key == key);
        if let Some(position) = previous {
            if !compatible_descriptor(arena, &object.properties[position].descriptor, &stored)? {
                return Err(ValueError::TypeError("Cannot redefine non-configurable property".into()));
            }
        } else if !object.extensible {
            return Err(ValueError::TypeError("Object is not extensible".into()));
        }
        if let Node::Array { length, length_writable, .. } = target
            && array_index(key).is_some_and(|index| index >= *length)
            && !*length_writable
        {
            return Err(ValueError::TypeError("Array length is read-only".into()));
        }
        let target = arena.get_mut(node)?;
        let object = target.object_mut().ok_or(ValueError::InvalidNode)?;
        match previous {
            Some(position) => object.properties[position].descriptor = stored,
            None => {
                object.properties.try_reserve(1).map_err(|_| ValueError::Allocation)?;
                object.properties.push(PropertyEntry { key: key.clone(), descriptor: stored });
            }
        }
        if let Node::Array { length, .. } = target
            && let Some(index) = array_index(key)
        {
            *length = (*length).max(index + 1);
        }
        Ok(())
    }

    fn store_descriptor(&self, descriptor: &PropertyDescriptor) -> ValueResult<StoredDescriptor> {
        Ok(match descriptor {
            PropertyDescriptor::Data { value, writable, enumerable, configurable } => StoredDescriptor::Data {
                value: self.admit(value)?,
                writable: *writable,
                enumerable: *enumerable,
                configurable: *configurable,
            },
            PropertyDescriptor::Accessor { get, set, enumerable, configurable } => {
                for target in [get, set].into_iter().flatten() {
                    if let CallableTarget::Compiled(key) = target {
                        self.admit_compiled_key(*key)?;
                    }
                }
                StoredDescriptor::Accessor {
                    get: *get,
                    set: *set,
                    enumerable: *enumerable,
                    configurable: *configurable,
                }
            }
        })
    }
    fn public_descriptor(self: &Arc<Self>, descriptor: StoredDescriptor) -> PropertyDescriptor {
        match descriptor {
            StoredDescriptor::Data { value, writable, enumerable, configurable } => {
                PropertyDescriptor::Data { value: self.handle(value), writable, enumerable, configurable }
            }
            StoredDescriptor::Accessor { get, set, enumerable, configurable } => {
                PropertyDescriptor::Accessor { get, set, enumerable, configurable }
            }
        }
    }
    pub fn own_descriptor(
        self: &Arc<Self>,
        value: &ValueHandle,
        key: &PropertyKey,
        runtime: &dyn ValueRuntime,
    ) -> ValueResult<Option<PropertyDescriptor>> {
        let node = self.admit(value)?;
        let plan = {
            let mut arena = self.lock()?;
            self.admit_key_locked(&arena, key)?;
            self.own_descriptor_locked(&mut arena, node, key)?
        };
        match plan {
            OwnDescriptorPlan::Ordinary(descriptor) => {
                Ok(descriptor.map(|descriptor| self.public_descriptor(descriptor)))
            }
            OwnDescriptorPlan::Opaque(capability) => {
                match self.runtime_reply(runtime.resolve(RuntimeRequest::Opaque {
                    capability,
                    target: value,
                    receiver: value,
                    operation: OpaqueOperation::OwnDescriptor(key),
                }))? {
                    RuntimeReply::Descriptor(descriptor) => {
                        if let Some(descriptor) = &descriptor {
                            self.store_descriptor(descriptor)?;
                        }
                        Ok(descriptor)
                    }
                    _ => Err(ValueError::WrongRuntimeReply("Descriptor")),
                }
            }
        }
    }
    pub fn own_get(
        self: &Arc<Self>,
        value: &ValueHandle,
        key: &PropertyKey,
        runtime: &dyn ValueRuntime,
    ) -> ValueResult<PropertyPresence> {
        match self.own_descriptor(value, key, runtime)? {
            None => Ok(PropertyPresence::Missing),
            Some(PropertyDescriptor::Data { value, .. }) => Ok(PropertyPresence::Present(value)),
            Some(PropertyDescriptor::Accessor { get, .. }) => {
                let result = match get {
                    Some(target) => self.invoke(target, value, &[], runtime)?,
                    None => self.undefined()?,
                };
                Ok(PropertyPresence::Present(result))
            }
        }
    }
    pub fn has_own(
        self: &Arc<Self>,
        value: &ValueHandle,
        key: &PropertyKey,
        runtime: &dyn ValueRuntime,
    ) -> ValueResult<bool> {
        Ok(self.own_descriptor(value, key, runtime)?.is_some())
    }
    pub fn prototype(
        self: &Arc<Self>,
        value: &ValueHandle,
        runtime: &dyn ValueRuntime,
    ) -> ValueResult<Option<ValueHandle>> {
        let node = self.admit(value)?;
        let plan = {
            let arena = self.lock()?;
            self.prototype_locked(&arena, node)?
        };
        match plan {
            PrototypePlan::Ordinary(prototype) => Ok(prototype.map(|node| self.handle(node))),
            PrototypePlan::Opaque(capability) => {
                match self.runtime_reply(runtime.resolve(RuntimeRequest::Opaque {
                    capability,
                    target: value,
                    receiver: value,
                    operation: OpaqueOperation::Prototype,
                }))? {
                    RuntimeReply::Prototype(Some(prototype)) => {
                        self.object_id(&prototype)?;
                        Ok(Some(prototype))
                    }
                    RuntimeReply::Prototype(None) => Ok(None),
                    _ => Err(ValueError::WrongRuntimeReply("Prototype")),
                }
            }
        }
    }
    pub fn get(
        self: &Arc<Self>,
        receiver: &ValueHandle,
        key: &PropertyKey,
        runtime: &dyn ValueRuntime,
    ) -> ValueResult<ValueHandle> {
        let node = self.admit(receiver)?;
        let plan = {
            let mut arena = self.lock()?;
            self.admit_key_locked(&arena, key)?;
            self.get_property_locked(&mut arena, node, key)?
        };
        match self.execute_property_plan(receiver, key, plan, runtime)? {
            GuardedPropertyValue::Read(value) => Ok(value),
            _ => Err(ValueError::WrongRuntimeReply("property read")),
        }
    }
    pub fn has(
        self: &Arc<Self>,
        receiver: &ValueHandle,
        key: &PropertyKey,
        runtime: &dyn ValueRuntime,
    ) -> ValueResult<bool> {
        self.object_id(receiver)?;
        self.admit_key(key)?;
        let mut current = Some(receiver.clone());
        let mut visited = HashSet::new();
        while let Some(value) = current {
            if !visited.insert(value.node) {
                return Err(ValueError::TypeError("Cyclic prototype lookup".into()));
            }
            if let ValueSnapshot::Opaque { capability, .. } = self.snapshot(&value)? {
                return self.runtime_bool(runtime.resolve(RuntimeRequest::Opaque {
                    capability,
                    target: &value,
                    receiver,
                    operation: OpaqueOperation::Has(key),
                }));
            }
            if self.own_descriptor(&value, key, runtime)?.is_some() {
                return Ok(true);
            }
            current = self.prototype(&value, runtime)?;
        }
        Ok(false)
    }
    pub fn own_keys(
        self: &Arc<Self>,
        value: &ValueHandle,
        runtime: &dyn ValueRuntime,
    ) -> ValueResult<Vec<PropertyKey>> {
        let node = self.admit(value)?;
        let (mut keys, opaque) = {
            let arena = self.lock()?;
            match arena.get(node)? {
                Node::Opaque { capability, .. } => (Vec::new(), Some(*capability)),
                Node::Null | Node::Undefined => {
                    return Err(ValueError::TypeError("Cannot enumerate null or undefined".into()));
                }
                Node::String(text) => {
                    let mut keys = (0..text.len())
                        .map(|index| PropertyKey::String(WireString::from(index.to_string())))
                        .collect::<Vec<_>>();
                    keys.push("length".into());
                    (keys, None)
                }
                node => {
                    let mut keys = node
                        .object()
                        .map(|object| object.properties.iter().map(|entry| entry.key.clone()).collect::<Vec<_>>())
                        .unwrap_or_default();
                    if matches!(node, Node::Array { .. }) {
                        keys.insert(0, "length".into());
                    }
                    (keys, None)
                }
            }
        };
        if let Some(capability) = opaque {
            keys = match self.runtime_reply(runtime.resolve(RuntimeRequest::Opaque {
                capability,
                target: value,
                receiver: value,
                operation: OpaqueOperation::OwnKeys,
            }))? {
                RuntimeReply::Keys(keys) => keys,
                _ => return Err(ValueError::WrongRuntimeReply("Keys")),
            };
            let mut unique = HashSet::new();
            for key in &keys {
                self.admit_key(key)?;
                if !unique.insert(key.clone()) {
                    return Err(ValueError::TypeError("Duplicate runtime own key".into()));
                }
            }
            return Ok(keys); // Exotic key ordering is an actual Host result.
        }
        keys.sort_by_key(|key| match key {
            PropertyKey::String(_) => array_index(key).map_or((1, 0), |index| (0, index)),
            PropertyKey::Symbol(_) => (2, 0),
        });
        Ok(keys)
    }
    pub fn own_enumerable_keys(
        self: &Arc<Self>,
        value: &ValueHandle,
        runtime: &dyn ValueRuntime,
    ) -> ValueResult<Vec<PropertyKey>> {
        let mut result = Vec::new();
        for key in self.own_keys(value, runtime)? {
            if self.own_descriptor(value, &key, runtime)?.is_some_and(|descriptor| descriptor.enumerable()) {
                result.push(key);
            }
        }
        Ok(result)
    }
    pub fn own_enumerable_string_keys(
        self: &Arc<Self>,
        value: &ValueHandle,
        runtime: &dyn ValueRuntime,
    ) -> ValueResult<Vec<WireString>> {
        let mut result = Vec::new();
        for key in self.own_keys(value, runtime)? {
            let PropertyKey::String(text) = &key else {
                continue;
            };
            if self.own_descriptor(value, &key, runtime)?.is_some_and(|descriptor| descriptor.enumerable()) {
                result.push(text.clone());
            }
        }
        Ok(result)
    }
    pub fn for_in_string_keys(
        self: &Arc<Self>,
        value: &ValueHandle,
        runtime: &dyn ValueRuntime,
    ) -> ValueResult<Vec<WireString>> {
        self.admit(value)?;
        if matches!(self.kind(value)?, ValueKind::Null | ValueKind::Undefined) {
            return Ok(Vec::new());
        }
        let mut result = Vec::new();
        let mut shadowed = HashSet::new();
        let mut visited = HashSet::new();
        let mut current = Some(value.clone());
        while let Some(value) = current {
            if !visited.insert(value.node) {
                return Err(ValueError::TypeError("Cyclic prototype enumeration".into()));
            }
            for key in self.own_keys(&value, runtime)? {
                let PropertyKey::String(text) = &key else {
                    continue;
                };
                // A deleted own key is not visited; a present non-enumerable key shadows inherited keys.
                let Some(descriptor) = self.own_descriptor(&value, &key, runtime)? else {
                    continue;
                };
                if shadowed.insert(text.clone()) && descriptor.enumerable() {
                    result.push(text.clone());
                }
            }
            current = self.prototype(&value, runtime)?;
        }
        Ok(result)
    }
    pub fn define_own_data(
        self: &Arc<Self>,
        receiver: &ValueHandle,
        key: PropertyKey,
        value: ValueHandle,
        runtime: &dyn ValueRuntime,
    ) -> ValueResult<()> {
        self.define_own(receiver, key, PropertyDescriptor::data(value), runtime)
    }
    pub fn define_own(
        self: &Arc<Self>,
        receiver: &ValueHandle,
        key: PropertyKey,
        descriptor: PropertyDescriptor,
        runtime: &dyn ValueRuntime,
    ) -> ValueResult<()> {
        let node = self.object_id(receiver)?;
        self.admit_key(&key)?;
        let stored = self.store_descriptor(&descriptor)?;
        if let ValueSnapshot::Opaque { capability, .. } = self.snapshot(receiver)? {
            return self.runtime_unit(runtime.resolve(RuntimeRequest::Opaque {
                capability,
                target: receiver,
                receiver,
                operation: OpaqueOperation::Define { key: &key, descriptor: &descriptor },
            }));
        }
        if self.kind(receiver)? == ValueKind::Array && is_length(&key) {
            let PropertyDescriptor::Data { value, writable, enumerable: false, configurable: false } = descriptor
            else {
                return Err(ValueError::TypeError("Cannot redefine array length descriptor".into()));
            };
            let number = self.to_number(&value, runtime)?;
            let length = valid_array_length(number)?;
            return self.set_array_length_descriptor(receiver, length, Some(writable));
        }
        let mut arena = self.lock()?;
        self.define_ordinary_locked(&mut arena, node, &key, stored)
    }
    pub fn assign(
        self: &Arc<Self>,
        receiver: &ValueHandle,
        key: PropertyKey,
        value: ValueHandle,
        runtime: &dyn ValueRuntime,
    ) -> ValueResult<()> {
        self.admit(receiver)?;
        let plan = {
            let mut arena = self.lock()?;
            let node = self.object_id_locked(&arena, receiver)?;
            self.admit_key_locked(&arena, &key)?;
            let value_node = self.admit(&value)?;
            self.assign_property_locked(&mut arena, node, &key, value_node)?
        };
        match self.execute_property_plan(receiver, &key, plan, runtime)? {
            GuardedPropertyValue::Assigned => Ok(()),
            _ => Err(ValueError::WrongRuntimeReply("property assignment")),
        }
    }
    pub fn delete_own(
        self: &Arc<Self>,
        receiver: &ValueHandle,
        key: &PropertyKey,
        runtime: &dyn ValueRuntime,
    ) -> ValueResult<bool> {
        let node = self.object_id(receiver)?;
        self.admit_key(key)?;
        if let ValueSnapshot::Opaque { capability, .. } = self.snapshot(receiver)? {
            return self.runtime_bool(runtime.resolve(RuntimeRequest::Opaque {
                capability,
                target: receiver,
                receiver,
                operation: OpaqueOperation::Delete(key),
            }));
        }
        if self.kind(receiver)? == ValueKind::Array && is_length(key) {
            return Ok(false);
        }
        let mut arena = self.lock()?;
        let object = arena.get_mut(node)?.object_mut().ok_or(ValueError::InvalidNode)?;
        let Some(position) = object.properties.iter().position(|entry| &entry.key == key) else {
            return Ok(true);
        };
        if !object.properties[position].descriptor.configurable() {
            return Ok(false);
        }
        object.properties.remove(position);
        Ok(true)
    }
    /// Object.isExtensible's primitive false result and actual object query.
    /// Opaque queries use the effective operation runtime, outside the guard.
    pub fn is_extensible(self: &Arc<Self>, value: &ValueHandle, runtime: &dyn ValueRuntime) -> ValueResult<bool> {
        let node = self.admit(value)?;
        if is_primitive_kind(self.kind(value)?) {
            return Ok(false);
        }
        if let ValueSnapshot::Opaque { capability, .. } = self.snapshot(value)? {
            return self.runtime_bool(runtime.resolve(RuntimeRequest::Opaque {
                capability,
                target: value,
                receiver: value,
                operation: OpaqueOperation::IsExtensible,
            }));
        }
        let arena = self.lock()?;
        Ok(arena.get(node)?.object().ok_or(ValueError::InvalidNode)?.extensible)
    }
    pub fn set_extensible(
        self: &Arc<Self>,
        receiver: &ValueHandle,
        extensible: bool,
        runtime: &dyn ValueRuntime,
    ) -> ValueResult<()> {
        let node = self.object_id(receiver)?;
        if let ValueSnapshot::Opaque { capability, .. } = self.snapshot(receiver)? {
            return self.runtime_unit(runtime.resolve(RuntimeRequest::Opaque {
                capability,
                target: receiver,
                receiver,
                operation: OpaqueOperation::SetExtensible(extensible),
            }));
        }
        let mut arena = self.lock()?;
        let object = arena.get_mut(node)?.object_mut().ok_or(ValueError::InvalidNode)?;
        if extensible && !object.extensible {
            return Err(ValueError::TypeError("Cannot make an object extensible again".into()));
        }
        object.extensible = extensible;
        Ok(())
    }
    pub fn set_prototype(
        self: &Arc<Self>,
        receiver: &ValueHandle,
        prototype: Option<&ValueHandle>,
        runtime: &dyn ValueRuntime,
    ) -> ValueResult<()> {
        let node = self.object_id(receiver)?;
        let new_prototype = prototype.map(|value| self.object_id(value)).transpose()?;
        if let ValueSnapshot::Opaque { capability, .. } = self.snapshot(receiver)? {
            return self.runtime_unit(runtime.resolve(RuntimeRequest::Opaque {
                capability,
                target: receiver,
                receiver,
                operation: OpaqueOperation::SetPrototype(prototype),
            }));
        }
        // Known ordinary prototype chains are checked atomically; opaque admission is a Host boundary.
        let mut arena = self.lock()?;
        let object = arena.get(node)?.object().ok_or(ValueError::InvalidNode)?;
        if object.prototype == new_prototype {
            return Ok(());
        }
        if !object.extensible {
            return Err(ValueError::TypeError("Object is not extensible".into()));
        }
        let mut current = new_prototype;
        let mut visited = HashSet::new();
        while let Some(candidate) = current {
            if candidate == node || !visited.insert(candidate) {
                return Err(ValueError::TypeError("Cyclic __proto__ value".into()));
            }
            let candidate = arena.get(candidate)?;
            if matches!(candidate, Node::Opaque { .. }) {
                return Err(ValueError::UnsupportedOperation(
                    "Ordinary prototype cycle admission across opaque value requires Host",
                ));
            }
            current = candidate.object().ok_or(ValueError::InvalidNode)?.prototype;
        }
        arena.get_mut(node)?.object_mut().ok_or(ValueError::InvalidNode)?.prototype = new_prototype;
        Ok(())
    }
    pub fn array_length(self: &Arc<Self>, receiver: &ValueHandle, runtime: &dyn ValueRuntime) -> ValueResult<u32> {
        let node = self.admit(receiver)?;
        let (length, opaque) = {
            let arena = self.lock()?;
            match arena.get(node)? {
                Node::Array { length, .. } => (Some(*length), None),
                Node::Opaque { kind: OpaqueKind::Array, capability, .. } => (None, Some(*capability)),
                _ => return Err(ValueError::TypeError("Value is not an array".into())),
            }
        };
        if let Some(length) = length {
            return Ok(length);
        }
        let capability = opaque.ok_or(ValueError::InvalidNode)?;
        match self.runtime_reply(runtime.resolve(RuntimeRequest::Opaque {
            capability,
            target: receiver,
            receiver,
            operation: OpaqueOperation::ArrayLength,
        }))? {
            RuntimeReply::Length(length) => Ok(length),
            _ => Err(ValueError::WrongRuntimeReply("Length")),
        }
    }
    /// Own slot distinguishes hole from Present(Undefined); use get/has for inherited slots.
    pub fn array_slot(
        self: &Arc<Self>,
        receiver: &ValueHandle,
        index: u32,
        runtime: &dyn ValueRuntime,
    ) -> ValueResult<PropertyPresence> {
        self.array_length(receiver, runtime)?;
        self.own_get(receiver, &index_key(index), runtime)
    }
    pub fn array_set_length(
        self: &Arc<Self>,
        receiver: &ValueHandle,
        length: u32,
        runtime: &dyn ValueRuntime,
    ) -> ValueResult<()> {
        if let ValueSnapshot::Opaque { capability, .. } = self.snapshot(receiver)? {
            let value = self.number(f64::from(length))?;
            let key = PropertyKey::from("length");
            return self.runtime_unit(runtime.resolve(RuntimeRequest::Opaque {
                capability,
                target: receiver,
                receiver,
                operation: OpaqueOperation::Set { key: &key, value: &value },
            }));
        }
        self.set_array_length_descriptor(receiver, length, None)
    }
    pub(super) fn set_array_length_descriptor(
        &self,
        receiver: &ValueHandle,
        new_length: u32,
        writable: Option<bool>,
    ) -> ValueResult<()> {
        let node = self.admit(receiver)?;
        let mut arena = self.lock()?;
        let Node::Array { object, length, length_writable } = arena.get_mut(node)? else {
            return Err(ValueError::TypeError("Value is not an array".into()));
        };
        if !*length_writable && (new_length != *length || writable == Some(true)) {
            return Err(ValueError::TypeError("Array length is read-only".into()));
        }
        if new_length < *length {
            // ArraySetLength deletes descending indices; previous deletes survive a blocked lower key.
            let mut indices = object
                .properties
                .iter()
                .filter_map(|entry| array_index(&entry.key).filter(|index| *index >= new_length))
                .collect::<Vec<_>>();
            indices.sort_unstable_by(|a, b| b.cmp(a));
            *length = new_length;
            for index in indices {
                let Some(position) = object.properties.iter().position(|entry| array_index(&entry.key) == Some(index))
                else {
                    continue;
                };
                if !object.properties[position].descriptor.configurable() {
                    *length = index + 1;
                    if writable == Some(false) {
                        *length_writable = false;
                    }
                    return Err(ValueError::TypeError("Cannot delete non-configurable array element".into()));
                }
                object.properties.remove(position);
            }
        } else {
            *length = new_length;
        }
        if let Some(writable) = writable {
            *length_writable = writable;
        }
        Ok(())
    }
    pub fn array_push(
        self: &Arc<Self>,
        receiver: &ValueHandle,
        values: &[ValueHandle],
        runtime: &dyn ValueRuntime,
    ) -> ValueResult<u32> {
        let mut length = u64::from(self.array_length(receiver, runtime)?);
        let count = u64::try_from(values.len())
            .map_err(|_| ValueError::TypeError("Push length exceeds safe integer range".into()))?;
        if length.checked_add(count).is_none_or(|length| length > 9_007_199_254_740_991) {
            return Err(ValueError::TypeError("Push length exceeds safe integer range".into()));
        }
        for value in values {
            // All writes above 2^32−2 are named properties before final invalid-length failure.
            self.assign(receiver, PropertyKey::from(WireString::from(length.to_string())), value.clone(), runtime)?;
            length += 1;
        }
        self.assign(receiver, "length".into(), self.number(length as f64)?, runtime)?;
        u32::try_from(length).map_err(|_| ValueError::RangeError("Invalid array length".into()))
    }
    pub fn call(
        self: &Arc<Self>,
        function: &ValueHandle,
        receiver: &ValueHandle,
        arguments: &[ValueHandle],
        runtime: &dyn ValueRuntime,
    ) -> ValueResult<ValueHandle> {
        self.admit(function)?;
        self.admit(receiver)?;
        for argument in arguments {
            self.admit(argument)?;
        }
        match self.snapshot(function)? {
            ValueSnapshot::Function(target) => self.invoke(target, receiver, arguments, runtime),
            ValueSnapshot::Opaque { kind: OpaqueKind::Function, capability, .. } => {
                self.runtime_value(runtime.resolve_call(capability, receiver, arguments, runtime))
            }
            _ => Err(ValueError::TypeError("Value is not callable".into())),
        }
    }
    pub(super) fn invoke(
        self: &Arc<Self>,
        target: CallableTarget,
        receiver: &ValueHandle,
        arguments: &[ValueHandle],
        runtime: &dyn ValueRuntime,
    ) -> ValueResult<ValueHandle> {
        self.admit(receiver)?;
        for argument in arguments {
            self.admit(argument)?;
        }
        let intrinsic = match target {
            CallableTarget::Host(capability) | CallableTarget::Intrinsic(Intrinsic::NativeMember(capability)) => {
                return self.runtime_value(runtime.resolve_call(capability, receiver, arguments, runtime));
            }
            CallableTarget::LocalCallback(token) => {
                return self.runtime_value(runtime.resolve_local_call(token, receiver, arguments, runtime));
            }
            CallableTarget::Compiled(key) => {
                return self.invoke_compiled(key, receiver, arguments, runtime);
            }
            CallableTarget::ArgumentProperty { index, key } => {
                let argument = arguments.get(index).cloned().unwrap_or(self.undefined()?);
                return self.get(&argument, &key.into(), runtime);
            }
            CallableTarget::Intrinsic(intrinsic) => intrinsic,
        };
        match intrinsic {
            Intrinsic::PrototypeGet => match self.prototype(receiver, runtime)? {
                Some(prototype) => Ok(prototype),
                None => self.null(),
            },
            Intrinsic::PrototypeSet => {
                if let Some(value) = arguments.first() {
                    match self.kind(value)? {
                        ValueKind::Null => self.set_prototype(receiver, None, runtime)?,
                        ValueKind::Object | ValueKind::Array | ValueKind::Function => {
                            self.set_prototype(receiver, Some(value), runtime)?
                        }
                        _ => {} // Native inherited setter ignores non-object/non-null values.
                    }
                }
                self.undefined()
            }
            Intrinsic::FunctionHasInstance => {
                let value = arguments.first().cloned().unwrap_or(self.undefined()?);
                self.boolean(self.ordinary_has_instance(receiver, &value, runtime)?)
            }
            Intrinsic::FunctionCall => {
                let this_argument = arguments.first().cloned().unwrap_or(self.undefined()?);
                self.call(receiver, &this_argument, arguments.get(1..).unwrap_or(&[]), runtime)
            }
            Intrinsic::ObjectValueOf | Intrinsic::SpeciesGet => self.adopt(receiver),
            Intrinsic::ObjectToString => {
                let tag = match self.kind(receiver)? {
                    ValueKind::Undefined => "Undefined",
                    ValueKind::Null => "Null",
                    ValueKind::Array => "Array",
                    ValueKind::Function => "Function",
                    ValueKind::String => "String",
                    ValueKind::Number => "Number",
                    ValueKind::Boolean => "Boolean",
                    ValueKind::BigInt => "BigInt",
                    ValueKind::Symbol => "Symbol",
                    ValueKind::Object => "Object",
                };
                self.string(WireString::from(format!("[object {tag}]")))
            }
            Intrinsic::PrimitiveToString => self.string(self.scalar_string(receiver, runtime)?),
            Intrinsic::PrimitiveValueOf => {
                if is_primitive_kind(self.kind(receiver)?) {
                    self.adopt(receiver)
                } else if let ValueSnapshot::Opaque { capability, .. } = self.snapshot(receiver)? {
                    self.runtime_value(runtime.resolve(RuntimeRequest::Opaque {
                        capability,
                        target: receiver,
                        receiver,
                        operation: OpaqueOperation::Unbox,
                    }))
                } else {
                    Err(ValueError::TypeError("Incompatible primitive receiver".into()))
                }
            }
            Intrinsic::ArrayToString => {
                let join = self.get(receiver, &"join".into(), runtime)?;
                if self.kind(&join)? == ValueKind::Function {
                    self.call(&join, receiver, &[], runtime)
                } else {
                    self.invoke(CallableTarget::Intrinsic(Intrinsic::ObjectToString), receiver, &[], runtime)
                }
            }
            Intrinsic::ArrayJoin => {
                let length = self.length_of_array_like(receiver, runtime)?;
                let separator = match arguments.first() {
                    Some(value) if self.kind(value)? == ValueKind::Symbol => {
                        return Err(ValueError::TypeError("Cannot convert a Symbol value to a string".into()));
                    }
                    Some(value) if self.kind(value)? != ValueKind::Undefined => self.to_string(value, runtime)?,
                    _ => WireString::from(","),
                };
                self.string(self.array_join(receiver, &separator, runtime, &mut HashSet::new(), Some(length))?)
            }
            // Generic Array.prototype.at: length is captured before index
            // conversion; the reached element is an ordinary live Get.
            Intrinsic::ArrayAt => {
                let length = self.length_of_array_like(receiver, runtime)?;
                let number =
                    arguments.first().map(|value| self.to_number(value, runtime)).transpose()?.unwrap_or(f64::NAN);
                let relative = if number.is_nan() { 0.0 } else { number.trunc() };
                let index = if relative < 0.0 { length as f64 + relative } else { relative };
                if index < 0.0 || index >= length as f64 {
                    self.undefined()
                } else {
                    self.get(receiver, &WireString::from((index as u64).to_string()).into(), runtime)
                }
            }
            Intrinsic::ArrayPush => self.number(f64::from(self.array_push(receiver, arguments, runtime)?)),
            Intrinsic::ArraySlice => {
                let start = arguments.first().map(|value| self.to_number(value, runtime)).transpose()?.unwrap_or(0.0);
                let end = match arguments.get(1) {
                    Some(value) if self.kind(value)? != ValueKind::Undefined => Some(self.to_number(value, runtime)?),
                    _ => None,
                };
                self.array_slice(receiver, start, end, runtime)
            }
            Intrinsic::ArraySplice => {
                let start = arguments.first().map(|value| self.to_number(value, runtime)).transpose()?.unwrap_or(0.0);
                let delete = match arguments.get(1) {
                    Some(value) => Some(self.to_number(value, runtime)?),
                    None if arguments.is_empty() => Some(0.0),
                    None => None,
                };
                self.array_splice(receiver, start, delete, arguments.get(2..).unwrap_or_default(), runtime)
            }
            Intrinsic::ArrayFind | Intrinsic::ArraySome | Intrinsic::ArrayEvery | Intrinsic::ArrayFilter => {
                self.array_predicate(intrinsic, receiver, arguments, runtime)
            }
            Intrinsic::ArrayIncludes => self.array_includes(receiver, arguments, runtime),
            Intrinsic::ArrayValues => self.array_values(receiver),
            Intrinsic::ArrayIteratorNext => self.array_iterator_next(receiver, runtime),
            Intrinsic::IteratorSelf => self.adopt(receiver),
            Intrinsic::SetAdd => self.set_add(receiver, arguments.first()),
            Intrinsic::SetHas => self.set_has(receiver, arguments.first()),
            Intrinsic::SetDelete => self.set_delete(receiver, arguments.first()),
            Intrinsic::SetClear => self.set_clear(receiver),
            Intrinsic::WeakSetAdd => self.weak_set_add(receiver, arguments.first()),
            Intrinsic::WeakSetHas => self.weak_set_has(receiver, arguments.first()),
            Intrinsic::WeakSetDelete => self.weak_set_delete(receiver, arguments.first()),
            Intrinsic::SetSizeGet => self.set_size(receiver),
            Intrinsic::SetValues => self.set_values(receiver),
            Intrinsic::SetIteratorNext => self.set_iterator_next(receiver),
            Intrinsic::MapGet => self.map_get(receiver, arguments.first()),
            Intrinsic::MapSet => self.map_set(receiver, arguments.first(), arguments.get(1)),
            Intrinsic::MapHas => self.map_has(receiver, arguments.first()),
            Intrinsic::MapDelete => self.map_delete(receiver, arguments.first()),
            Intrinsic::MapClear => self.map_clear(receiver),
            Intrinsic::MapSizeGet => self.map_size(receiver),
            Intrinsic::MapKeys => self.map_iterator(receiver, map::MapIteration::Keys),
            Intrinsic::MapValues => self.map_iterator(receiver, map::MapIteration::Values),
            Intrinsic::MapEntries => self.map_iterator(receiver, map::MapIteration::Entries),
            Intrinsic::MapIteratorNext => self.map_iterator_next(receiver),
            Intrinsic::NumberToString => self.number_to_string(receiver, arguments, runtime),
            Intrinsic::BigIntToString => self.bigint_to_string(receiver, arguments, runtime),
            Intrinsic::StringTrim => self.string_trim(receiver, runtime),
            Intrinsic::StringStartsWith => self.string_starts_with(receiver, arguments, runtime),
            Intrinsic::StringEndsWith => self.string_ends_with(receiver, arguments, runtime),
            Intrinsic::StringSlice => self.string_slice(receiver, arguments, runtime),
            Intrinsic::StringTrimStart
            | Intrinsic::StringTrimEnd
            | Intrinsic::StringToLowerCase
            | Intrinsic::StringToUpperCase
            | Intrinsic::StringIncludes
            | Intrinsic::StringIndexOf
            | Intrinsic::StringLastIndexOf
            | Intrinsic::StringSplit
            | Intrinsic::StringCharCodeAt
            | Intrinsic::StringPadStart
            | Intrinsic::StringRepeat => self.string_method(intrinsic, receiver, arguments, runtime),
            Intrinsic::StringToWellFormed => self.string_method(intrinsic, receiver, arguments, runtime),
            Intrinsic::StringReplace => self.string_replace(receiver, arguments, runtime),
            Intrinsic::StringReplaceAll => self.string_replace_all(receiver, arguments, runtime),
            Intrinsic::ArrayMap => self.array_map(receiver, arguments, runtime),
            Intrinsic::ArrayFlatMap => self.array_flat_map(receiver, arguments, runtime),
            Intrinsic::ArrayShift => self.array_shift(receiver, runtime),
            Intrinsic::ArrayPop => self.array_pop(receiver, runtime),
            Intrinsic::ArraySort => self.array_sort(receiver, arguments, runtime),
            Intrinsic::RegExpExec
            | Intrinsic::RegExpTest
            | Intrinsic::RegExpToString
            | Intrinsic::RegExpReplace
            | Intrinsic::RegExpSplit
            | Intrinsic::RegExpMatch
            | Intrinsic::RegExpGlobalGet
            | Intrinsic::RegExpIgnoreCaseGet
            | Intrinsic::RegExpDotAllGet
            | Intrinsic::RegExpStickyGet
            | Intrinsic::RegExpUnicodeGet
            | Intrinsic::RegExpFalseFlagGet
            | Intrinsic::RegExpSourceGet
            | Intrinsic::RegExpFlagsGet => self.regexp_method(intrinsic, receiver, arguments, runtime),
            Intrinsic::MapConstructor => Err(ValueError::TypeError("Constructor Map requires 'new'".into())),
            Intrinsic::ObjectConstructor | Intrinsic::ArrayConstructor | Intrinsic::SetConstructor => {
                Err(ValueError::UnsupportedOperation("General builtin constructor invocation belongs to Host"))
            }
            Intrinsic::NativeMember(_) => Err(ValueError::InvalidNode),
        }
    }
    /// The checker reuses the existing Native conversion order before its
    /// numeric relational comparison; this does not convert BigInt to Number.
    pub fn to_primitive(
        self: &Arc<Self>,
        value: &ValueHandle,
        string_hint: bool,
        runtime: &dyn ValueRuntime,
    ) -> ValueResult<ValueHandle> {
        self.primitive(value, string_hint, runtime)
    }

    fn primitive(
        self: &Arc<Self>,
        value: &ValueHandle,
        string_hint: bool,
        runtime: &dyn ValueRuntime,
    ) -> ValueResult<ValueHandle> {
        if is_primitive_kind(self.kind(value)?) {
            return self.adopt(value);
        }
        if let ValueSnapshot::Opaque { capability, .. } = self.snapshot(value)? {
            let result = self.runtime_value(runtime.resolve(RuntimeRequest::Opaque {
                capability,
                target: value,
                receiver: value,
                operation: OpaqueOperation::ToPrimitive { string_hint },
            }))?;
            if !is_primitive_kind(self.kind(&result)?) {
                return Err(ValueError::TypeError("Opaque conversion returned an object".into()));
            }
            return Ok(result);
        }
        let exotic = self.get(value, &self.to_primitive_key(), runtime)?;
        if !matches!(self.kind(&exotic)?, ValueKind::Undefined | ValueKind::Null) {
            let hint = self.string(WireString::from(if string_hint { "string" } else { "number" }))?;
            let result = self.call(&exotic, value, &[hint], runtime)?;
            if is_primitive_kind(self.kind(&result)?) {
                return Ok(result);
            }
            return Err(ValueError::TypeError("Symbol.toPrimitive returned an object".into()));
        }
        for name in if string_hint { ["toString", "valueOf"] } else { ["valueOf", "toString"] } {
            let method = self.get(value, &name.into(), runtime)?;
            if self.kind(&method)? == ValueKind::Function {
                let result = self.call(&method, value, &[], runtime)?;
                if is_primitive_kind(self.kind(&result)?) {
                    return Ok(result);
                }
            }
        }
        Err(ValueError::TypeError("Cannot convert object to primitive value".into()))
    }
    fn scalar_string(self: &Arc<Self>, value: &ValueHandle, runtime: &dyn ValueRuntime) -> ValueResult<WireString> {
        Ok(match self.snapshot(value)? {
            ValueSnapshot::Undefined => "undefined".into(),
            ValueSnapshot::Null => "null".into(),
            ValueSnapshot::Boolean(value) => {
                if value {
                    "true".into()
                } else {
                    "false".into()
                }
            }
            ValueSnapshot::Number(value) if value.is_nan() => "NaN".into(),
            ValueSnapshot::Number(value) if value == f64::INFINITY => "Infinity".into(),
            ValueSnapshot::Number(value) if value == f64::NEG_INFINITY => "-Infinity".into(),
            ValueSnapshot::Number(value) => WireValue::Number(value).stringify().into(),
            ValueSnapshot::String(value) => value,
            ValueSnapshot::BigInt(value) => value.as_str().into(),
            ValueSnapshot::Symbol { description, .. } => {
                let mut units = WireString::from("Symbol(").units().to_vec();
                if let Some(description) = description {
                    units.extend_from_slice(description.units());
                }
                units.push(0x29);
                WireString::from_units(units)
            }
            ValueSnapshot::Opaque { capability, .. } => {
                let primitive = self.runtime_value(runtime.resolve(RuntimeRequest::Opaque {
                    capability,
                    target: value,
                    receiver: value,
                    operation: OpaqueOperation::ToPrimitive { string_hint: true },
                }))?;
                if !is_primitive_kind(self.kind(&primitive)?) || primitive.same_identity(value) {
                    return Err(ValueError::TypeError("Opaque conversion did not produce a scalar".into()));
                }
                return self.scalar_string(&primitive, runtime);
            }
            _ => return Err(ValueError::TypeError("Value is not primitive".into())),
        })
    }
    pub fn to_string(self: &Arc<Self>, value: &ValueHandle, runtime: &dyn ValueRuntime) -> ValueResult<WireString> {
        if self.kind(value)? == ValueKind::Symbol {
            return self.scalar_string(value, runtime);
        }
        let primitive = self.primitive(value, true, runtime)?;
        if self.kind(&primitive)? == ValueKind::Symbol {
            return Err(ValueError::TypeError("Cannot implicitly convert Symbol to string".into()));
        }
        self.scalar_string(&primitive, runtime)
    }
    pub fn to_number(self: &Arc<Self>, value: &ValueHandle, runtime: &dyn ValueRuntime) -> ValueResult<f64> {
        let primitive = self.primitive(value, false, runtime)?;
        match self.snapshot(&primitive)? {
            ValueSnapshot::Undefined => Ok(f64::NAN),
            ValueSnapshot::Null => Ok(0.0),
            ValueSnapshot::Boolean(value) => Ok(if value { 1.0 } else { 0.0 }),
            ValueSnapshot::Number(value) => Ok(value),
            ValueSnapshot::String(value) => Ok(string_number(&value)),
            _ => Err(ValueError::TypeError("Cannot convert BigInt or Symbol to Number".into())),
        }
    }
    fn array_join(
        self: &Arc<Self>,
        value: &ValueHandle,
        separator: &WireString,
        runtime: &dyn ValueRuntime,
        active: &mut HashSet<NodeId>,
        captured_length: Option<u64>,
    ) -> ValueResult<WireString> {
        if !active.insert(self.admit(value)?) {
            return Ok(WireString::from(""));
        }
        let result = (|| {
            let length = match captured_length {
                Some(length) => length,
                None => self.length_of_array_like(value, runtime)?,
            };
            let mut output = Vec::new();
            for index in 0..length {
                if index != 0 {
                    output.extend_from_slice(separator.units());
                }
                let item = self.get(value, &WireString::from(index.to_string()).into(), runtime)?;
                match self.kind(&item)? {
                    ValueKind::Null | ValueKind::Undefined => {}
                    ValueKind::Symbol => {
                        return Err(ValueError::TypeError("Cannot convert Symbol to string in Array.join".into()));
                    }
                    ValueKind::Array => {
                        // The default nested Array.toString join shares this operation's cycle guard.
                        let to_string = self.get(&item, &"toString".into(), runtime)?;
                        let join = self.get(&item, &"join".into(), runtime)?;
                        if matches!(
                            self.snapshot(&to_string)?,
                            ValueSnapshot::Function(CallableTarget::Intrinsic(Intrinsic::ArrayToString))
                        ) && matches!(
                            self.snapshot(&join)?,
                            ValueSnapshot::Function(CallableTarget::Intrinsic(Intrinsic::ArrayJoin))
                        ) {
                            output.extend_from_slice(
                                self.array_join(&item, &WireString::from(","), runtime, active, None)?.units(),
                            );
                        } else {
                            output.extend_from_slice(self.to_string(&item, runtime)?.units());
                        }
                    }
                    _ => output.extend_from_slice(self.to_string(&item, runtime)?.units()),
                }
            }
            Ok(WireString::from_units(output))
        })();
        active.remove(&value.node);
        result
    }
}

fn same_stored_value(arena: &Arena, left: NodeId, right: NodeId) -> ValueResult<bool> {
    if left == right {
        return Ok(true);
    }
    Ok(match (arena.get(left)?, arena.get(right)?) {
        (Node::Undefined, Node::Undefined) | (Node::Null, Node::Null) => true,
        (Node::Boolean(a), Node::Boolean(b)) => a == b,
        (Node::Number(a), Node::Number(b)) => a.to_bits() == b.to_bits() || (a.is_nan() && b.is_nan()),
        (Node::String(a), Node::String(b)) => a == b,
        (Node::BigInt(a), Node::BigInt(b)) => a == b,
        _ => false,
    })
}
fn compatible_descriptor(arena: &Arena, old: &StoredDescriptor, new: &StoredDescriptor) -> ValueResult<bool> {
    if old.configurable() {
        return Ok(true);
    }
    if new.configurable() || old.enumerable() != new.enumerable() {
        return Ok(false);
    }
    match (old, new) {
        (
            StoredDescriptor::Data { value: old, writable: false, .. },
            StoredDescriptor::Data { value: new, writable: false, .. },
        ) => same_stored_value(arena, *old, *new),
        (StoredDescriptor::Data { writable: true, .. }, StoredDescriptor::Data { .. }) => Ok(true),
        (StoredDescriptor::Accessor { get: a, set: b, .. }, StoredDescriptor::Accessor { get: c, set: d, .. }) => {
            Ok(a == c && b == d)
        }
        _ => Ok(false),
    }
}
pub(super) fn valid_array_length(number: f64) -> ValueResult<u32> {
    if !number.is_finite() || number < 0.0 || number > f64::from(u32::MAX) || number.fract() != 0.0 {
        return Err(ValueError::RangeError("Invalid array length".into()));
    }
    Ok(number as u32)
}
fn string_number(value: &WireString) -> f64 {
    fn whitespace(unit: u16) -> bool {
        matches!(unit, 0x09..=0x0d | 0x20 | 0xa0 | 0x1680 | 0x2000..=0x200a | 0x2028..=0x2029 | 0x202f | 0x205f | 0x3000 | 0xfeff)
    }
    let mut units = value.units();
    while units.first().copied().is_some_and(whitespace) {
        units = &units[1..];
    }
    while units.last().copied().is_some_and(whitespace) {
        units = &units[..units.len() - 1];
    }
    if units.is_empty() {
        return 0.0;
    }
    let Ok(text) = String::from_utf16(units) else {
        return f64::NAN;
    };
    match text.as_str() {
        "Infinity" | "+Infinity" => return f64::INFINITY,
        "-Infinity" => return f64::NEG_INFINITY,
        _ => {}
    }
    if let Some((radix, digits)) = text
        .strip_prefix("0x")
        .or_else(|| text.strip_prefix("0X"))
        .map(|digits| (16, digits))
        .or_else(|| text.strip_prefix("0b").or_else(|| text.strip_prefix("0B")).map(|digits| (2, digits)))
        .or_else(|| text.strip_prefix("0o").or_else(|| text.strip_prefix("0O")).map(|digits| (8, digits)))
    {
        if digits.is_empty() {
            return f64::NAN;
        }
        return exact_radix_number(digits, radix);
    }
    // Number(String) decimal syntax, not another JSON grammar/parser.
    let bytes = text.as_bytes();
    let mut position = usize::from(bytes.first().is_some_and(|byte| matches!(*byte, b'+' | b'-')));
    let start = position;
    while bytes.get(position).is_some_and(u8::is_ascii_digit) {
        position += 1;
    }
    let mut digits = position - start;
    if bytes.get(position) == Some(&b'.') {
        position += 1;
        let start = position;
        while bytes.get(position).is_some_and(u8::is_ascii_digit) {
            position += 1;
        }
        digits += position - start;
    }
    if digits == 0 {
        return f64::NAN;
    }
    if bytes.get(position).is_some_and(|byte| matches!(*byte, b'e' | b'E')) {
        position += 1;
        if bytes.get(position).is_some_and(|byte| matches!(*byte, b'+' | b'-')) {
            position += 1;
        }
        let start = position;
        while bytes.get(position).is_some_and(u8::is_ascii_digit) {
            position += 1;
        }
        if position == start {
            return f64::NAN;
        }
    }
    if position != bytes.len() {
        return f64::NAN;
    }
    text.parse().unwrap_or(f64::NAN)
}

// Reuse the Native global integer conversion strategy: accumulate exact decimal
// digits, then round once with the existing correctly rounded f64 parser. The
// original full digit validation must precede the finite-range shortcut.
fn exact_radix_number(digits: &str, radix: u32) -> f64 {
    if digits.chars().any(|digit| digit.to_digit(radix).is_none()) {
        return f64::NAN;
    }
    let digits = digits.trim_start_matches('0');
    if digits.is_empty() {
        return 0.0;
    }
    // Radices here are 2, 8 and 16. More than 1024 nonleading-zero digits
    // already proves an integer at least 2^1024, without unbounded state growth.
    if digits.len() > 1024 {
        return f64::INFINITY;
    }
    let mut decimal = vec![0_u32];
    for digit in digits.chars() {
        let mut carry = digit.to_digit(radix).expect("all radix digits already validated");
        for item in &mut decimal {
            let sum = *item * radix + carry;
            *item = sum % 10;
            carry = sum / 10;
        }
        while carry > 0 {
            decimal.push(carry % 10);
            carry /= 10;
        }
    }
    let text: String = decimal.into_iter().rev().map(|digit| char::from(b'0' + digit as u8)).collect();
    text.parse::<f64>().unwrap_or(f64::INFINITY)
}
