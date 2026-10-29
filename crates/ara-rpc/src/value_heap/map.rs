//! Native lexical Map dependencies of fixed OMP596f2da transform-messages.ts.
//! MIT; Copyright (c) 2025 Mario Zechner, 2025-2026 Can Bölük,
//! 2026 Stencil Labs, Inc. See THIRD_PARTY.md.
//! Internal slots retain insertion order and NodeId edges, never Host leases.
use super::normalization::same_value_zero;
use super::*;

#[derive(Clone, Copy, Debug)]
pub(super) enum MapIteration {
    Keys,
    Values,
    Entries,
}

impl ValueHeap {
    /// Fixed `new Map(iterable)` intrinsic. The adder is selected once before
    /// iterator acquisition; entry Get(0), Get(1), and Call remain observable.
    pub fn new_map(self: &Arc<Self>, iterable: &ValueHandle, runtime: &dyn ValueRuntime) -> ValueResult<ValueHandle> {
        let map = self.allocate(Node::Object(ObjectData::new(Some(self.intrinsics.map_proto))))?;
        self.initialize_map_receiver(&map, iterable, runtime)
    }

    /// The constructor's original neutral receiver keeps its identity and
    /// newTarget prototype. Brand it before any adder or iterator observation.
    pub fn initialize_map_receiver(
        self: &Arc<Self>,
        map: &ValueHandle,
        iterable: &ValueHandle,
        runtime: &dyn ValueRuntime,
    ) -> ValueResult<ValueHandle> {
        self.admit(iterable)?;
        let node = self.admit(map)?;
        {
            let mut arena = self.lock()?;
            let receiver = arena.get_mut(node)?;
            let Node::Object(object) = receiver else {
                return Err(ValueError::TypeError("Map constructor receiver is not uninitialized".into()));
            };
            *receiver = Node::Map { object: object.clone(), entries: Vec::new() };
        }
        if matches!(self.kind(iterable)?, ValueKind::Null | ValueKind::Undefined) {
            return Ok(map.clone());
        }
        let adder = self.get(map, &"set".into(), runtime)?;
        if self.kind(&adder)? != ValueKind::Function {
            return Err(ValueError::TypeError("Map adder is not callable".into()));
        }
        let mut iterator = self.get_iterator(iterable, runtime)?;
        while let Some(entry) = self.iterator_next(&mut iterator, runtime)? {
            let step = (|| {
                if !matches!(self.kind(&entry)?, ValueKind::Object | ValueKind::Array | ValueKind::Function) {
                    return Err(ValueError::TypeError("Map iterable entry is not an object".into()));
                }
                let key = self.get(&entry, &"0".into(), runtime)?;
                let value = self.get(&entry, &"1".into(), runtime)?;
                self.call(&adder, map, &[key, value], runtime).map(|_| ())
            })();
            if let Err(original) = step {
                return Err(match self.iterator_close(&mut iterator, runtime) {
                    Ok(()) => original,
                    Err(close) if original.is_native_exception() && close.is_native_exception() => original,
                    Err(close) => ValueError::IteratorClose { original: Box::new(original), close: Box::new(close) },
                });
            }
        }
        Ok(map.clone())
    }

