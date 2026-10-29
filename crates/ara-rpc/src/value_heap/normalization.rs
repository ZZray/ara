//! Native dependencies of fixed OMP validation.ts: new Set(required),
//! Object.entries(properties), and value.trim(). Same arena and resolver;
//! session-persistence.ts also consumes the native Set.prototype.size getter.
//! These operations do not implement the remaining general Set surface.
//! Source: OMP 596f2da7101178214aa27a753529d15e6b7ad91d (MIT),
//! packages/ai/src/utils/validation.ts:76-96, 758-831. See THIRD_PARTY.md.
//! Copyright (c) 2025 Mario Zechner; Copyright (c) 2025-2026 Can Bölük;
//! Copyright (c) 2026 Stencil Labs, Inc.

use super::*;

impl ValueHeap {
    pub(super) fn set_size(self: &Arc<Self>, receiver: &ValueHandle) -> ValueResult<ValueHandle> {
        let node = self.admit(receiver)?;
        let length = {
            let arena = self.lock()?;
            let Node::Set { entries, .. } = arena.get(node)? else {
                return Err(ValueError::TypeError("Set.size receiver has no Set data".into()));
            };
            entries.iter().flatten().count()
        };
        self.number(length as f64)
    }

    /// The selected add method is captured before acquiring the iterator.
    /// Stored entries are ordinary same-arena edges, not copied JSON keys.
    pub fn new_set(self: &Arc<Self>, iterable: &ValueHandle, runtime: &dyn ValueRuntime) -> ValueResult<ValueHandle> {
        let set = self.allocate(Node::Object(ObjectData::new(Some(self.intrinsics.set_proto))))?;
        self.initialize_set_receiver(&set, iterable, runtime)
    }

    /// The constructor's original neutral receiver keeps its identity and
    /// newTarget prototype. Brand it before any adder or iterator observation.
    pub fn initialize_set_receiver(
        self: &Arc<Self>,
        set: &ValueHandle,
        iterable: &ValueHandle,
        runtime: &dyn ValueRuntime,
    ) -> ValueResult<ValueHandle> {
        self.admit(iterable)?;
        let node = self.admit(set)?;
        {
            let mut arena = self.lock()?;
            let receiver = arena.get_mut(node)?;
            let Node::Object(object) = receiver else {
                return Err(ValueError::TypeError("Set constructor receiver is not uninitialized".into()));
            };
            *receiver = Node::Set { object: object.clone(), entries: Vec::new() };
        }
        if matches!(self.kind(iterable)?, ValueKind::Null | ValueKind::Undefined) {
            return Ok(set.clone());
        }
        let add = self.get(set, &"add".into(), runtime)?;
        if self.kind(&add)? != ValueKind::Function {
            return Err(ValueError::TypeError("Set adder is not callable".into()));
        }
        let mut iterator = self.get_iterator(iterable, runtime)?;
        while let Some(value) = self.iterator_next(&mut iterator, runtime)? {
            if let Err(error) = self.call(&add, set, &[value], runtime) {
                return Err(match self.iterator_close(&mut iterator, runtime) {
                    Ok(()) => error,
                    Err(close) if error.is_native_exception() && close.is_native_exception() => error,
                    Err(close) => ValueError::IteratorClose { original: Box::new(error), close: Box::new(close) },
                });
            }
        }
        Ok(set.clone())
    }

    pub(super) fn set_add(
        self: &Arc<Self>,
        receiver: &ValueHandle,
        value: Option<&ValueHandle>,
    ) -> ValueResult<ValueHandle> {
        let node = self.admit(receiver)?;
        let value = value.cloned().unwrap_or(self.undefined()?);
        let mut entry = self.admit(&value)?;
        let mut arena = self.lock()?;
        let Node::Set { entries, .. } = arena.get(node)? else {
            return Err(ValueError::TypeError("Set.add receiver has no Set data".into()));
        };
        for candidate in entries.iter().flatten() {
            if same_value_zero(&arena, *candidate, entry)? {
                return Ok(receiver.clone());
            }
        }
        if matches!(arena.get(entry)?, Node::Number(number) if *number == 0.0 && number.is_sign_negative()) {
            entry = arena.allocate(Node::Number(0.0))?;
        }
        let Node::Set { entries, .. } = arena.get_mut(node)? else {
            return Err(ValueError::InvalidNode);
        };
        entries.try_reserve(1).map_err(|_| ValueError::Allocation)?;
        entries.push(Some(entry));
        Ok(receiver.clone())
    }

