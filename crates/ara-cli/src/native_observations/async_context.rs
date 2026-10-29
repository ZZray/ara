//! Context/epoch observations inside the existing Native realm coarse helper.
//! Executes the installed compiled storage methods and effective runtime chain.
//! Captured-scope restoration is synchronous; this is not an await/Promise gate.

use ara_cli::native_realm::{NativeRealm, context::NativeScope};
use ara_rpc::value_heap::{
    CapabilityId, CompiledFunctionForm, CompiledFunctionSpec, OpaqueKind, OpaqueOperation, PropertyKey, RuntimeReply,
    RuntimeRequest, ValueError, ValueHandle, ValueHeap, ValueRuntime, ValueSnapshot,
};
use std::sync::{
    Arc,
    atomic::{AtomicUsize, Ordering},
};

type Result<T> = std::result::Result<T, ValueError>;

// call_with_local_callback supplies [callback, ...tail]. This trusted adapter
// calls the actual installed storage.run with its original [store, callback]
// order, so the operation-local callback must survive the inner scope wrapper.
static RUN_STORAGE: CompiledFunctionSpec = CompiledFunctionSpec {
    source: "controlled installed Native storage.run argument-order adapter",
    name: "runControlledStorageCallback",
    length: 1,
    form: CompiledFunctionForm::Builtin,
    capture_count: 3,
    body: |heap, captures, _, arguments, effective| {
        let callback = arguments.first().ok_or(ValueError::InvalidNode)?;
        heap.call(&captures[1], &captures[0], &[captures[2].clone(), callback.clone()], effective)
    },
};

fn adapter(
    heap: &Arc<ValueHeap>,
    storage: &ValueHandle,
    run: &ValueHandle,
    store: &ValueHandle,
) -> Result<ValueHandle> {
    heap.fresh_compiled_function(&RUN_STORAGE, &[storage.clone(), run.clone(), store.clone()])
}

fn assert_store(
    heap: &Arc<ValueHeap>,
    storage: &ValueHandle,
    get_store: &ValueHandle,
    expected: &ValueHandle,
    effective: &dyn ValueRuntime,
) -> Result<()> {
    assert!(heap.call(get_store, storage, &[], effective)?.same_identity(expected));
    Ok(())
}

fn assert_undefined(heap: &Arc<ValueHeap>, value: &ValueHandle) -> Result<()> {
    assert!(matches!(heap.snapshot(value)?, ValueSnapshot::Undefined));
    Ok(())
}

/// Controlled opaque Get must receive the current inner ScopedRuntime.
/// The ordinary resolve path deliberately cannot satisfy this probe.
struct EffectiveProbe<'a> {
    base: &'a dyn ValueRuntime,
    capability: CapabilityId,
    target: &'a ValueHandle,
    storage: &'a ValueHandle,
    get_store: &'a ValueHandle,
    expected: &'a ValueHandle,
    calls: AtomicUsize,
}

impl ValueRuntime for EffectiveProbe<'_> {
    fn resolve(&self, request: RuntimeRequest<'_>) -> Result<RuntimeReply> {
        if request.capability() == self.capability {
            return Err(ValueError::UnsupportedOperation("Context probe requires the effective opaque runtime"));
        }
        self.base.resolve(request)
    }

    fn resolve_effective(&self, request: RuntimeRequest<'_>, effective: &dyn ValueRuntime) -> Result<RuntimeReply> {
        if request.capability() != self.capability {
            return self.base.resolve_effective(request, effective);
        }
        match request {
            RuntimeRequest::Opaque {
                target,
                receiver,
                operation: OpaqueOperation::Get(PropertyKey::String(key)),
                ..
            } if key.equals_ascii("inner") => {
                assert!(target.same_identity(self.target));
                assert!(receiver.same_identity(self.target));
                let observed = target.heap().call(self.get_store, self.storage, &[], effective)?;
                assert!(observed.same_identity(self.expected));
                self.calls.fetch_add(1, Ordering::Relaxed);
                Ok(RuntimeReply::Value(observed))
            }
            _ => Err(ValueError::UnsupportedOperation("Unexpected controlled context probe request")),
        }
    }

    fn resolve_call(
        &self,
        capability: CapabilityId,
        receiver: &ValueHandle,
        arguments: &[ValueHandle],
        effective: &dyn ValueRuntime,
    ) -> Result<RuntimeReply> {
        self.base.resolve_call(capability, receiver, arguments, effective)
    }

    fn resolve_local_call(
        &self,
        token: u64,
        receiver: &ValueHandle,
        arguments: &[ValueHandle],
        effective: &dyn ValueRuntime,
    ) -> Result<RuntimeReply> {
        self.base.resolve_local_call(token, receiver, arguments, effective)
    }
}

