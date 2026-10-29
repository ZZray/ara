//! Fixed OMP596f2da ast-edit.ts:582-605, renderCall only.
//! Copyright (c) 2025 Mario Zechner; Copyright (c) 2025-2026 Can Bölük;
//! Copyright (c) 2026 Stencil Labs, Inc. MIT; see THIRD_PARTY_NOTICES.md.
//! Original Graph operand reads precede the existing zero-padding Text adapter.
use super::{graph_renderer::DisplayGraph, github_output_block as ui, github_renderer::Component};
use ara_rpc::value_heap::{ValueError,ValueHandle,ValueSnapshot};
type Result<T> = std::result::Result<T,ValueError>;

fn optional_get(g:&DisplayGraph<'_>,receiver:&ValueHandle,key:&str)->Result<ValueHandle>{
    if g.nullish(receiver)? { g.0.heap().undefined() } else { g.get(receiver,key) }
}
fn flatten(g:&DisplayGraph<'_>,value:&ValueHandle)->Result<ValueHandle>{
    let replace=g.get(value,"replace")?;
    let regex=g.0.heap().regexp(r"\r\n?|\n".into(),"g")?;
    g.0.heap().call(&replace,value,&[regex,ui::literal(g," ")?],g.runtime())
}
fn pattern_preview(g:&DisplayGraph<'_>,pattern:&ValueHandle)->Result<ValueHandle>{
    let collapsed=if g.nullish(pattern)? {g.0.heap().undefined()?}else{
        let replace=g.get(pattern,"replace")?;
        let regex=g.0.heap().regexp(r"\s+".into(),"g")?;
        let replaced=g.0.heap().call(&replace,pattern,&[regex,ui::literal(g," ")?],g.runtime())?;
        g.method(&replaced,"trim",&[])?
    };
    if g.truth(&collapsed)? {Ok(collapsed)}else{g.0.heap().undefined()}
}
fn status_line(g:&DisplayGraph<'_>,theme:&ValueHandle,description:&ValueHandle,meta:Vec<ValueHandle>)->Result<ValueHandle>{
    let heap=g.0.heap();
    let icon=ui::status_icon(g,theme,"pending",&heap.undefined()?)?;
    // In each original theme.fg(...), Get fg occurs before evaluating its
    // programmable flattened argument or meta.join(theme.sep.dot).
    let fg=g.get(theme,"fg")?;
    let title=flatten(g,&ui::literal(g,"AST Edit")?)?;
    let title=heap.call(&fg,theme,&[ui::literal(g,"accent")?,title],g.runtime())?;
    let mut line=if g.truth(&icon)? {ui::concat(g,&[icon,ui::literal(g," ")?,title])?}else{title};
    if g.truth(description)? {
        let fg=g.get(theme,"fg")?;
        let value=flatten(g,description)?;
        let value=heap.call(&fg,theme,&[ui::literal(g,"muted")?,value],g.runtime())?;
        line=ui::concat(g,&[line,ui::literal(g,": ")?,value])?;
    }
    let mut nonempty=Vec::new();
    for value in meta {
        let value=flatten(g,&value)?;
        let trimmed=g.method(&value,"trim",&[])?;
        let length=g.get(&trimmed,"length")?;
        if heap.to_number(&length,g.runtime())?>0.0 {nonempty.push(value)}
    }
    if !nonempty.is_empty() {
        let fg=g.get(theme,"fg")?;
        let array=heap.array(nonempty)?;
        let join=g.get(&array,"join")?;
        let dot=g.get(&g.get(theme,"sep")?,"dot")?;
        let joined=heap.call(&join,&array,&[dot],g.runtime())?;
        let value=heap.call(&fg,theme,&[ui::literal(g,"dim")?,joined],g.runtime())?;
        line=ui::concat(g,&[line,ui::literal(g," ")?,value])?;
    }
    Ok(line)
}
pub(crate) fn call(g:&DisplayGraph<'_>,args:&ValueHandle,theme:&ValueHandle)->Result<Component>{
    let mut meta=Vec::new();
    let paths=g.get(args,"paths")?;
    if g.truth(&optional_get(g,&paths,"length")?)? {
        let paths=g.get(args,"paths")?;
        let join=g.get(&paths,"join")?;
        let paths=g.0.heap().call(&join,&paths,&[ui::literal(g,", ")?],g.runtime())?;
        meta.push(ui::concat(g,&[ui::literal(g,"in ")?,paths])?);
    }
    let ops=g.get(args,"ops")?;
    let length=optional_get(g,&ops,"length")?;
    let count=if g.nullish(&length)? {g.0.heap().number(0.0)?}else{length};
    if g.0.heap().to_number(&count,g.runtime())?>1.0 {
        meta.push(ui::concat(g,&[count.clone(),ui::literal(g," rewrites")?])?);
    }
    let description=if matches!(g.0.heap().snapshot(&count)?,ValueSnapshot::Number(value) if value==1.0) {
        let ops=g.get(args,"ops")?;
        let first=optional_get(g,&ops,"0")?;
        pattern_preview(g,&optional_get(g,&first,"pat")?)?
    }else if g.truth(&count)? {
        ui::concat(g,&[count,ui::literal(g," rewrites")?])?
    }else{ui::literal(g,"?")?};
    Ok(Component::text(status_line(g,theme,&description,meta)?))
}

