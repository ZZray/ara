//! Checked graph identity for deferred consumers, without an arena lease.
//! The owner supplies the current heap on restoration; this is not authority.

use super::{NodeId, ValueError, ValueHandle, ValueHeap, ValueResult};
use std::sync::Arc;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct DeferredValue {
    heap_token: u64,
    node: NodeId,
}

impl ValueHeap {
    pub fn capture_deferred(&self, value: &ValueHandle) -> ValueResult<DeferredValue> {
        let node = self.admit(value)?;
        self.lock()?.get(node)?;
        Ok(DeferredValue { heap_token: self.token, node })
    }

    pub fn restore_deferred(self: &Arc<Self>, edge: &DeferredValue) -> ValueResult<ValueHandle> {
        if edge.heap_token != self.token {
            return Err(ValueError::ForeignHeap);
        }
        self.lock()?.get(edge.node)?;
        Ok(self.handle(edge.node))
    }
}
