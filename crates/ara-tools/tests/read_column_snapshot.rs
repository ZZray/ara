//! Complete fixed OMP596f2da read-column-truncation-snapshot original5.
use ara_agent::{AgentTool, ToolOutput, UpdateFn};
use ara_ai::{JsonObject, UserBlock};
use ara_tools::{ToolContext, read::ReadTool, edit::EditTool};
use serde_json::json;
use std::{path::Path, sync::Arc};
use tokio_util::sync::CancellationToken;

const COLUMN_CAP:usize=64;
const LONG_LINE_LEN:usize=COLUMN_CAP*3;
fn text(output:&ToolOutput)->String {output.content.iter().filter_map(|c|match c{UserBlock::Text(t)=>Some(t.text.clone()),_=>None}).collect::<Vec<_>>().join("\n")}
fn args(v:serde_json::Value)->JsonObject{v.as_object().unwrap().clone()}
fn noop()->UpdateFn{Arc::new(|_|{})}
fn session(root:&Path)->ToolContext{ToolContext::new(root).with_edit(ara_edit::EditMode::Hashline,true).with_read_output_max_columns(COLUMN_CAP as f64)}
async fn read(ctx:&ToolContext,path:String)->ToolOutput{ReadTool{ctx:ctx.clone()}.execute("call-read",args(json!({"path":path})),CancellationToken::new(),noop()).await.unwrap()}
fn header(text:&str)->(String,String){
    let re=regex::Regex::new(r"(?m)^\[([^#\r\n]+)#([0-9A-F]{4})\]$").unwrap();
    let captures=re.captures(text).unwrap_or_else(||panic!("no hashline header in:\n{text}"));
    (captures[0].to_owned(),captures[2].to_owned())
}
async fn edit(ctx:&ToolContext,header:&str,patch:&str){
    EditTool::new(ctx.clone()).execute("call-edit",args(json!({"input":format!("{header}\n{patch}")})),CancellationToken::new(),noop()).await.unwrap();
}

#[tokio::test]
async fn original_column_truncation_snapshot_five_cases(){
    let dir=tempfile::tempdir().unwrap();let root=dir.path();
    // B-c12d9892b9: snapshot keeps untruncated content for a full-file read.
    {
        let path=root.join("wide.txt");let long="x".repeat(LONG_LINE_LEN);let full=format!("first short line\n{long}\nthird short line\n");std::fs::write(&path,&full).unwrap();
        let ctx=session(root);let result=read(&ctx,path.to_string_lossy().into_owned()).await;let body=text(&result);
        assert!(body.contains('…'));assert!(!body.contains(&long));let (_,tag)=header(&body);
        let snapshot=ctx.edit_store.by_hash(&ara_edit::path_policy::canonical_key(&path),&tag);assert!(snapshot.is_some());
        let snapshot=snapshot.unwrap();assert_eq!(snapshot.text.as_ref(),full);assert_eq!(snapshot.text.split('\n').nth(1),Some(long.as_str()));
    }
    // B-07647d3b4b: range read snapshot keeps untruncated content.
    {
        let path=root.join("wide-range.txt");let long="y".repeat(LONG_LINE_LEN);let full=format!("head\n{long}\ntail\n");std::fs::write(&path,&full).unwrap();
        let ctx=session(root);let body=text(&read(&ctx,format!("{}:1-3",path.display())).await);assert!(body.contains('…'));let (_,tag)=header(&body);
        let snapshot=ctx.edit_store.by_hash(&ara_edit::path_policy::canonical_key(&path),&tag).unwrap();assert_eq!(snapshot.text.split('\n').nth(1),Some(long.as_str()));
    }
    // B-22b739a6e7: multi-range read snapshot retains both original long rows.
    {
        let path=root.join("wide-multi.txt");let long="z".repeat(LONG_LINE_LEN);let full=["a",&long,"c","d","e",&long,"g"].join("\n");std::fs::write(&path,&full).unwrap();
        let ctx=session(root);let body=text(&read(&ctx,format!("{}:1-2,6-7",path.display())).await);assert!(body.contains('…'));let (_,tag)=header(&body);
        let snapshot=ctx.edit_store.by_hash(&ara_edit::path_policy::canonical_key(&path),&tag).unwrap();assert_eq!(snapshot.text.split('\n').nth(1),Some(long.as_str()));assert_eq!(snapshot.text.split('\n').nth(5),Some(long.as_str()));
    }
    // B-e98685d5db: actual hashline edit without a second READ.
    {
        let path=root.join("editable-wide.txt");let long="w".repeat(LONG_LINE_LEN);let original=format!("intro\n{long}\noutro\n");std::fs::write(&path,&original).unwrap();
        let ctx=session(root);let body=text(&read(&ctx,path.to_string_lossy().into_owned()).await);let (header,_)=header(&body);
        edit(&ctx,&header,"PUT 3-3:\n+epilogue\n").await;assert_eq!(std::fs::read_to_string(&path).unwrap(),format!("intro\n{long}\nepilogue\n"));
    }
    // B-94e67a6bc4: genuine blank row editable; terminal EOF sentinel hidden.
    {
        let path=root.join("eof-blank.txt");std::fs::write(&path,"first\n\nlast\n").unwrap();let ctx=session(root);
        let body=text(&read(&ctx,path.to_string_lossy().into_owned()).await);assert!(body.contains("1:first\n2:\n3:last"));assert!(!body.contains("\n4:"));
        let (header,_)=header(&body);edit(&ctx,&header,"CUT 2\n").await;assert_eq!(std::fs::read_to_string(&path).unwrap(),"first\nlast\n");
    }
    // Supplemental cap contract boundaries inside the original coarse group.
    {
        let path=root.join("cap-boundaries.txt");std::fs::write(&path,"abcdef\nnext\n").unwrap();
        let ctx=session(root).with_read_output_max_columns(2.5);let output=read(&ctx,path.to_string_lossy().into_owned()).await;let body=text(&output);
        assert!(body.contains("1:ab…"));assert_eq!(output.details.as_ref().unwrap()["meta"]["limits"]["columnTruncated"]["maxColumn"],json!(2.5));
        let raw=text(&read(&ctx,format!("{}:raw",path.display())).await);assert!(raw.contains("abcdef"));assert!(!raw.contains('…'));
        let off=ctx.clone().with_read_output_max_columns(0.0);assert!(text(&read(&off,path.to_string_lossy().into_owned()).await).contains("abcdef"));
        std::fs::write(&path,"A😀B\n").unwrap();let boundary=ctx.clone().with_read_output_max_columns(3.0);assert!(text(&read(&boundary,path.to_string_lossy().into_owned()).await).contains("A😀…"));
        let split=ctx.with_read_output_max_columns(2.0);let error=ReadTool{ctx:split}.execute("surrogate",args(json!({"path":path})),CancellationToken::new(),noop()).await.unwrap_err();assert!(error.0.contains("splits a UTF-16 surrogate pair"));
    }
}
