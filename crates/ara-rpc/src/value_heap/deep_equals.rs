//! Same-graph comparison for fixed MCP structuredContentAlreadyInText.
//! Reference: Root's pinned Bun 1.4.0 selected non-strict observations.
//! Input handles are retained, never JSON/serde copied. This explicit stack
//! permits deep graph traversal without recursive Rust calls.
use super::*;

#[derive(Clone, Copy, PartialEq, Eq)]
enum ComparisonContainer {
    Object,
    Array,
    Other,
}
enum Frame {
    Compare(ValueHandle, ValueHandle),
    Object {
        left: ValueHandle,
        right: ValueHandle,
        left_keys: Vec<PropertyKey>,
        right_keys: Vec<PropertyKey>,
        index: usize,
        extras: bool,
    },
    Array {
        left: ValueHandle,
        right: ValueHandle,
        indices: Vec<u32>,
        index: usize,
    },
    Leave,
}
fn push(frames: &mut Vec<Frame>, frame: Frame) -> ValueResult<()> {
    frames.try_reserve(1).map_err(|_| ValueError::Allocation)?;
    frames.push(frame);
    Ok(())
}
impl ValueHeap {
    fn comparison_container(&self, value: &ValueHandle) -> ValueResult<ComparisonContainer> {
        let node = self.admit(value)?;
        let arena = self.lock()?;
        Ok(match arena.get(node)? {
            Node::Object(_) | Node::Opaque { kind: OpaqueKind::Object, .. } => ComparisonContainer::Object,
            Node::Array { .. } | Node::Opaque { kind: OpaqueKind::Array, .. } => ComparisonContainer::Array,
            _ => ComparisonContainer::Other,
        })
    }
    /// Selected Bun non-strict comparison consumed by fixed MCP. Getters use
    /// the effective runtime outside arena locks. Only the active traversal
    /// path enforces cycle correspondence; aliases may match equal copies.
    pub fn mcp_deep_equals(
        self: &Arc<Self>,
        left: &ValueHandle,
        right: &ValueHandle,
        runtime: &dyn ValueRuntime,
    ) -> ValueResult<bool> {
        self.admit(left)?;
        self.admit(right)?;
        let mut frames = Vec::new();
        let mut path: Vec<(NodeId, NodeId)> = Vec::new();
        push(&mut frames, Frame::Compare(left.clone(), right.clone()))?;
        while let Some(frame) = frames.pop() {
            match frame {
                Frame::Compare(left, right) => {
                    if self.object_is(&left, &right)? {
                        continue;
                    }
                    let kind = self.kind(&left)?;
                    if kind != self.kind(&right)? || !matches!(kind, ValueKind::Object | ValueKind::Array) {
                        return Ok(false);
                    }
                    let container = self.comparison_container(&left)?;
                    if container == ComparisonContainer::Other || container != self.comparison_container(&right)? {
                        return Ok(false);
                    }
                    let left_id = self.admit(&left)?;
                    let right_id = self.admit(&right)?;
                    let previous_left = path.iter().position(|(a, _)| *a == left_id);
                    let previous_right = path.iter().position(|(_, b)| *b == right_id);
                    if previous_left.is_some() || previous_right.is_some() {
                        if previous_left.is_none() || previous_left != previous_right {
                            return Ok(false);
                        }
                        continue;
                    }
                    path.try_reserve(1).map_err(|_| ValueError::Allocation)?;
                    path.push((left_id, right_id));
                    push(&mut frames, Frame::Leave)?;
                    if container == ComparisonContainer::Object {
                        let left_keys = self.own_enumerable_keys(&left, runtime)?;
                        let right_keys = self.own_enumerable_keys(&right, runtime)?;
                        push(
                            &mut frames,
                            Frame::Object { left, right, left_keys, right_keys, index: 0, extras: false },
                        )?;
                    } else {
                        // Non-strict arrays ignore named/symbol extras and
                        // trailing holes/undefined. Enumerate actual indices,
                        // avoiding dense allocation for arbitrary sparse length.
                        let mut indices = Vec::new();
                        for owner in [&left, &right] {
                            for key in self.own_keys(owner, runtime)? {
                                if let Some(index) = array_index(&key) {
                                    if !indices.contains(&index) {
                                        indices.try_reserve(1).map_err(|_| ValueError::Allocation)?;
                                        indices.push(index);
                                    }
                                }
                            }
                        }
                        indices.sort_unstable();
                        push(&mut frames, Frame::Array { left, right, indices, index: 0 })?;
                    }
                }
                Frame::Object { left, right, left_keys, right_keys, index, extras: false } => {
                    if index == left_keys.len() {
                        push(
                            &mut frames,
                            Frame::Object { left, right, left_keys, right_keys, index: 0, extras: true },
                        )?;
                        continue;
                    }
                    let key = &left_keys[index];
                    // No key-count preflight: reach the left getter before
                    // missing-key failure, both getters before an extra key.
                    let a = self.get(&left, key, runtime)?;
                    let b = if right_keys.contains(key) { self.get(&right, key, runtime)? } else { self.undefined()? };
                    push(
                        &mut frames,
                        Frame::Object { left, right, left_keys, right_keys, index: index + 1, extras: false },
                    )?;
                    push(&mut frames, Frame::Compare(a, b))?;
                }
                Frame::Object { left, right, left_keys, right_keys, mut index, extras: true } => {
                    while index < right_keys.len() && left_keys.contains(&right_keys[index]) {
                        index += 1;
                    }
                    if index == right_keys.len() {
                        continue;
                    }
                    let value = self.get(&right, &right_keys[index], runtime)?;
                    if !matches!(self.snapshot(&value)?, ValueSnapshot::Undefined) {
                        return Ok(false);
                    }
                    push(
                        &mut frames,
                        Frame::Object { left, right, left_keys, right_keys, index: index + 1, extras: true },
                    )?;
                }
                Frame::Array { left, right, indices, index } => {
                    if index == indices.len() {
                        continue;
                    }
                    let key = index_key(indices[index]);
                    let a = match self.own_get(&left, &key, runtime)? {
                        PropertyPresence::Present(value) => value,
                        PropertyPresence::Missing => self.undefined()?,
                    };
                    let b = match self.own_get(&right, &key, runtime)? {
                        PropertyPresence::Present(value) => value,
                        PropertyPresence::Missing => self.undefined()?,
                    };
                    push(&mut frames, Frame::Array { left, right, indices, index: index + 1 })?;
                    push(&mut frames, Frame::Compare(a, b))?;
                }
                Frame::Leave => {
                    path.pop();
                }
            }
        }
        Ok(true)
    }
}
