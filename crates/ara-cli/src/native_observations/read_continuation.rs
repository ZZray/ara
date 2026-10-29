//! Original Read continuation observations in the existing CLI coarse entrance.
//! Calls actual compiled bodies; synchronous entry is not a real await claim.

use ara_cli::{actual_tools::read_continuation::NativeReadContinuation, native_realm::NativeRealm};
use ara_rpc::value_heap::{
    CallableTarget, CapabilityId, PropertyDescriptor, PropertyKey, RuntimeReply, RuntimeRequest, ValueError,
    ValueHandle, ValueHeap, ValueRuntime, ValueSnapshot,
};
use std::sync::{Arc, Mutex};

type Result<T> = std::result::Result<T, ValueError>;

fn result(
    heap: &Arc<ValueHeap>,
    text: &str,
    runtime: &dyn ValueRuntime,
) -> Result<(ValueHandle, ValueHandle, ValueHandle)> {
    let first = heap.object()?;
    let second = heap.object()?;
    for (block, value) in [(&first, text), (&second, "second text stays unchanged")] {
        heap.define_own_data(block, "type".into(), heap.string("text".into())?, runtime)?;
        heap.define_own_data(block, "text".into(), heap.string(value.into())?, runtime)?;
    }
    let result = heap.object()?;
    heap.define_own_data(&result, "content".into(), heap.array(vec![first.clone(), second.clone()])?, runtime)?;
    Ok((result, first, second))
}
fn text(heap: &Arc<ValueHeap>, block: &ValueHandle, runtime: &dyn ValueRuntime) -> Result<String> {
    match heap.snapshot(&heap.get(block, &"text".into(), runtime)?)? {
        ValueSnapshot::String(text) => text.to_utf8().map_err(|_| ValueError::InvalidNode),
        _ => Err(ValueError::InvalidNode),
    }
}
fn entry(heap: &Arc<ValueHeap>, tracker: &ValueHandle, path: &str, runtime: &dyn ValueRuntime) -> Result<ValueHandle> {
    let get = heap.get(tracker, &"get".into(), runtime)?;
    heap.call(&get, tracker, &[heap.string(path.into())?], runtime)
}

