//! OMP b91db66e schema/stamps.ts: weak identity side tables.
//! Caller-owned graphs never receive bookkeeping properties. DeferredValue
//! carries no heap lease; dead heap tables are pruned on the next operation.
/*
MIT License

Copyright (c) 2025 Mario Zechner
Copyright (c) 2025-2026 Can Bölük
Copyright (c) 2026 Stencil Labs, Inc.

Permission is hereby granted, free of charge, to any person obtaining a copy
of this software and associated documentation files (the "Software"), to deal
in the Software without restriction, including without limitation the rights
to use, copy, modify, merge, publish, distribute, sublicense, and/or sell
copies of the Software, and to permit persons to whom the Software is
furnished to do so, subject to the following conditions:

The above copyright notice and this permission notice shall be included in all
copies or substantial portions of the Software.

THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF ANY KIND, EXPRESS OR
IMPLIED, INCLUDING BUT NOT LIMITED TO THE WARRANTIES OF MERCHANTABILITY,
FITNESS FOR A PARTICULAR PURPOSE AND NONINFRINGEMENT. IN NO EVENT SHALL THE
AUTHORS OR COPYRIGHT HOLDERS BE LIABLE FOR ANY CLAIM, DAMAGES OR OTHER
LIABILITY, WHETHER IN AN ACTION OF CONTRACT, TORT OR OTHERWISE, ARISING FROM,
OUT OF OR IN CONNECTION WITH THE SOFTWARE OR THE USE OR OTHER DEALINGS IN THE
SOFTWARE.
*/
use crate::graph_values::{GraphExecution, GraphValueFactory};
use ara_rpc::value_heap::{DeferredValue, PropertyKey, ValueError, ValueHandle, ValueHeap, ValueKind};
use std::collections::HashMap;
use std::sync::{Arc, LazyLock, Mutex, Weak};
use std::sync::atomic::{AtomicU64, Ordering};