    pub(super) fn set_has(
        self: &Arc<Self>,
        receiver: &ValueHandle,
        value: Option<&ValueHandle>,
    ) -> ValueResult<ValueHandle> {
        let node = self.admit(receiver)?;
        let value = value.cloned().unwrap_or(self.undefined()?);
        let entry = self.admit(&value)?;
        let found = {
            let arena = self.lock()?;
            let Node::Set { entries, .. } = arena.get(node)? else {
                return Err(ValueError::TypeError("Set.has receiver has no Set data".into()));
            };
            let mut found = false;
            for candidate in entries.iter().flatten() {
                if same_value_zero(&arena, *candidate, entry)? {
                    found = true;
                    break;
                }
            }
            found
        };
        self.boolean(found)
    }

    /// Tombstones preserve live Set iterator indices; delete/readd appends.
    pub(super) fn set_delete(
        self: &Arc<Self>,
        receiver: &ValueHandle,
        value: Option<&ValueHandle>,
    ) -> ValueResult<ValueHandle> {
        let node = self.admit(receiver)?;
        let value = value.cloned().unwrap_or(self.undefined()?);
        let entry = self.admit(&value)?;
        let removed = {
            let mut arena = self.lock()?;
            let Node::Set { entries, .. } = arena.get(node)? else {
                return Err(ValueError::TypeError("Set.delete receiver has no Set data".into()));
            };
            let mut found = None;
            for (index, candidate) in entries.iter().enumerate() {
                if let Some(candidate) = candidate {
                    if same_value_zero(&arena, *candidate, entry)? {
                        found = Some(index);
                        break;
                    }
                }
            }
            if let Some(index) = found {
                let Node::Set { entries, .. } = arena.get_mut(node)? else {
                    return Err(ValueError::InvalidNode);
                };
                entries[index] = None;
                true
            } else {
                false
            }
        };
        self.boolean(removed)
    }

    /// Clear preserves the Set data list and live iterator indices. Existing
    /// slots become empty; later add appends and remains visible to live iterators.
    pub(super) fn set_clear(self: &Arc<Self>, receiver: &ValueHandle) -> ValueResult<ValueHandle> {
        let node = self.admit(receiver)?;
        {
            let mut arena = self.lock()?;
            let Node::Set { entries, .. } = arena.get_mut(node)? else {
                return Err(ValueError::TypeError("Set.clear receiver has no Set data".into()));
            };
            for entry in entries {
                *entry = None;
            }
        }
        self.undefined()
    }

    /// EnumerableOwnProperties performs each descriptor/Get pair in sequence.
    /// An earlier getter can delete or alter a later property's descriptor.
    pub fn object_entries(
        self: &Arc<Self>,
        source: &ValueHandle,
        runtime: &dyn ValueRuntime,
    ) -> ValueResult<ValueHandle> {
        self.admit(source)?;
        if matches!(self.kind(source)?, ValueKind::Null | ValueKind::Undefined) {
            return Err(ValueError::TypeError("Object.entries receiver is null or undefined".into()));
        }
        let mut entries = Vec::new();
        for key in self.own_keys(source, runtime)? {
            let PropertyKey::String(text) = &key else {
                continue;
            };
            if !self.own_descriptor(source, &key, runtime)?.is_some_and(|descriptor| descriptor.enumerable()) {
                continue;
            }
            let value = self.get(source, &key, runtime)?;
            let pair = self.array(vec![self.string(text.clone())?, value])?;
            entries.try_reserve(1).map_err(|_| ValueError::Allocation)?;
            entries.push(pair);
        }
        self.array(entries)
    }