pub fn observe(realm: &Arc<NativeRealm>) -> Result<Vec<ValueHandle>> {
    let heap = realm.values.heap();
    let runtime = realm.values.runtime();
    let module = NativeReadContinuation::prepare(realm)?;
    let receiver = heap.object()?;
    let params = heap.object()?;
    let old_session = heap.object()?;
    let session = heap.object()?;
    heap.define_own_data(&receiver, "session".into(), old_session.clone(), runtime)?;
    heap.define_own_data(&params, "path".into(), heap.string("old.txt".into())?, runtime)?;
    let after = module.after_inner(&receiver, &params)?;
    // Capturing the continuation must not capture properties before suspension.
    heap.assign(&receiver, "session".into(), session.clone(), runtime)?;
    heap.assign(&params, "path".into(), heap.string("current.txt".into())?, runtime)?;
    let undefined = heap.undefined()?;
    let mut retained = vec![receiver.clone(), params.clone(), old_session.clone(), session.clone(), after.clone()];
    for count in 1..=4 {
        let (result, first, second) = result(heap, "same output", runtime)?;
        let returned = heap.call(&after, &undefined, &[result.clone()], runtime)?;
        assert!(returned.same_identity(&result), "The public continuation returns the same result graph");
        let expected = if count < 3 {
            "same output".to_string()
        } else {
            format!(
                "same output\n\n[You have received this identical output {count} times. Re-reading 'current.txt' will not change it — use a narrower selector (path:A-B), or proceed with the edit.]"
            )
        };
        assert_eq!(text(heap, &first, runtime)?, expected);
        assert_eq!(text(heap, &second, runtime)?, "second text stays unchanged");
        retained.extend([result, first, second]);
    }
    assert!(heap.own_keys(&old_session, runtime)?.is_empty());
    let symbols = heap
        .own_keys(&session, runtime)?
        .into_iter()
        .filter(|key| matches!(key, PropertyKey::Symbol(_)))
        .collect::<Vec<_>>();
    assert_eq!(symbols.len(), 1, "One private tracker belongs to this Session");
    let tracker = heap.get(&session, &symbols[0], runtime)?;
    assert!(matches!(heap.snapshot(&entry(heap, &tracker, "old.txt", runtime)?)?, ValueSnapshot::Undefined));
    let record = entry(heap, &tracker, "current.txt", runtime)?;
    assert!(matches!(heap.snapshot(&heap.get(&record, &"count".into(), runtime)?)?, ValueSnapshot::Number(4.0)));
    let (changed, first, _) = result(heap, "changed output", runtime)?;
    heap.call(&after, &undefined, &[changed], runtime)?;
    assert_eq!(text(heap, &first, runtime)?, "changed output");
    let record = entry(heap, &tracker, "current.txt", runtime)?;
    assert!(matches!(heap.snapshot(&heap.get(&record, &"count".into(), runtime)?)?, ValueSnapshot::Number(1.0)));

    // Source clears at >64, not >=64. Reuse the real dynamic Map methods.
    let clear = heap.get(&tracker, &"clear".into(), runtime)?;
    heap.call(&clear, &tracker, &[], runtime)?;
    let set = heap.get(&tracker, &"set".into(), runtime)?;
    for i in 0..64 {
        heap.call(&set, &tracker, &[heap.string(format!("existing-{i}").into())?, heap.object()?], runtime)?;
    }
    let (result65, _, _) = result(heap, "capacity output", runtime)?;
    heap.call(&after, &undefined, &[result65], runtime)?;
    assert!(matches!(heap.snapshot(&heap.get(&tracker, &"size".into(), runtime)?)?, ValueSnapshot::Number(65.0)));
    let (reset, _, _) = result(heap, "capacity output", runtime)?;
    heap.call(&after, &undefined, &[reset], runtime)?;
    assert!(matches!(heap.snapshot(&heap.get(&tracker, &"size".into(), runtime)?)?, ValueSnapshot::Number(1.0)));

    // Verify exact post order and original throw, using controlled getters.
    struct Getters<'a> {
        base: &'a dyn ValueRuntime,
        session: ValueHandle,
        reason: ValueHandle,
        trace: Mutex<Vec<&'static str>>,
    }
    impl ValueRuntime for Getters<'_> {
        fn resolve(&self, request: RuntimeRequest<'_>) -> Result<RuntimeReply> {
            self.resolve_effective(request, self)
        }
        fn resolve_effective(&self, request: RuntimeRequest<'_>, effective: &dyn ValueRuntime) -> Result<RuntimeReply> {
            match request {
                RuntimeRequest::Call { capability, receiver, arguments } => {
                    self.resolve_call(capability, receiver, arguments, effective)
                }
                request => self.base.resolve_effective(request, effective),
            }
        }
        fn resolve_call(
            &self,
            capability: CapabilityId,
            receiver: &ValueHandle,
            args: &[ValueHandle],
            effective: &dyn ValueRuntime,
        ) -> Result<RuntimeReply> {
            match capability.0 {
                982601 => {
                    self.trace.lock().unwrap().push("session");
                    Ok(RuntimeReply::Value(self.session.clone()))
                }
                982602 => {
                    self.trace.lock().unwrap().push("path");
                    Err(ValueError::Thrown(self.reason.clone()))
                }
                _ => self.base.resolve_call(capability, receiver, args, effective),
            }
        }
    }
    let original = heap.object()?;
    let getters =
        Getters { base: runtime, session: session.clone(), reason: original.clone(), trace: Mutex::new(Vec::new()) };
    for (object, name, id) in [(&receiver, "session", 982601), (&params, "path", 982602)] {
        heap.define_own(
            object,
            name.into(),
            PropertyDescriptor::Accessor {
                get: Some(CallableTarget::Host(CapabilityId(id))),
                set: None,
                enumerable: true,
                configurable: true,
            },
            runtime,
        )?;
    }
    let (throwing, _, _) = result(heap, "not appended", runtime)?;
    match heap.call(&after, &undefined, &[throwing.clone()], &getters) {
        Err(ValueError::Thrown(reason)) => assert!(reason.same_identity(&original)),
        other => panic!("Original post getter throw must be returned unchanged: {other:?}"),
    }
    assert_eq!(*getters.trace.lock().unwrap(), vec!["session", "path"]);
    retained.extend([tracker, record, throwing, original, module.helper().clone()]);
    Ok(retained)
}
