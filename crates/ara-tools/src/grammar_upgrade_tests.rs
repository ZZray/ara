//! G09 actual Wasmtime and public tool consumers. NOT RUN in candidate preparation.
use super::*;
use std::sync::atomic::{AtomicUsize,Ordering};
use crate::{grammars::GrammarHostPort,ast_grep::{AstGrepTool,AstGrepOsValues},write::WriteTool};
const INI:&[u8]=include_bytes!("../../vendor/ara-ast/src/language/testdata/ini.wasm");
fn text(output:&ToolOutput)->String {
    output.content.iter().filter_map(|part|match part {ara_ai::UserBlock::Text(text)=>Some(text.text.as_str()),_=>None}).collect::<Vec<_>>().join("\n")
}
struct Installer {dir:PathBuf,calls:AtomicUsize}
#[async_trait]
impl GrammarHostPort for Installer {
    async fn ensure_grammars(&self,languages:&[String],cancel:CancellationToken)->Result<bool,ToolError> {
        assert!(!cancel.is_cancelled());self.calls.fetch_add(1,Ordering::SeqCst);
        if languages.iter().any(|name|name=="ini") {
            std::fs::write(self.dir.join(ara_ast::language::wasm_grammars::INI.file),INI).unwrap();Ok(true)
        } else {Ok(false)}
    }
}
#[tokio::test]
async fn grammar_preview_search_missing_aggregate_once_and_fresh_apply_bank() {
    let dir=tempfile::tempdir().unwrap();let cache=tempfile::tempdir().unwrap();
    ara_ast::language::grammar::set_grammar_dir(cache.path().to_owned());
    std::fs::write(dir.path().join("calls.ts"),"foo(1);\n").unwrap();
    std::fs::write(dir.path().join("config.ini"),"[core]\nname = ara\n").unwrap();
    std::fs::write(dir.path().join("main.tf"),"name = \"ara\"\n").unwrap();
    let ctx=ToolContext::new(dir.path()).with_edit(ara_edit::EditMode::Hashline,true);
    let host=Arc::new(Installer {dir:cache.path().to_owned(),calls:AtomicUsize::new(0)});ctx.set_grammar_host(host.clone());
    let preview_cancel=CancellationToken::new();
    let output=AstEditTool {ctx:ctx.clone()}.execute_os_values(AstEditOsValues {
        ops:vec![("foo($A)".into(),"bar($A)".into())],paths:vec![".".into()],
    },preview_cancel.clone()).await.unwrap();
    assert_eq!(host.calls.load(Ordering::SeqCst),1,"aggregate preview acquisition once only");
    assert_eq!(std::fs::read_to_string(dir.path().join("calls.ts")).unwrap(),"foo(1);\n");
    assert_eq!(output.details.as_ref().unwrap()["missingGrammars"],json!(["hcl"]));
    assert!(text(&output).contains("grammar could not be downloaded"));
    assert_eq!(output.details.as_ref().unwrap()["totalReplacements"],1);
    // The apply is a different call with a fresh current context and lifetime.
    preview_cancel.cancel();
    let fresh=ctx.clone().with_edit(ara_edit::EditMode::Patch,false);
    let applied=WriteTool::new(fresh).execute_os_values("xd://resolve","reviewed AST change",CancellationToken::new()).await.unwrap();
    assert_eq!(std::fs::read_to_string(dir.path().join("calls.ts")).unwrap(),"bar(1);\n");
    assert_eq!(host.calls.load(Ordering::SeqCst),1,"apply never downloads or replays");
    assert!(!text(&applied).contains("[calls.ts#"),"use current Write display policy");
    assert!(text(&applied).contains("grammar could not be downloaded"));
    // Actual read-only consumer is reached from AgentTool and Native paths.
    let searched=AstGrepTool {ctx:ctx.clone()}.execute_os_values(AstGrepOsValues {
        pattern:"[core]\nname = ara\n".into(),language:Some("ini".into()),path:Some("config.ini".into()),skip:0.0,
    },CancellationToken::new()).await.unwrap();
    assert!(searched.details.as_ref().unwrap()["matchCount"].as_u64().unwrap()>0,"actual installed wasm search matches");
    assert!(searched.details.as_ref().unwrap().get("missingGrammars").is_none());
    let misses=AstGrepTool {ctx}.execute_os_values(AstGrepOsValues {
        pattern:"bar($A)".into(),language:None,path:Some(".".into()),skip:0.0,
    },CancellationToken::new()).await.unwrap();
    assert_eq!(misses.details.as_ref().unwrap()["missingGrammars"],json!(["hcl"]));
    assert!(text(&misses).contains("grammar could not be downloaded"));
}

