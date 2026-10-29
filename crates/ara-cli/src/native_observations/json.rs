//! Exercises the actual installed Native JSON.stringify and original realm
//! TypeError prototypes in the existing CLI realm coarse family.
use super::*;
#[path = "../../../ara-rpc/tests/value_heap/json_native.rs"]
mod shared;

pub(super) fn observe(realm: &Arc<NativeRealm>) -> Result<Vec<ValueHandle>> {
    let heap = realm.values.heap();
    let runtime = realm.values.runtime();
    let json = global(realm, "JSON")?;
    assert!(json.same_identity(&realm.globals.json));
    let stringify = method(realm, &json, "stringify")?;
    assert!(stringify.same_identity(&method(realm, &json, "stringify")?));
    assert!(matches!(heap.own_descriptor(&json, &"stringify".into(), runtime)?,
        Some(PropertyDescriptor::Data { value, writable:true, enumerable:false, configurable:true })
        if value.same_identity(&stringify)));
    assert!(
        matches!(heap.snapshot(&method(realm, &stringify, "name")?)?,ValueSnapshot::String(s) if s.equals_ascii("stringify"))
    );
    assert_eq!(number(realm, &method(realm, &stringify, "length")?)?, 3.0);
    assert!(heap.own_descriptor(&stringify, &"prototype".into(), runtime)?.is_none());
    // The expression-dependent upstream constructor message is not a universal
    // function contract. Exercise nonconstructability without inventing that text.
    assert!(matches!(heap.construct_compiled_function(&stringify, &[], runtime), Err(ValueError::TypeError(_))));
    let borrowed = heap.call(&stringify, &heap.null()?, &[heap.import_serde(&serde_json::json!({"a":1}))?], runtime)?;
    assert!(matches!(heap.snapshot(&borrowed)?,ValueSnapshot::String(s) if s.equals_ascii("{\"a\":1}")));
    let type_prototype = method(realm, &realm.globals.errors.type_error, "prototype")?;
    let undefined = heap.undefined()?;
    let mut receipts = shared::observe_with(heap, Some(&type_prototype), |args, effective_runtime| {
        heap.call(&stringify, &undefined, args, effective_runtime)
    });
    receipts.push(borrowed);
    Ok(receipts)
}