    fn map_key(self: &Arc<Self>, key: Option<&ValueHandle>) -> ValueResult<NodeId> {
        self.admit(&key.cloned().unwrap_or(self.undefined()?))
    }
    fn map_position(arena: &Arena, map: NodeId, key: NodeId) -> ValueResult<Option<usize>> {
        let Node::Map { entries, .. } = arena.get(map)? else {
            return Err(ValueError::TypeError("Map method receiver has no Map data".into()));
        };
        for (index, entry) in entries.iter().enumerate() {
            if let Some((candidate, _)) = entry {
                if same_value_zero(arena, *candidate, key)? {
                    return Ok(Some(index));
                }
            }
        }
        Ok(None)
    }
    pub(super) fn map_get(
        self: &Arc<Self>,
        receiver: &ValueHandle,
        key: Option<&ValueHandle>,
    ) -> ValueResult<ValueHandle> {
        let map = self.admit(receiver)?;
        let key = self.map_key(key)?;
        let found = {
            let arena = self.lock()?;
            match Self::map_position(&arena, map, key)? {
                Some(index) => {
                    let Node::Map { entries, .. } = arena.get(map)? else {
                        return Err(ValueError::InvalidNode);
                    };
                    Some(entries[index].ok_or(ValueError::InvalidNode)?.1)
                }
                None => None,
            }
        };
        match found {
            Some(value) => Ok(self.handle(value)),
            None => self.undefined(),
        }
    }
    pub(super) fn map_has(
        self: &Arc<Self>,
        receiver: &ValueHandle,
        key: Option<&ValueHandle>,
    ) -> ValueResult<ValueHandle> {
        let map = self.admit(receiver)?;
        let key = self.map_key(key)?;
        let found = {
            let arena = self.lock()?;
            Self::map_position(&arena, map, key)?.is_some()
        };
        self.boolean(found)
    }
    pub(super) fn map_set(
        self: &Arc<Self>,
        receiver: &ValueHandle,
        key: Option<&ValueHandle>,
        value: Option<&ValueHandle>,
    ) -> ValueResult<ValueHandle> {
        let map = self.admit(receiver)?;
        let mut key = self.map_key(key)?;
        let value = self.admit(&value.cloned().unwrap_or(self.undefined()?))?;
        let mut arena = self.lock()?;
        let position = Self::map_position(&arena, map, key)?;
        if let Some(index) = position {
            let Node::Map { entries, .. } = arena.get_mut(map)? else {
                return Err(ValueError::InvalidNode);
            };
            entries[index].as_mut().ok_or(ValueError::InvalidNode)?.1 = value;
        } else {
            if matches!(arena.get(key)?, Node::Number(number) if *number == 0.0 && number.is_sign_negative()) {
                key = arena.allocate(Node::Number(0.0))?;
            }
            let Node::Map { entries, .. } = arena.get_mut(map)? else {
                return Err(ValueError::InvalidNode);
            };
            entries.try_reserve(1).map_err(|_| ValueError::Allocation)?;
            entries.push(Some((key, value)));
        }
        Ok(receiver.clone())
    }
    pub(super) fn map_delete(
        self: &Arc<Self>,
        receiver: &ValueHandle,
        key: Option<&ValueHandle>,
    ) -> ValueResult<ValueHandle> {
        let map = self.admit(receiver)?;
        let key = self.map_key(key)?;
        let deleted = {
            let mut arena = self.lock()?;
            let position = Self::map_position(&arena, map, key)?;
            if let Some(index) = position {
                let Node::Map { entries, .. } = arena.get_mut(map)? else {
                    return Err(ValueError::InvalidNode);
                };
                entries[index] = None;
                true
            } else {
                false
            }
        };
        self.boolean(deleted)
    }
    pub(super) fn map_clear(self: &Arc<Self>, receiver: &ValueHandle) -> ValueResult<ValueHandle> {
        let map = self.admit(receiver)?;
        {
            let mut arena = self.lock()?;
            let Node::Map { entries, .. } = arena.get_mut(map)? else {
                return Err(ValueError::TypeError("Map.clear receiver has no Map data".into()));
            };
            for entry in entries {
                *entry = None;
            }
        }
        self.undefined()
    }
    pub(super) fn map_size(self: &Arc<Self>, receiver: &ValueHandle) -> ValueResult<ValueHandle> {
        let map = self.admit(receiver)?;
        let count = {
            let arena = self.lock()?;
            let Node::Map { entries, .. } = arena.get(map)? else {
                return Err(ValueError::TypeError("Map.size receiver has no Map data".into()));
            };
            entries.iter().filter(|entry| entry.is_some()).count()
        };
        self.number(count as f64)
    }
    pub(super) fn map_iterator(
        self: &Arc<Self>,
        receiver: &ValueHandle,
        flavor: MapIteration,
    ) -> ValueResult<ValueHandle> {
        let target = self.admit(receiver)?;
        if !matches!(self.lock()?.get(target)?, Node::Map { .. }) {
            return Err(ValueError::TypeError("Map iterator receiver has no Map data".into()));
        }
        self.allocate(Node::MapIterator {
            object: ObjectData::new(Some(self.intrinsics.map_iterator_proto)),
            target: Some(target),
            next_index: 0,
            flavor,
        })
    }
    pub(super) fn map_iterator_next(self: &Arc<Self>, receiver: &ValueHandle) -> ValueResult<ValueHandle> {
        let node = self.admit(receiver)?;
        let (entry, flavor) = {
            let mut arena = self.lock()?;
            let Node::MapIterator { target, next_index, flavor, .. } = arena.get(node)? else {
                return Err(ValueError::TypeError("Incompatible Map Iterator receiver".into()));
            };
            let (target, mut index, flavor) = (*target, *next_index, *flavor);
            let mut found = None;
            if let Some(target) = target {
                let Node::Map { entries, .. } = arena.get(target)? else {
                    return Err(ValueError::InvalidNode);
                };
                while index < entries.len() {
                    let entry = entries[index];
                    index += 1;
                    if entry.is_some() {
                        found = entry;
                        break;
                    }
                }
            }
            let Node::MapIterator { target, next_index, .. } = arena.get_mut(node)? else {
                return Err(ValueError::InvalidNode);
            };
            if found.is_some() {
                *next_index = index;
            } else {
                *target = None;
            }
            (found, flavor)
        };
        let Some((key, value)) = entry else {
            return self.iterator_result(self.undefined()?, true);
        };
        let value = match flavor {
            MapIteration::Keys => self.handle(key),
            MapIteration::Values => self.handle(value),
            MapIteration::Entries => self.array(vec![self.handle(key), self.handle(value)])?,
        };
        self.iterator_result(value, false)
    }
}