#[test]
fn grammar_target_aggregate_keeps_missing_languages_unique_bank() {
    let mut aggregate=AstResult {applied:true,..Default::default()};
    for missing in [vec!["ini".into(),"hcl".into()],vec!["hcl".into(),"ruby".into()]] {
        ast_edit_engine::merge_native_target(&mut aggregate,AstResult {applied:false,missing_grammars:missing,..Default::default()});
    }
    assert_eq!(aggregate.missing_grammars,["ini","hcl","ruby"]);assert!(!aggregate.applied);
}

struct MappingPort {path:std::sync::RwLock<Option<PathBuf>>,calls:AtomicUsize}
#[async_trait]
impl crate::ContentUriPort for MappingPort {
    fn route(&self,url:&str)->crate::ContentUriRoute {
        if url=="artifact://current" {crate::ContentUriRoute::Registered {writable:true}} else {crate::ContentUriRoute::Unregistered}
    }
    async fn search_path_only(&self,_:&str,cancel:CancellationToken)->Result<Option<PathBuf>,ToolError> {
        assert!(!cancel.is_cancelled(),"apply must not inherit preview signal");
        self.calls.fetch_add(1,Ordering::SeqCst);Ok(self.path.read().unwrap().clone())
    }
    async fn read(&self,_:&str,_:CancellationToken)->Result<crate::UriResource,ToolError> {panic!("AST uses pathOnly")}
    async fn write(&self,_:&str,_:&str,_:CancellationToken)->Result<(),ToolError> {panic!("AST writes selected backing path")}
}
#[tokio::test]
async fn grammar_apply_resolves_current_uri_backing_and_removed_mapping_bank() {
    for removed in [false,true] {
        let dir=tempfile::tempdir().unwrap();let old=dir.path().join("old.ts");let new=dir.path().join("current.ts");
        for path in [&old,&new] {std::fs::write(path,"foo();\n").unwrap();}
        let port=Arc::new(MappingPort {path:std::sync::RwLock::new(Some(old.clone())),calls:AtomicUsize::new(0)});
        let ctx=ToolContext::new(dir.path()).with_uri_port(port.clone());let cancel=CancellationToken::new();
        let preview=AstEditTool {ctx:ctx.clone()}.execute_os_values(AstEditOsValues {
            ops:vec![("foo()".into(),"bar()".into())],paths:vec!["artifact://current".into()],
        },cancel.clone()).await.unwrap();
        assert_eq!(preview.details.as_ref().unwrap()["totalReplacements"],1);assert_eq!(port.calls.load(Ordering::SeqCst),1);
        let pending=ctx.pending_resolution.pending_head().unwrap().0;
        *port.path.write().unwrap()=if removed {None} else {Some(new.clone())};cancel.cancel();
        let output=WriteTool::new(ctx.clone()).execute_os_values("xd://resolve","apply current backing context",CancellationToken::new()).await;
        assert_eq!(port.calls.load(Ordering::SeqCst),2);
        assert_eq!(std::fs::read_to_string(&old).unwrap(),"foo();\n","old preview backing is never written");
        if removed {
            assert!(output.unwrap_err().0.contains("without a backing file"));
            assert_eq!(ctx.pending_resolution.pending_head().unwrap().0,pending);
            assert_eq!(std::fs::read_to_string(new).unwrap(),"foo();\n");
        } else {
            let output=output.unwrap();assert!(output.is_error,"changed backing path is reported stale after settled apply");
            assert_eq!(std::fs::read_to_string(new).unwrap(),"bar();\n");
            assert!(ctx.pending_resolution.pending_head().is_none());
        }
    }
}
