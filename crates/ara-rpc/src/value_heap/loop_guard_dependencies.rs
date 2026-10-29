//! Native dependencies of fixed OMP596f2da loop guard and prompt formatter.
//! ECMAScript Array.sort/Array.pop operations use the original heap/runtime.
//! MIT; see THIRD_PARTY_NOTICES.md. No callback runs under an arena lock.
use super::*;

impl ValueHeap {
    /// Captured module-private identity tables have no publicly discoverable
    /// own/prototype/symbol edge. Private brand reads bypass Map.prototype.
    pub fn compiled_private_get(
        self: &Arc<Self>,
        table: &ValueHandle,
        receiver: &ValueHandle,
    ) -> ValueResult<Option<ValueHandle>> {
        let present = self.map_has(table, Some(receiver))?;
        if !self.truthy(&present)? {
            return Ok(None);
        }
        self.map_get(table, Some(receiver)).map(Some)
    }
    pub fn compiled_private_insert(
        self: &Arc<Self>,
        table: &ValueHandle,
        receiver: &ValueHandle,
        value: &ValueHandle,
    ) -> ValueResult<()> {
        self.map_set(table, Some(receiver), Some(value)).map(|_| ())
    }
    fn loop_guard_array_receiver(&self, receiver: &ValueHandle) -> ValueResult<()> {
        match self.kind(receiver)? {
            ValueKind::Object | ValueKind::Array | ValueKind::Function => Ok(()),
            ValueKind::Null | ValueKind::Undefined => {
                Err(ValueError::TypeError("Array method receiver is null or undefined".into()))
            }
            _ => Err(ValueError::UnsupportedOperation("Array method primitive receiver boxing is not connected")),
        }
    }
    pub(super) fn array_pop(
        self: &Arc<Self>,
        receiver: &ValueHandle,
        runtime: &dyn ValueRuntime,
    ) -> ValueResult<ValueHandle> {
        self.loop_guard_array_receiver(receiver)?;
        let length = self.length_of_array_like(receiver, runtime)?;
        if length == 0 {
            self.assign(receiver, "length".into(), self.number(0.0)?, runtime)?;
            return self.undefined();
        }
        let key = PropertyKey::from(WireString::from((length - 1).to_string()));
        let value = self.get(receiver, &key, runtime)?;
        if !self.delete_own(receiver, &key, runtime)? {
            return Err(ValueError::TypeError("Array.pop cannot delete final indexed property".into()));
        }
        self.assign(receiver, "length".into(), self.number((length - 1) as f64)?, runtime)?;
        Ok(value)
    }

    pub(super) fn array_sort(
        self: &Arc<Self>,
        receiver: &ValueHandle,
        arguments: &[ValueHandle],
        runtime: &dyn ValueRuntime,
    ) -> ValueResult<ValueHandle> {
        // Compare function validation precedes receiver/length consumption.
        let compare = arguments.first().cloned().unwrap_or(self.undefined()?);
        let compare = if self.kind(&compare)? == ValueKind::Undefined {
            None
        } else if self.kind(&compare)? == ValueKind::Function {
            Some(compare)
        } else {
            return Err(ValueError::TypeError("Array.sort comparator is not callable".into()));
        };
        self.loop_guard_array_receiver(receiver)?;
        let length = self.length_of_array_like(receiver, runtime)?;
        let mut values = Vec::new();
        // SortIndexedProperties skips holes but includes inherited values.
        for index in 0..length {
            let key = PropertyKey::from(WireString::from(index.to_string()));
            if self.has(receiver, &key, runtime)? {
                values.try_reserve(1).map_err(|_| ValueError::Allocation)?;
                values.push(self.get(receiver, &key, runtime)?);
            }
        }
        // Stable, fallible comparisons. Do not pre-convert/copy values: native
        // conversion/compare callbacks must execute on original handles.
        for right in 1..values.len() {
            let mut index = right;
            while index > 0 && self.sort_compare(&values[index - 1], &values[index], compare.as_ref(), runtime)? > 0.0 {
                values.swap(index - 1, index);
                index -= 1;
            }
        }
        for (index, value) in values.iter().enumerate() {
            self.assign(receiver, WireString::from(index.to_string()).into(), value.clone(), runtime)?;
        }
        for index in values.len() as u64..length {
            if !self.delete_own(receiver, &WireString::from(index.to_string()).into(), runtime)? {
                return Err(ValueError::TypeError("Array.sort cannot delete indexed property".into()));
            }
        }
        self.adopt(receiver)
    }
    fn sort_compare(
        self: &Arc<Self>,
        left: &ValueHandle,
        right: &ValueHandle,
        compare: Option<&ValueHandle>,
        runtime: &dyn ValueRuntime,
    ) -> ValueResult<f64> {
        let left_undefined = self.kind(left)? == ValueKind::Undefined;
        let right_undefined = self.kind(right)? == ValueKind::Undefined;
        if left_undefined && right_undefined {
            return Ok(0.0);
        }
        if left_undefined {
            return Ok(1.0);
        }
        if right_undefined {
            return Ok(-1.0);
        }
        if let Some(compare) = compare {
            let compared = self.call(compare, &self.undefined()?, &[left.clone(), right.clone()], runtime)?;
            let compared = self.to_number(&compared, runtime)?;
            return Ok(if compared.is_nan() { 0.0 } else { compared });
        }
        let left = self.implicit_text(left, runtime)?;
        let right = self.implicit_text(right, runtime)?;
        Ok(match left.units().cmp(right.units()) {
            std::cmp::Ordering::Less => -1.0,
            std::cmp::Ordering::Equal => 0.0,
            std::cmp::Ordering::Greater => 1.0,
        })
    }
}
