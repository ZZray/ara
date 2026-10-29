//! Promise/queue observations in the existing realm coarse entrance.
//! No Host await, RPC effect owner or production driver is claimed.

use ara_cli::native_realm::{
    NativeRealm,
    promise::{NativeDrain, NativePromiseRun, NativePromiseState},
};
use ara_rpc::value_heap::{
    CallableTarget, CompiledFunctionForm, CompiledFunctionSpec, PropertyDescriptor, ValueError, ValueHandle, ValueHeap,
    ValueRuntime, ValueSnapshot,
};
use std::sync::Arc;
type Result<T> = std::result::Result<T, ValueError>;

static SAVE: CompiledFunctionSpec = CompiledFunctionSpec {
    source: "controlled actual Promise executor retains both original resolving functions",
    name: "save",
    length: 2,
    form: CompiledFunctionForm::Builtin,
    capture_count: 1,
    body: |heap, captures, _, args, runtime| {
        assert_eq!(args.len(), 2);
        heap.define_own_data(&captures[0], "resolve".into(), args[0].clone(), runtime)?;
        heap.define_own_data(&captures[0], "reject".into(), args[1].clone(), runtime)?;
        heap.undefined()
    },
};
static RETURN: CompiledFunctionSpec = CompiledFunctionSpec {
    source: "controlled Promise handler original return identity",
    name: "return",
    length: 1,
    form: CompiledFunctionForm::Builtin,
    capture_count: 1,
    body: |_, captures, _, _, _| Ok(captures[0].clone()),
};
static THROW: CompiledFunctionSpec = CompiledFunctionSpec {
    source: "controlled Promise handler original thrown reason",
    name: "throw",
    length: 1,
    form: CompiledFunctionForm::Builtin,
    capture_count: 1,
    body: |_, captures, _, _, _| Err(ValueError::Thrown(captures[0].clone())),
};
static GET: CompiledFunctionSpec = CompiledFunctionSpec {
    source: "controlled then/species getter exact observation count and identity",
    name: "get",
    length: 0,
    form: CompiledFunctionForm::Builtin,
    capture_count: 2,
    body: |heap, captures, _, _, runtime| {
        heap.array_push(&captures[0], &[heap.number(1.0)?], runtime)?;
        Ok(captures[1].clone())
    },
};
static GET_THROW: CompiledFunctionSpec = CompiledFunctionSpec {
    source: "controlled then/species/constructor getter preserves original throw",
    name: "get",
    length: 0,
    form: CompiledFunctionForm::Builtin,
    capture_count: 2,
    body: |heap, captures, _, _, runtime| {
        heap.array_push(&captures[0], &[heap.number(1.0)?], runtime)?;
        Err(ValueError::Thrown(captures[1].clone()))
    },
};
static THENABLE: CompiledFunctionSpec = CompiledFunctionSpec {
    source: "controlled dynamic then calls fresh resolve then reject and throws after resolution",
    name: "then",
    length: 2,
    form: CompiledFunctionForm::Builtin,
    capture_count: 3,
    body: |heap, captures, _, args, runtime| {
        assert_eq!(args.len(), 2);
        heap.array_push(&captures[0], &[heap.string("then".into())?], runtime)?;
        heap.call(&args[0], &heap.undefined()?, &[captures[1].clone()], runtime)?;
        heap.call(&args[1], &heap.undefined()?, &[captures[2].clone()], runtime)?;
        Err(ValueError::Thrown(captures[2].clone()))
    },
};
static SPECIES: CompiledFunctionSpec = CompiledFunctionSpec {
    source: "controlled genuine compiled species constructor returns actual non-native capability object",
    name: "Species",
    length: 1,
    form: CompiledFunctionForm::Ordinary,
    capture_count: 3,
    body: |heap, captures, _, args, runtime| {
        heap.call(&args[0], &heap.undefined()?, &[captures[1].clone(), captures[2].clone()], runtime)?;
        Ok(captures[0].clone())
    },
};
static RECORD: CompiledFunctionSpec = CompiledFunctionSpec {
    source: "controlled custom capability resolving function records original argument",
    name: "resolve",
    length: 1,
    form: CompiledFunctionForm::Builtin,
    capture_count: 1,
    body: |heap, captures, _, args, runtime| {
        heap.array_push(&captures[0], args, runtime)?;
        heap.undefined()
    },
};
static DUPLICATE_SPECIES: CompiledFunctionSpec = CompiledFunctionSpec {
    source: "controlled compiled species attempts to bind NewPromiseCapability executor twice",
    name: "DuplicateSpecies",
    length: 1,
    form: CompiledFunctionForm::Ordinary,
    capture_count: 3,
    body: |heap, captures, _, args, runtime| {
        for _ in 0..2 {
            heap.call(&args[0], &heap.undefined()?, &[captures[1].clone(), captures[2].clone()], runtime)?;
        }
        Ok(captures[0].clone())
    },
};
static IN_STORE: CompiledFunctionSpec = CompiledFunctionSpec {
    source: "controlled installed storage.run local-callback argument adapter",
    name: "scope",
    length: 1,
    form: CompiledFunctionForm::Builtin,
    capture_count: 3,
    body: |heap, captures, _, args, runtime| {
        heap.call(&captures[1], &captures[0], &[captures[2].clone(), args[0].clone()], runtime)
    },
};
static SCOPED_HANDLER: CompiledFunctionSpec = CompiledFunctionSpec {
    source: "controlled Promise reaction reads actual storage.getStore in its registration frame",
    name: "scoped",
    length: 1,
    form: CompiledFunctionForm::Builtin,
    capture_count: 5,
    body: |heap, captures, _, _, runtime| {
        let store = heap.call(&captures[1], &captures[0], &[], runtime)?;
        assert!(store.same_identity(&captures[2]));
        heap.array_push(&captures[3], &[store], runtime)?;
        Ok(captures[4].clone())
    },
};
fn member(heap: &Arc<ValueHeap>, object: &ValueHandle, name: &str, runtime: &dyn ValueRuntime) -> Result<ValueHandle> {
    heap.get(object, &name.into(), runtime)
}
fn getter(
    heap: &Arc<ValueHeap>,
    object: &ValueHandle,
    key: ara_rpc::value_heap::PropertyKey,
    spec: &'static CompiledFunctionSpec,
    captures: &[ValueHandle],
    runtime: &dyn ValueRuntime,
) -> Result<()> {
    let function = heap.fresh_compiled_function(spec, captures)?;
    let ValueSnapshot::Function(target) = heap.snapshot(&function)? else {
        return Err(ValueError::InvalidNode);
    };
    heap.define_own(
        object,
        key,
        PropertyDescriptor::Accessor { get: Some(target), set: None, enumerable: true, configurable: true },
        runtime,
    )
}
fn pending(
    realm: &Arc<NativeRealm>,
    constructor: &ValueHandle,
    runtime: &dyn ValueRuntime,
) -> Result<(ValueHandle, ValueHandle)> {
    let heap = realm.values.heap();
    let holder = heap.object()?;
    let executor = heap.fresh_compiled_function(&SAVE, &[holder.clone()])?;
    Ok((heap.construct(constructor, &[executor], runtime)?, holder))
}
fn state(realm: &Arc<NativeRealm>, promise: &ValueHandle, expected: &ValueHandle, rejected: bool) -> Result<()> {
    let heap = realm.values.heap();
    let edge = match realm.promise().snapshot(heap, promise)? {
        NativePromiseState::Rejected(edge) if rejected => edge,
        NativePromiseState::Fulfilled(edge) if !rejected => edge,
        other => panic!("Unexpected actual Promise state {other:?}"),
    };
    assert!(heap.restore_deferred(&edge)?.same_identity(expected));
    Ok(())
}
fn drain(run: &mut NativePromiseRun) -> Result<()> {
    let mut previous = 0;
    for _ in 0..16 {
        match run.drain_one()? {
            NativeDrain::Idle => return Ok(()),
            NativeDrain::RanJob { id } => {
                assert!(id > previous, "FIFO job identity cannot repeat or wrap");
                previous = id;
            }
        }
    }
    Err(ValueError::UnsupportedOperation("Controlled Promise observation job bound exhausted"))
}