    pub(super) fn string_trim(
        self: &Arc<Self>,
        receiver: &ValueHandle,
        runtime: &dyn ValueRuntime,
    ) -> ValueResult<ValueHandle> {
        match self.kind(receiver)? {
            ValueKind::Null | ValueKind::Undefined => {
                return Err(ValueError::TypeError("String.trim receiver is null or undefined".into()));
            }
            ValueKind::Symbol => {
                return Err(ValueError::TypeError("Cannot implicitly convert Symbol to string".into()));
            }
            _ => {}
        }
        let text = self.to_string(receiver, runtime)?;
        let mut units = text.units();
        fn whitespace(unit: u16) -> bool {
            matches!(unit, 0x09..=0x0d | 0x20 | 0xa0 | 0x1680 | 0x2000..=0x200a | 0x2028..=0x2029 | 0x202f | 0x205f | 0x3000 | 0xfeff)
        }
        while units.first().copied().is_some_and(whitespace) {
            units = &units[1..];
        }
        while units.last().copied().is_some_and(whitespace) {
            units = &units[..units.len() - 1];
        }
        self.string(WireString::from_units(units.to_vec()))
    }
    pub(super) fn string_method_receiver(
        self: &Arc<Self>,
        receiver: &ValueHandle,
        runtime: &dyn ValueRuntime,
    ) -> ValueResult<WireString> {
        match self.kind(receiver)? {
            ValueKind::Null | ValueKind::Undefined => {
                Err(ValueError::TypeError("String method receiver is null or undefined".into()))
            }
            ValueKind::Symbol => Err(ValueError::TypeError("Cannot implicitly convert Symbol to string".into())),
            _ => self.to_string(receiver, runtime),
        }
    }
    /// Fixed draft.ts supplies a primitive literal search argument. Object
    /// searches require the not-yet-connected RegExp/@@match domain and fail
    /// explicitly after receiver conversion, rather than skipping IsRegExp.
    pub(super) fn string_starts_with(
        self: &Arc<Self>,
        receiver: &ValueHandle,
        arguments: &[ValueHandle],
        runtime: &dyn ValueRuntime,
    ) -> ValueResult<ValueHandle> {
        let text = self.string_method_receiver(receiver, runtime)?;
        let search = arguments.first().cloned().unwrap_or(self.undefined()?);
        if self.is_regexp(&search, runtime)? {
            return Err(ValueError::TypeError("String.startsWith search is a RegExp".into()));
        }
        if self.kind(&search)? == ValueKind::Symbol {
            return Err(ValueError::TypeError("Cannot implicitly convert Symbol to string".into()));
        }
        let search = self.to_string(&search, runtime)?;
        let position = arguments.get(1).map(|v| self.to_number(v, runtime)).transpose()?.unwrap_or(0.0);
        let position = if position.is_nan() { 0.0 } else { position.trunc() };
        let start = position.clamp(0.0, text.len() as f64) as usize;
        self.boolean(text.units()[start..].starts_with(search.units()))
    }
    /// Session loader's truncation marker comparison uses native UTF16 units.
    /// Receiver, @@match, search and end conversions retain their source order.
    pub(super) fn string_ends_with(
        self: &Arc<Self>,
        receiver: &ValueHandle,
        arguments: &[ValueHandle],
        runtime: &dyn ValueRuntime,
    ) -> ValueResult<ValueHandle> {
        let text = self.string_method_receiver(receiver, runtime)?;
        let search = arguments.first().cloned().unwrap_or(self.undefined()?);
        if self.is_regexp(&search, runtime)? {
            return Err(ValueError::TypeError("String.endsWith search is a RegExp".into()));
        }
        if self.kind(&search)? == ValueKind::Symbol {
            return Err(ValueError::TypeError("Cannot implicitly convert Symbol to string".into()));
        }
        let search = self.to_string(&search, runtime)?;
        let position = match arguments.get(1) {
            Some(value) if self.kind(value)? != ValueKind::Undefined => self.to_number(value, runtime)?,
            _ => text.len() as f64,
        };
        let position = if position.is_nan() { 0.0 } else { position.trunc() };
        let end = position.clamp(0.0, text.len() as f64) as usize;
        self.boolean(text.units()[..end].ends_with(search.units()))
    }
    pub(super) fn string_slice(
        self: &Arc<Self>,
        receiver: &ValueHandle,
        arguments: &[ValueHandle],
        runtime: &dyn ValueRuntime,
    ) -> ValueResult<ValueHandle> {
        let text = self.string_method_receiver(receiver, runtime)?;
        let start = arguments.first().map(|v| self.to_number(v, runtime)).transpose()?.unwrap_or(0.0);
        let end = match arguments.get(1) {
            Some(value) if self.kind(value)? != ValueKind::Undefined => self.to_number(value, runtime)?,
            _ => text.len() as f64,
        };
        let start = string_slice_index(start, text.len());
        let end = string_slice_index(end, text.len()).max(start);
        self.string(WireString::from_units(text.units()[start..end].to_vec()))
    }
}

fn string_slice_index(relative: f64, length: usize) -> usize {
    let relative = if relative.is_nan() { 0.0 } else { relative.trunc() };
    let absolute = if relative < 0.0 { length as f64 + relative } else { relative };
    absolute.clamp(0.0, length as f64) as usize
}

/// SameValueZero is a primitive comparison with no callback or coercion.
/// The single arena guard covers equality and insertion atomically.
pub(super) fn same_value_zero(arena: &Arena, left: NodeId, right: NodeId) -> ValueResult<bool> {
    if left == right {
        return Ok(true);
    }
    Ok(match (arena.get(left)?, arena.get(right)?) {
        (Node::Undefined, Node::Undefined) | (Node::Null, Node::Null) => true,
        (Node::Boolean(left), Node::Boolean(right)) => left == right,
        (Node::Number(left), Node::Number(right)) => left == right || (left.is_nan() && right.is_nan()),
        (Node::String(left), Node::String(right)) => left == right,
        (Node::BigInt(left), Node::BigInt(right)) => left == right,
        _ => false,
    })
}
