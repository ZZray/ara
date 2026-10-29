//! Fixed OMP five blackbox test rows plus production after-apply composition.
//! Real temp files; controlled Host/model ports are not real model acceptance.
use ara_agent::{AgentTool, ToolOutput};
use ara_ai::UserBlock;
use ara_edit::EditMode;
use ara_tools::edit_host::*;
use ara_tools::{ToolContext, edit::EditTool, edit_post_apply::*};
use async_trait::async_trait;
use serde_json::{Value, json};
use std::{
    path::PathBuf,
    sync::{
        Arc, Mutex,
        atomic::{AtomicUsize, Ordering},
    },
};
use tokio_util::sync::CancellationToken;

const SOURCE: &str = "export function value(): number {\n\treturn 1;\n}\n";
const BROKEN: &str = "export function value(): number {\n\treturn (;\n}\n";
struct Host {
    dir: PathBuf,
    enabled: bool,
    repair: Option<Arc<Completion>>,
    recover: Option<PathBuf>,
    events: Arc<Mutex<Vec<String>>>,
}
#[async_trait]
impl EditPostApplyHost for Host {
    fn agent_dir(&self) -> PathBuf {
        self.dir.clone()
    }
    fn blackbox_enabled(&self) -> bool {
        self.enabled
    }
    fn active_model_string(&self) -> Option<String> {
        Some("openai/gpt-5.6".into())
    }
    async fn prepare_repair(
        &self,
        _: Option<CancellationToken>,
    ) -> Result<Option<Arc<dyn EditRepairCompletion>>, String> {
        self.events.lock().unwrap().push("prepare-key".into());
        if let Some(path) = &self.recover {
            std::fs::write(path, SOURCE).unwrap();
        }
        Ok(self.repair.clone().map(|r| r as Arc<dyn EditRepairCompletion>))
    }
    fn diagnostic(&self, message: &str) {
        self.events.lock().unwrap().push(message.into());
    }
}
struct Completion {
    calls: AtomicUsize,
    reject: bool,
    events: Arc<Mutex<Vec<String>>>,
}
#[async_trait]
impl EditRepairCompletion for Completion {
    fn model_name(&self) -> String {
        "fixture/smol".into()
    }
    async fn complete(&self, prompt: String, _: CancellationToken) -> Result<String, String> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        self.events.lock().unwrap().push("complete".into());
        if self.reject {
            return Ok("return (;".into());
        }
        Ok(prompt
            .split("AFTER (broken):")
            .nth(1)
            .unwrap()
            .split("```")
            .nth(1)
            .unwrap()
            .trim()
            .replace("return (;", "return (2);"))
    }
}
fn text(output: &ToolOutput) -> String {
    output
        .content
        .iter()
        .filter_map(|b| match b {
            UserBlock::Text(t) => Some(t.text.as_str()),
            _ => None,
        })
        .collect::<Vec<_>>()
        .join("\n")
}
async fn run(tool: &EditTool, arg: Value) -> ToolOutput {
    tool.execute("fixed-family", arg.as_object().unwrap().clone(), CancellationToken::new(), Arc::new(|_| {}))
        .await
        .unwrap()
}
fn context(
    root: &std::path::Path,
    mode: EditMode,
    enabled: bool,
    repair: Option<Arc<Completion>>,
    recover: Option<PathBuf>,
    events: Arc<Mutex<Vec<String>>>,
) -> ToolContext {
    let ctx = ToolContext::new(root).with_edit(mode, true);
    ctx.set_edit_post_apply_host(Arc::new(Host { dir: root.join("agent"), enabled, repair, recover, events }));
    ctx
}