static EPOCH_BITS: AtomicU64 = AtomicU64::new(0);
#[derive(Default)]
struct Tables {
    heap: Weak<ValueHeap>,
    memos: HashMap<(DeferredValue, PropertyKey), DeferredValue>,
    epochs: HashMap<DeferredValue, f64>,
    depths: HashMap<DeferredValue, f64>,
}
// The current ValueHeap arena retains nodes until heap release, as does its
// existing WeakSet. Tokens never retain the heap or create an Arc cycle.
static TABLES: LazyLock<Mutex<HashMap<u64, Tables>>> = LazyLock::new(|| Mutex::new(HashMap::new()));
fn table<T>(heap: &Arc<ValueHeap>, body: impl FnOnce(&mut Tables) -> T) -> Result<T, ValueError> {
    let mut tables = TABLES.lock().map_err(|_| ValueError::PoisonedHeap)?;
    tables.retain(|_, entry| entry.heap.strong_count() != 0);
    let entry = tables.entry(heap.heap_token()).or_insert_with(|| Tables { heap: Arc::downgrade(heap), ..Tables::default() });
    Ok(body(entry))
}
fn target(execution: GraphExecution<'_>, value: &ValueHandle) -> Result<DeferredValue, ValueError> {
    let heap = execution.heap();
    heap.adopt(value)?;
    if !matches!(heap.kind(value)?, ValueKind::Object | ValueKind::Array | ValueKind::Function) {
        return Err(ValueError::TypeError("Invalid value used as weak map key".into()));
    }
    heap.capture_deferred(value)
}
pub(crate) fn epoch_next() -> f64 {
    let mut previous = EPOCH_BITS.load(Ordering::Relaxed);
    loop {
        let next = (f64::from_bits(previous) + 1.0).to_bits();
        match EPOCH_BITS.compare_exchange_weak(previous, next, Ordering::Relaxed, Ordering::Relaxed) {
            Ok(_) => return f64::from_bits(next), Err(actual) => previous = actual,
        }
    }
}
pub(crate) fn stamp<F>(factory: &GraphValueFactory, value: &ValueHandle, key: &PropertyKey, compute: F) -> Result<ValueHandle, ValueError>
where F: FnOnce(&ValueHandle) -> Result<ValueHandle, ValueError> {
    stamp_with_execution(GraphExecution::from_factory(factory), value, key, compute)
}
pub(crate) fn stamp_with_execution<F>(execution: GraphExecution<'_>, value: &ValueHandle, key: &PropertyKey, compute: F) -> Result<ValueHandle, ValueError>
where F: FnOnce(&ValueHandle) -> Result<ValueHandle, ValueError> {
    let heap = execution.heap();
    let identity = target(execution, value)?;
    let existing = table(heap, |slots| slots.memos.get(&(identity, key.clone())).copied())?;
    if let Some(existing) = existing {
        let existing = heap.restore_deferred(&existing)?;
        if heap.kind(&existing)? != ValueKind::Undefined { return Ok(existing); }
    }
    // Neither compute nor any runtime observation runs under the table mutex.
    let computed = compute(value)?;
    let captured = heap.capture_deferred(&computed)?;
    table(heap, |slots| slots.memos.insert((identity, key.clone()), captured))?;
    Ok(computed)
}
pub(crate) fn once_with_execution(execution: GraphExecution<'_>, value: &ValueHandle, epoch: f64) -> Result<bool, ValueError> {
    let identity = target(execution, value)?;
    table(execution.heap(), |slots| {
        if slots.epochs.get(&identity).is_some_and(|current| *current >= epoch) { return false; }
        slots.epochs.insert(identity, epoch); true
    })
}
pub(crate) fn enter_with_execution(execution: GraphExecution<'_>, value: &ValueHandle) -> Result<bool, ValueError> {
    let identity = target(execution, value)?;
    table(execution.heap(), |slots| {
        if slots.depths.get(&identity).is_some_and(|current| *current != 0.0) { return false; }
        slots.depths.insert(identity, 1.0); true
    })
}
pub(crate) fn exit_with_execution(execution: GraphExecution<'_>, value: &ValueHandle) -> Result<(), ValueError> {
    let identity = target(execution, value)?;
    table(execution.heap(), |slots| { if let Some(current) = slots.depths.get_mut(&identity) { *current -= 1.0; } })
}

#[cfg(test)]
pub(crate) fn observe_weak_stamp_family() {
    use ara_rpc::value_heap::NoRuntime;
    let weak = {
        let heap = ValueHeap::new().unwrap();
        let ex = GraphExecution::new(&heap, &NoRuntime);
        let host = heap.object().unwrap();
        let key = heap.module_symbol("append-upgrade-test", "cache").unwrap();
        let sentinel = heap.object().unwrap();
        let mut computes = 0;
        let cached = stamp_with_execution(ex, &host, &key, |_| { computes += 1; Ok(sentinel.clone()) }).unwrap();
        assert!(cached.same_identity(&sentinel));
        heap.set_extensible(&host, false, &NoRuntime).unwrap();
        assert!(stamp_with_execution(ex, &host, &key, |_| panic!("cached after freeze")).unwrap().same_identity(&cached));
        assert!(heap.own_keys(&host, &NoRuntime).unwrap().is_empty());
        assert_eq!(computes, 1);
        let fresh = heap.object().unwrap();
        heap.set_extensible(&fresh, false, &NoRuntime).unwrap();
        assert!(stamp_with_execution(ex, &fresh, &key, |_| Ok(sentinel.clone())).unwrap().same_identity(&sentinel));
        assert!(stamp_with_execution(ex, &fresh, &key, |_| panic!("sealed cache")).unwrap().same_identity(&sentinel));
        let undefined_key = heap.module_symbol("append-upgrade-test", "undefined").unwrap();
        for _ in 0..2 { stamp_with_execution(ex, &host, &undefined_key, |_| { computes += 1; heap.undefined() }).unwrap(); }
        assert_eq!(computes, 3);
        let throw_key = heap.module_symbol("append-upgrade-test", "throw").unwrap();
        assert!(matches!(stamp_with_execution(ex, &host, &throw_key, |_| Err(ValueError::Thrown(sentinel.clone()))), Err(ValueError::Thrown(value)) if value.same_identity(&sentinel)));
        assert!(stamp_with_execution(ex, &host, &throw_key, |_| Ok(sentinel.clone())).unwrap().same_identity(&sentinel));
        // Reentrant compute must run without a table lock; outer publication wins.
        let reentrant = heap.module_symbol("append-upgrade-test", "reentrant").unwrap();
        stamp_with_execution(ex, &host, &reentrant, |_| {
            stamp_with_execution(ex, &host, &reentrant, |_| heap.number(1.0))?;
            heap.number(2.0)
        }).unwrap();
        assert!(matches!(heap.snapshot(&stamp_with_execution(ex, &host, &reentrant, |_| unreachable!()).unwrap()).unwrap(), ara_rpc::value_heap::ValueSnapshot::Number(2.0)));
        let epoch = epoch_next();
        assert!(once_with_execution(ex, &host, epoch).unwrap());
        assert!(!once_with_execution(ex, &host, epoch).unwrap());
        assert!(once_with_execution(ex, &host, epoch_next()).unwrap());
        assert!(enter_with_execution(ex, &host).unwrap());
        assert!(!enter_with_execution(ex, &host).unwrap());
        exit_with_execution(ex, &host).unwrap();
        assert!(enter_with_execution(ex, &host).unwrap());
        exit_with_execution(ex, &host).unwrap();
        assert!(heap.own_keys(&host, &NoRuntime).unwrap().is_empty());
        Arc::downgrade(&heap)
    };
    assert!(weak.upgrade().is_none(), "memo entries must not lease their heap");
}
