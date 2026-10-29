//! Neutral WeakSet storage for fixed OMP MCP recursion consumers.
//! Entries are weak identity tokens, never ValueHandle/Arc/realm leases.
//! This arena does not collect or recycle individual nodes; no GC observation
//! is claimed. A future collector must not trace these membership tokens.
use super::*;

impl ValueHeap {
    pub fn weak_set_prototype(self: &Arc<Self>) -> ValueResult<ValueHandle> {
        Ok(self.handle(self.intrinsics.weak_set_proto))
    }
    pub fn new_weak_set(
        self: &Arc<Self>,
        iterable: &ValueHandle,
        runtime: &dyn ValueRuntime,
    ) -> ValueResult<ValueHandle> {
        self.admit(iterable)?;
        let set = self.allocate(Node::WeakSet {
            object: ObjectData::new(Some(self.intrinsics.weak_set_proto)),
            weak_entries: Vec::new(),
        })?;
        if matches!(self.kind(iterable)?, ValueKind::Null | ValueKind::Undefined) {
            return Ok(set);
        }
        let add = self.get(&set, &"add".into(), runtime)?;
        if self.kind(&add)? != ValueKind::Function {
            return Err(ValueError::TypeError("WeakSet adder is not callable".into()));
        }
        let mut iterator = self.get_iterator(iterable, runtime)?;
        while let Some(value) = self.iterator_next(&mut iterator, runtime)? {
            if let Err(error) = self.call(&add, &set, &[value], runtime) {
                return Err(match self.iterator_close(&mut iterator, runtime) {
                    Ok(()) => error,
                    Err(close) if error.is_native_exception() && close.is_native_exception() => error,
                    Err(close) => ValueError::IteratorClose { original: Box::new(error), close: Box::new(close) },
                });
            }
        }
        Ok(set)
    }
    pub(super) fn weak_set_add(
        self: &Arc<Self>,
        receiver: &ValueHandle,
        value: Option<&ValueHandle>,
    ) -> ValueResult<ValueHandle> {
        let receiver_id = self.admit(receiver)?;
        let value = value.cloned().unwrap_or(self.undefined()?);
        let value_id = self.admit(&value)?;
        let mut arena = self.lock()?;
        if !matches!(arena.get(receiver_id)?, Node::WeakSet { .. }) {
            return Err(ValueError::TypeError("WeakSet.add receiver has no WeakSet data".into()));
        }
        if !can_be_held_weakly(&arena, value_id)? {
            return Err(ValueError::TypeError("Invalid value used in weak set".into()));
        }
        let Node::WeakSet { weak_entries, .. } = arena.get_mut(receiver_id)? else {
            return Err(ValueError::InvalidNode);
        };
        if !weak_entries.contains(&value_id) {
            weak_entries.try_reserve(1).map_err(|_| ValueError::Allocation)?;
            weak_entries.push(value_id);
        }
        Ok(receiver.clone())
    }
    pub(super) fn weak_set_has(
        self: &Arc<Self>,
        receiver: &ValueHandle,
        value: Option<&ValueHandle>,
    ) -> ValueResult<ValueHandle> {
        let receiver_id = self.admit(receiver)?;
        let value = value.cloned().unwrap_or(self.undefined()?);
        let value_id = self.admit(&value)?;
        let found = {
            let arena = self.lock()?;
            let Node::WeakSet { weak_entries, .. } = arena.get(receiver_id)? else {
                return Err(ValueError::TypeError("WeakSet.has receiver has no WeakSet data".into()));
            };
            can_be_held_weakly(&arena, value_id)? && weak_entries.contains(&value_id)
        };
        self.boolean(found)
    }
    pub(super) fn weak_set_delete(
        self: &Arc<Self>,
        receiver: &ValueHandle,
        value: Option<&ValueHandle>,
    ) -> ValueResult<ValueHandle> {
        let receiver_id = self.admit(receiver)?;
        let value = value.cloned().unwrap_or(self.undefined()?);
        let value_id = self.admit(&value)?;
        let deleted = {
            let mut arena = self.lock()?;
            if !matches!(arena.get(receiver_id)?, Node::WeakSet { .. }) {
                return Err(ValueError::TypeError("WeakSet.delete receiver has no WeakSet data".into()));
            }
            if !can_be_held_weakly(&arena, value_id)? {
                false
            } else {
                let Node::WeakSet { weak_entries, .. } = arena.get_mut(receiver_id)? else {
                    return Err(ValueError::InvalidNode);
                };
                if let Some(index) = weak_entries.iter().position(|entry| *entry == value_id) {
                    weak_entries.remove(index);
                    true
                } else {
                    false
                }
            }
        };
        self.boolean(deleted)
    }
}
fn can_be_held_weakly(arena: &Arena, value: NodeId) -> ValueResult<bool> {
    let node = arena.get(value)?;
    Ok(match node.kind() {
        ValueKind::Object | ValueKind::Array | ValueKind::Function => true,
        ValueKind::Symbol => !arena.symbol_registry.values().any(|registered| *registered == value),
        _ => false,
    })
}
