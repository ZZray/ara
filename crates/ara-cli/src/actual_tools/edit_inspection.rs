//! Fixed OMP596f2da edit/index.ts matcherDigest/Paths/Entries.
//! Copyright (c) 2025 Mario Zechner; Copyright (c) 2025-2026 Can Bölük;
//! Copyright (c) 2026 Stencil Labs, Inc. MIT; see THIRD_PARTY.md.
//! Original arguments are stringified in their original realm; only the fresh
//! pure inspection projection is imported. Inspection grants no write authority.

use ara_edit::EditMode;
use ara_rpc::value_heap::{
    CompiledFunctionForm, CompiledFunctionSpec, StringifyMode, ValueError, ValueHandle, ValueHeap, ValueRuntime,
    ValueSnapshot,
};
use serde_json::{Value, json};
use std::sync::Arc;

#[derive(Clone, Copy)]
enum Projection {
    Digest,
    Paths,
    Entries,
}

fn original_mode(heap:&Arc<ValueHeap>,source:&ValueHandle,effective:&dyn ValueRuntime)->Result<ValueHandle,ValueError>{heap.get(source,&"mode".into(),effective)}
fn inspection_value(heap:&Arc<ValueHeap>,mode:EditMode,args:&ValueHandle,effective:&dyn ValueRuntime)->Result<ValueHandle,ValueError>{
    let args=if matches!(heap.snapshot(args)?,ValueSnapshot::Undefined|ValueSnapshot::Null){heap.object()?}else{args.clone()};
    let serialized=heap.stringify(&args,StringifyMode::Plain,None,effective)?.ok_or(ValueError::WrongRuntimeReply("Edit inspection JSON.stringify"))?;
    let json=serialized.to_utf8().map_err(|_|ValueError::WrongRuntimeReply("Edit inspection JSON text"))?;
    let inspected=ara_tools::edit::inspect_args_json(mode,&json);
    let file_ops=inspected.file_ops.into_iter().map(|op|match op{ara_edit::FileOpIntent::Delete{path}=>json!({"kind":"delete","path":path}),ara_edit::FileOpIntent::Move{from,to}=>json!({"kind":"move","path":from,"to":to})}).collect::<Vec<_>>();
    heap.import_serde(&json!({"paths":inspected.paths,"entries":inspected.entries.into_iter().map(|(path,digest)|json!({"path":path,"digest":digest})).collect::<Vec<_>>(),"fileOps":file_ops}))
}
static MAP_DIGEST:CompiledFunctionSpec=CompiledFunctionSpec{source:"OMP b91 edit/index.ts entry=>entry.digest",name:"",length:1,form:CompiledFunctionForm::Arrow,capture_count:0,body:|heap,_,_,args,effective|heap.get(&args.first().cloned().unwrap_or(heap.undefined()?),&"digest".into(),effective)};
static MAP_PATH:CompiledFunctionSpec=CompiledFunctionSpec{source:"OMP b91 edit/index.ts entry=>entry.path",name:"",length:1,form:CompiledFunctionForm::Arrow,capture_count:0,body:|heap,_,_,args,effective|heap.get(&args.first().cloned().unwrap_or(heap.undefined()?),&"path".into(),effective)};
static BOOLEAN:CompiledFunctionSpec=CompiledFunctionSpec{source:"OMP b91 edit/index.ts filter(Boolean)",name:"Boolean",length:1,form:CompiledFunctionForm::Builtin,capture_count:0,body:|heap,_,_,args,_|heap.boolean(heap.truthy(&args.first().cloned().unwrap_or(heap.undefined()?))?)};
fn method(heap:&Arc<ValueHeap>,receiver:&ValueHandle,key:&str,args:&[ValueHandle],effective:&dyn ValueRuntime)->Result<ValueHandle,ValueError>{let function=heap.get(receiver,&key.into(),effective)?;heap.call(&function,receiver,args,effective)}
fn matcher(heap:&Arc<ValueHeap>,captures:&[ValueHandle],arguments:&[ValueHandle],effective:&dyn ValueRuntime,projection:Projection)->Result<ValueHandle,ValueError>{
    let source=&captures[0];let args=arguments.first().cloned().unwrap_or(heap.undefined()?);
    let tagged=matches!(heap.snapshot(&args)?,ValueSnapshot::Object|ValueSnapshot::Array|ValueSnapshot::Opaque{..});
    let key=heap.module_symbol("coding-agent/edit","edit.inspection")?;
    let cached=if tagged{heap.get(&args,&key,effective)?}else{heap.undefined()?};
    let cached_mode=if matches!(heap.snapshot(&cached)?,ValueSnapshot::Null|ValueSnapshot::Undefined){heap.undefined()?}else{heap.get(&cached,&"mode".into(),effective)?};
    let mode=original_mode(heap,source,effective)?;
    let inspection=if heap.object_is(&cached_mode,&mode)?{heap.get(&cached,&"inspection".into(),effective)?}
    else {
        let inspected=(||{let mode=original_mode(heap,source,effective)?;let ValueSnapshot::String(mode)=heap.snapshot(&mode)?else{return Err(ValueError::WrongRuntimeReply("Edit inspection live mode"))};let mode=mode.to_utf8().ok().as_deref().and_then(EditMode::parse).ok_or(ValueError::WrongRuntimeReply("Edit inspection live mode value"))?;inspection_value(heap,mode,&args,effective)})();
        let inspection=match inspected{Ok(value)=>value,Err(_)=>heap.import_serde(&json!({"paths":[],"entries":[],"fileOps":[]}))?};
        if tagged {let cached=heap.object()?;let mode=original_mode(heap,source,effective)?;heap.define_own_data(&cached,"mode".into(),mode,effective)?;heap.define_own_data(&cached,"inspection".into(),inspection.clone(),effective)?;heap.assign(&args,key,cached,effective)?;}
        inspection
    };
    let entries=heap.get(&inspection,&"entries".into(),effective)?;
    match projection {
        Projection::Digest=>{let mapped=method(heap,&entries,"map",&[heap.fresh_compiled_function(&MAP_DIGEST,&[])?],effective)?;let filtered=method(heap,&mapped,"filter",&[heap.fresh_compiled_function(&BOOLEAN,&[])?],effective)?;let digest=method(heap,&filtered,"join",&[heap.string("\n".into())?],effective)?;if heap.truthy(&digest)?{Ok(digest)}else{heap.undefined()}},
        Projection::Paths=>{let length=heap.get(&entries,&"length".into(),effective)?;let paths=if heap.to_number(&length,effective)?>0.0{method(heap,&entries,"map",&[heap.fresh_compiled_function(&MAP_PATH,&[])?],effective)?}else{heap.get(&inspection,&"paths".into(),effective)?};let length=heap.get(&paths,&"length".into(),effective)?;if heap.to_number(&length,effective)?>0.0{Ok(paths)}else{heap.undefined()}},
        Projection::Entries=>{let length=heap.get(&entries,&"length".into(),effective)?;if heap.to_number(&length,effective)?>0.0{Ok(entries)}else{heap.undefined()}}
    }
}