pub fn observe(realm: &Arc<NativeRealm>) -> Result<Vec<ValueHandle>> {
    let heap = realm.values.heap();
    let base = realm.values.runtime();
    let undefined = heap.undefined()?;
    let constructor = member(heap, &realm.globals.environment, "Promise", base)?;
    assert!(heap.is_constructor(&constructor, base)?);
    assert!(heap.get(&constructor, &heap.species_key(), base)?.same_identity(&constructor));
    let prototype = member(heap, &constructor, "prototype", base)?;
    let then = member(heap, &prototype, "then", base)?;
    assert!(matches!(heap.snapshot(&then)?, ValueSnapshot::Function(CallableTarget::Compiled(_))));
    match heap.call(&constructor, &undefined, &[], base) {
        Err(ValueError::Thrown(reason)) => {
            assert!(heap.instance_of(&reason, &realm.globals.errors.type_error, base)?)
        }
        other => panic!("Promise call must throw its original realm TypeError: {other:?}"),
    }
    assert!(matches!(heap.construct(&constructor, &[], base), Err(ValueError::UnsupportedOperation(_))));
    let mut run = realm.begin_promise_run()?;
    assert!(realm.begin_promise_run().is_err());
    let kernel = run.prepare_pending(run.root_scope(), run.root_parent())?;
    let execute_result = run.prepare_pending(run.root_scope(), run.root_parent())?;
    assert!(!kernel.promise.same_identity(&execute_result.promise));
    assert_eq!(run.ticket_state(&kernel.ticket())?, NativePromiseState::Pending);
    assert_eq!(run.ticket_state(&execute_result.ticket())?, NativePromiseState::Pending);
    let wake = run.wake();
    let wake_future = wake.notified();
    tokio::pin!(wake_future);
    assert!(!wake_future.as_mut().enable());
    let value = heap.object()?;
    let other = heap.object()?;
    let original = heap.object()?;
    let trace = heap.array(vec![])?;
    let (
        promise,
        holder,
        result,
        throwing,
        self_promise,
        thrown_get,
        adopt,
        inner,
        outer,
        custom,
        scope_result,
        orphan,
        orphan_holder,
    ) = {
        let root = run.scoped(run.root_scope(), run.root_parent())?;
        heap.call(&kernel.resolve, &undefined, &[value.clone()], &root)?;
        assert!(matches!(run.ticket_state(&kernel.ticket())?, NativePromiseState::Fulfilled(_)));
        assert_eq!(run.ticket_state(&execute_result.ticket())?, NativePromiseState::Pending);
        let ticket_gets = heap.array(vec![])?;
        getter(
            heap,
            &execute_result.promise,
            "then".into(),
            &GET_THROW,
            &[ticket_gets.clone(), original.clone()],
            &root,
        )?;
        assert_eq!(run.ticket_state(&execute_result.ticket())?, NativePromiseState::Pending);
        heap.call(&execute_result.resolve, &undefined, &[value.clone()], &root)?;
        assert!(matches!(run.ticket_state(&execute_result.ticket())?, NativePromiseState::Fulfilled(_)));
        assert_eq!(heap.array_length(&ticket_gets, &root)?, 0);
        let (promise, holder) = pending(realm, &constructor, &root)?;
        let handler = heap.fresh_compiled_function(&RETURN, &[value.clone()])?;
        let result = heap.call(&then, &promise, &[handler], &root)?;
        let throwing =
            heap.call(&then, &promise, &[heap.fresh_compiled_function(&THROW, &[original.clone()])?], &root)?;
        assert_eq!(realm.promise().snapshot(heap, &result)?, NativePromiseState::Pending);
        heap.call(&member(heap, &holder, "resolve", &root)?, &undefined, &[value.clone()], &root)?;
        heap.call(&member(heap, &holder, "reject", &root)?, &undefined, &[original.clone()], &root)?;
        state(realm, &promise, &value, false)?;
        assert_eq!(realm.promise().snapshot(heap, &result)?, NativePromiseState::Pending);

        let (self_promise, self_holder) = pending(realm, &constructor, &root)?;
        heap.call(&member(heap, &self_holder, "resolve", &root)?, &undefined, &[self_promise.clone()], &root)?;
        let NativePromiseState::Rejected(reason) = realm.promise().snapshot(heap, &self_promise)? else {
            panic!("Self resolution must reject");
        };
        assert!(heap.instance_of(&heap.restore_deferred(&reason)?, &realm.globals.errors.type_error, &root)?);

        let (thrown_get, get_holder) = pending(realm, &constructor, &root)?;
        let get_object = heap.object()?;
        let get_trace = heap.array(vec![])?;
        getter(heap, &get_object, "then".into(), &GET_THROW, &[get_trace.clone(), original.clone()], &root)?;
        heap.call(&member(heap, &get_holder, "resolve", &root)?, &undefined, &[get_object], &root)?;
        heap.call(&member(heap, &get_holder, "resolve", &root)?, &undefined, &[other.clone()], &root)?;
        assert_eq!(heap.array_length(&get_trace, &root)?, 1);
        state(realm, &thrown_get, &original, true)?;

        let (adopt, adopt_holder) = pending(realm, &constructor, &root)?;
        let thenable = heap.object()?;
        let get_trace = heap.array(vec![])?;
        let dynamic = heap.fresh_compiled_function(&THENABLE, &[trace.clone(), value.clone(), original.clone()])?;
        getter(heap, &thenable, "then".into(), &GET, &[get_trace.clone(), dynamic], &root)?;
        heap.call(&member(heap, &adopt_holder, "resolve", &root)?, &undefined, &[thenable], &root)?;
        heap.call(&member(heap, &adopt_holder, "resolve", &root)?, &undefined, &[other.clone()], &root)?;
        assert_eq!(heap.array_length(&get_trace, &root)?, 1);
        assert_eq!(realm.promise().snapshot(heap, &adopt)?, NativePromiseState::Pending);

        let (inner, inner_holder) = pending(realm, &constructor, &root)?;
        heap.call(&member(heap, &inner_holder, "resolve", &root)?, &undefined, &[value.clone()], &root)?;
        heap.define_own_data(
            &inner,
            "then".into(),
            heap.fresh_compiled_function(&THENABLE, &[trace.clone(), other.clone(), original.clone()])?,
            &root,
        )?;
        let (outer, outer_holder) = pending(realm, &constructor, &root)?;
        heap.call(&member(heap, &outer_holder, "resolve", &root)?, &undefined, &[inner.clone()], &root)?;
        assert_eq!(realm.promise().snapshot(heap, &outer)?, NativePromiseState::Pending);

        let custom = heap.object()?;
        let recorded = heap.array(vec![])?;
        let record = heap.fresh_compiled_function(&RECORD, &[recorded.clone()])?;
        let species = heap.fresh_compiled_function(&SPECIES, &[custom.clone(), record.clone(), record])?;
        heap.define_own_data(&species, heap.species_key(), species.clone(), &root)?;
        heap.define_own_data(&promise, "constructor".into(), species, &root)?;
        assert!(heap.call(&then, &promise, &[], &root)?.same_identity(&custom));
        heap.define_own_data(&custom, "recorded".into(), recorded.clone(), &root)?;
        assert_eq!(heap.array_length(&recorded, &root)?, 0);
        let record = heap.fresh_compiled_function(&RECORD, &[recorded])?;
        let duplicate = heap.fresh_compiled_function(&DUPLICATE_SPECIES, &[custom.clone(), record.clone(), record])?;
        heap.define_own_data(&duplicate, heap.species_key(), duplicate.clone(), &root)?;
        heap.define_own_data(&promise, "constructor".into(), duplicate, &root)?;
        match heap.call(&then, &promise, &[], &root) {
            Err(ValueError::Thrown(reason)) => {
                assert!(heap.instance_of(&reason, &realm.globals.errors.type_error, &root)?)
            }
            other => panic!("Duplicate capability binding must throw original realm TypeError: {other:?}"),
        }

        let fake = heap.object()?;
        let before_brand = heap.array(vec![])?;
        getter(heap, &fake, "constructor".into(), &GET_THROW, &[before_brand.clone(), original.clone()], &root)?;
        match heap.call(&then, &fake, &[], &root) {
            Err(ValueError::Thrown(reason)) => {
                assert!(heap.instance_of(&reason, &realm.globals.errors.type_error, &root)?)
            }
            other => panic!("False brand must throw TypeError before constructor Get: {other:?}"),
        }
        assert_eq!(heap.array_length(&before_brand, &root)?, 0);
        let throwing_species = heap.object()?;
        getter(
            heap,
            &throwing_species,
            heap.species_key(),
            &GET_THROW,
            &[before_brand.clone(), original.clone()],
            &root,
        )?;
        heap.define_own_data(&promise, "constructor".into(), throwing_species, &root)?;
        match heap.call(&then, &promise, &[], &root) {
            Err(ValueError::Thrown(reason)) => assert!(reason.same_identity(&original)),
            other => panic!("Species Get throw identity lost: {other:?}"),
        }
        assert_eq!(heap.array_length(&before_brand, &root)?, 1);
        heap.define_own_data(&promise, "constructor".into(), constructor.clone(), &root)?;

        let storage = realm.context().prepare_storage(heap)?;
        let storage_run = member(heap, &storage, "run", &root)?;
        let get_store = member(heap, &storage, "getStore", &root)?;
        let a = heap.object()?;
        let b = heap.object()?;
        let scope_trace = heap.array(vec![])?;
        let (scope_promise, scope_holder) = pending(realm, &constructor, &root)?;
        let in_a = heap.fresh_compiled_function(&IN_STORE, &[storage.clone(), storage_run.clone(), a.clone()])?;
        let in_b = heap.fresh_compiled_function(&IN_STORE, &[storage.clone(), storage_run, b])?;
        let scoped_handler = heap
            .fresh_compiled_function(&SCOPED_HANDLER, &[storage, get_store, a, scope_trace.clone(), value.clone()])?;
        let mut register = |_: &ValueHandle, _: &[ValueHandle], effective: &dyn ValueRuntime| {
            heap.call(&then, &scope_promise, &[scoped_handler.clone()], effective)
        };
        let scope_result = heap.call_with_local_callback(&in_a, &undefined, &mut register, &[], &root)?;
        let mut resolve_in_b = |_: &ValueHandle, _: &[ValueHandle], effective: &dyn ValueRuntime| {
            heap.call(&member(heap, &scope_holder, "resolve", effective)?, &undefined, &[value.clone()], effective)
        };
        heap.call_with_local_callback(&in_b, &undefined, &mut resolve_in_b, &[], &root)?;
        heap.define_own_data(&scope_result, "scopeTrace".into(), scope_trace, &root)?;

        let static_resolve = member(heap, &constructor, "resolve", &root)?;
        assert!(heap.call(&static_resolve, &constructor, &[promise.clone()], &root)?.same_identity(&promise));
        let static_reject = member(heap, &constructor, "reject", &root)?;
        let rejected = heap.call(&static_reject, &constructor, &[original.clone()], &root)?;
        state(realm, &rejected, &original, true)?;
        let catch = member(heap, &prototype, "catch", &root)?;
        let caught =
            heap.call(&catch, &rejected, &[heap.fresh_compiled_function(&RETURN, &[value.clone()])?], &root)?;
        heap.define_own_data(&scope_result, "caught".into(), caught, &root)?;
        let (orphan, orphan_holder) = pending(realm, &constructor, &root)?;
        (
            promise,
            holder,
            result,
            throwing,
            self_promise,
            thrown_get,
            adopt,
            inner,
            outer,
            custom,
            scope_result,
            orphan,
            orphan_holder,
        )
    };
    assert!(
        futures::FutureExt::now_or_never(wake_future.as_mut()).is_some(),
        "Registered wake observes the real settlement/queue notification"
    );
    drain(&mut run)?;
    state(realm, &result, &value, false)?;
    state(realm, &throwing, &original, true)?;
    state(realm, &adopt, &value, false)?;
    state(realm, &inner, &value, false)?;
    state(realm, &outer, &other, false)?;
    state(realm, &scope_result, &value, false)?;
    state(realm, &member(heap, &scope_result, "caught", base)?, &value, false)?;
    assert_eq!(heap.array_length(&member(heap, &scope_result, "scopeTrace", base)?, base)?, 1);
    assert_eq!(heap.array_length(&trace, base)?, 2);
    let recorded = member(heap, &custom, "recorded", base)?;
    assert_eq!(heap.array_length(&recorded, base)?, 1);
    assert!(heap.get(&recorded, &"0".into(), base)?.same_identity(&value));
    observe_registered_parents(realm, &mut run)?;
    let old_revoker = run.revoker();
    old_revoker.revoke();
    assert!(matches!(run.drain_one(), Err(ValueError::ExpiredCapability(_))));
    assert!(realm.begin_promise_run().is_err());
    drop(run);
    let mut next = realm.begin_promise_run()?;
    let retained_result = {
        let root = next.scoped(next.root_scope(), next.root_parent())?;
        old_revoker.revoke();
        assert!(matches!(next.ticket_state(&execute_result.ticket()), Err(ValueError::ExpiredCapability(_))));
        assert!(matches!(heap.call(&then, &orphan, &[], &root), Err(ValueError::ExpiredCapability(_))));
        assert!(matches!(
            heap.call(&member(heap, &orphan_holder, "resolve", &root)?, &undefined, &[value.clone()], &root),
            Err(ValueError::ExpiredCapability(_))
        ));
        heap.call(&member(heap, &holder, "resolve", &root)?, &undefined, &[other], &root)?;
        heap.call(&then, &promise, &[], &root)?
    };
    drain(&mut next)?;
    state(realm, &promise, &value, false)?;
    state(realm, &retained_result, &value, false)?;
    let isolated = NativeRealm::prepare()?;
    let isolated_domain = isolated.promise().clone();
    let isolated_heap = Arc::downgrade(isolated.values.heap());
    let isolated_run = isolated.begin_promise_run()?;
    let isolated_revoker = isolated_run.revoker();
    let isolated_pending = isolated_run.prepare_pending(isolated_run.root_scope(), isolated_run.root_parent())?;
    drop(isolated_pending);
    drop(isolated_run);
    drop(isolated);
    assert!(isolated_heap.upgrade().is_none(), "Retained Promise metadata/domain/revoker cannot own the heap");
    isolated_revoker.revoke();
    assert!(Arc::strong_count(&isolated_domain) >= 1);
    assert!(isolated_heap.upgrade().is_none());
    Ok(vec![
        promise,
        result,
        throwing,
        self_promise,
        thrown_get,
        adopt,
        inner,
        outer,
        custom,
        scope_result,
        orphan,
        retained_result,
    ])
}

