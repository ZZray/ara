//! Target b91db66e G05: real Host + real controlled LSP child. No real model.
use ara_cli::host_lsp::{client::ClientPool, config::{LspConfig, ServerConfig}, writethrough::HostLspWritethrough};
use ara_tools::edit_host::*;
use serde_json::{Value,json};
use std::{collections::BTreeMap, path::{Path,PathBuf}, sync::Arc};

fn fixture() -> PathBuf {
    std::env::var_os("ARA_LSP_FIXTURE_PATH").map(PathBuf::from).unwrap_or_else(|| {
        std::env::current_exe().unwrap().parent().unwrap().parent().unwrap()
            .join(if cfg!(windows) { "ara-lsp-fixture.exe" } else { "ara-lsp-fixture" })
    })
}
fn pool(root: &Path, mode: &str) -> Arc<ClientPool> {
    let executable = fixture(); assert!(executable.is_file(), "compiled existing LSP fixture is required: {}", executable.display());
    let server = ServerConfig {
        name:"typescript-fixture".into(), custom:false, command:executable.to_string_lossy().into_owned(), resolved_command:executable,
        args:vec!["--mode".into(),mode.into(),"--log".into(),root.join("frames.jsonl").to_string_lossy().into_owned()],
        env:BTreeMap::new(),file_types:vec!["fixture".into()],root_markers:vec![".".into()],language_id:None,
        is_linter:false,init_options:json!({}),settings:json!({}),warmup_timeout_ms:Some(5000),workspace_ready_timings:json!({}),raw:json!({}),
    };
    ClientPool::new(root.to_owned(),root.join("agent"),LspConfig { servers:vec![server],idle_timeout_ms:None,source_snapshot:vec![],unresolved_plugin_sources:false },false)
}
async fn write(writer: &dyn EditLspWritethrough, root: &Path, path: &Path, content: &str, batch: Option<EditLspBatch>) -> EditLspWritethroughResult {
    writer.write_through_result(EditLspWriteRequest {
        cwd:root,absolute_path:path,content,options:EditLspOptions { enable_format:true,enable_diagnostics:false,deduplicate_diagnostics:true },
        batch,cancel:None,get_deferred:None,transform_diagnostics:None,file_fallbacks:None,mutation_session_id:None,
    }).await.unwrap()
}
#[tokio::test(flavor="multi_thread",worker_threads=2)]
async fn created_formatted_result_batch_requested_path_reload_and_external_reconcile() {
    let directory=tempfile::tempdir().unwrap();let root=directory.path();std::fs::create_dir(root.join("agent")).unwrap();
    let pool=pool(root,"pull-tsserver");let writer=HostLspWritethrough::new(pool.clone());
    let path=root.join("created.fixture");
    let result=write(writer.as_ref(),root,&path,"UNFORMATTED ERROR\n",None).await;
    assert_eq!(result.final_content.as_deref(),Some("FORMATTED ERROR\n"));
    assert_eq!(std::fs::read_to_string(&path).unwrap(),"FORMATTED ERROR\n");
    assert_eq!(result.diagnostics.as_ref().unwrap()["formatter"],"formatted");
    let rows=std::fs::read_to_string(root.join("frames.jsonl")).unwrap().lines().map(|line|serde_json::from_str::<Value>(line).unwrap()).collect::<Vec<_>>();
    let reload=rows.iter().position(|row|row["frame"]["message"]["method"]=="workspace/executeCommand").unwrap();
    let open=rows.iter().position(|row|row["frame"]["event"]=="disk-at-open").unwrap();
    assert!(reload<open);assert_eq!(rows[open]["frame"]["content"],"UNFORMATTED ERROR\n","new path exists before first didOpen");
    let second=root.join("other.fixture");
    let batch=|flush|Some(EditLspBatch{id:"requested-first".into(),flush});
    write(writer.as_ref(),root,&path,"UNFORMATTED first\n",batch(false)).await;
    write(writer.as_ref(),root,&second,"UNFORMATTED other\n",batch(false)).await;
    let flushed=write(writer.as_ref(),root,&path,"UNFORMATTED requested\n",batch(true)).await;
    assert_eq!(flushed.final_content.as_deref(),Some("FORMATTED requested\n"));
    assert_eq!(std::fs::read_to_string(&second).unwrap(),"FORMATTED other\n");
    std::fs::write(&path,"WARNING external\n").unwrap();
    let servers=pool.servers_for_file(&path);
    assert_eq!(pool.reconcile_file_from_disk(&path,&servers,None).await.unwrap().len(),1);
    assert!(pool.reconcile_file_from_disk(&path,&servers,None).await.unwrap().is_empty());
    let diagnostics=pool.diagnostics(&path,&servers,None,None,1000,None).await.unwrap().unwrap();
    assert!(diagnostics["messages"].to_string().contains("fixture warning"));
    writer.shutdown().await;pool.shutdown().await.unwrap();
}
