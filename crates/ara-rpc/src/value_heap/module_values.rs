//! Fixed OMP 596f2da7101178214aa27a753529d15e6b7ad91d, packages/ai/src/utils/validation.ts.
//! MIT: Copyright (c) 2025 Mario Zechner; Copyright (c) 2025-2026 Can Bölük;
//! Copyright (c) 2026 Stencil Labs, Inc. See THIRD_PARTY.md.
//! Module literal publication with callback-safe initialization ownership.
//! Ready and failed state contains NodeIds/static metadata, never heap leases.

use super::*;

pub(super) type StringSetDefinitions = &'static [(&'static str, &'static [&'static str])];

#[derive(Debug, Clone)]
pub(super) enum StoredModuleFailure {
    Thrown(NodeId),
    IteratorClose { original: Box<Self>, close: Box<Self> },
    // Constructed only after the lease-containing variants above are removed.
    Leaf(ValueError),
}

impl ValueHeap {
    /// Fixed normalize.ts OPEN_SUBSCHEMA_WIDENING. One literal per Native
    /// realm: the root is shallow-frozen, while its array and type objects
    /// retain their ordinary mutable descriptors. No runtime lease is stored.
    pub fn module_open_subschema_widening(self: &Arc<Self>) -> ValueResult<ValueHandle> {
        const KEY: (&str, &str) = ("ara-ai.schema.normalize", "OPEN_SUBSCHEMA_WIDENING");
        let mut arena = self.lock()?;
        if let Some((node, definition)) = arena.module_values.get(&KEY) {
            return if definition == &StaticModuleValue::OpenSubschemaWidening {
                Ok(self.handle(*node))
            } else {
                Err(ValueError::SourceIdentityCollision)
            };
        }
        arena.module_values.try_reserve(1).map_err(|_| ValueError::Allocation)?;
        let mut array = ObjectData::new(Some(self.intrinsics.array_proto));
        array.properties.try_reserve(6).map_err(|_| ValueError::Allocation)?;
        for (index, name) in ["string", "number", "boolean", "object", "array", "null"].iter().enumerate() {
            let text = arena.allocate(Node::String((*name).into()))?;
            let mut member = ObjectData::new(Some(self.intrinsics.object_proto));
            member.properties.try_reserve(1).map_err(|_| ValueError::Allocation)?;
            member.properties.push(PropertyEntry {
                key: "type".into(),
                descriptor: StoredDescriptor::Data {
                    value: text,
                    writable: true,
                    enumerable: true,
                    configurable: true,
                },
            });
            let member = arena.allocate(Node::Object(member))?;
            array.properties.push(PropertyEntry {
                key: PropertyKey::String(index.to_string().into()),
                descriptor: StoredDescriptor::Data {
                    value: member,
                    writable: true,
                    enumerable: true,
                    configurable: true,
                },
            });
        }
        let array = arena.allocate(Node::Array { object: array, length: 6, length_writable: true })?;
        let mut root = ObjectData::new(Some(self.intrinsics.object_proto));
        root.properties.try_reserve(1).map_err(|_| ValueError::Allocation)?;
        root.extensible = false;
        root.properties.push(PropertyEntry {
            key: "anyOf".into(),
            descriptor: StoredDescriptor::Data { value: array, writable: false, enumerable: true, configurable: false },
        });
        let node = arena.allocate(Node::Object(root))?;
        arena.module_values.insert(KEY, (node, StaticModuleValue::OpenSubschemaWidening));
        Ok(self.handle(node))
    }