// Grouped additions to the existing coarse entrance, not new test entrances.
const PARENT_PROBE: ara_rpc::value_heap::CapabilityId = ara_rpc::value_heap::CapabilityId(0x4152415f4f425350);
struct ProbeParent {
    original: Arc<dyn ValueRuntime>,
    tag: u8,
    calls: Arc<std::sync::atomic::AtomicUsize>,
}
impl ValueRuntime for ProbeParent {
    fn resolve(&self, request: ara_rpc::value_heap::RuntimeRequest<'_>) -> Result<ara_rpc::value_heap::RuntimeReply> {
        self.resolve_effective(request, self)
    }
    fn resolve_effective(
        &self,
        request: ara_rpc::value_heap::RuntimeRequest<'_>,
        effective: &dyn ValueRuntime,
    ) -> Result<ara_rpc::value_heap::RuntimeReply> {
        match request {
            ara_rpc::value_heap::RuntimeRequest::Call { capability, receiver, arguments } => {
                self.resolve_call(capability, receiver, arguments, effective)
            }
            request => self.original.resolve_effective(request, effective),
        }
    }
    fn resolve_call(
        &self,
        capability: ara_rpc::value_heap::CapabilityId,
        receiver: &ValueHandle,
        arguments: &[ValueHandle],
        effective: &dyn ValueRuntime,
    ) -> Result<ara_rpc::value_heap::RuntimeReply> {
        if capability == PARENT_PROBE {
            assert!(matches!(receiver.heap().snapshot(receiver)?, ValueSnapshot::Undefined));
            assert_eq!(arguments.len(), 1);
            self.calls.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            return Ok(ara_rpc::value_heap::RuntimeReply::Value(receiver.heap().number(f64::from(self.tag))?));
        }
        self.original.resolve_call(capability, receiver, arguments, effective)
    }
    fn resolve_is_constructor(
        &self,
        capability: ara_rpc::value_heap::CapabilityId,
        constructor: &ValueHandle,
        effective: &dyn ValueRuntime,
    ) -> Result<bool> {
        self.original.resolve_is_constructor(capability, constructor, effective)
    }
    fn resolve_construct(
        &self,
        capability: ara_rpc::value_heap::CapabilityId,
        constructor: &ValueHandle,
        arguments: &[ValueHandle],
        effective: &dyn ValueRuntime,
    ) -> Result<ValueHandle> {
        self.original.resolve_construct(capability, constructor, arguments, effective)
    }
    fn resolve_local_call(
        &self,
        token: u64,
        receiver: &ValueHandle,
        arguments: &[ValueHandle],
        effective: &dyn ValueRuntime,
    ) -> Result<ara_rpc::value_heap::RuntimeReply> {
        self.original.resolve_local_call(token, receiver, arguments, effective)
    }
}
static PARENT_RESULT: CompiledFunctionSpec = CompiledFunctionSpec {
    source: "controlled actual registered parent A/B reaction dispatch",
    name: "parentResult",
    length: 1,
    form: CompiledFunctionForm::Builtin,
    capture_count: 1,
    body: |heap, captures, _, args, runtime| heap.call(&captures[0], &heap.undefined()?, args, runtime),
};
static NATIVE_SETTLE: CompiledFunctionSpec = CompiledFunctionSpec {
    source: "controlled private native await settles only after original kernel adoption",
    name: "nativeSettle",
    length: 1,
    form: CompiledFunctionForm::Builtin,
    capture_count: 1,
    body: |heap, captures, _, args, runtime| {
        heap.call(&captures[0], &heap.undefined()?, args, runtime)?;
        heap.undefined()
    },
};
fn observe_registered_parents(realm: &Arc<NativeRealm>, run: &mut NativePromiseRun) -> Result<()> {
    let heap = realm.values.heap();
    let admission = run.admission();
    let root_id = admission.root_parent();
    let scope = admission.root_scope();
    let root = admission.scoped(scope, root_id)?;
    assert_eq!(admission.capture_parent(&root)?, root_id);
    assert_eq!(admission.capture_scope(&root)?, scope);
    assert!(matches!(admission.capture_parent(realm.values.runtime()), Err(ValueError::UnsupportedOperation(_))));
    let calls_a = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let calls_b = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let actual_a = Arc::new(ProbeParent { original: realm.runtime.clone(), tag: 1, calls: calls_a.clone() });
    let actual_b = Arc::new(ProbeParent { original: realm.runtime.clone(), tag: 2, calls: calls_b.clone() });
    let weak_a = Arc::downgrade(&actual_a);
    let weak_b = Arc::downgrade(&actual_b);
    let parent_a = admission.register_parent(actual_a)?;
    let parent_b = admission.register_parent(actual_b)?;
    let a = admission.scoped(scope, parent_a)?;
    let b = admission.scoped(scope, parent_b)?;
    assert_eq!(admission.capture_parent(&a)?, parent_a);
    assert_eq!(admission.capture_parent(&b)?, parent_b);
    assert_ne!(parent_a, root_id);
    assert_ne!(parent_b, parent_a);
    let value = heap.object()?;
    let slot_a = admission.prepare_pending(scope, parent_a)?;
    let slot_root = admission.prepare_pending(scope, root_id)?;
    let slot_b = admission.prepare_pending(scope, parent_b)?;
    let probe = heap.function(PARENT_PROBE)?;
    let probe = heap.fresh_compiled_function(&PARENT_RESULT, &[probe])?;
    let identity = heap.fresh_compiled_function(&RETURN, &[value.clone()])?;
    let then = member(heap, &slot_a.promise, "then", &a)?;
    let result_a = heap.call(&then, &slot_a.promise, &[probe.clone()], &a)?;
    let result_root = heap.call(&then, &slot_root.promise, &[identity], &root)?;
    let result_b = heap.call(&then, &slot_b.promise, &[probe], &b)?;
    // All three are settled by root; each reaction must keep its registration parent.
    for slot in [&slot_a, &slot_root, &slot_b] {
        heap.call(&slot.resolve, &heap.undefined()?, &[value.clone()], &root)?;
    }
    drop(a);
    drop(b);
    admission.release_parent_registration(parent_a)?;
    admission.release_parent_registration(parent_b)?;
    assert!(weak_a.upgrade().is_some(), "Queued last A reaction retains its exact original Arc");
    assert!(weak_b.upgrade().is_some(), "Queued last B reaction retains its exact original Arc");
    assert!(matches!(run.drain_one()?, NativeDrain::RanJob { .. }));
    assert!(weak_a.upgrade().is_none(), "A releases after its last reaction, independently of B");
    assert!(weak_b.upgrade().is_some());
    drain(run)?;
    assert!(weak_b.upgrade().is_none(), "B releases only after its own last reaction");
    for (result, expected) in [(&result_a, 1.0), (&result_b, 2.0)] {
        let NativePromiseState::Fulfilled(edge) = realm.promise().snapshot(heap, result)? else {
            panic!("Registered parent probe must fulfill");
        };
        assert!(
            matches!(heap.snapshot(&heap.restore_deferred(&edge)?)?, ValueSnapshot::Number(value) if value == expected)
        );
    }
    state(realm, &result_root, &value, false)?;
    assert_eq!(calls_a.load(std::sync::atomic::Ordering::Relaxed), 1);
    assert_eq!(calls_b.load(std::sync::atomic::Ordering::Relaxed), 1);
    assert!(matches!(admission.scoped(scope, parent_a), Err(ValueError::ExpiredCapability(_))));

    let kernel = admission.prepare_pending(scope, root_id)?;
    let execute = admission.prepare_pending(scope, root_id)?;
    let fulfilled = heap.fresh_compiled_function(&NATIVE_SETTLE, &[execute.resolve.clone()])?;
    let rejected = heap.fresh_compiled_function(&NATIVE_SETTLE, &[execute.reject.clone()])?;
    let gets = heap.array(vec![])?;
    getter(heap, &kernel.promise, "then".into(), &GET_THROW, &[gets.clone(), value.clone()], &root)?;
    admission.attach_native_await(&kernel.ticket(), &fulfilled, &rejected, scope, root_id)?;
    let trace = heap.array(vec![])?;
    let thenable = heap.object()?;
    heap.define_own_data(
        &thenable,
        "then".into(),
        heap.fresh_compiled_function(&THENABLE, &[trace.clone(), value.clone(), heap.object()?])?,
        &root,
    )?;
    heap.call(&kernel.resolve, &heap.undefined()?, &[thenable], &root)?;
    assert_eq!(admission.ticket_state(&kernel.ticket())?, NativePromiseState::Pending);
    assert_eq!(admission.ticket_state(&execute.ticket())?, NativePromiseState::Pending);
    assert!(matches!(run.drain_one()?, NativeDrain::RanJob { .. }));
    assert!(matches!(admission.ticket_state(&kernel.ticket())?, NativePromiseState::Fulfilled(_)));
    assert_eq!(
        admission.ticket_state(&execute.ticket())?,
        NativePromiseState::Pending,
        "Kernel adoption alone cannot settle the authoritative execute-result"
    );
    assert!(matches!(run.drain_one()?, NativeDrain::RanJob { .. }));
    state(realm, &execute.promise, &value, false)?;
    assert_eq!(heap.array_length(&gets, &root)?, 0, "Private await never Gets public then");
    assert_eq!(heap.array_length(&trace, &root)?, 1, "Kernel uses the original dynamic adoption once");
    assert_eq!(run.drain_one()?, NativeDrain::Idle);
    Ok(())
}
