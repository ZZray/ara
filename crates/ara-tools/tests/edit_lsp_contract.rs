//! Additive rich-result/once-version/live-dedup bank. Controlled supplier only.
use ara_agent::AgentTool;
use ara_ai::UserBlock;
use ara_tools::{ToolContext,edit::EditTool,write::WriteTool,edit_host::*};
use async_trait::async_trait;
use serde_json::{Value,json};
use std::{path::{Path,PathBuf},sync::{Arc,Mutex,atomic::{AtomicBool,Ordering}}};
use tokio_util::sync::CancellationToken;

#[derive(Default)]
struct Host { state:Arc<EditDiagnosticsSession>,dedup:Arc<AtomicBool>, format:AtomicBool,diagnostics:AtomicBool,bridge:AtomicBool,calls:Arc<Mutex<Vec<(bool,bool,bool)>>> }
impl EditHostPort for Host {
    fn lsp_options(&self)->EditLspOptions { EditLspOptions {enable_format:self.format.load(Ordering::SeqCst),enable_diagnostics:self.diagnostics.load(Ordering::SeqCst),deduplicate_diagnostics:self.dedup.load(Ordering::SeqCst)} }
    fn lsp_writethrough(&self)->Option<Arc<dyn EditLspWritethrough>> {Some(Arc::new(Supplier {calls:self.calls.clone()}))}
    fn client_bridge(&self)->Option<Arc<dyn EditClientBridge>> {self.bridge.load(Ordering::SeqCst).then(||Arc::new(FormattingBridge) as Arc<dyn EditClientBridge>)}
    fn deferred_diagnostics_host(&self)->Option<Arc<dyn EditDeferredDiagnosticsHost>> {Some(self.state.clone())}
    fn bump_file_mutation_version(&self,p:&Path){self.state.bump_file_mutation_version(p);}
}
struct FormattingBridge;
#[async_trait]
impl EditClientBridge for FormattingBridge {
    fn can_write_text_file(&self)->bool {true}
    async fn write_text_file(&self,path:&Path,content:&str)->std::io::Result<()> {
        std::fs::write(path,content.replace("return 2;","return 2000;"))
    }
}
struct Supplier {calls:Arc<Mutex<Vec<(bool,bool,bool)>>>}
#[async_trait]
impl EditLspWritethrough for Supplier {
    async fn write_through(&self,r:EditLspWriteRequest<'_>)->std::io::Result<Option<Value>>{ Ok(self.write_through_result(r).await?.diagnostics) }
    async fn write_through_result(&self,r:EditLspWriteRequest<'_>)->std::io::Result<EditLspWritethroughResult>{
        let deferred=r.get_deferred.as_ref().and_then(|get|get(r.absolute_path));
        self.calls.lock().unwrap().push((r.options.enable_diagnostics,r.get_deferred.is_some(),deferred.is_some()));
        let written=if r.options.enable_format {r.content.replace("return 2;","return 20;")}else{r.content.to_owned()};
        std::fs::write(r.absolute_path,&written)?;
        let d=json!({"server":"controlled","messages":[],"summary":"OK","errored":false});
        if let Some(deferred)=deferred { deferred.finalize(Some(&d)); }
        Ok(EditLspWritethroughResult { diagnostics:Some(d),final_content:Some(written) })
    }
    async fn notify_watched_files(&self,_:&Path,_:&[EditFileChange],_:Option<CancellationToken>)->std::io::Result<()>{Ok(())}
    async fn flush_batch(&self,_:&str,_:&Path,_:Option<CancellationToken>)->std::io::Result<Option<Value>>{Ok(None)}
}
fn text(output:&ara_agent::ToolOutput)->String{output.content.iter().filter_map(|b|match b{UserBlock::Text(t)=>Some(t.text.as_str()),_=>None}).collect::<Vec<_>>().join("\n")}

#[tokio::test]
async fn same_tool_current_formatter_final_content_tag_and_single_mutation_version(){
    let dir=tempfile::tempdir().unwrap();let root=dir.path();let path=root.join("value.ts");
    let host=Arc::new(Host::default());let ctx=ToolContext::new(root).with_edit(ara_edit::EditMode::Replace,true).with_edit_host(host.clone());
    std::fs::write(&path,"export function value() { return 1; }\n").unwrap();
    let edit=EditTool::new(ctx.clone());host.format.store(true,Ordering::SeqCst);
    let result=edit.execute("rich",json!({"path":"value.ts","old_string":"return 1;","new_string":"return 2;"}).as_object().unwrap().clone(),CancellationToken::new(),Arc::new(|_|{})).await.unwrap();
    assert!(!result.is_error);assert_eq!(std::fs::read_to_string(&path).unwrap(),"export function value() { return 20; }\n");
    assert!(serde_json::to_string(&result.details).unwrap().contains("return 20;"),"engine snapshots/diff consume supplier final bytes");
    assert_eq!(host.state.get_file_mutation_version(&path),1,"begin already claimed this edit");
    assert_eq!(*host.calls.lock().unwrap(),vec![(false,true,true)],"EDIT retains its target begin callback even with diagnostics disabled");
    // Construct once while diagnostics are initially off; retain the owner so
    // enabling diagnostics on this same WriteTool later supplies its callback.
    let write=WriteTool::new(ctx.clone());host.format.store(false,Ordering::SeqCst);
    let requested="// 中😀\nreturn 2;\n";
    let result=write.execute("plain",json!({"path":"value.ts","content":requested}).as_object().unwrap().clone(),CancellationToken::new(),Arc::new(|_|{})).await.unwrap();
    assert_eq!(std::fs::read_to_string(&path).unwrap(),requested);
    assert_eq!(text(&result),format!("Successfully wrote {} bytes to value.ts",requested.len()));
    assert_ne!(requested.len(),requested.encode_utf16().count());
    assert_eq!(host.state.get_file_mutation_version(&path),2,"disabled callback uses exactly one fallback mutation bump");
    assert_eq!(host.state.pending_count(),0);
    host.format.store(true,Ordering::SeqCst);host.diagnostics.store(true,Ordering::SeqCst);
    let formatted=requested.replace("return 2;","return 20;");
    let result=write.execute("formatted",json!({"path":"value.ts","content":requested}).as_object().unwrap().clone(),CancellationToken::new(),Arc::new(|_|{})).await.unwrap();
    assert_eq!(std::fs::read_to_string(&path).unwrap(),formatted);
    assert_eq!(text(&result),format!("Successfully wrote {} bytes to value.ts",formatted.len()));
    assert_ne!(formatted.len(),requested.len(),"formatter changes final byte count");
    assert_eq!(host.state.get_file_mutation_version(&path),3,"enabled begin owns exactly one mutation bump");
    host.diagnostics.store(false,Ordering::SeqCst);
    let result=write.execute("disabled-again",json!({"path":"value.ts","content":requested}).as_object().unwrap().clone(),CancellationToken::new(),Arc::new(|_|{})).await.unwrap();
    assert_eq!(text(&result),format!("Successfully wrote {} bytes to value.ts",formatted.len()));
    assert_eq!(host.state.get_file_mutation_version(&path),4,"later disablement returns to exactly one fallback bump");
    assert_eq!(host.state.pending_count(),0);
    assert_eq!(*host.calls.lock().unwrap(),vec![(false,true,true),(false,false,false),(true,true,true),(false,false,false)],"per-call diagnostics option, callback presence and returned handle track the live WRITE setting");
    host.bridge.store(true,Ordering::SeqCst);
    let result=write.execute("acp",json!({"path":"value.ts","content":requested}).as_object().unwrap().clone(),CancellationToken::new(),Arc::new(|_|{})).await.unwrap();
    let bridge_final=requested.replace("return 2;","return 2000;");
    assert_eq!(std::fs::read_to_string(&path).unwrap(),bridge_final);
    assert_ne!(bridge_final.len(),requested.len());
    assert_eq!(text(&result),format!("Successfully wrote {} bytes to value.ts",requested.len()),"ACP retains requested cleanContent UTF8 byte receipt despite client format-on-save drift");
    assert_eq!(host.state.get_file_mutation_version(&path),5);
    assert_eq!(host.calls.lock().unwrap().len(),4,"bridge path does not invoke ordinary LSP write supplier");
}

#[test]
fn in_flight_handle_reads_current_dedup_and_keeps_mutation_version(){
    let state=Arc::new(EditDiagnosticsSession::default());let dedup=Arc::new(AtomicBool::new(true));
    let owner=EditDeferredDiagnostics::with_live_dedup(state.clone(),{let dedup=dedup.clone();Arc::new(move||dedup.load(Ordering::SeqCst))});
    let path=PathBuf::from("late.ts");let diagnostics=json!({"messages":["late.ts:1:1 [error] unchanged"],"summary":"1 error(s)","errored":true});
    state.reduce_diagnostics(&path,diagnostics.clone()).unwrap();
    let first=owner.begin(&path);first.finalize(None);dedup.store(false,Ordering::SeqCst);
    assert!(first.on_deferred_diagnostics(diagnostics.clone()).unwrap());
    let entries=state.drain_lazy().survivors();assert_eq!(entries.len(),1);assert!(!entries[0].is_stale());
    let second=owner.begin(&path);second.finalize(None);dedup.store(true,Ordering::SeqCst);
    assert!(!second.on_deferred_diagnostics(diagnostics).unwrap());
    assert_eq!(state.pending_count(),0);assert!(entries[0].is_stale());
}