    /// Array literal construction has no adder or iterator callback. Its
    /// identity and later dynamic iteration remain the actual module value.
    pub fn module_string_array(
        self: &Arc<Self>,
        module: &'static str,
        name: &'static str,
        values: &'static [&'static str],
    ) -> ValueResult<ValueHandle> {
        let definition = StaticModuleValue::StringArray(values);
        let mut arena = self.lock()?;
        if let Some((node, prior)) = arena.module_values.get(&(module, name)) {
            return if prior == &definition {
                Ok(self.handle(*node))
            } else {
                Err(ValueError::SourceIdentityCollision)
            };
        }
        arena.module_values.try_reserve(1).map_err(|_| ValueError::Allocation)?;
        let length = u32::try_from(values.len()).map_err(|_| ValueError::RangeError("Invalid array length".into()))?;
        let mut object = ObjectData::new(Some(self.intrinsics.array_proto));
        object.properties.try_reserve(values.len()).map_err(|_| ValueError::Allocation)?;
        for (index, value) in values.iter().enumerate() {
            let value = arena.allocate(Node::String((*value).into()))?;
            object.properties.push(PropertyEntry {
                key: WireString::from(index.to_string()).into(),
                descriptor: StoredDescriptor::Data { value, writable: true, enumerable: true, configurable: true },
            });
        }
        let node = arena.allocate(Node::Array { object, length, length_writable: true })?;
        arena.module_values.insert((module, name), (node, definition));
        Ok(self.handle(node))
    }

    fn retain_module_failure(&self, error: &ValueError) -> ValueResult<StoredModuleFailure> {
        Ok(match error {
            ValueError::Thrown(value) => StoredModuleFailure::Thrown(self.admit(value)?),
            ValueError::IteratorClose { original, close } => StoredModuleFailure::IteratorClose {
                original: Box::new(self.retain_module_failure(original)?),
                close: Box::new(self.retain_module_failure(close)?),
            },
            error => StoredModuleFailure::Leaf(error.clone()),
        })
    }

    fn restore_module_failure(self: &Arc<Self>, error: &StoredModuleFailure) -> ValueError {
        match error {
            StoredModuleFailure::Thrown(node) => ValueError::Thrown(self.handle(*node)),
            StoredModuleFailure::IteratorClose { original, close } => ValueError::IteratorClose {
                original: Box::new(self.restore_module_failure(original)),
                close: Box::new(self.restore_module_failure(close)),
            },
            StoredModuleFailure::Leaf(error) => error.clone(),
        }
    }

    /// The domain owner calls this at module preparation, before publishing its
    /// factory. Cold initialization uses actual Set adder/iterator operations.
    /// A second thread may wait; same-thread reentry fails without self-wait.
    pub fn prepare_module_string_sets(
        self: &Arc<Self>,
        module: &'static str,
        definitions: StringSetDefinitions,
        runtime: &dyn ValueRuntime,
    ) -> ValueResult<()> {
        let owner = std::thread::current().id();
        {
            let mut arena = self.lock()?;
            loop {
                if let Some((prior, failure)) = arena.module_set_failures.get(module) {
                    if *prior != definitions {
                        return Err(ValueError::SourceIdentityCollision);
                    }
                    return Err(self.restore_module_failure(failure));
                }
                if let Some(prior) = arena.module_set_ready.get(module) {
                    return if *prior == definitions { Ok(()) } else { Err(ValueError::SourceIdentityCollision) };
                }
                if let Some((active, prior)) = arena.module_set_initializing.get(module) {
                    if *prior != definitions {
                        return Err(ValueError::SourceIdentityCollision);
                    }
                    if *active == owner {
                        return Err(ValueError::UnsupportedOperation("Module initializer reentry is not ready"));
                    }
                    arena = self.module_initialized.wait(arena).map_err(|_| ValueError::PoisonedHeap)?;
                    continue;
                }
                // Reserve publication space before invoking any external code.
                arena.module_set_initializing.try_reserve(1).map_err(|_| ValueError::Allocation)?;
                arena.module_set_failures.try_reserve(1).map_err(|_| ValueError::Allocation)?;
                arena.module_set_ready.try_reserve(1).map_err(|_| ValueError::Allocation)?;
                arena.module_values.try_reserve(definitions.len()).map_err(|_| ValueError::Allocation)?;
                for (name, values) in definitions {
                    if let Some((_, prior)) = arena.module_values.get(&(module, *name))
                        && prior != &StaticModuleValue::StringSet(values)
                    {
                        return Err(ValueError::SourceIdentityCollision);
                    }
                }
                arena.module_set_initializing.insert(module, (owner, definitions));
                break;
            }
        }
        let initialize = (|| {
            let mut ready = Vec::new();
            ready.try_reserve(definitions.len()).map_err(|_| ValueError::Allocation)?;
            for (name, values) in definitions {
                let literals =
                    values.iter().map(|value| self.string((*value).into())).collect::<ValueResult<Vec<_>>>()?;
                let set = self.new_set(&self.array(literals)?, runtime)?;
                ready.push((*name, *values, self.admit(&set)?));
            }
            Ok(ready)
        })();
        let stored = match &initialize {
            Ok(_) => None,
            Err(error) => {
                Some(self.retain_module_failure(error).unwrap_or(StoredModuleFailure::Leaf(ValueError::ForeignHeap)))
            }
        };
        let mut arena = self.lock()?;
        arena.module_set_initializing.remove(module);
        // A conflicting registration during a reentrant initializer cannot
        // be overwritten when its caller returns.
        let initialize = initialize.and_then(|ready| {
            for (name, values, _) in &ready {
                if let Some((_, prior)) = arena.module_values.get(&(module, *name))
                    && prior != &StaticModuleValue::StringSet(values)
                {
                    return Err(ValueError::SourceIdentityCollision);
                }
            }
            Ok(ready)
        });
        match initialize {
            Ok(ready) => {
                for (name, values, node) in ready {
                    arena.module_values.insert((module, name), (node, StaticModuleValue::StringSet(values)));
                }
                arena.module_set_ready.insert(module, definitions);
                self.module_initialized.notify_all();
                Ok(())
            }
            Err(error) => {
                let retained = stored.unwrap_or(StoredModuleFailure::Leaf(ValueError::SourceIdentityCollision));
                arena.module_set_failures.insert(module, (definitions, retained));
                self.module_initialized.notify_all();
                Err(error)
            }
        }
    }

    /// Resolve only an already prepared module constant. Leaf calls never run
    /// an initializer after author hooks or retry a failed source module.
    pub fn module_string_set(
        self: &Arc<Self>,
        module: &'static str,
        name: &'static str,
        values: &'static [&'static str],
    ) -> ValueResult<ValueHandle> {
        let arena = self.lock()?;
        if let Some((_, error)) = arena.module_set_failures.get(module) {
            return Err(self.restore_module_failure(error));
        }
        let (node, definition) = arena
            .module_values
            .get(&(module, name))
            .ok_or(ValueError::UnsupportedOperation("Module literal Set must be prepared before author hooks"))?;
        if definition != &StaticModuleValue::StringSet(values) {
            return Err(ValueError::SourceIdentityCollision);
        }
        Ok(self.handle(*node))
    }
}