// Original B-7e4e746473, fixed write-xdev-dispatch.test.ts:299-341.
// This display body needs no AST process or executable local tool supplier.
#[cfg(test)]
pub(crate) fn observe_original_call(){
    use super::{theme::Theme,types::{RenderOptions,FrameContext,EditRenderArgs},tool_execution::ToolExecutionComponent,write_renderer,xdev_renderer::XdevComponent,render_utils};
    use ara_rpc::WireString;
    use std::sync::Arc;
    let realm=crate::native_realm::NativeRealm::prepare().unwrap();
    let callbacks=realm.begin_display_callbacks().unwrap();
    let runtime=callbacks.runtime().unwrap();
    let graph=DisplayGraph(&realm.values,&runtime);let heap=realm.values.heap();
    let theme=Arc::new(Theme::dark());
    let options=RenderOptions{expanded:false,is_partial:true,..Default::default()};
    let content=r#"{"ops":[{"pat":"legacyWrap($A, $B)","out":"modernWrap($A, $B)"}],"paths":["/tmp/legacy.ts"]}"#;
    let partial=heap.import_serde(&serde_json::json!({"path":"xd://ast_e"})).unwrap();
    let args=heap.import_serde(&serde_json::json!({"path":"xd://ast_edit","content":content})).unwrap();
    let rows=|root:&ValueHandle,started:bool,complete:bool|{
        let parsed=write_renderer::args_from_graph(&graph,root).unwrap();
        let direct=XdevComponent::build(&graph,&parsed,None,options,started,complete,&theme,None).unwrap();
        let direct=direct.map(|mut value|value.render(&graph,80,&theme).unwrap()).unwrap_or_default();
        let mut write=ToolExecutionComponent::new("B-7e4e746473".into(),"write".into(),EditRenderArgs::default(),ara_edit::EditMode::Hashline,theme.clone());
        write.graph_args_root=Some(root.clone());write.component.options=options;
        write.args_complete=complete;write.execution_started=started;
        assert!(write.rebuild_in(&graph).is_empty(),"original AST WRITE construction failed");
        let rows=write.render_in(&graph,80,&FrameContext::default(),0,usize::MAX,std::time::Instant::now()).unwrap();
        assert_eq!(rows,direct,"actual WRITE and its original Component rows differ");
        (rows.clone(),rows.iter().map(|row|render_utils::sanitize(&row.as_str().into())).collect::<Vec<_>>().join("\n"))
    };
    eprintln!("[original22] B-7e4e746473 begin");
    assert!(rows(&partial,false,false).0.is_empty(),"partial xd:// URL must be hidden");
    let queued=rows(&args,false,false);
    assert!(!queued.0.is_empty());assert!(queued.1.contains("queued"));assert!(queued.1.contains("ast_edit"));
    assert!(rows(&args,false,true).1.contains("queued"));
    let executing=rows(&args,true,true);
    assert!(!executing.0.is_empty());assert!(!executing.1.contains("queued"));
    // Supplemental supplier identity checks, not additional original IDs.
    assert!(executing.1.contains("AST Edit"),"generic card is not the AST renderer");
    assert!(executing.1.contains("legacyWrap($A, $B)"));
    assert!(executing.1.contains("in /tmp/legacy.ts"));
    let _original_wire_content=WireString::from(content);
    eprintln!("[original22] B-7e4e746473 assertions passed");
}