#[derive(Clone, Default)]
struct WriterHost {
    bridge_calls: Arc<AtomicUsize>,
    lsp_calls: Arc<AtomicUsize>,
    invalidations: Arc<AtomicUsize>,
    bumps: Arc<AtomicUsize>,
}
impl EditHostPort for WriterHost {
    fn client_bridge(&self) -> Option<Arc<dyn EditClientBridge>> {
        Some(Arc::new(self.clone()))
    }
    fn lsp_writethrough(&self) -> Option<Arc<dyn EditLspWritethrough>> {
        Some(Arc::new(self.clone()))
    }
    fn invalidate_after_write(&self, _: &std::path::Path) {
        self.invalidations.fetch_add(1, Ordering::SeqCst);
    }
    fn bump_file_mutation_version(&self, _: &std::path::Path) {
        self.bumps.fetch_add(1, Ordering::SeqCst);
    }
}
#[async_trait]
impl EditClientBridge for WriterHost {
    fn can_write_text_file(&self) -> bool {
        true
    }
    async fn write_text_file(&self, path: &std::path::Path, content: &str) -> std::io::Result<()> {
        self.bridge_calls.fetch_add(1, Ordering::SeqCst);
        std::fs::write(path, content)
    }
}
#[async_trait]
impl EditLspWritethrough for WriterHost {
    async fn write_through(&self, request: EditLspWriteRequest<'_>) -> std::io::Result<Option<Value>> {
        self.lsp_calls.fetch_add(1, Ordering::SeqCst);
        assert!(request.batch.is_none(), "repair must not inherit original batch");
        assert!(
            request.cancel.as_ref().is_some_and(|c| !c.is_cancelled()),
            "repair commit retains caller, not cancelled completion child"
        );
        std::fs::write(request.absolute_path, request.content)?;
        Ok(None)
    }
    async fn notify_watched_files(
        &self,
        _: &std::path::Path,
        _: &[EditFileChange],
        _: Option<CancellationToken>,
    ) -> std::io::Result<()> {
        Ok(())
    }
    async fn flush_batch(
        &self,
        _: &str,
        _: &std::path::Path,
        _: Option<CancellationToken>,
    ) -> std::io::Result<Option<Value>> {
        Ok(None)
    }
}

#[tokio::test]
async fn fixed_blackbox_family_five_original_rows_all_modes() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    std::fs::create_dir(root.join("agent")).unwrap();
    let log = root.join("agent/edit-blackbox.jsonl");
    let events = Arc::new(Mutex::new(Vec::new()));
    // B-f0a7d14059: disabled recorder still returns regression warning.
    std::fs::write(root.join("disabled.ts"), SOURCE).unwrap();
    let output = run(
        &EditTool::new(context(root, EditMode::Replace, false, None, None, events.clone())),
        json!({"path":"disabled.ts","old_string":"return 1;","new_string":"return (;"}),
    )
    .await;
    assert!(text(&output).contains("no longer parses after this edit"));
    assert!(!log.exists());
    // B-e4ed6d7aec, B-854c27b1cb, B-e4d7090dd9: clean/already-invalid/empty.
    let ctx = context(root, EditMode::Replace, true, None, None, events.clone());
    let tool = EditTool::new(ctx);
    for (name, source, old, new) in [
        ("valid.ts", SOURCE, "return 1;", "return 2;"),
        ("invalid.ts", "export const value = (;\n", "value", "next"),
        ("empty.ts", SOURCE, SOURCE, ""),
    ] {
        std::fs::write(root.join(name), source).unwrap();
        let output = run(&tool, json!({"path":name,"old_string":old,"new_string":new})).await;
        assert!(!text(&output).contains("no longer parses"));
        assert!(!log.exists());
    }
    assert_eq!(std::fs::read_to_string(root.join("empty.ts")).unwrap(), "");
    // B-34184a0c25: append seed plus exact fields/args for all five modes.
    std::fs::write(&log, "{\"seed\":true}\n").unwrap();
    let mut expected = vec![json!({"seed":true})];
    for (mode, variant) in [
        (EditMode::Replace, "replace"),
        (EditMode::Patch, "patch"),
        (EditMode::ApplyPatch, "apply_patch"),
        (EditMode::Hashline, "hashline"),
        (EditMode::Sloppy, "sloppy"),
    ] {
        let name = format!("{variant}.ts");
        let path = root.join(&name);
        std::fs::write(&path, SOURCE).unwrap();
        let ctx = context(root, mode, true, None, None, events.clone());
        let arg = match mode {
            EditMode::Replace => json!({"path":name,"old_string":"return 1;","new_string":"return (;"}),
            EditMode::Patch => json!({"path":name,"edits":[{"op":"update","diff":"@@\n-\treturn 1;\n+\treturn (;"}]}),
            EditMode::ApplyPatch => {
                json!({"input":format!("*** Begin Patch\n*** Update File: {name}\n@@\n-\treturn 1;\n+\treturn (;\n*** End Patch\n")})
            }
            EditMode::Hashline => {
                let tag = ctx.edit_store.record(&path, SOURCE, None);
                json!({"input":format!("[{name}#{tag}]\nPUT 2-2:\n+\treturn (;")})
            }
            EditMode::Sloppy => {
                json!({"input":format!("*** Edit File: {name}\n*** Find\n\treturn 1;\n*** Replace\n\treturn (;")})
            }
        };
        let output = run(&EditTool::new(ctx), arg.clone()).await;
        assert!(!output.is_error, "{}", text(&output));
        expected.push(json!({"prev":SOURCE,"new":std::fs::read_to_string(path).unwrap(),"model":"openai/gpt-5.6","variant":variant,"arg":arg}));
    }
    let actual: Vec<Value> =
        std::fs::read_to_string(log).unwrap().lines().map(|l| serde_json::from_str(l).unwrap()).collect();
    assert_eq!(actual, expected);
}