pub fn observe(realm: &Arc<NativeRealm>) -> Result<Vec<ValueHandle>> {
    let heap = realm.values.heap();
    let base = realm.values.runtime();
    let domain = realm.context();
    let storage = domain.prepare_storage(heap)?;
    let get_store = heap.get(&storage, &"getStore".into(), base)?;
    let storage_run = heap.get(&storage, &"run".into(), base)?;
    let undefined = heap.undefined()?;
    let a = heap.object()?;
    let b = heap.object()?;
    let c = heap.object()?;
    let original_throw = heap.object()?;
    let enter_a = adapter(heap, &storage, &storage_run, &a)?;
    let enter_b = adapter(heap, &storage, &storage_run, &b)?;
    let enter_c = adapter(heap, &storage, &storage_run, &c)?;

    assert_undefined(heap, &heap.call(&get_store, &storage, &[], base)?)?;
    assert!(domain.capture_scope(heap, base)?.is_none());
    assert!(matches!(
        heap.call(&storage_run, &storage, &[a.clone(), get_store.clone()], base),
        Err(ValueError::UnsupportedOperation(_))
    ));
    assert!(domain.capture_scope(heap, base)?.is_none(), "A base call cannot create a host Run");

    let host_run = realm.begin_context_run()?;
    assert!(matches!(realm.begin_context_run(), Err(ValueError::UnsupportedOperation(_))));
    assert!(domain.capture_scope(heap, base)?.is_none(), "An active driver does not grant the base runtime a frame");
    assert!(matches!(
        heap.call(&storage_run, &storage, &[a.clone(), get_store.clone()], base),
        Err(ValueError::UnsupportedOperation(_))
    ));

    let capability = CapabilityId(9_728_401);
    let probe = heap.opaque(OpaqueKind::Object, capability, 1)?;
    let parent = EffectiveProbe {
        base,
        capability,
        target: &probe,
        storage: &storage,
        get_store: &get_store,
        expected: &a,
        calls: AtomicUsize::new(0),
    };
    let root_scope = host_run.root_scope();
    let root = host_run.scoped(root_scope, &parent)?;
    assert!(domain.capture_scope(heap, &root)?.is_some());
    assert_undefined(heap, &heap.call(&get_store, &storage, &[], &root)?)?;

    let mut captured_a: Option<NativeScope> = None;
    let mut captured_b: Option<NativeScope> = None;
    let mut callbacks = 0;
    let mut callback_a = |receiver: &ValueHandle, arguments: &[ValueHandle], effective: &dyn ValueRuntime| {
        assert_undefined(heap, receiver)?;
        assert!(arguments.is_empty());
        callbacks += 1;
        assert_store(heap, &storage, &get_store, &a, effective)?;
        captured_a = domain.capture_scope(heap, effective)?;
        assert!(captured_a.is_some());
        let observed = heap.get(&probe, &"inner".into(), effective)?;
        assert!(observed.same_identity(&a));

        let mut callback_b = |receiver: &ValueHandle, arguments: &[ValueHandle], inner: &dyn ValueRuntime| {
            assert_undefined(heap, receiver)?;
            assert!(arguments.is_empty());
            callbacks += 1;
            assert_store(heap, &storage, &get_store, &b, inner)?;
            captured_b = domain.capture_scope(heap, inner)?;
            assert!(captured_b.is_some());
            Ok(b.clone())
        };
        let output = heap.call_with_local_callback(&enter_b, &undefined, &mut callback_b, &[], effective)?;
        assert!(output.same_identity(&b));
        assert_store(heap, &storage, &get_store, &a, effective)?;

        // Restore the captured inner B frame using this invocation's exact
        // effective parent. This models the driver's later synchronous entry,
        // without claiming that a Host await has happened.
        let restored_b =
            host_run.scoped(captured_b.expect("B callback must capture its actual inner frame"), effective)?;
        assert_store(heap, &storage, &get_store, &b, &restored_b)?;
        assert_store(heap, &storage, &get_store, &a, effective)?;

        let mut throwing_b = |_: &ValueHandle, _: &[ValueHandle], inner: &dyn ValueRuntime| {
            callbacks += 1;
            assert_store(heap, &storage, &get_store, &b, inner)?;
            Err(ValueError::Thrown(original_throw.clone()))
        };
        match heap.call_with_local_callback(&enter_b, &undefined, &mut throwing_b, &[], effective) {
            Err(ValueError::Thrown(reason)) => assert!(reason.same_identity(&original_throw)),
            other => panic!("Installed storage.run must preserve the original Thrown identity: {other:?}"),
        }
        assert_store(heap, &storage, &get_store, &a, effective)?;
        Ok(a.clone())
    };
    assert!(heap.call_with_local_callback(&enter_a, &undefined, &mut callback_a, &[], &root)?.same_identity(&a));
    assert_undefined(heap, &heap.call(&get_store, &storage, &[], &root)?)?;

    let mut sibling_c = |_: &ValueHandle, _: &[ValueHandle], effective: &dyn ValueRuntime| {
        callbacks += 1;
        assert_store(heap, &storage, &get_store, &c, effective)?;
        Ok(c.clone())
    };
    assert!(heap.call_with_local_callback(&enter_c, &undefined, &mut sibling_c, &[], &root)?.same_identity(&c));
    assert_eq!(callbacks, 4, "Every local callback executes through its retained operation parent exactly once");
    assert_eq!(parent.calls.load(Ordering::Relaxed), 1);
    assert_undefined(heap, &heap.call(&get_store, &storage, &[], &root)?)?;
    let old_a = captured_a.expect("A callback must capture its actual inner frame");
    let old_b = captured_b.expect("B callback must capture its actual inner frame");
    let restored_a = host_run.scoped(old_a, &parent)?;
    assert_store(heap, &storage, &get_store, &a, &restored_a)?;
    let restored_b = host_run.scoped(old_b, &parent)?;
    assert_store(heap, &storage, &get_store, &b, &restored_b)?;
    assert_undefined(heap, &heap.call(&get_store, &storage, &[], &root)?)?;

    let copied = heap.object()?;
    heap.define_own_data(&copied, "getStore".into(), get_store.clone(), base)?;
    heap.define_own_data(&copied, "run".into(), storage_run.clone(), base)?;
    assert!(matches!(heap.call(&get_store, &copied, &[], &root), Err(ValueError::UnsupportedOperation(_))));
    assert!(matches!(
        heap.call(&storage_run, &copied, &[a.clone(), get_store.clone()], &root),
        Err(ValueError::UnsupportedOperation(_))
    ));

    let foreign = NativeRealm::prepare()?;
    let foreign_run = foreign.begin_context_run()?;
    assert!(matches!(domain.prepare_storage(foreign.values.heap()), Err(ValueError::ForeignHeap)));
    assert!(matches!(domain.capture_scope(heap, foreign.values.runtime()), Err(ValueError::ForeignHeap)));
    assert!(host_run.scoped(foreign_run.root_scope(), &parent).is_err());
    assert!(foreign_run.scoped(root_scope, foreign.values.runtime()).is_err());
    let foreign_root = foreign_run.scoped(foreign_run.root_scope(), foreign.values.runtime())?;
    assert!(heap.call(&get_store, &storage, &[], &foreign_root).is_err());

    let revoker = host_run.revoker();
    revoker.revoke();
    assert!(matches!(host_run.scoped(old_a, &parent), Err(ValueError::ExpiredCapability(_))));
    assert!(matches!(host_run.scoped(old_b, &parent), Err(ValueError::ExpiredCapability(_))));
    assert!(matches!(heap.call(&get_store, &storage, &[], &root), Err(ValueError::ExpiredCapability(_))));
    assert!(matches!(heap.call(&get_store, &storage, &[], &restored_a), Err(ValueError::ExpiredCapability(_))));
    assert!(matches!(domain.capture_scope(heap, &restored_b), Err(ValueError::ExpiredCapability(_))));
    assert!(
        matches!(realm.begin_context_run(), Err(ValueError::UnsupportedOperation(_))),
        "Revoke keeps the driver exclusive"
    );
    assert_undefined(heap, &heap.call(&get_store, &storage, &[], base)?)?;
    assert!(domain.capture_scope(heap, base)?.is_none());

    drop(restored_b);
    drop(restored_a);
    drop(root);
    drop(host_run);
    let next_run = realm.begin_context_run()?;
    assert!(matches!(next_run.scoped(old_a, &parent), Err(ValueError::ExpiredCapability(_))));
    assert!(matches!(next_run.scoped(old_b, &parent), Err(ValueError::ExpiredCapability(_))));
    let fresh_root = next_run.scoped(next_run.root_scope(), &parent)?;
    assert_undefined(heap, &heap.call(&get_store, &storage, &[], &fresh_root)?)?;
    revoker.revoke();
    assert_undefined(heap, &heap.call(&get_store, &storage, &[], &fresh_root)?)?;
    assert!(domain.capture_scope(heap, &fresh_root)?.is_some(), "An old revoker cannot close the next Run");
    assert!(domain.capture_scope(heap, base)?.is_none());
    drop(fresh_root);
    drop(next_run);

    let isolated = NativeRealm::prepare()?;
    let isolated_domain = isolated.context().clone();
    let isolated_run = isolated.begin_context_run()?;
    let isolated_revoker = isolated_run.revoker();
    let isolated_heap = Arc::downgrade(isolated.values.heap());
    let isolated_storage = isolated_domain.prepare_storage(isolated.values.heap())?;
    drop(isolated_storage);
    drop(isolated_run);
    drop(isolated);
    assert!(isolated_heap.upgrade().is_none(), "A retained context domain/revoker must not lease the heap");
    isolated_revoker.revoke();
    assert!(Arc::strong_count(&isolated_domain) >= 1);
    assert!(isolated_heap.upgrade().is_none());

    Ok(vec![storage, a, b, c, original_throw, probe, copied])
}