static DIGEST: CompiledFunctionSpec = CompiledFunctionSpec {
    source: "OMP edit/index.ts matcherDigest -> original #inspect -> native edit_inspect",
    name: "matcherDigest",
    length: 1,
    form: CompiledFunctionForm::Builtin,
    capture_count: 1,
    body: |heap, captures, _, args, effective| matcher(heap, captures, args, effective, Projection::Digest),
};
static PATHS: CompiledFunctionSpec = CompiledFunctionSpec {
    source: "OMP edit/index.ts matcherPaths -> original #inspect -> native edit_inspect",
    name: "matcherPaths",
    length: 1,
    form: CompiledFunctionForm::Builtin,
    capture_count: 1,
    body: |heap, captures, _, args, effective| matcher(heap, captures, args, effective, Projection::Paths),
};
static ENTRIES: CompiledFunctionSpec = CompiledFunctionSpec {
    source: "OMP edit/index.ts matcherEntries -> original #inspect -> native edit_inspect",
    name: "matcherEntries",
    length: 1,
    form: CompiledFunctionForm::Builtin,
    capture_count: 1,
    body: |heap, captures, _, args, effective| matcher(heap, captures, args, effective, Projection::Entries),
};

pub(super) fn install(
    heap: &Arc<ValueHeap>,
    source: &ValueHandle,
    _mode: EditMode,
    runtime: &dyn ValueRuntime,
) -> Result<(), ValueError> {
    let owner = source.clone();
    for (name, body) in [("matcherDigest", &DIGEST), ("matcherPaths", &PATHS), ("matcherEntries", &ENTRIES)] {
        let callable = heap.fresh_compiled_function(body, &[owner.clone()])?;
        heap.define_own_data(source, name.into(), callable, runtime)?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::actual_tools::{BuiltinKind, BuiltinToolSource};
    use ara_agent::{ActualToolSource, SessionValues};
    use ara_rpc::value_heap::{NoRuntime,RuntimeRequest,RuntimeReply};
    struct MatcherRuntime(Arc<BuiltinToolSource>);
    impl ValueRuntime for MatcherRuntime {
        fn resolve(&self,request:RuntimeRequest<'_>)->Result<RuntimeReply,ValueError>{
            if let Some(reply)=self.0.resolve_context_signal(&request){reply}else{NoRuntime.resolve(request)}
        }
    }

    static RETURN_CAPTURE: CompiledFunctionSpec = CompiledFunctionSpec {
        source: "Test original toJSON keeps the original args realm",
        name: "toJSON",
        length: 1,
        form: CompiledFunctionForm::Builtin,
        capture_count: 1,
        body: |_, captures, _, _, _| Ok(captures[0].clone()),
    };

    #[test]
    fn fixed_five_mode_matcher_contracts_through_original_builtin_graph_family() {
        crate::host_edit_session::observe_fixed_session_host_family();
        let directory = tempfile::tempdir().unwrap();
        let runtime = Arc::new(NoRuntime);
        let heap = ValueHeap::new().unwrap();
        let values = Arc::new(SessionValues::new(heap.clone(), runtime.clone()).unwrap());
        let sloppy = [
            "*** Edit File: sloppy.ts",
            "*** Find",
            "old",
            "*** Replace",
            "sloppy",
        ]
        .join("\n");
        for (mode, args, target, digest) in [
            (
                EditMode::Replace,
                json!({"path":"replace.ts", "old_string":"old", "new_string":"replacement"}),
                "replace.ts",
                "replacement",
            ),
            (
                EditMode::Patch,
                json!({"path":"patch.ts", "edits":[{"op":"update", "diff":"@@\n-old\n+patched"}]}),
                "patch.ts",
                "patched",
            ),
            (
                EditMode::ApplyPatch,
                json!({"input":"*** Begin Patch\n*** Update File: apply.ts\n@@\n-old\n+applied\n*** End Patch\n"}),
                "apply.ts",
                "applied",
            ),
            (EditMode::Hashline, json!({"input":"[hash.ts#ABCD]\nPUT 1-1:\n+hashed\n"}), "hash.ts", "hashed"),
            (EditMode::Sloppy, json!({"input":sloppy}), "sloppy.ts", "«\nold\n»\nsloppy"),
        ] {
            let source = BuiltinToolSource::new(
                &values,
                ara_tools::ToolContext::new(directory.path()).with_edit(mode, true),
                &[BuiltinKind::Edit],
            )
            .unwrap();
            let collection = source.current_tools(&values).unwrap().unwrap();
            // Dynamic source.mode is resolved by this same actual source owner.
            let runtime=MatcherRuntime(source.clone());
            let tool = heap.get(&collection, &"0".into(), &runtime).unwrap();
            let original = heap.import_serde(&args).unwrap();
            let proxy = heap.object().unwrap();
            let to_json = heap.fresh_compiled_function(&RETURN_CAPTURE, &[original.clone()]).unwrap();
            heap.define_own_data(&proxy, "toJSON".into(), to_json.clone(), &runtime).unwrap();
            for (name, expected) in [
                ("matcherDigest", json!(digest)),
                ("matcherPaths", json!([target])),
                ("matcherEntries", json!([{"path":target, "digest":digest}])),
            ] {
                let method = heap.get(&tool, &name.into(), &runtime).unwrap();
                for arguments in [&original, &proxy] {
                    let result = heap.call(&method, &tool, &[arguments.clone()], &runtime).unwrap();
                    assert_eq!(heap.export_serde(&result).unwrap(), expected, "{} {name}", mode.as_str());
                }
                let empty = heap.call(&method, &tool, &[heap.null().unwrap()], &runtime).unwrap();
                assert!(matches!(heap.snapshot(&empty).unwrap(), ValueSnapshot::Undefined));
                let cycle = heap.object().unwrap();
                heap.define_own_data(&cycle, "self".into(), cycle.clone(), &runtime).unwrap();
                let failed = heap.call(&method, &tool, &[cycle], &runtime).unwrap();
                assert!(matches!(heap.snapshot(&failed).unwrap(), ValueSnapshot::Undefined));
            }
            assert!(heap.get(&proxy, &"toJSON".into(), &runtime).unwrap().same_identity(&to_json));
            assert_eq!(std::fs::read_dir(directory.path()).unwrap().count(), 0);
        }
    }
}