#[tokio::test]
async fn after_apply_family_current_disk_repair_rejection_and_diagnostic_only() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    std::fs::create_dir(root.join("agent")).unwrap();
    let events = Arc::new(Mutex::new(Vec::new()));
    let completion = Arc::new(Completion { calls: AtomicUsize::new(0), reject: false, events: events.clone() });
    // Actual EditTool write -> record -> prepare key -> reread disk -> repair
    // -> write -> store invalidate -> original note; initial details retained.
    let path = root.join("repair.ts");
    std::fs::write(&path, SOURCE).unwrap();
    let ctx = context(root, EditMode::Replace, true, Some(completion.clone()), None, events.clone());
    let tag = ctx.edit_store.record(&path, SOURCE, None);
    let store = ctx.edit_store.clone();
    let output =
        run(&EditTool::new(ctx), json!({"path":"repair.ts","old_string":"return 1;","new_string":"return (;"})).await;
    assert!(!output.is_error);
    assert!(text(&output).contains("automatic syntax repair (fixture/smol)"));
    assert!(text(&output).contains("Review the repaired region; adjust it if the repair guessed wrong."));
    assert!(std::fs::read_to_string(&path).unwrap().contains("return (2);"));
    assert!(store.by_hash(&path, &tag).is_none());
    let record: Value = serde_json::from_str(
        std::fs::read_to_string(root.join("agent/edit-blackbox.jsonl")).unwrap().lines().next().unwrap(),
    )
    .unwrap();
    assert_eq!(record["new"], BROKEN);
    assert_eq!(
        output.details.as_ref().unwrap()["newText"],
        BROKEN,
        "initial outcome is not rewritten as repair details"
    );
    // Same-call/on-write recovery is reread after auth; no candidate is asked.
    let path = root.join("recovered.ts");
    std::fs::write(&path, SOURCE).unwrap();
    let calls = completion.calls.load(Ordering::SeqCst);
    let ctx = context(root, EditMode::Replace, false, Some(completion.clone()), Some(path.clone()), events.clone());
    let output =
        run(&EditTool::new(ctx), json!({"path":"recovered.ts","old_string":"return 1;","new_string":"return (;"}))
            .await;
    assert_eq!(completion.calls.load(Ordering::SeqCst), calls);
    assert!(text(&output).contains("Warning:"));
    assert_eq!(std::fs::read_to_string(path).unwrap(), SOURCE);
    // Two rejected candidates leave committed broken edit + warning.
    let reject = Arc::new(Completion { calls: AtomicUsize::new(0), reject: true, events: events.clone() });
    std::fs::write(root.join("rejected.ts"), SOURCE).unwrap();
    let output = run(
        &EditTool::new(context(root, EditMode::Replace, false, Some(reject.clone()), None, events.clone())),
        json!({"path":"rejected.ts","old_string":"return 1;","new_string":"return (;"}),
    )
    .await;
    assert!(!output.is_error);
    assert_eq!(reject.calls.load(Ordering::SeqCst), 2);
    assert!(text(&output).contains("Warning:"));
    assert_eq!(std::fs::read_to_string(root.join("rejected.ts")).unwrap(), BROKEN);
    // Missing agentDir is a logging diagnostic, never a failed applied edit.
    let no_agent = tempfile::tempdir().unwrap();
    std::fs::write(no_agent.path().join("missing.ts"), SOURCE).unwrap();
    let output = run(
        &EditTool::new(context(no_agent.path(), EditMode::Replace, true, None, None, events.clone())),
        json!({"path":"missing.ts","old_string":"return 1;","new_string":"return (;"}),
    )
    .await;
    assert!(!output.is_error);
    assert!(!no_agent.path().join("agent").exists());
    assert!(events.lock().unwrap().iter().any(|e| e.contains("blackbox recording failed")));
    // Original ACP apply may coexist with the captured LSP repair callback.
    // Repair skips ACP/batch and bumps no additional mutation version.
    let writer = Arc::new(WriterHost::default());
    std::fs::write(root.join("routed.ts"), SOURCE).unwrap();
    let ctx = context(root, EditMode::Replace, false, Some(completion), None, events).with_edit_host(writer.clone());
    let tool = EditTool::new(ctx);
    let arg = json!({"path":"routed.ts","old_string":"return 1;","new_string":"return (;"});
    let mut call = tool.call_from_json(&arg.to_string());
    call.set_blackbox_serializer(Arc::new(TypedEditBlackboxSerializer(arg)));
    let output = tool
        .execute_os_call_with_request(
            call,
            ara_edit::ApplyRequest { lsp_batch_id: Some("original-batch".into()), lsp_flush: false },
            CancellationToken::new(),
        )
        .await
        .unwrap();
    assert!(!output.is_error);
    assert!(text(&output).contains("automatic syntax repair"));
    assert_eq!(writer.bridge_calls.load(Ordering::SeqCst), 1);
    assert_eq!(writer.lsp_calls.load(Ordering::SeqCst), 1);
    assert_eq!(writer.invalidations.load(Ordering::SeqCst), 2);
    assert_eq!(writer.bumps.load(Ordering::SeqCst), 1);
}


struct BudgetCompletion {
    calls: AtomicUsize,
    finished: AtomicUsize,
    cancel_parent: Option<CancellationToken>,
}
#[async_trait]
impl EditRepairCompletion for BudgetCompletion {
    fn model_name(&self) -> String { "fixture/budget".into() }
    async fn complete(&self, _prompt: String, cancel: CancellationToken) -> Result<String,String> {
        let attempt=self.calls.fetch_add(1,Ordering::SeqCst);
        if let Some(parent)=&self.cancel_parent { parent.cancel(); }
        if attempt==0 && self.cancel_parent.is_none() {
            // Eleven seconds spent by the first rejected candidate must leave
            // only nine seconds for the second request, not another20s budget.
            tokio::time::sleep(std::time::Duration::from_secs(11)).await;
            self.finished.fetch_add(1,Ordering::SeqCst);
            return Ok("return (;".into());
        }
        cancel.cancelled().await;
        self.finished.fetch_add(1,Ordering::SeqCst);
        Err("controlled request settled after cancellation".into())
    }
}
struct BudgetHost {
    root:PathBuf,
    completion:Arc<BudgetCompletion>,
    diagnostics:Mutex<Vec<String>>,
}
#[async_trait]
impl EditPostApplyHost for BudgetHost {
    fn agent_dir(&self)->PathBuf {self.root.clone()}
    async fn prepare_repair(&self,_:Option<CancellationToken>)->Result<Option<Arc<dyn EditRepairCompletion>>,String>{Ok(Some(self.completion.clone()))}
    fn diagnostic(&self,message:&str){self.diagnostics.lock().unwrap().push(message.into());}
}
#[tokio::test]
async fn repair_shared_twenty_second_budget_and_caller_abort_keep_committed_edit() {
    assert_eq!(ara_edit::auto_repair::REPAIR_TIMEOUT_MS,20_000);
    for caller_abort in [false,true] {
        let dir=tempfile::tempdir().unwrap();let path=dir.path().join("budget.ts");std::fs::write(&path,SOURCE).unwrap();
        let caller=CancellationToken::new();
        let completion=Arc::new(BudgetCompletion {calls:AtomicUsize::new(0),finished:AtomicUsize::new(0),cancel_parent:caller_abort.then(||caller.clone())});
        let host=Arc::new(BudgetHost {root:dir.path().to_owned(),completion:completion.clone(),diagnostics:Mutex::new(Vec::new())});
        let ctx=ToolContext::new(dir.path()).with_edit(EditMode::Replace,true);ctx.set_edit_post_apply_host(host.clone());
        let tool=EditTool::new(ctx);let start=std::time::Instant::now();
        let output=tokio::time::timeout(std::time::Duration::from_secs(24),tool.execute("budget",json!({"path":"budget.ts","old_string":"return 1;","new_string":"return (;"}).as_object().unwrap().clone(),caller,Arc::new(|_|{}))).await.expect("one shared repair bound").unwrap();
        assert!(text(&output).contains("no longer parses"));assert_eq!(std::fs::read_to_string(&path).unwrap(),BROKEN);
        let diagnostics=host.diagnostics.lock().unwrap().join("\n");
        if caller_abort {
            assert!(diagnostics.contains("auto-repair aborted"),"{diagnostics}");assert!(!diagnostics.contains("timed out"));assert_eq!(completion.calls.load(Ordering::SeqCst),1);
        } else {
            assert!(diagnostics.contains("timed out after 20000ms waiting on fixture/budget"),"{diagnostics}");assert!(start.elapsed()>=std::time::Duration::from_secs(19));assert_eq!(completion.calls.load(Ordering::SeqCst),2);
        }
        let expected=completion.calls.load(Ordering::SeqCst);
        tokio::time::timeout(std::time::Duration::from_secs(1),async{while completion.finished.load(Ordering::SeqCst)!=expected {tokio::task::yield_now().await;}}).await.expect("admitted typed request retains settlement owner");
    }
}
